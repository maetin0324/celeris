//! ADR 2026-11-23-project-observability-publish D2.1〜D2.3: a person adopts, rejects or comments on a chat
//! proposal (`POST /chat/threads/{t}/messages/{m}/review`). The review is one new message of the same thread,
//! written in the person's post transaction (`chat_message_post_as_also`) with `metadata_json.proposal_review`.
//! No table, column, migration or `chat_events` type is added, and nothing is stored about the proposal itself:
//! its state is derived on every read ([`crate::proposal_review_wire`]).
//!
//! Outside [`crate::chat`] like [`crate::chat_revision`] (the `chat` module sits at repo health's
//! `size.responsibility-growth` limit). It uses `chat` and the leaf; `chat` uses only the leaf.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;

use crate::chat::store::ChatError;
use crate::chat::{ChatDiscussionMode, ChatMessagePosted, ChatPostMessageRequest, ChatSendMode};
use crate::chat_citation::{ChatCitation, body_sha256, citation_shape_ok};
use crate::proposal_review_wire::{
    METADATA_KEY, ProposalKind, ProposalReviewRecord, ProposalVerdict, attachment_sha256,
    strategy_drafts,
};
use crate::store::SqliteStore;

pub use crate::proposal_review_wire::{
    ChatMessageExternal, ChatMessageProposal, ChatMessageProposalMarks, ProposalReviewState,
};

/// D2.1: the longest `note`, in characters.
pub const NOTE_MAX_CHARS: usize = 2_000;

/// D2.1: `POST /chat/threads/{t}/messages/{m}/review`. `expected_revision` and `sha256` are the proposal as the
/// person saw it; for a CoS draft `attachment` names it and `sha256` is the attachment's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalReviewRequest {
    pub verdict: ProposalVerdict,
    pub expected_revision: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<String>,
    /// Up to 2,000 characters; required (1 or more) for `comment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Idempotency (the chat key): the same key and text return the review already posted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_message_id: Option<String>,
}

/// Why a review is refused. `code()` is the problem code.
#[derive(Debug, thiserror::Error)]
pub enum ProposalReviewError {
    /// 422 `proposal_review_not_a_proposal`.
    #[error("not a proposal: {0}")]
    NotAProposal(String),
    /// 409 `proposal_review_stale`: another revision, body or attachment than the person saw, or retracted.
    #[error("stale: {0}")]
    Stale(String),
    /// 422 `validation`: the note or the shape of the request.
    #[error("invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Chat(#[from] ChatError),
}

impl From<rusqlite::Error> for ProposalReviewError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Chat(e.into())
    }
}

impl ProposalReviewError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAProposal(_) => "proposal_review_not_a_proposal",
            Self::Stale(_) => "proposal_review_stale",
            Self::Invalid(_) => "validation",
            Self::Chat(_) => "chat",
        }
    }
}

/// D2.3: the fixed text of a review (the proposal's text is never copied in), then the person's note.
pub fn review_text(
    verdict: ProposalVerdict,
    target: &ChatCitation,
    attachment: Option<&str>,
    note: Option<&str>,
) -> String {
    let what = format!(
        "案 {}（rev {}、sha256:{}）",
        target.message_id, target.revision, target.sha256
    );
    let draft = attachment.and_then(crate::strategy_critic::strategy_artifact);
    let mut text = match (verdict, draft) {
        (ProposalVerdict::Adopt, Some((id, v))) => format!(
            "{what}を採る。task の成果物 `{}` を基に、決定 `{}` で人の承認を求めよ。これは承認ではない。",
            crate::strategy::artifact_name(&id, v),
            crate::strategy::approval_decision_key(v)
        ),
        (ProposalVerdict::Adopt, None) => format!(
            "{what}を採る。task の成果物 `strategy/<strategy_id>/v<n>.json` にし、決定 `strategy-approve-v<n>` で人の承認を求めよ。これは承認ではない。"
        ),
        (ProposalVerdict::Reject, _) => format!("{what}を却下する。この案を実行しない。"),
        (ProposalVerdict::Comment, _) => format!("{what}への人のコメント（議論だけ）。"),
    };
    if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
        text.push_str("\n\n");
        text.push_str(note);
    }
    text
}

fn check_note(req: &ProposalReviewRequest) -> Result<(), ProposalReviewError> {
    let chars = req.note.as_deref().map_or(0, |n| n.chars().count());
    if chars > NOTE_MAX_CHARS {
        return Err(ProposalReviewError::Invalid(format!(
            "note is {chars} characters; at most {NOTE_MAX_CHARS}"
        )));
    }
    if req.verdict == ProposalVerdict::Comment
        && req.note.as_deref().is_none_or(|n| n.trim().is_empty())
    {
        return Err(ProposalReviewError::Invalid(
            "a comment needs a note".into(),
        ));
    }
    Ok(())
}

