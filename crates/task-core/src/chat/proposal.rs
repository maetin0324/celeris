//! ADR 2026-11-04-project-controller §A4: the general CoS hands its own proposal to a project CoS.
//!
//! `chat.relay_forward` carries only the person's own words. A proposal is CoS's words, so it is a
//! separate path: a `role=system` queued input of the project thread with a [`ChatCosProposal`]
//! record and a fixed text prefix. `role=system` keeps it out of `instructed_by` (which requires a
//! person's `role=user` message or a relay), so the project CoS can act on it only within what the
//! person already approved (its own ops and the triage types), never as a new human instruction.
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use time::OffsetDateTime;

use super::store::{
    MESSAGE_SELECT, NewMessage, chat_ts, emit_message, emit_queue, insert_message,
    message_from_raw, message_raw, message_require, relay_record, thread_require, touch_thread,
};
use super::*;

/// Prefix of the proposal message's `client_message_id`.
pub const COS_PROPOSAL_KEY_PREFIX: &str = "cos-proposal:";
/// First line of every proposal text (written by Rust; the body cannot drop it).
pub const COS_PROPOSAL_TEXT_PREFIX: &str = "【統括 CoS の提案（人の指示ではない）】";

/// `chat.proposal_forward`: the general CoS (`from_thread`, run `run_id`) posts `req.text` to the
/// project thread `to_thread` as a queued `role=system` input. Refused as `Invalid` (422): the
/// source is not the general thread, the target is not a project thread, blank text, no task,
/// a task outside the project, or a missing / past expiry. The same `client_message_id` with the
/// same text returns the first message; a different text is a conflict.
pub fn chat_proposal_forward_tx(
    tx: &rusqlite::Connection,
    from_thread: &str,
    to_thread: &str,
    req: &ChatProposalRequest,
    run_id: &str,
    operation_id: &str,
    now: OffsetDateTime,
) -> Result<ChatMessage, ChatError> {
    let from = thread_require(tx, from_thread)?;
    if from.kind != ChatThreadKind::General {
        return Err(ChatError::Invalid(format!(
            "proposals start from the general CoS thread (thread {from_thread} is {:?})",
            from.kind
        )));
    }
    let to = thread_require(tx, to_thread)?;
    if to.kind != ChatThreadKind::Project {
        return Err(ChatError::Invalid(format!(
            "proposals go to a project CoS thread (thread {to_thread} is {:?})",
            to.kind
        )));
    }
    if req.text.trim().is_empty() {
        return Err(ChatError::Invalid("a proposal needs text".into()));
    }
    if req.task_ids.is_empty() {
        return Err(ChatError::Invalid(
            "a proposal names the tasks it is about (task_ids must not be empty)".into(),
        ));
    }
    if req.expires_at.trim().is_empty() {
        return Err(ChatError::Invalid("a proposal needs expires_at".into()));
    }
    let key = format!("{COS_PROPOSAL_KEY_PREFIX}{}", req.client_message_id);
    if key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
        return Err(ChatError::TooLarge("proposal client_message_id".into()));
    }
    let text = format!("{COS_PROPOSAL_TEXT_PREFIX}\n{}", req.text.trim());
    if text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES {
        return Err(ChatError::TooLarge("proposal text".into()));
    }
    // The same scope checks as a relay (tasks exist, belong to the project, expiry after now).
    let scope = relay_record(
        tx,
        to_thread,
        &ChatRelayRequest {
            client_message_id: req.client_message_id.clone(),
            text: req.text.clone(),
            said_at: chat_ts(now),
            task_ids: req.task_ids.clone(),
            expires_at: Some(req.expires_at.clone()),
        },
        "cos",
        now,
    )?;
    let existing = tx
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE thread_id=?1 AND client_message_id=?2"),
            params![to_thread, key],
            message_raw,
        )
        .optional()?;
    if let Some(raw) = existing {
        let message = message_from_raw(tx, raw)?;
        if message.role == ChatMessageRole::System && message.text == text {
            return Ok(message);
        }
        return Err(ChatError::Conflict(format!(
            "proposal key {} reused with another text",
            req.client_message_id
        )));
    }
    let proposal = ChatCosProposal {
        proposed_by: format!("cos:{from_thread}"),
        from_thread_id: from_thread.to_owned(),
        run_id: run_id.to_owned(),
        operation_id: operation_id.to_owned(),
        task_ids: scope.scope.task_ids,
        expires_at: scope
            .scope
            .expires_at
            .unwrap_or_else(|| req.expires_at.clone()),
    };
    let at = chat_ts(now);
    let id = insert_message(
        tx,
        NewMessage {
            thread_id: to_thread,
            role: ChatMessageRole::System,
            text: &text,
            state: ChatMessageState::Queued,
            client_message_id: Some(&key),
            reply_to_id: None,
            run_id: None,
            metadata: &json!({"posted_by": "cos", "cos_proposal": proposal}),
        },
        &at,
    )?;
    let message = message_require(tx, to_thread, &id)?;
    touch_thread(tx, to_thread, &at)?;
    emit_message(tx, &message, &at)?;
    emit_queue(tx, to_thread, &at)?;
    Ok(message)
}

impl crate::store::SqliteStore {
    /// §A4: the general CoS thread that created `task_id` through an applied `task.create`
    /// operation (the thread the request came from), if any. A task the person or a project CoS
    /// created returns `None`, and its completion goes to the project thread as before.
    pub fn cos_origin_thread_for_task(&self, task_id: &str) -> Result<Option<String>, ChatError> {
        let conn = super::store::writer(self)?;
        Ok(conn
            .query_row(
                "SELECT o.thread_id FROM cos_operations o JOIN chat_threads t ON t.id=o.thread_id \
                 WHERE o.target_kind='task' AND o.target_id=?1 AND o.action='task.create' \
                 AND o.state='applied' AND t.kind='general' ORDER BY o.created_at,o.id LIMIT 1",
                [task_id],
                |r| r.get(0),
            )
            .optional()?)
    }
}
