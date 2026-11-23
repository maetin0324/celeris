//! ADR 2026-11-23-project-observability-publish D2.2・D2.4: the wire marks of a chat message that is a proposal
//! (`proposal`: its kind and the review state derived from the person's review messages) or an external text
//! (`external`: `untrusted`), and the record a review message keeps in `metadata_json.proposal_review`.
//!
//! A leaf like [`crate::chat_citation`]: it depends on nothing in this crate but the leaves `chat_citation`,
//! `chat_revision_wire` and the pure name parser [`crate::strategy_ref::strategy_artifact`], so [`crate::chat`]
//! builds the wire fields from it without a cycle and without growing `chat` (repo health
//! `size.responsibility-growth`). The write is [`crate::proposal_review`]. Nothing here is stored: the state is
//! derived on every read from the review messages of the same thread (規約 4).

use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::chat_citation::{
    ChatCitation, ChatCitationState, CitedMessage, body_sha256, citation_state,
};
use crate::chat_revision_wire::ChatMessageEditState;

/// D2.3: the key of a review message's record in `metadata_json`.
pub const METADATA_KEY: &str = "proposal_review";

/// D2.1: what the person decided about a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalVerdict {
    Adopt,
    Reject,
    Comment,
}

/// D2.2: the three kinds of message a person may review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// An external LLM's propose (`role=user`, `author = mcp:<client_id>`, `metadata_json.exchange`).
    ExternalLlm,
    /// A summary or quote a person imported (`exchange.source.kind` is `human_paste` / `markdown_import`).
    HumanImport,
    /// A CoS reply with a strategy draft attached (`strategy/<id>/v<n>.json`).
    CosStrategy,
}

/// D2.4: the review state of a proposal, derived on every read (never stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalReviewState {
    /// No person has reviewed it.
    Pending,
    /// The last adopt / reject of a person was adopt. This is not an approval (子 2 D2.2).
    Adopted,
    Rejected,
    /// Only comments.
    Commented,
    /// The proposal was edited or retracted after the revision the person reviewed.
    Stale,
}

/// D2.3: `metadata_json.proposal_review` of a review message, written by Rust from the checked request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalReviewRecord {
    pub verdict: ProposalVerdict,
    /// The proposal message at the revision the person saw; `sha256` is the body's, or the attachment's.
    pub target: ChatCitation,
    /// The strategy draft (`strategy/<id>/v<n>.json`) of a CoS proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<String>,
}

/// D2.4: `ChatMessage.proposal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageProposal {
    pub kind: ProposalKind,
    pub review_state: ProposalReviewState,
    /// The review message the state comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_message_id: Option<String>,
    /// `human:<name>` who wrote that review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_at: Option<String>,
}

/// D2.4: `ChatMessage.external`, the mark of a text that came from outside Celeris. `untrusted` is always `true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageExternal {
    /// `exchange.source.kind`: `mcp_client`, `human_paste` or `markdown_import`.
    pub source_kind: String,
    pub source_label: String,
    pub untrusted: bool,
}

/// The two wire fields, flattened into `ChatMessage` (boxed there, like the edit state).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessageProposalMarks {
    /// D2.4: present on a message a person may adopt, reject or comment on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ChatMessageProposal>,
    /// D2.4: present on a text from an external LLM or imported by a person (untrusted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ChatMessageExternal>,
}

const EXTERNAL_KINDS: [&str; 3] = ["mcp_client", "human_paste", "markdown_import"];

fn exchange(meta: &Value) -> Option<&Value> {
    meta.get("exchange").filter(|v| v.is_object())
}

fn source_kind(meta: &Value) -> Option<&str> {
    exchange(meta)?.pointer("/source/kind")?.as_str()
}

/// The strategy drafts (`strategy/<id>/v<n>.json`) in `metadata_json.workspace_files`: `(path, attachment_id)`.
pub fn strategy_drafts(meta: &Value) -> Vec<(String, String)> {
    let Some(files) = meta.get("workspace_files").and_then(Value::as_array) else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|f| {
            let path = f.get("path")?.as_str()?;
            let attachment = f.get("attachment_id")?.as_str()?;
            crate::strategy_ref::strategy_artifact(path)?;
            Some((path.to_owned(), attachment.to_owned()))
        })
        .collect()
}

/// D2.2: the kind of proposal a message is (`None`: not a proposal; the general CoS's `cos_proposal` is not one).
pub fn proposal_kind(role: &str, author: Option<&str>, meta: &Value) -> Option<ProposalKind> {
    match (role, source_kind(meta)) {
        ("user", Some("human_paste" | "markdown_import")) => Some(ProposalKind::HumanImport),
        ("user", Some(_)) if author.is_some_and(|a| a.starts_with("mcp:")) => {
            Some(ProposalKind::ExternalLlm)
        }
        ("assistant", None) if !strategy_drafts(meta).is_empty() => Some(ProposalKind::CosStrategy),
        _ => None,
    }
}

