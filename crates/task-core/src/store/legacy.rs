//! The CoS node's `messages` rows on the CoS chat (`POST /org/cos/messages` and the delivery
//! notice). The Console readers were removed (ADR 2026-11-17（Console・旧 API・aider の撤去）).
//!
//! ADR 2026-10-22-cos-fixed-threads D2': Console input to the CoS is queued on the fixed
//! general or project CoS thread (never on a legacy thread). The one-time projection of the
//! pre-chat Console rows (`backfill_legacy_cos`) reuses the legacy threads migration 0092 kept
//! and otherwise lands on the fixed CoS thread of the scope: nothing creates a legacy thread.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use time::OffsetDateTime;
use ulid::Ulid;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::Message;
use crate::chat::store::{
    chat_ts, cos_thread_ensure_tx, emit_message, emit_queue, immediate, message_require, writer,
};
use crate::chat::{CHAT_MESSAGE_TEXT_MAX_BYTES, CHAT_QUEUE_MAX, ChatError};

const CURSOR: &str = "cos_chat_legacy_messages_rowid";

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;

struct OldMessage {
    rowid: i64,
    id: String,
    project_id: Option<String>,
    role: String,
    text: String,
    run_id: Option<String>,
    task_id: Option<String>,
    metadata_json: Option<String>,
    created_at: String,
}

