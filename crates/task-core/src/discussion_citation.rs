//! ADR 2026-11-21-chat-discussion-mode D6: reads the facts about a cited message as it is now
//! ([`CitedMessage`]: its thread's project, revision, body sha256, retracted) for the pure checks of
//! [`crate::chat_citation`]. Read on every check and never stored, so a citation's state follows
//! the message's revision (`metadata_json.revision`, D5.2). Kept outside [`crate::chat`] (repo
//! health `size.responsibility-growth`); it depends on `chat` one way.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::chat::store::{ChatError, read_tx};
use crate::chat_citation::CitedMessage;
use crate::chat_revision_wire::ChatMessageEditState;
use crate::project_proposal::sha256_hex;
use crate::store::SqliteStore;

/// The message `message_id` of thread `thread_id` as it is now, or `None` if the thread has no
/// such message. Usable inside the caller's transaction (the attach of cite-attach).
pub fn cited_message(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> Result<Option<CitedMessage>, ChatError> {
    let row = conn
        .query_row(
            "SELECT m.seq, m.text, m.metadata_json, t.project_id, t.kind \
             FROM chat_messages m JOIN chat_threads t ON t.id=m.thread_id \
             WHERE m.id=?1 AND m.thread_id=?2",
            params![message_id, thread_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((seq, text, meta, project_id, kind)) = row else {
        return Ok(None);
    };
    let meta: Value = serde_json::from_str(&meta)?;
    let state = ChatMessageEditState::of_metadata(&meta)?;
    Ok(Some(CitedMessage {
        thread_id: thread_id.to_string(),
        message_id: message_id.to_string(),
        project_id,
        general_thread: kind == "general",
        seq: u64::try_from(seq).unwrap_or_default(),
        revision: state.revision,
        sha256: sha256_hex(&text),
        retracted: state.retracted.is_some(),
    }))
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
}

#[cfg(test)]
#[path = "discussion_citation_tests.rs"]
mod tests;