/// D2.4: the external mark of a message (from `metadata_json.exchange` only; the text never sets it).
pub fn external_mark(meta: &Value) -> Option<ChatMessageExternal> {
    let kind = source_kind(meta).filter(|k| EXTERNAL_KINDS.contains(k))?;
    let label = exchange(meta)
        .and_then(|x| x.pointer("/source/label"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    Some(ChatMessageExternal {
        source_kind: kind.to_owned(),
        source_label: label.to_owned(),
        untrusted: true,
    })
}

/// One review message of a proposal, in thread order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewMessage {
    pub message_id: String,
    pub author: String,
    pub created_at: String,
    pub record: ProposalReviewRecord,
}

/// D2.4: the state of a proposal from its review messages (thread order) and the proposal as it is now (`now_for`
/// the review's `attachment`; for a CoS draft its `sha256` is the attachment's). The last adopt / reject decides; with only comments the state is
/// `commented`; none is `pending`. If the deciding review saw another revision or body than `now`, it is `stale`.
/// Only reviews by a person (`human:<name>`) count, so a text that claims to adopt changes nothing (T1・T2).
pub fn proposal_review_state(
    kind: ProposalKind,
    reviews: &[ReviewMessage],
    now_for: impl Fn(Option<&str>) -> Option<CitedMessage>,
) -> ChatMessageProposal {
    let by_person: Vec<&ReviewMessage> = reviews
        .iter()
        .filter(|r| crate::is_human_principal(&r.author))
        .collect();
    let deciding = by_person
        .iter()
        .rev()
        .find(|r| r.record.verdict != ProposalVerdict::Comment)
        .or_else(|| by_person.last());
    let Some(review) = deciding else {
        return ChatMessageProposal {
            kind,
            review_state: ProposalReviewState::Pending,
            review_message_id: None,
            reviewed_by: None,
            reviewed_at: None,
        };
    };
    let now = now_for(review.record.attachment.as_deref());
    let review_state =
        if citation_state(&review.record.target, now.as_ref()) != ChatCitationState::Current {
            ProposalReviewState::Stale
        } else {
            match review.record.verdict {
                ProposalVerdict::Adopt => ProposalReviewState::Adopted,
                ProposalVerdict::Reject => ProposalReviewState::Rejected,
                ProposalVerdict::Comment => ProposalReviewState::Commented,
            }
        };
    ChatMessageProposal {
        kind,
        review_state,
        review_message_id: Some(review.message_id.clone()),
        reviewed_by: Some(review.author.clone()),
        reviewed_at: Some(review.created_at.clone()),
    }
}

/// The `sha256` of a ready chat attachment (`None`: deleted or unknown).
pub fn attachment_sha256(
    conn: &Connection,
    attachment_id: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT sha256 FROM chat_attachments WHERE id=?1 AND state='ready'",
        params![attachment_id],
        |row| row.get(0),
    )
    .optional()
}

/// The review messages of `message_id` in `thread_id`, in thread order (a retracted review does not count).
pub fn review_messages(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
) -> rusqlite::Result<Vec<ReviewMessage>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, metadata_json, created_at FROM chat_messages WHERE thread_id=?1 AND role='user' \
         AND json_extract(metadata_json,'$.proposal_review.target.message_id')=?2 \
         AND json_type(metadata_json,'$.retracted') IS NULL ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![thread_id, message_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, meta, created_at) = row?;
        let meta: Value = serde_json::from_str(&meta).unwrap_or(Value::Null);
        let record = meta
            .get(METADATA_KEY)
            .and_then(|v| serde_json::from_value::<ProposalReviewRecord>(v.clone()).ok());
        let author = meta.get("author").and_then(Value::as_str);
        if let (Some(record), Some(author)) = (record, author) {
            out.push(ReviewMessage {
                message_id: id,
                author: author.to_owned(),
                created_at,
                record,
            });
        }
    }
    Ok(out)
}

/// The proposal as it is now, as a [`CitedMessage`] whose `sha256` is the body's, or for a CoS draft the
/// attachment's (`attachment`: the draft path the review named).
pub fn proposal_now(
    conn: &Connection,
    thread_id: &str,
    message_id: &str,
    text: &str,
    meta: &Value,
    attachment: Option<&str>,
) -> rusqlite::Result<Option<CitedMessage>> {
    let edit = ChatMessageEditState::of_metadata(meta).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let sha256 = match attachment {
        None => body_sha256(text),
        Some(path) => {
            let Some((_, id)) = strategy_drafts(meta).into_iter().find(|(p, _)| p == path) else {
                return Ok(None);
            };
            match attachment_sha256(conn, &id)? {
                Some(sha) => sha,
                None => return Ok(None),
            }
        }
    };
    Ok(Some(CitedMessage {
        thread_id: thread_id.to_owned(),
        message_id: message_id.to_owned(),
        project_id: None,
        general_thread: false,
        seq: 0,
        revision: edit.revision,
        sha256,
        retracted: edit.retracted.is_some(),
    }))
}

impl ChatMessageProposalMarks {
    /// The wire marks of one message (`role` as stored), the state derived now from the thread's review messages.
    pub fn of_message(
        conn: &Connection,
        thread_id: &str,
        message_id: &str,
        role: &str,
        text: &str,
        meta: &Value,
    ) -> rusqlite::Result<Box<Self>> {
        let author = meta.get("author").and_then(Value::as_str);
        let proposal = match proposal_kind(role, author, meta) {
            None => None,
            Some(kind) => {
                let reviews = review_messages(conn, thread_id, message_id)?;
                let mut nows = vec![(
                    None,
                    proposal_now(conn, thread_id, message_id, text, meta, None)?,
                )];
                for (path, _) in strategy_drafts(meta) {
                    let now = proposal_now(conn, thread_id, message_id, text, meta, Some(&path))?;
                    nows.push((Some(path), now));
                }
                let now_for = |a: Option<&str>| {
                    nows.iter()
                        .find(|(p, _)| p.as_deref() == a)
                        .and_then(|(_, n)| n.clone())
                };
                Some(proposal_review_state(kind, &reviews, now_for))
            }
        };
        Ok(Box::new(Self {
            proposal,
            external: external_mark(meta),
        }))
    }
}