impl SqliteStore {
    /// ADR 2026-11-17（Console・旧 API・aider の撤去） D2: project the CoS node's `messages`
    /// rows (the delivery notice for the human) onto the fixed CoS thread right away, with the same
    /// projection the writable open runs. A row already projected is never projected again.
    pub fn cos_messages_project(&self) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        Self::backfill_legacy_cos(&mut conn)
    }

    /// Queue a Console input once, keyed by its old message id, on the fixed CoS thread of the
    /// scope (ADR 2026-10-22-cos-fixed-threads D2'). The old message stays in `messages` and the
    /// queue drives the CoS run.
    pub fn chat_console_enqueue(
        &self,
        message: &Message,
        now: OffsetDateTime,
    ) -> Result<String, ChatError> {
        if message.text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES {
            return Err(ChatError::TooLarge(
                "legacy message exceeds chat text limit".into(),
            ));
        }
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let at = chat_ts(now);
        let project = message.project_id.map(|p| p.to_string());
        let thread_id = cos_thread_ensure_tx(&tx, project.as_deref(), &at)?;
        let legacy_id = message.id.to_string();
        if let Some(id) = tx
            .query_row(
                "SELECT id FROM chat_messages WHERE legacy_message_id=?1",
                [&legacy_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            tx.commit()?;
            return Ok(id);
        }
        let queued: i64 = tx.query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE thread_id=?1 AND role='user' AND state='queued'",
            [&thread_id], |r| r.get(0),
        )?;
        if queued as usize >= CHAT_QUEUE_MAX {
            return Err(ChatError::QueueFull {
                limit: CHAT_QUEUE_MAX,
            });
        }
        let seq: i64 = tx.query_row(
            "SELECT next_seq FROM chat_threads WHERE id=?1",
            [&thread_id],
            |r| r.get(0),
        )?;
        let id = Ulid::new().to_string();
        let metadata = serde_json::json!({"mode":"queue", "interrupt":false,
            "attachment_ids":[], "legacy_source":{"message_id":legacy_id,
            "node_id":message.node_id,"task_id":message.task_id.map(|id| id.to_string()),
            "metadata":message.metadata}});
        tx.execute(
            "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,legacy_message_id,metadata_json,created_at,updated_at) \
             VALUES(?1,?2,?3,'user',?4,'queued',?5,?6,?7,?7)",
            params![id, thread_id, seq, message.text, legacy_id, metadata.to_string(), at],
        )?;
        tx.execute(
            "UPDATE chat_threads SET next_seq=next_seq+1,updated_at=?2 WHERE id=?1",
            params![thread_id, at],
        )?;
        let queued_message = message_require(&tx, &thread_id, &id)?;
        emit_message(&tx, &queued_message, &at)?;
        emit_queue(&tx, &thread_id, &at)?;
        tx.commit()?;
        Ok(id)
    }

    /// Called after schema migration on the daemon's normal writable open path.
    /// The rowid watermark and all projected rows commit together. The anti-join is
    /// authoritative because SQLite may renumber implicit rowids during VACUUM.
    pub(crate) fn backfill_legacy_cos(conn: &mut Connection) -> Result<(), StoreError> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let watermark: i64 = tx
            .query_row(
                "SELECT value FROM feed_cursor WHERE name=?1",
                [CURSOR],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|raw| {
                raw.parse::<i64>()
                    .map_err(|_| StoreError::Invalid("invalid CoS legacy watermark".into()))
            })
            .transpose()?
            .unwrap_or(0);
        let mut query = tx.prepare(
            "SELECT m.rowid,m.id,m.project_id,m.role,m.text,m.run_id,m.task_id,m.metadata_json,m.created_at \
             FROM messages m WHERE m.node_id='cos' AND NOT EXISTS \
             (SELECT 1 FROM chat_messages c WHERE c.legacy_message_id=m.id) \
             ORDER BY m.created_at,m.id",
        )?;
        let messages = query
            .query_map([], |row| {
                Ok(OldMessage {
                    rowid: row.get(0)?,
                    id: row.get(1)?,
                    project_id: row.get(2)?,
                    role: row.get(3)?,
                    text: row.get(4)?,
                    run_id: row.get(5)?,
                    task_id: row.get(6)?,
                    metadata_json: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(query);

        let mut highest = watermark;
        for old in messages {
            highest = highest.max(old.rowid);
            let role = match old.role.as_str() {
                "user" => "user",
                "node" => "assistant",
                other => {
                    return Err(StoreError::Invalid(format!(
                        "invalid CoS legacy message role: {other}"
                    )));
                }
            };
            let thread_id = backfill_thread(&tx, old.project_id.as_deref(), &old.created_at)?;
            let source_metadata = old
                .metadata_json
                .as_deref()
                .map(|raw| serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!(raw)));
            let metadata = json!({
                "legacy_source": {
                    "message_id": old.id,
                    "node_id": "cos",
                    "task_id": old.task_id,
                    "metadata": source_metadata,
                }
            });
            let seq: i64 = tx.query_row(
                "SELECT next_seq FROM chat_threads WHERE id=?1",
                [&thread_id],
                |row| row.get(0),
            )?;
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO chat_messages \
                 (id,thread_id,seq,role,text,state,run_id,legacy_message_id,metadata_json,created_at,updated_at) \
                 VALUES (?1,?2,?3,?4,?5,'completed',?6,?7,?8,?9,?9)",
                params![format!("legacy:message:{}", old.id), thread_id, seq, role,
                    old.text, old.run_id, old.id, metadata.to_string(), old.created_at],
            )?;
            if inserted == 1 {
                tx.execute(
                    "UPDATE chat_threads SET next_seq=next_seq+1,updated_at=?2 WHERE id=?1",
                    params![thread_id, old.created_at],
                )?;
            }
        }
        crate::feed::feed_cursor_put(&tx, CURSOR, &highest.to_string())?;
        let retired_at = format_rfc3339(OffsetDateTime::now_utc())?;
        tx.execute(
            "UPDATE node_sessions SET retired_at=?1 WHERE node_id='cos' \
             AND kind='conversation' AND retired_at IS NULL",
            [retired_at],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// Where a projected pre-chat Console row goes: the legacy thread of its scope when migration
/// 0092 kept one, otherwise the fixed general or project CoS thread. Legacy threads are never
/// created here (ADR 2026-10-22-cos-fixed-threads D1).
fn backfill_thread(
    tx: &rusqlite::Transaction<'_>,
    project_id: Option<&str>,
    at: &str,
) -> Result<String, StoreError> {
    let legacy_id = match project_id {
        Some(id) => format!("legacy:project:{id}"),
        None => "legacy:global".to_owned(),
    };
    let kept: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_threads WHERE id=?1 AND kind='legacy')",
        [&legacy_id],
        |r| r.get(0),
    )?;
    if kept {
        return Ok(legacy_id);
    }
    cos_thread_ensure_tx(tx, project_id, at).map_err(|e| StoreError::Invalid(e.to_string()))
}
