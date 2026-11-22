//! ADR 2026-11-21-chat-discussion-mode D6: reads the facts about a cited message as it is now
//! ([`CitedMessage`]: its thread's project, revision, body sha256, retracted) for the pure checks of
//! [`crate::chat_citation`]. Read on every check and never stored, so a citation's state follows
//! the message's revision (`metadata_json.revision`, D5.2). Kept outside [`crate::chat`] (repo
//! health `size.responsibility-growth`); it depends on `chat` one way.

use crate::chat::store::{ChatError, read_tx};
use crate::chat_citation::CitedMessage;
use crate::store::SqliteStore;

impl SqliteStore {
    /// The message `message_id` of thread `thread_id` as it is now, or `None` if the thread has
    /// no such message ([`crate::chat_citation::cited_message_conn`] in its own read transaction;
    /// the strategy gate of `task.create` and the file check of the run-end attach).
    pub fn chat_cited_message(
        &self,
        thread_id: &str,
        message_id: &str,
    ) -> Result<Option<CitedMessage>, ChatError> {
        read_tx(self, |conn| {
            Ok(crate::chat_citation::cited_message_conn(
                conn, thread_id, message_id,
            )?)
        })
    }
}

#[cfg(test)]
#[path = "discussion_citation_tests.rs"]
mod tests;
