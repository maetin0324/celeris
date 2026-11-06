//! ADR 2026-10-27-relayed-instructions 付記 A2: an escalated item gets a second CoS judgment when a
//! relayed instruction that covers it arrives after its triage run started. The store picks the
//! pairs (state = `cos_inbox_items.message_id` and `chat_messages.seq`); this pass maps each item
//! to its tasks through the derived inbox and checks the relay's `task_ids`. No LLM, no judgment.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use task_core::TaskId;
use task_core::chat::relay_rejudge::RelayRejudge;
use task_core::chat::{ChatMessage, ChatRelay};

use super::launch::CosChatLaunch;
use super::triage::item_tasks;
use crate::dispatcher::Dispatcher;

/// R1: the relay covers the item when it names no task (the whole thread/project) or one of the
/// item's tasks.
fn covers(relay: &ChatRelay, tasks: &HashSet<String>) -> bool {
    relay.scope.task_ids.is_empty() || relay.scope.task_ids.iter().any(|t| tasks.contains(t))
}

impl CosChatLaunch {
    /// One pass: hand each covered escalated item, with the ids of the relays covering it, to one
    /// run per idle thread. R2: a thread with a live run waits (the store re-checks in the claim
    /// transaction), so the running triage run and the re-judging run never overlap.
    pub(super) fn relay_rejudge(&mut self, dispatcher: &mut Dispatcher) -> Result<(), String> {
        let now = dispatcher.now_utc();
        let candidates = self
            .store
            .cos_triage_relay_candidates(now)
            .map_err(|e| e.to_string())?;
        let seen = &self.triage.rejudge_settled;
        let fresh: Vec<_> = candidates
            .into_iter()
            .filter(|c| !self.running.contains_key(&c.thread_id))
            .filter(|c| {
                c.relays
                    .iter()
                    .any(|r| !seen.contains(&(c.item_id.clone(), r.id.clone())))
            })
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        // ADR 2026-11-06-cos-chat-launch-scaling D3: the derived inbox is built for the
        // candidates' tasks only (the same items as the full inbox), so this pass does not scan
        // every task of the store. A candidate whose key names no task falls back to the full
        // inbox once.
        let keyed: Vec<(_, Option<TaskId>)> = fresh
            .into_iter()
            .map(|c| {
                let task = task_ops::human_inbox::item_task_id(self.store.as_ref(), &c.source_key);
                (c, task)
            })
            .collect();
        let inbox = if keyed.iter().all(|(_, task)| task.is_some()) {
            let tasks: BTreeSet<TaskId> = keyed.iter().filter_map(|(_, task)| *task).collect();
            self.triage.relay_inbox_builds.scoped += 1;
            self.human_inbox_touching(dispatcher, now, &tasks)?.items
        } else {
            self.triage.relay_inbox_builds.full += 1;
            self.human_inbox_items(dispatcher, now)?
        };
        let by_id: HashMap<&str, _> = inbox.iter().map(|i| (i.id.as_str(), i)).collect();
        let mut per_thread: BTreeMap<String, Vec<RelayRejudge>> = BTreeMap::new();
        for (c, _) in keyed {
            // The wait closed (the person answered): the stale sweep resolves the item. The pairs
            // are settled so the next tick does not build the inbox for them again.
            let Some(item) = by_id.get(c.source_key.as_str()) else {
                for m in &c.relays {
                    self.triage
                        .rejudge_settled
                        .insert((c.item_id.clone(), m.id.clone()));
                }
                continue;
            };
            let tasks: HashSet<String> = item_tasks(item).map(|t| t.to_string()).collect();
            let (inside, outside): (Vec<&ChatMessage>, Vec<&ChatMessage>) = c
                .relays
                .iter()
                .partition(|m| m.relay.as_ref().is_some_and(|r| covers(r, &tasks)));
            for m in outside {
                self.triage
                    .rejudge_settled
                    .insert((c.item_id.clone(), m.id.clone()));
            }
            if !inside.is_empty() {
                per_thread
                    .entry(c.thread_id)
                    .or_default()
                    .push(RelayRejudge {
                        item_id: c.item_id,
                        source_key: c.source_key,
                        relay_message_ids: inside.into_iter().map(|m| m.id.clone()).collect(),
                    });
            }
        }
        for (thread, rejudge) in per_thread {
            if self.running.contains_key(&thread) || !self.has_capacity(dispatcher) {
                continue;
            }
            let run_id = ulid::Ulid::new().to_string();
            let Some(claim) = self
                .store
                .cos_triage_relay_claim(&thread, &run_id, &rejudge, now)
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            let run = self
                .store
                .chat_run_get(&claim.thread_id, &claim.run_id)
                .map_err(|e| e.to_string())?;
            self.run_configs.insert(thread.clone(), self.config.clone());
            self.launch_routes(dispatcher, &thread, &run, 0, None, now);
        }
        Ok(())
    }
}

impl CosChatLaunch {
    /// ADR 2026-10-30-cos-review-open-waits D4: one pass that hands each escalated plan approval
    /// whose task's decisions the person has answered since the judgment to one run per idle
    /// thread. The store holds the state (the item's input message); no LLM, no judgment here.
    pub(super) fn plan_rejudge(&mut self, dispatcher: &mut Dispatcher) -> Result<(), String> {
        let now = dispatcher.now_utc();
        let candidates = self
            .store
            .cos_triage_plan_rejudge_candidates()
            .map_err(|e| e.to_string())?;
        let mut per_thread: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for c in candidates {
            if !self.running.contains_key(&c.thread_id) {
                per_thread.entry(c.thread_id).or_default().push(c.item_id);
            }
        }
        for (thread, items) in per_thread {
            if self.running.contains_key(&thread) || !self.has_capacity(dispatcher) {
                continue;
            }
            let run_id = ulid::Ulid::new().to_string();
            let Some(claim) = self
                .store
                .cos_triage_plan_rejudge_claim(&thread, &run_id, &items, now)
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            let run = self
                .store
                .chat_run_get(&claim.thread_id, &claim.run_id)
                .map_err(|e| e.to_string())?;
            self.run_configs.insert(thread.clone(), self.config.clone());
            self.launch_routes(dispatcher, &thread, &run, 0, None, now);
        }
        Ok(())
    }
}