/// D2.2: the proposal as the person saw it, checked against the message as it is now (in the write transaction).
fn check_target(
    tx: &Connection,
    thread_id: &str,
    message_id: &str,
    req: &ProposalReviewRequest,
) -> Result<(), ProposalReviewError> {
    let message = crate::chat::store::message_require(tx, thread_id, message_id)?;
    let meta: serde_json::Value = tx
        .query_row(
            "SELECT metadata_json FROM chat_messages WHERE id=?1",
            params![message_id],
            |row| row.get::<_, String>(0),
        )
        .map(|raw| serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null))?;
    let Some(kind) = message.marks.proposal.as_ref().map(|p| p.kind) else {
        return Err(ProposalReviewError::NotAProposal(format!(
            "message {message_id} is not a proposal (an external LLM's propose, a person's import or a CoS strategy draft)"
        )));
    };
    if message.edit_state.retracted.is_some() {
        return Err(ProposalReviewError::Stale(format!(
            "message {message_id} was retracted"
        )));
    }
    if message.edit_state.revision != req.expected_revision {
        return Err(ProposalReviewError::Stale(format!(
            "message {message_id} is at revision {}, not {}",
            message.edit_state.revision, req.expected_revision
        )));
    }
    let current = match (kind, req.attachment.as_deref()) {
        (ProposalKind::CosStrategy, Some(path)) => {
            let Some((_, id)) = strategy_drafts(&meta).into_iter().find(|(p, _)| p == path) else {
                return Err(ProposalReviewError::Invalid(format!(
                    "{path} is not a strategy draft attached to message {message_id}"
                )));
            };
            let sha = attachment_sha256(tx, &id)?.ok_or_else(|| {
                ProposalReviewError::Stale(format!("the attachment {path} was deleted"))
            })?;
            sha
        }
        (ProposalKind::CosStrategy, None) => {
            return Err(ProposalReviewError::Invalid(
                "a CoS proposal is reviewed by its attachment (strategy/<id>/v<n>.json)".into(),
            ));
        }
        (_, Some(_)) => {
            return Err(ProposalReviewError::Invalid(
                "attachment is only for a CoS strategy draft".into(),
            ));
        }
        (_, None) => body_sha256(&message.text),
    };
    if current != req.sha256 {
        return Err(ProposalReviewError::Stale(format!(
            "message {message_id} is not the version with sha256 {}",
            req.sha256
        )));
    }
    Ok(())
}

impl SqliteStore {
    /// D2.3: records the review of proposal `message_id` by `by` (`human:<name>`; the API checks the principal)
    /// as a new `role=user` message of the same thread. `adopt` is an `execute` instruction (not an approval);
    /// `reject` and `comment` are `discuss`. `also` runs in the same transaction (the person's `human_epoch`).
    pub fn chat_proposal_review<F>(
        &self,
        thread_id: &str,
        message_id: &str,
        by: &str,
        req: &ProposalReviewRequest,
        now: OffsetDateTime,
        also: F,
    ) -> Result<ChatMessagePosted, ProposalReviewError>
    where
        F: FnOnce(&Connection, &ChatMessagePosted) -> Result<(), ChatError>,
    {
        check_note(req)?;
        let record = ProposalReviewRecord {
            verdict: req.verdict,
            target: ChatCitation {
                thread_id: thread_id.to_owned(),
                message_id: message_id.to_owned(),
                revision: req.expected_revision,
                sha256: req.sha256.clone(),
            },
            attachment: req.attachment.clone(),
        };
        if !citation_shape_ok(&record.target) {
            return Err(ProposalReviewError::Invalid(
                "expected_revision is 1 or more and sha256 is 64 lowercase hex".into(),
            ));
        }
        let post = ChatPostMessageRequest {
            client_message_id: req
                .client_message_id
                .clone()
                .unwrap_or_else(|| format!("proposal-review:{}", ulid::Ulid::new())),
            text: review_text(
                req.verdict,
                &record.target,
                record.attachment.as_deref(),
                req.note.as_deref(),
            ),
            attachment_ids: Vec::new(),
            reply_to_id: None,
            mode: ChatSendMode::Queue,
            resume_queue: false,
            discussion_mode: match req.verdict {
                ProposalVerdict::Adopt => None,
                ProposalVerdict::Reject | ProposalVerdict::Comment => {
                    Some(ChatDiscussionMode::Discuss)
                }
            },
        };
        let stored = serde_json::to_string(&json!(record)).map_err(ChatError::from)?;
        let mut refused = None;
        let posted = self.chat_message_post_as_also(thread_id, &post, by, now, |tx, posted| {
            if posted.created {
                // D2.2: the proposal as it is now, in the write transaction (no window between check and write).
                if let Err(e) = check_target(tx, thread_id, message_id, req) {
                    let detail = e.to_string();
                    refused = Some(e);
                    return Err(ChatError::Conflict(detail));
                }
                tx.execute(
                    &format!(
                        "UPDATE chat_messages SET metadata_json=json_set(metadata_json,'$.{METADATA_KEY}',json(?2)) \
                         WHERE id=?1"
                    ),
                    params![posted.response.message.id, stored],
                )?;
            }
            also(tx, posted)
        });
        match (posted, refused) {
            (_, Some(e)) => Err(e),
            (posted, None) => Ok(posted?),
        }
    }
}

#[cfg(test)]
#[path = "proposal_review_tests.rs"]
mod tests;
