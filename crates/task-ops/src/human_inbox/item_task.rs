//! ADR 2026-10-27-relayed-instructions 付記 A2 R3: the task of an inbox item read from its id, for
//! an item that is no longer in the derived inbox (the person answered it, the wait closed).
use task_core::{TaskId, TaskStore};

/// Kinds whose id is `<kind>-<task id>[-…]` (`Builder` in `human_inbox.rs`).
const TASK_KEYED: [&str; 10] = [
    "question",
    "failed",
    "unroutable",
    "phase_gate",
    "plan_gate",
    "delivery_skipped",
    "browser_prerequisite",
    "stalled",
    "review_resync_halted",
    "unintegrated",
];

/// The task an inbox item id stands for: the approval's task (`authorization-` /
/// `acceptance_check-`), the decision's task (`decision-`), or the task in the id. `None` for
/// kinds without a task (cluster login, credential, browser waits, …) or unknown ids.
pub fn item_task_id(store: &dyn TaskStore, item_id: &str) -> Option<TaskId> {
    let (kind, rest) = item_id.split_once('-')?;
    match kind {
        "authorization" | "acceptance_check" => {
            store
                .approval_get(rest.parse().ok()?)
                .ok()
                .flatten()?
                .task_id
        }
        "decision" => store.decision_get(rest).ok().flatten().map(|d| d.task_id),
        _ if TASK_KEYED.contains(&kind) => rest.get(..26)?.parse().ok(),
        _ => None,
    }
}
