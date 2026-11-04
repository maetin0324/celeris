//! ADR 2026-11-04-project-controller D12: 人の発言と `human_epoch` を同じ transaction で書く口
//! （`chat/store.rs` は source-size の上限にあるので分けた）。

use rusqlite::Connection;
use time::OffsetDateTime;

use super::store::{
    ChatError, ChatMessagePosted, MessageProvenance, immediate, message_post_with, relay_record,
    thread_require, writer,
};
use super::{ChatPostMessageRequest, ChatRelayRequest, ChatSendMode, ChatThreadKind};
use crate::store::SqliteStore;

/// ADR 2026-11-04-project-controller D12: the project whose `human_epoch` this post counts for. Only
/// a new message (`created`) in the project's fixed thread counts; a repeated `client_message_id`
/// and the general and review threads do not.
pub fn chat_posted_project_tx(
    tx: &Connection,
    thread_id: &str,
    posted: &ChatMessagePosted,
) -> Result<Option<String>, ChatError> {
    if !posted.created {
        return Ok(None);
    }
    let thread = thread_require(tx, thread_id)?;
    Ok(match thread.kind {
        ChatThreadKind::Project => thread.project_id,
        _ => None,
    })
}

impl SqliteStore {
    /// [`Self::chat_message_post_as`] に `also` を同じ transaction で足す（ADR 2026-11-04-project-controller
    /// D12 `human_epoch`）。`also` が失敗すれば発言も残らない。
    pub fn chat_message_post_as_also<F>(
        &self,
        thread_id: &str,
        req: &ChatPostMessageRequest,
        author: &str,
        now: OffsetDateTime,
        also: F,
    ) -> Result<ChatMessagePosted, ChatError>
    where
        F: FnOnce(&Connection, &ChatMessagePosted) -> Result<(), ChatError>,
    {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let provenance = MessageProvenance {
            author: Some(author.to_owned()),
            ..MessageProvenance::default()
        };
        let posted = message_post_with(&tx, thread_id, req, &provenance, now)?;
        also(&tx, &posted)?;
        tx.commit()?;
        Ok(posted)
    }

    /// [`Self::chat_relay_post`] に `also` を同じ transaction で足す（D12 `human_epoch`）。
    pub fn chat_relay_post_also<F>(
        &self,
        thread_id: &str,
        req: &ChatRelayRequest,
        relayed_by: &str,
        now: OffsetDateTime,
        also: F,
    ) -> Result<ChatMessagePosted, ChatError>
    where
        F: FnOnce(&Connection, &ChatMessagePosted) -> Result<(), ChatError>,
    {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let relay = relay_record(&tx, thread_id, req, relayed_by, now)?;
        let posted = message_post_with(
            &tx,
            thread_id,
            &ChatPostMessageRequest {
                client_message_id: req.client_message_id.clone(),
                text: req.text.clone(),
                attachment_ids: Vec::new(),
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            &MessageProvenance {
                author: Some(relayed_by.to_owned()),
                relay: Some(relay),
                mcp_receipt: false,
                review_input: false,
            },
            now,
        )?;
        also(&tx, &posted)?;
        tx.commit()?;
        Ok(posted)
    }
}
