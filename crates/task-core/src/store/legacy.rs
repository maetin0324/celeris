//! One-time, resumable projection of the old CoS Console into legacy chat threads.
//! The old messages remain available to the Console compatibility API.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use time::OffsetDateTime;
use ulid::Ulid;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::chat::store::{
    chat_ts, emit_message, emit_queue, immediate, message_require, read_tx, writer,
};
use crate::chat::{CHAT_MESSAGE_TEXT_MAX_BYTES, CHAT_QUEUE_MAX, ChatError};
use crate::{Message, MessageId, MessageRole, ProjectId, TaskId};

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
    /// MCP console_reply resolves its reserved task id through the chat run.
    pub fn chat_legacy_reply(
        &self,
        task_id: TaskId,
    ) -> Result<Option<(String, Option<String>)>, ChatError> {
        read_tx(self, |conn| {
            conn.query_row(
                "SELECT COALESCE(r.state,'queued'),o.text FROM messages m \
             JOIN chat_messages i ON i.legacy_message_id=m.id \
             LEFT JOIN chat_runs r ON r.input_message_id=i.id \
             LEFT JOIN chat_messages o ON o.id=r.output_message_id \
             WHERE m.task_id=?1 AND m.role='user' AND m.node_id='cos' \
             ORDER BY r.started_at DESC LIMIT 1",
                [task_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(ChatError::from)
        })
    }

    /// New CoS chat rows visible in the old Console. Projected legacy rows are
    /// already present in `messages`; run id also suppresses a duplicate reply.
    pub fn chat_legacy_console_messages(
        &self,
        project_id: Option<ProjectId>,
        node_id: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>, ChatError> {
        if node_id.is_some_and(|id| id != crate::COS_ID) {
            return Ok(Vec::new());
        }
        read_tx(self, |conn| {
            let order = if after.is_some() { "ASC" } else { "DESC" };
            let mut stmt = conn.prepare(&format!(
            "SELECT c.id,t.project_id,c.role,c.text,c.run_id,c.metadata_json, \
             CASE WHEN c.role='assistant' THEN c.updated_at ELSE c.created_at END, \
             COALESCE(json_extract(c.metadata_json,'$.legacy_source.task_id'), \
                      json_extract(input.metadata_json,'$.legacy_source.task_id')) \
             FROM chat_messages c JOIN chat_threads t ON t.id=c.thread_id \
             LEFT JOIN chat_messages input ON input.id=c.reply_to_id \
             WHERE c.legacy_message_id IS NULL AND c.role IN ('user','assistant') \
             AND (c.role='user' AND c.state!='cancelled' OR c.role='assistant' AND c.state='completed') \
             AND (?1 IS NULL OR t.project_id=?1) \
             AND (?2 IS NULL OR CASE WHEN c.role='assistant' THEN c.updated_at ELSE c.created_at END>=?2) \
             AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.node_id='cos' \
               AND m.project_id IS t.project_id AND m.run_id=c.run_id \
               AND c.role='assistant') \
             ORDER BY 7 {order},c.id {order} LIMIT ?3",
        ))?;
            let raw = stmt
                .query_map(
                    params![project_id.map(|p| p.to_string()), after, limit as i64],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, Option<String>>(7)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter()
                .map(
                    |(id, project, role, text, run_id, metadata, created, task)| {
                        let meta: serde_json::Value = serde_json::from_str(&metadata)?;
                        let task_id = task.and_then(|s| s.parse::<TaskId>().ok());
                        let author = meta
                            .pointer("/legacy_source/metadata/author")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        Ok(Message {
                            id: id.parse::<MessageId>().map_err(|_| {
                                ChatError::Invalid("invalid chat message id".into())
                            })?,
                            node_id: crate::COS_ID.to_owned(),
                            project_id: project.and_then(|p| p.parse::<ProjectId>().ok()),
                            role: if role == "user" {
                                MessageRole::User
                            } else {
                                MessageRole::Node
                            },
                            text,
                            run_id,
                            task_id,
                            metadata: author.map(|author| crate::MessageMetadata {
                                author: Some(author),
                                ..Default::default()
                            }),
                            created_at: OffsetDateTime::parse(
                                &created,
                                &time::format_description::well_known::Rfc3339,
                            )
                            .map_err(|e| ChatError::Invalid(e.to_string()))?,
                        })
                    },
                )
                .collect()
        })
    }

    /// The Console's default is scoped independently of the human chat UI.
    pub fn chat_legacy_default_thread(
        &self,
        project_id: Option<crate::ProjectId>,
        now: OffsetDateTime,
    ) -> Result<String, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let id = legacy_thread(&tx, project_id, &chat_ts(now), false)?;
        tx.commit()?;
        Ok(id)
    }

    /// Start a fresh compatibility conversation without archiving other threads.
    pub fn chat_legacy_new_conversation(
        &self,
        project_id: Option<crate::ProjectId>,
        now: OffsetDateTime,
    ) -> Result<String, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let id = chat_legacy_new_conversation_tx(&tx, project_id, now)?;
        tx.commit()?;
        Ok(id)
    }

    /// Queue a legacy input once, keyed by its old message id. The old message
    /// remains readable by /console and the queue drives the CoS run.
    pub fn chat_legacy_enqueue(
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
        let thread_id = legacy_thread(&tx, message.project_id, &at, false)?;
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

fn legacy_thread(
    tx: &rusqlite::Transaction<'_>,
    project_id: Option<crate::ProjectId>,
    at: &str,
    rotate: bool,
) -> Result<String, ChatError> {
    if let Some(project_id) = project_id {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [project_id.to_string()],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(ChatError::Invalid(format!(
                "project {project_id} does not exist"
            )));
        }
    }
    let scope = project_id.map_or_else(|| "global".to_owned(), |id| format!("project:{id}"));
    let key = format!("cos_chat_legacy_default:{scope}");
    let current: Option<String> = tx
        .query_row("SELECT value FROM feed_cursor WHERE name=?1", [&key], |r| {
            r.get(0)
        })
        .optional()?;
    let mut id = if rotate {
        format!("legacy:{}", Ulid::new())
    } else {
        current.unwrap_or_else(|| format!("legacy:{scope}"))
    };
    let archived: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_threads WHERE id=?1 AND status='archived')",
        [&id],
        |r| r.get(0),
    )?;
    if archived {
        id = format!("legacy:{}", Ulid::new());
    }
    tx.execute(
        "INSERT OR IGNORE INTO chat_threads(id,kind,title,project_id,status,created_at,updated_at) \
         VALUES(?1,'legacy','旧 CoS 会話',?2,'open',?3,?3)",
        params![id, project_id.map(|p| p.to_string()), at],
    )?;
    tx.execute(
        "INSERT INTO feed_cursor(name,value) VALUES(?1,?2) \
         ON CONFLICT(name) DO UPDATE SET value=excluded.value",
        params![key, id],
    )?;
    Ok(id)
}

/// [`SqliteStore::chat_legacy_new_conversation`] inside the caller's transaction.
pub fn chat_legacy_new_conversation_tx(
    tx: &rusqlite::Transaction<'_>,
    project_id: Option<crate::ProjectId>,
    now: OffsetDateTime,
) -> Result<String, ChatError> {
    legacy_thread(tx, project_id, &chat_ts(now), true)
}
