//! ADR 2026-10-08-cos-workspace-files-in-chat D1 and ADR 2026-11-21-chat-discussion-mode D3・D6:
//! the files of a CoS run's workspace pinned to its reply. The attach records the workspace paths
//! (`metadata_json.workspace_files`) and, for the summary and draft files that passed the check of
//! `task_dispatch::cos_context::discussion_files`, their citations (`metadata_json.citations`, no state) in
//! one transaction, after checking the citations again in it. The sha256s already attached under
//! a path are the version-conflict input of a draft (D3).
//!
//! Kept outside [`crate::chat`] (repo health `size.responsibility-growth`); it depends on `chat`
//! and the leaf [`crate::chat_citation`] one way.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::chat::store::{ChatError, chat_ts, immediate, message_require, read_tx, writer};
use crate::chat::{ChatMessage, ChatMessageRole, ChatWorkspaceFile};
use crate::chat_citation::{ChatFileCitation, cited_message_conn, verify_citation};
use crate::store::SqliteStore;

impl SqliteStore {
    /// ADR 2026-10-08-cos-workspace-files-in-chat D1: pins workspace files (already uploaded as
    /// attachments of the same thread) to an assistant reply and records their workspace paths in
    /// `metadata_json.workspace_files`, in one transaction. Repeating the same files is idempotent.
    /// No event is emitted here: the run's terminal `message` event carries the result.
    pub fn chat_message_attach_workspace_files(
        &self,
        thread_id: &str,
        message_id: &str,
        files: &[ChatWorkspaceFile],
        citations: &[ChatFileCitation],
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let message = message_require(&tx, thread_id, message_id)?;
        // ADR 2026-11-21-chat-discussion-mode D6: the citations are checked again in this
        // transaction (an edit between the file check and here makes the attach a conflict).
        for f in citations {
            let c = &f.citation;
            let now_msg = cited_message_conn(&tx, &c.thread_id, &c.message_id)?;
            if let Err(refusal) = verify_citation(thread_id, c, now_msg.as_ref()) {
                let code = refusal.code();
                return Err(ChatError::Conflict(format!("{code}: {}", f.path)));
            }
        }
        if message.role != ChatMessageRole::Assistant {
            return Err(ChatError::Invalid(format!(
                "message {message_id} is not an assistant reply"
            )));
        }
        let mut seen = HashSet::new();
        for file in files {
            if file.path.is_empty() || !seen.insert(file.attachment_id.as_str()) {
                return Err(ChatError::Invalid(
                    "duplicate or empty workspace file".into(),
                ));
            }
            let owner: Option<String> = tx
                .query_row(
                    "SELECT thread_id FROM chat_attachments WHERE id=?1 AND state='ready'",
                    [&file.attachment_id],
                    |r| r.get(0),
                )
                .optional()?;
            match owner {
                Some(owner) if owner == thread_id => {}
                Some(_) => {
                    return Err(ChatError::Conflict(format!(
                        "attachment {} belongs to another thread",
                        file.attachment_id
                    )));
                }
                None => return Err(ChatError::not_found("attachment", &file.attachment_id)),
            }
            crate::chat::attachments::add_ref_tx(
                &tx,
                &file.attachment_id,
                "message",
                message_id,
                now,
            )
            .map_err(|e| ChatError::Invalid(e.to_string()))?;
        }
        let raw: String = tx.query_row(
            "SELECT metadata_json FROM chat_messages WHERE id=?1",
            [message_id],
            |r| r.get(0),
        )?;
        let mut meta: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
        if !meta.is_object() {
            meta = json!({});
        }
        meta["workspace_files"] = serde_json::to_value(files)?;
        if !citations.is_empty() {
            meta["citations"] = serde_json::to_value(citations)?;
        }
        tx.execute(
            "UPDATE chat_messages SET metadata_json=?2,updated_at=?3 WHERE id=?1",
            params![message_id, serde_json::to_string(&meta)?, chat_ts(now)],
        )?;
        let message = message_require(&tx, thread_id, message_id)?;
        tx.commit()?;
        Ok(message)
    }

    /// D3: the sha256 of every file already attached in `thread_id` under the workspace path
    /// `path` (`ChatMessage.workspace_files`). A draft `strategy/<id>/v<n>.json` with another
    /// sha256 than these is a version conflict.
    pub fn chat_workspace_file_sha256s(
        &self,
        thread_id: &str,
        path: &str,
    ) -> Result<Vec<String>, ChatError> {
        read_tx(self, |conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT a.sha256 FROM chat_messages m, \
                 json_each(m.metadata_json, '$.workspace_files') f \
                 JOIN chat_attachments a ON a.id = json_extract(f.value, '$.attachment_id') \
                 WHERE m.thread_id=?1 AND json_extract(f.value, '$.path')=?2 \
                 AND json_valid(m.metadata_json) ORDER BY m.seq",
            )?;
            let rows = stmt
                .query_map(params![thread_id, path], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
    }
}
