//! ADR 2026-11-21-chat-discussion-mode D5: a person edits or retracts their own `role=user`
//! message of a CoS thread. The previous body is appended to `chat_events` (`message_revised`) in
//! the same transaction, and a thread summary or a live CoS session that already covered the
//! message is dropped (D5.4). The revision lives in `chat_messages.metadata_json` (D5.2); the FTS5
//! index follows `chat_messages.text` through its existing update trigger.
//! Kept outside [`crate::chat`] (repo health `size.responsibility-growth` of that module); it
//! depends on `chat` one way and nothing in `chat` depends on it.

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::chat::session::retire_conn;
use crate::chat::store::{
    ChatError, append_event, chat_ts, emit_message, enum_str, message_require, read_tx,
    thread_require, touch_thread,
};
use crate::chat::{
    CHAT_MESSAGE_TEXT_MAX_BYTES, ChatCheckpointReset, ChatEvent, ChatEventData,
    ChatEventListResponse, ChatEventType, ChatMessage, ChatMessageChange, ChatMessageEditRequest,
    ChatMessageRetractRequest, ChatMessageRevisedData, ChatMessageRevisionKind, ChatMessageRole,
    ChatMessageState,
};
use crate::project_proposal::sha256_hex;
use crate::store::SqliteStore;

/// D5.1: the longest retract reason, in characters.
pub const CHAT_RETRACT_REASON_MAX_CHARS: usize = 500;

