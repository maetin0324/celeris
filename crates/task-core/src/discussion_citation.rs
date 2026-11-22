//! ADR 2026-11-21-chat-discussion-mode D6: reads the facts about a cited message as it is now
//! ([`CitedMessage`]: its thread's project, revision, body sha256, retracted) for the pure checks of
//! [`crate::chat_citation`]. Read on every check and never stored, so a citation's state follows
//! the message's revision (`metadata_json.revision`, D5.2). Kept outside [`crate::chat`] (repo
//! health `size.responsibility-growth`); it depends on `chat` one way.

use rusqlite::Connection;

use crate::chat::store::{ChatError, read_tx};
use crate::chat_citation::CitedMessage;
use crate::store::SqliteStore;

/// The message `message_id` of thread `thread_id` as it is now, or `None` if the thread has no
/// such message. Usable inside the caller's transaction (the attach of cite-attach).
pub fn cited_message(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> Result<Option<CitedMessage>, ChatError> {
    Ok(crate::chat_citation::cited_message_conn(
        conn, thread_id, message_id,
    )?)
}

impl SqliteStore {
    /// [`cited_message`] in its own read transaction (the strategy gate of `task.create`).
    pub fn chat_cited_message(
        &self,
        thread_id: &str,
        message_id: &str,
    ) -> Result<Option<CitedMessage>, ChatError> {
        read_tx(self, |conn| cited_message(conn, thread_id, message_id))
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
                .query_map(rusqlite::params![thread_id, path], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
    }
}

#[cfg(test)]
#[path = "discussion_citation_tests.rs"]
mod tests;
