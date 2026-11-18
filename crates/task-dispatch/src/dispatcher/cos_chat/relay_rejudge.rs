//! ADR 2026-10-27-relayed-instructions 付記 A2: an escalated item gets a second CoS judgment when a
//! relayed instruction that covers it arrives after its triage run started. The store picks the
//! pairs (state = `cos_inbox_items.message_id` and `chat_messages.seq`); this file maps each item
//! to its tasks through the derived inbox and checks the relay's `task_ids`. No LLM, no judgment.
//! The entry (`launch.rs`) reads the candidates and the inbox, claims and launches; the functions
//! here only choose, from values (ADR 2026-11-18-dispatcher-store-access-pilot).
use std::collections::{BTreeMap, HashMap, HashSet};

use task_core::chat::plan_rejudge::PlanRejudgeCandidate;
use task_core::chat::relay_rejudge::{RelayRejudge, RelayRejudgeCandidate};
use task_core::chat::{ChatMessage, ChatRelay};
use task_ops::human_inbox::InboxItem;

use super::triage::item_tasks;

/// Re-judging runs per thread, and the (item, relay) pairs to settle in this process.
pub(super) type RelayPick = (BTreeMap<String, Vec<RelayRejudge>>, Vec<(String, String)>);

/// R1: the relay covers the item when it names no task (the whole thread/project) or one of the
/// item's tasks.
fn covers(relay: &ChatRelay, tasks: &HashSet<String>) -> bool {
    relay.scope.task_ids.is_empty() || relay.scope.task_ids.iter().any(|t| tasks.contains(t))
}

/// Candidates on a thread without a live run that still have a relay not settled for the item.
pub(super) fn fresh<V>(
    candidates: Vec<RelayRejudgeCandidate>,
    running: &HashMap<String, V>,
    seen: &HashSet<(String, String)>,
) -> Vec<RelayRejudgeCandidate> {
    candidates
        .into_iter()
        .filter(|c| !running.contains_key(&c.thread_id))
        .filter(|c| {
            c.relays
                .iter()
                .any(|r| !seen.contains(&(c.item_id.clone(), r.id.clone())))
        })
        .collect()
}

/// Each candidate's item in `inbox`, with the relays covering it, grouped per thread. A relay
/// outside the item's tasks, and every relay of an item whose wait closed (the person answered:
/// the stale sweep resolves the item), is settled so the next tick does not build the inbox again.
pub(super) fn pick(fresh: Vec<RelayRejudgeCandidate>, inbox: &[InboxItem]) -> RelayPick {
    let by_id: HashMap<&str, _> = inbox.iter().map(|i| (i.id.as_str(), i)).collect();
    let (mut per_thread, mut settled) = (BTreeMap::<_, Vec<_>>::new(), Vec::new());
    for c in fresh {
        let pair = |m: &ChatMessage| (c.item_id.clone(), m.id.clone());
        let Some(item) = by_id.get(c.source_key.as_str()) else {
            settled.extend(c.relays.iter().map(pair));
            continue;
        };
        let tasks: HashSet<String> = item_tasks(item).map(|t| t.to_string()).collect();
        let (inside, outside): (Vec<&ChatMessage>, Vec<&ChatMessage>) = c
            .relays
            .iter()
            .partition(|m| m.relay.as_ref().is_some_and(|r| covers(r, &tasks)));
        settled.extend(outside.into_iter().map(pair));
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
    (per_thread, settled)
}

/// ADR 2026-10-30-cos-review-open-waits D4: the plan approvals to re-judge, per thread without a
/// live run, in the store's order.
pub(super) fn plan_per_thread<V>(
    candidates: Vec<PlanRejudgeCandidate>,
    running: &HashMap<String, V>,
) -> BTreeMap<String, Vec<String>> {
    let mut per_thread: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in candidates {
        if !running.contains_key(&c.thread_id) {
            per_thread.entry(c.thread_id).or_default().push(c.item_id);
        }
    }
    per_thread
}
