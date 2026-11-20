//! The resolve API's judgment of a triage item already read from the store (ADR 2026-10-06
//! cos-inbox-triage): an item that already has an outcome is refused with 409 (fail closed). The
//! rows are read by `task_core::cos_inbox` on the store's read connection and the outcome mark is
//! written by `task_core::chat::triage::cos_triage_resolve_tx` inside the `cos_operation_apply`
//! transaction of its audit row (ADR 2026-11-19-chat-attachment-store-connection D2).
use task_core::chat::ChatError;
use task_core::cos_inbox::{CosInboxItem, cos_inbox_fallback_unsent};
use task_core::store::SqliteStore;

use super::inbox::revision_conflict;
use crate::problem::ApiProblem;

/// Why `resolve` refuses an item that already has an outcome, or `None` when CoS may still judge it.
/// ADR 2026-11-11-cos-fallback-timing D2: a fallback the notifier has not sent yields to CoS's
/// judgment; a sent one stays with the person (this judgment is recorded as rejected and nothing
/// is sent again).
pub(super) fn terminal_conflict(
    store: &SqliteStore,
    item: &CosInboxItem,
) -> Result<Option<ApiProblem>, ChatError> {
    if !item.is_terminal()
        || (item.state == "fallback" && cos_inbox_fallback_unsent(store, &item.id)?)
    {
        return Ok(None);
    }
    let i = &item.id;
    Ok(Some(revision_conflict(if item.state == "fallback" {
        format!(
            "triage item {i} was already sent to the person by the fallback; this judgment is recorded and nothing is sent again"
        )
    } else {
        format!("triage item {i} is already {}", item.state)
    })))
}
