//! ADR 2026-10-27-relayed-instructions 付記 A2: an escalated item gets a second CoS judgment when a
//! relayed instruction that covers it arrives after its triage run started. The store picks the
//! pairs (state = `cos_inbox_items.message_id` and `chat_messages.seq`); this pass maps each item
//! to its tasks through the derived inbox and checks the relay's `task_ids`. No LLM, no judgment.
use std::collections::{BTreeMap, HashMap, HashSet};

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
        let seen = &self.triage.rejudge_out_of_scope;
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
        let inbox = self.human_inbox_items(dispatcher, now)?;
        let by_id: HashMap<&str, _> = inbox.iter().map(|i| (i.id.as_str(), i)).collect();
        let mut per_thread: BTreeMap<String, Vec<RelayRejudge>> = BTreeMap::new();
        for c in fresh {
            // The wait closed (the person answered): the stale sweep resolves the item.
            let Some(item) = by_id.get(c.source_key.as_str()) else {
                continue;
            };
            let tasks: HashSet<String> = item_tasks(item).map(|t| t.to_string()).collect();
            let (inside, outside): (Vec<&ChatMessage>, Vec<&ChatMessage>) = c
                .relays
                .iter()
                .partition(|m| m.relay.as_ref().is_some_and(|r| covers(r, &tasks)));
            for m in outside {
                self.triage
                    .rejudge_out_of_scope
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
