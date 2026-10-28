//! ADR 2026-10-28-cos-review-back-thread: the general CoS's back thread for the periodic review.
//!
//! D1: one thread of kind `review` (no project), opened on first use and hidden from the person's
//! lists. Its only input is the cron's review input ([`SqliteStore::cos_review_input_post`]).
//! D4': the review run's report to the person is one assistant message of the general thread per
//! review input ([`cos_review_report_tx`]), written inside the caller's CoS operation transaction.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use time::OffsetDateTime;
use ulid::Ulid;

use super::store::{
    MESSAGE_SELECT, MessageProvenance, NewMessage, chat_ts, emit_message, emit_thread, immediate,
    insert_message, message_from_raw, message_get_conn, message_post_with, message_raw,
    message_require, read_tx, thread_get_conn, thread_require, writer,
};
use super::*;
use crate::store::SqliteStore;

/// D1: the title of the review thread.
pub const COS_REVIEW_THREAD_TITLE: &str = "統括 CoS（定期見直し）";
/// ADR 2026-10-27-cron-cos-review D3: the `client_message_id` prefix of a review input.
pub const COS_REVIEW_INPUT_KEY_PREFIX: &str = "cron-cos-review:";
/// D4': the `client_message_id` prefix of a report (`cos-review-report:<review input id>`).
pub const COS_REVIEW_REPORT_KEY_PREFIX: &str = "cos-review-report:";
/// D4': report text limit (characters).
pub const COS_REVIEW_REPORT_TEXT_MAX_CHARS: usize = 4000;

/// [`cos_review_report_tx`] result: `created=false` means the review input already had its
/// report, which is returned unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatReviewReported {
    pub message: ChatMessage,
    pub created: bool,
}

/// D1: the one review thread, inserted if missing. Returns its id and whether it was created.
pub fn cos_review_thread_ensure_tx(tx: &Connection, at: &str) -> Result<(String, bool), ChatError> {
    let found: Option<String> = tx
        .query_row(
            "SELECT id FROM chat_threads WHERE kind='review'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = found {
        return Ok((id, false));
    }
    let id = Ulid::new().to_string();
    tx.execute(
        "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) \
         VALUES(?1,'review',?2,'open',?3,?3)",
        params![id, COS_REVIEW_THREAD_TITLE, at],
    )?;
    emit_thread(tx, &id, at)?;
    Ok((id, true))
}

impl SqliteStore {
    /// D1 `cos_review_thread_open`: the review thread, created on first use (idempotent).
    pub fn cos_review_thread_open(
        &self,
        now: OffsetDateTime,
    ) -> Result<ChatThreadCreated, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let (id, created) = cos_review_thread_ensure_tx(&tx, &chat_ts(now))?;
        let thread = thread_require(&tx, &id)?;
        tx.commit()?;
        Ok(ChatThreadCreated { thread, created })
    }

    /// D6 `GET /cos/review-thread`: the review thread if it exists (never creates it).
    pub fn cos_review_thread_get(&self) -> Result<Option<ChatThread>, ChatError> {
        read_tx(self, |conn| {
            let id: Option<String> = conn
                .query_row(
                    "SELECT id FROM chat_threads WHERE kind='review'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            match id {
                Some(id) => thread_get_conn(conn, &id),
                None => Ok(None),
            }
        })
    }

    /// D2: the cron's review input, queued (`mode = queue`) in the review thread (created on
    /// first use). The only post the review thread accepts. `client_message_id` must start with
    /// [`COS_REVIEW_INPUT_KEY_PREFIX`]; a replay of the same key returns the same message.
    pub fn cos_review_input_post(
        &self,
        client_message_id: &str,
        text: &str,
        now: OffsetDateTime,
    ) -> Result<ChatMessagePosted, ChatError> {
        if !client_message_id.starts_with(COS_REVIEW_INPUT_KEY_PREFIX) {
            return Err(ChatError::Invalid(format!(
                "a review input key starts with {COS_REVIEW_INPUT_KEY_PREFIX}"
            )));
        }
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let (id, _) = cos_review_thread_ensure_tx(&tx, &chat_ts(now))?;
        let posted = message_post_with(
            &tx,
            &id,
            &ChatPostMessageRequest {
                client_message_id: client_message_id.to_owned(),
                text: text.to_owned(),
                attachment_ids: Vec::new(),
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            &MessageProvenance {
                review_input: true,
                ..MessageProvenance::default()
            },
            now,
        )?;
        tx.commit()?;
        Ok(posted)
    }
}

