//! One-time, resumable projection of the old CoS Console into legacy chat threads.
//! The old messages remain available to the Console compatibility API.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};

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
            let thread_id = match &old.project_id {
                Some(id) => format!("legacy:project:{id}"),
                None => "legacy:global".to_owned(),
            };
            tx.execute(
                "INSERT OR IGNORE INTO chat_threads \
                 (id,kind,title,project_id,status,created_at,updated_at) \
                 VALUES (?1,'legacy','旧 CoS 会話',?2,'open',?3,?3)",
                params![thread_id, old.project_id, old.created_at],
            )?;
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
        tx.execute(
            "INSERT INTO feed_cursor(name,value) VALUES(?1,?2) \
             ON CONFLICT(name) DO UPDATE SET value=excluded.value",
            params![CURSOR, highest.to_string()],
        )?;
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
