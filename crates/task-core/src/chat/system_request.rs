//! ADR 2026-10-28-cos-inbox-chat-summary D4: daemon requests queued as CoS run inputs
//! (`role=system`, state `queued`) on the general thread, and their retry after a failed run.

use rusqlite::{OptionalExtension, params};
use serde_json::json;
use time::OffsetDateTime;

use super::store::{
    MESSAGE_SELECT, NewMessage, chat_ts, cos_thread_ensure_tx, emit_message, emit_queue, immediate,
    insert_message, message_from_raw, message_raw, message_require, thread_require, touch_thread,
    writer,
};
use super::*;
use crate::store::SqliteStore;

impl SqliteStore {
    /// Whether a person's message waits in the thread's queue (ADR 2026-10-28-cos-inbox-chat-summary
    /// D4: a daemon request queued as `role=system` yields to new triage items, a person's does not).
    pub fn chat_thread_has_queued_user(&self, thread_id: &str) -> Result<bool, ChatError> {
        let conn = writer(self)?;
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE thread_id=?1 AND role='user' AND state='queued')",
            [thread_id],
            |r| r.get(0),
        )?)
    }

    /// The general CoS thread id, if it exists (ADR 2026-10-28-cos-inbox-chat-summary D4).
    pub fn cos_general_thread_id(&self) -> Result<Option<String>, ChatError> {
        let conn = writer(self)?;
        Ok(conn
            .query_row(
                "SELECT id FROM chat_threads WHERE kind='general'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The general CoS thread id, created on first use (ADR 2026-10-28-cos-inbox-chat-summary D4).
    pub fn cos_general_thread_ensure(&self, now: OffsetDateTime) -> Result<String, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let id = cos_thread_ensure_tx(&tx, None, &chat_ts(now))?;
        tx.commit()?;
        Ok(id)
    }

    /// ADR 2026-10-28-cos-inbox-chat-summary D4: a daemon request queued as a CoS run input. It is
    /// a `role=system` message in state `queued`, so the normal queue claim runs CoS on it and the
    /// run's final text becomes the assistant reply, while `instructed_by` (which requires
    /// `role=user`) can never cite it. The stable key makes it idempotent across ticks and restarts.
    pub fn chat_system_queue_once(
        &self,
        thread_id: &str,
        key: &str,
        text: &str,
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        if key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
            return Err(ChatError::TooLarge("system message key".into()));
        }
        if text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES {
            return Err(ChatError::TooLarge("system message text".into()));
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        thread_require(&tx, thread_id)?;
        let existing = tx
            .query_row(
                &format!("{MESSAGE_SELECT} WHERE thread_id=?1 AND client_message_id=?2"),
                params![thread_id, key],
                message_raw,
            )
            .optional()?;
        if let Some(raw) = existing {
            let message = message_from_raw(&tx, raw)?;
            if message.role == ChatMessageRole::System && message.text == text {
                return Ok(message);
            }
            return Err(ChatError::Conflict(format!(
                "system message key {key} reused"
            )));
        }
        let id = insert_message(
            &tx,
            NewMessage {
                thread_id,
                role: ChatMessageRole::System,
                text,
                state: ChatMessageState::Queued,
                client_message_id: Some(key),
                reply_to_id: None,
                run_id: None,
                metadata: &json!({}),
            },
            &at,
        )?;
        let message = message_require(&tx, thread_id, &id)?;
        touch_thread(&tx, thread_id, &at)?;
        emit_message(&tx, &message, &at)?;
        emit_queue(&tx, thread_id, &at)?;
        tx.commit()?;
        Ok(message)
    }

    /// ADR 2026-10-28-cos-inbox-chat-summary D4: queues again each daemon request (`role=system`,
    /// key starting with `key_prefix`) whose CoS run failed, as a new message with key
    /// `<base>#<n>`, until `max_attempts` runs in all. A cancelled or interrupted request (a person
    /// stopped it) is not retried. Returns the requests queued by this call.
    pub fn chat_system_queue_retry_failed(
        &self,
        thread_id: &str,
        key_prefix: &str,
        max_attempts: u32,
        now: OffsetDateTime,
    ) -> Result<Vec<ChatMessage>, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        thread_require(&tx, thread_id)?;
        let failed: Vec<(String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT client_message_id,text FROM chat_messages WHERE thread_id=?1 AND state='failed' \
                 AND role='system' AND substr(client_message_id,1,length(?2))=?2 ORDER BY seq",
            )?;
            stmt.query_map(params![thread_id, key_prefix], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?
        };
        let mut queued = Vec::new();
        for (key, text) in failed {
            let (base, attempt) = match key.rsplit_once('#') {
                Some((base, n)) => match n.parse::<u32>() {
                    Ok(n) => (base, n),
                    Err(_) => (key.as_str(), 1),
                },
                None => (key.as_str(), 1),
            };
            if attempt >= max_attempts {
                continue;
            }
            let next = format!("{base}#{}", attempt + 1);
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE thread_id=?1 AND client_message_id=?2)",
                params![thread_id, next],
                |r| r.get(0),
            )?;
            if exists {
                continue;
            }
            let id = insert_message(
                &tx,
                NewMessage {
                    thread_id,
                    role: ChatMessageRole::System,
                    text: &text,
                    state: ChatMessageState::Queued,
                    client_message_id: Some(&next),
                    reply_to_id: None,
                    run_id: None,
                    metadata: &json!({}),
                },
                &at,
            )?;
            let message = message_require(&tx, thread_id, &id)?;
            emit_message(&tx, &message, &at)?;
            queued.push(message);
        }
        if !queued.is_empty() {
            touch_thread(&tx, thread_id, &at)?;
            emit_queue(&tx, thread_id, &at)?;
        }
        tx.commit()?;
        Ok(queued)
    }
}