/// D4' `chat.review_report`: write the review run's report as one completed assistant message of
/// the general thread, inside the caller's transaction (the CoS operation transaction that also
/// writes the audit row). The run's identity (a run of the review thread) is checked by the
/// caller; `run_id` is recorded in the provenance.
///
/// Refused as `Invalid` (422): `review_message_id` is not a `role=user` message of the review
/// thread whose key starts with [`COS_REVIEW_INPUT_KEY_PREFIX`], or `text` is blank or longer
/// than [`COS_REVIEW_REPORT_TEXT_MAX_CHARS`]. One report per review input: a second call for the
/// same input returns the first report (`created=false`) and writes nothing. The message is not a
/// queued input, so no general CoS run starts; it counts as unread in the general thread.
pub fn cos_review_report_tx(
    tx: &Connection,
    review_message_id: &str,
    text: &str,
    run_id: Option<&str>,
    operation_id: &str,
    now: OffsetDateTime,
) -> Result<ChatReviewReported, ChatError> {
    let review_thread: Option<String> = tx
        .query_row(
            "SELECT id FROM chat_threads WHERE kind='review'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(review_thread) = review_thread else {
        return Err(ChatError::Invalid("there is no review thread".into()));
    };
    let input = message_get_conn(tx, &review_thread, review_message_id)?.ok_or_else(|| {
        ChatError::Invalid(format!(
            "message {review_message_id} is not a message of the review thread"
        ))
    })?;
    let is_review_input = input.role == ChatMessageRole::User
        && input
            .client_message_id
            .as_deref()
            .is_some_and(|key| key.starts_with(COS_REVIEW_INPUT_KEY_PREFIX));
    if !is_review_input {
        return Err(ChatError::Invalid(format!(
            "message {review_message_id} is not a review input"
        )));
    }
    let at = chat_ts(now);
    let general = cos_thread_ensure_tx(tx, None, &at)?;
    let key = format!("{COS_REVIEW_REPORT_KEY_PREFIX}{review_message_id}");
    let existing = tx
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE thread_id=?1 AND client_message_id=?2"),
            params![general, key],
            message_raw,
        )
        .optional()?;
    if let Some(raw) = existing {
        return Ok(ChatReviewReported {
            message: message_from_raw(tx, raw)?,
            created: false,
        });
    }
    let chars = text.chars().count();
    if text.trim().is_empty() || chars > COS_REVIEW_REPORT_TEXT_MAX_CHARS {
        return Err(ChatError::Invalid(format!(
            "report text must be 1 to {COS_REVIEW_REPORT_TEXT_MAX_CHARS} characters (got {chars})"
        )));
    }
    let provenance = ChatProvenance {
        kind: ChatProvenanceKind::CosReviewReport,
        review_thread_id: review_thread,
        review_message_id: review_message_id.to_owned(),
        run_id: run_id.map(str::to_owned),
    };
    let meta = json!({
        "posted_by": "cos",
        "operation_id": operation_id,
        "provenance": provenance,
    });
    let id = insert_message(
        tx,
        NewMessage {
            thread_id: &general,
            role: ChatMessageRole::Assistant,
            text,
            state: ChatMessageState::Completed,
            client_message_id: Some(&key),
            reply_to_id: None,
            run_id: None,
            metadata: &meta,
        },
        &at,
    )?;
    let message = message_require(tx, &general, &id)?;
    emit_message(tx, &message, &at)?;
    Ok(ChatReviewReported {
        message,
        created: true,
    })
}