enum Change<'a> {
    Edit(&'a str),
    Retract(&'a str),
}

impl SqliteStore {
    /// D5.1 `PATCH /chat/threads/{t}/messages/{m}`: `by` (the caller's `human:<name>`) replaces
    /// the text of a message they wrote. The mode and the attachments stay as they were.
    pub fn chat_message_edit(
        &self,
        thread_id: &str,
        message_id: &str,
        by: &str,
        req: &ChatMessageEditRequest,
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        self.write_tx(|tx| {
            revise_tx(
                tx,
                (thread_id, message_id, by),
                req.expected_revision,
                Change::Edit(&req.text),
                now,
            )
        })
    }

    /// D5.1 `POST /chat/threads/{t}/messages/{m}/retract`: `by` withdraws a delivered message
    /// they wrote; its text becomes empty (a queued one is cancelled with `DELETE` instead).
    pub fn chat_message_retract(
        &self,
        thread_id: &str,
        message_id: &str,
        by: &str,
        req: &ChatMessageRetractRequest,
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        self.write_tx(|tx| {
            revise_tx(
                tx,
                (thread_id, message_id, by),
                req.expected_revision,
                Change::Retract(&req.reason),
                now,
            )
        })
    }

    /// D5.1 `GET /chat/threads/{t}/messages/{m}/revisions`: the message's `message_revised`
    /// events, oldest first (each holds the body before that change).
    pub fn chat_message_revisions(
        &self,
        thread_id: &str,
        message_id: &str,
    ) -> Result<ChatEventListResponse, ChatError> {
        read_tx(self, |conn| {
            thread_require(conn, thread_id)?;
            message_require(conn, thread_id, message_id)?;
            let mut stmt = conn.prepare_cached(
                "SELECT id,run_id,payload_json,created_at FROM chat_events \
                 WHERE thread_id=?1 AND message_id=?2 AND type=?3 ORDER BY id",
            )?;
            let rows = stmt.query_map(
                params![
                    thread_id,
                    message_id,
                    enum_str(&ChatEventType::MessageRevised)?
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?;
            let mut items = Vec::new();
            for row in rows {
                let (id, run_id, payload, at) = row?;
                items.push(ChatEvent {
                    id: id.to_string(),
                    event_type: ChatEventType::MessageRevised,
                    thread_id: thread_id.to_string(),
                    run_id,
                    message_id: Some(message_id.to_string()),
                    at,
                    data: ChatEventData::MessageRevised(serde_json::from_str(&payload)?),
                });
            }
            Ok(ChatEventListResponse {
                items,
                next_cursor: None,
            })
        })
    }
}

/// D5.1–D5.4 in the caller's transaction: check who and what, record the previous body, drop a
/// summary / session that covered the message, then write the new text and stream the message.
fn revise_tx(
    tx: &Connection,
    (thread_id, message_id, by): (&str, &str, &str),
    expected_revision: u64,
    change: Change<'_>,
    now: OffsetDateTime,
) -> Result<ChatMessage, ChatError> {
    let at = chat_ts(now);
    thread_require(tx, thread_id)?;
    let message = message_require(tx, thread_id, message_id)?;
    if message.role != ChatMessageRole::User
        || message.relay.is_some()
        || !by.starts_with("human:")
        || message.author.as_deref() != Some(by)
    {
        return Err(ChatError::EditForbidden(format!(
            "only the person who wrote message {message_id} can edit or retract it"
        )));
    }
    let refused = match (&change, message.state) {
        _ if message.retracted.is_some() => Some("retracted"),
        (_, ChatMessageState::Running) => Some("being read by a CoS run"),
        (_, ChatMessageState::Cancelled) => Some("cancelled"),
        (Change::Retract(_), ChatMessageState::Queued) => {
            Some("queued; cancel it with DELETE instead")
        }
        _ => None,
    };
    if let Some(why) = refused {
        return Err(ChatError::Conflict(format!(
            "message {message_id} is {why}"
        )));
    }
    if expected_revision != message.revision {
        return Err(ChatError::Conflict(format!(
            "message {message_id} is at revision {}, not {expected_revision}",
            message.revision
        )));
    }
    match change {
        Change::Edit(text) if text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES => {
            return Err(ChatError::TooLarge(format!(
                "text is {} bytes (max {CHAT_MESSAGE_TEXT_MAX_BYTES})",
                text.len()
            )));
        }
        Change::Edit(text) if text.trim().is_empty() && message.attachment_ids.is_empty() => {
            return Err(ChatError::Invalid("blank text needs an attachment".into()));
        }
        Change::Edit(text) if text == message.text => {
            return Err(ChatError::Invalid("text is unchanged".into()));
        }
        Change::Retract(reason) if reason.chars().count() > CHAT_RETRACT_REASON_MAX_CHARS => {
            return Err(ChatError::Invalid(format!(
                "reason is longer than {CHAT_RETRACT_REASON_MAX_CHARS} characters"
            )));
        }
        _ => {}
    }
    let seq = i64::try_from(message.seq).unwrap_or(i64::MAX);
    let checkpoint_reset = reset_checkpoint(tx, thread_id, seq, &at)?;
    let session_saw: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM node_sessions WHERE kind='cos_chat' AND thread_id=?1 \
         AND retired_at IS NULL AND delivered_through_seq>=?2)",
        params![thread_id, seq],
        |row| row.get(0),
    )?;
    if session_saw {
        retire_conn(tx, thread_id, &at)?;
    }
    let raw: String = tx.query_row(
        "SELECT metadata_json FROM chat_messages WHERE id=?1",
        params![message_id],
        |row| row.get(0),
    )?;
    let mut meta: Value = serde_json::from_str(&raw)?;
    let Some(fields) = meta.as_object_mut() else {
        return Err(ChatError::Invalid(format!(
            "message {message_id} has unreadable metadata"
        )));
    };
    let revision = message.revision + 1;
    fields.insert("revision".into(), json!(revision));
    let (kind, text, reason) = match change {
        Change::Edit(text) => (ChatMessageRevisionKind::Edit, text, None),
        Change::Retract(reason) => (ChatMessageRevisionKind::Retract, "", Some(reason)),
    };
    let mark = ChatMessageChange {
        by: by.to_string(),
        at: at.clone(),
        reason: reason.map(str::to_owned),
    };
    let key = match kind {
        ChatMessageRevisionKind::Edit => "edited",
        ChatMessageRevisionKind::Retract => "retracted",
    };
    fields.insert(key.into(), serde_json::to_value(&mark)?);
    tx.execute(
        "UPDATE chat_messages SET text=?2,metadata_json=?3,updated_at=?4 WHERE id=?1",
        params![message_id, text, serde_json::to_string(&meta)?, at],
    )?;
    let data = ChatMessageRevisedData {
        message_id: message_id.to_string(),
        kind,
        revision,
        previous_revision: message.revision,
        previous_sha256: sha256_hex(&message.text),
        previous_text: message.text,
        by: by.to_string(),
        reason: mark.reason,
        checkpoint_reset,
    };
    append_event(
        tx,
        thread_id,
        None,
        Some(message_id),
        ChatEventType::MessageRevised,
        &ChatEventData::MessageRevised(data),
        &at,
    )?;
    touch_thread(tx, thread_id, &at)?;
    let message = message_require(tx, thread_id, message_id)?;
    emit_message(tx, &message, &at)?;
    Ok(message)
}

/// D5.4: a thread summary that covers `seq` is cleared (a running checkpoint then fails its
/// `expected_summary_through_seq` match); the cleared text and cursor go into the event.
fn reset_checkpoint(
    tx: &Connection,
    thread_id: &str,
    seq: i64,
    at: &str,
) -> Result<Option<ChatCheckpointReset>, ChatError> {
    let (summary, through): (String, i64) = tx.query_row(
        "SELECT summary,summary_through_seq FROM chat_threads WHERE id=?1",
        params![thread_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if through <= 0 || seq > through {
        return Ok(None);
    }
    tx.execute(
        "UPDATE chat_threads SET summary='',summary_through_seq=0,updated_at=?2 WHERE id=?1",
        params![thread_id, at],
    )?;
    Ok(Some(ChatCheckpointReset {
        previous_through_seq: u64::try_from(through).unwrap_or(0),
        previous_summary: summary,
    }))
}

#[cfg(test)]
#[path = "chat_revision_tests.rs"]
mod tests;
