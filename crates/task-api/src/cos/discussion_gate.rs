//! ADR 2026-11-21-chat-discussion-mode D2: a CoS run whose inputs chose `discuss` or `propose`
//! changes nothing through its credential. Every registered `/cos/operations` action and
//! `/cos/inbox/{i}/resolve` is refused (403 `discussion_mode_forbidden`) and recorded as a rejected
//! operation; an `execute` run (the default) goes on to the existing checks unchanged.

use axum::http::StatusCode;
use serde_json::Value;
use task_core::SqliteStore;
use task_core::chat::ChatDiscussionMode;

use super::operations::OperationAudit;
use crate::chat::chat_problem;
use crate::problem::ApiProblem;

/// D2.3: problem code of an operation refused by the run's discussion mode.
pub(crate) const DISCUSSION_MODE_FORBIDDEN: &str = "discussion_mode_forbidden";

/// D2.3: refuses `action` unless the caller's run is `execute` (derived from the run's input
/// messages by [`task_core::discussion_mode::discussion_mode_of_run`]). Called by
/// [`super::operations::bind_run_policy`] once the policy version matches, before `instructed_by`
/// is read, so citing a person's message cannot lift the refusal. The audit payload names the mode
/// and the messages that decided it (D7).
pub(crate) fn check(
    store: &SqliteStore,
    audit: &mut OperationAudit,
    target_kind: &str,
    target_id: &str,
    action: &str,
) -> Result<(), ApiProblem> {
    let run = store
        .chat_run_discussion_mode(&audit.ctx.thread_id, &audit.ctx.run_id)
        .map_err(chat_problem)?;
    if run.mode == ChatDiscussionMode::Execute {
        return Ok(());
    }
    if let (Value::Object(payload), Ok(value)) = (&mut audit.payload, serde_json::to_value(&run)) {
        payload.insert("discussion_mode".into(), value);
    }
    let problem = ApiProblem::new(
        StatusCode::FORBIDDEN,
        DISCUSSION_MODE_FORBIDDEN,
        format!(
            "run mode is {}; {action} needs an execute-mode human message",
            run.mode.as_str()
        ),
    );
    Err(audit.reject(store, target_kind, target_id, problem))
}
