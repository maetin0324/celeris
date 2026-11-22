//! ADR 2026-11-21-chat-discussion-mode D6・D3: a citation of one chat message at one revision
//! ([`ChatCitation`]), the check that a summary or a draft may cite it, the derivation of its state
//! (`current`・`edited`・`retracted`・`missing`; never stored), and the check of a discussion
//! summary file (`celeris.discussion-summary/1`). The strategy document's `chat` reference
//! (`crate::strategy::Reference::Chat`) is the same type and goes through the same functions.
//!
//! A leaf of pure functions: it depends on nothing in this crate, so [`crate::chat`] re-exports the
//! types and [`crate::strategy`] uses the functions without a module cycle. The facts about the
//! cited message ([`CitedMessage`]) are read by `crate::discussion_citation`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// D6: the schema of a discussion summary file `discussion/summary/<name>.json`.
pub const DISCUSSION_SUMMARY_SCHEMA: &str = "celeris.discussion-summary/1";
/// D6: the longest `body` (Markdown), in UTF-8 bytes.
pub const DISCUSSION_SUMMARY_BODY_MAX_BYTES: usize = 16_384;
/// D6: a summary cites 1 to this many messages.
pub const DISCUSSION_SUMMARY_CITATIONS_MAX: usize = 256;

/// D6: one cited message at the revision the citing text read. `sha256` is the SHA-256 (64
/// lowercase hex) of that revision's body. The state is not part of it: it is derived from the
/// message every time ([`citation_state`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatCitation {
    pub thread_id: String,
    pub message_id: String,
    pub revision: u64,
    pub sha256: String,
}

/// D6: the state of a citation, derived from the cited message as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatCitationState {
    /// The message is at the cited revision and its body has the cited sha256.
    Current,
    /// The author edited the message after the cited revision.
    Edited,
    /// The author retracted the message.
    Retracted,
    /// No such message in the cited thread, or the cited revision / body never existed.
    Missing,
}

/// The cited message as it is now (read from `chat_messages` / `chat_threads`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedMessage {
    pub thread_id: String,
    pub message_id: String,
    /// The project of the message's thread (`None` for a thread without a project).
    pub project_id: Option<String>,
    /// The thread is the general CoS thread (統括; it has no project).
    pub general_thread: bool,
    pub seq: u64,
    /// `metadata_json.revision` (1 when never changed; D5.2).
    pub revision: u64,
    /// SHA-256 (64 lowercase hex) of the message's current `text`.
    pub sha256: String,
    pub retracted: bool,
}

/// Why a citation cannot be attached (D6). `code()` is the run status / refusal code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCitationRefusal {
    /// Empty ids, `revision` 0, or a `sha256` that is not 64 lowercase hex.
    Shape,
    Missing,
    /// The message is in another thread that is not the general thread.
    OtherThread,
    Edited,
    Retracted,
}

impl ChatCitationRefusal {
    pub fn code(self) -> &'static str {
        match self {
            Self::Shape => "chat_citation_invalid",
            Self::Missing => "chat_citation_missing",
            Self::OtherThread => "chat_citation_other_thread",
            Self::Edited => "chat_citation_edited",
            Self::Retracted => "chat_citation_retracted",
        }
    }
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The form of a citation: both ids, `revision` 1 or more, `sha256` 64 lowercase hex.
pub fn citation_shape_ok(c: &ChatCitation) -> bool {
    !c.thread_id.trim().is_empty()
        && !c.message_id.trim().is_empty()
        && c.revision >= 1
        && is_lower_hex(&c.sha256, 64)
}

/// D6: the state of `c` given the cited message as it is now (`None`: not found in the cited
/// thread). A revision after the message's own, or the message's revision with another body, never
/// existed and is `missing`.
pub fn citation_state(c: &ChatCitation, now: Option<&CitedMessage>) -> ChatCitationState {
    let Some(m) = now.filter(|m| m.thread_id == c.thread_id && m.message_id == c.message_id) else {
        return ChatCitationState::Missing;
    };
    if m.retracted {
        ChatCitationState::Retracted
    } else if c.revision < m.revision {
        ChatCitationState::Edited
    } else if c.revision == m.revision && c.sha256 == m.sha256 {
        ChatCitationState::Current
    } else {
        ChatCitationState::Missing
    }
}

/// D6: may a file attached in `citing_thread_id` cite `c`? The message must exist in the cited
/// thread, that thread must be the citing thread or the general thread, and the cited revision and
/// body sha256 must be the message's current ones.
pub fn verify_citation(
    citing_thread_id: &str,
    c: &ChatCitation,
    now: Option<&CitedMessage>,
) -> Result<(), ChatCitationRefusal> {
    if !citation_shape_ok(c) {
        return Err(ChatCitationRefusal::Shape);
    }
    let state = citation_state(c, now);
    if state == ChatCitationState::Missing {
        return Err(ChatCitationRefusal::Missing);
    }
    if c.thread_id != citing_thread_id && !now.is_some_and(|m| m.general_thread) {
        return Err(ChatCitationRefusal::OtherThread);
    }
    match state {
        ChatCitationState::Current => Ok(()),
        ChatCitationState::Edited => Err(ChatCitationRefusal::Edited),
        ChatCitationState::Retracted => Err(ChatCitationRefusal::Retracted),
        ChatCitationState::Missing => Err(ChatCitationRefusal::Missing),
    }
}

/// D6: a discussion summary file (`celeris.discussion-summary/1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscussionSummary {
    pub schema: String,
    /// The project of the summarised thread (`null` for the general thread).
    pub project_id: Option<String>,
    pub thread_id: String,
    /// The summary covers the thread's messages with `seq <= through_seq`.
    pub through_seq: u64,
    pub body: String,
    pub citations: Vec<ChatCitation>,
}

/// One reason a summary file is refused (`code` is the run status code; `path` names the field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SummaryViolation {
    pub code: &'static str,
    pub path: String,
    pub detail: String,
}

pub const SUMMARY_INVALID: &str = "discussion_summary_invalid";
pub const SUMMARY_LIMIT: &str = "discussion_summary_limit";
pub const SUMMARY_OTHER_THREAD: &str = "discussion_summary_other_thread";
pub const SUMMARY_SOURCE_MISSING: &str = "discussion_summary_source_missing";

fn violation(
    code: &'static str,
    path: impl Into<String>,
    detail: impl Into<String>,
) -> SummaryViolation {
    SummaryViolation {
        code,
        path: path.into(),
        detail: detail.into(),
    }
}

impl DiscussionSummary {
    /// Reads a summary file (UTF-8 JSON with exactly the D6 fields).
    pub fn parse(bytes: &[u8]) -> Result<Self, SummaryViolation> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| violation(SUMMARY_INVALID, "", "the file is not UTF-8"))?;
        serde_json::from_str(text).map_err(|e| violation(SUMMARY_INVALID, "", e.to_string()))
    }
}

/// The thread a summary is attached in (the CoS reply's thread) and the cited messages as they are
/// now (looked up by the caller for `summary.citations`; a citation without an entry is missing).
#[derive(Debug, Clone, Copy)]
pub struct SummaryContext<'a> {
    pub thread_id: &'a str,
    pub project_id: Option<&'a str>,
    pub cited: &'a [CitedMessage],
}

/// D6: every reason `summary` may not be attached in `ctx.thread_id` (empty = it may).
pub fn check_discussion_summary(
    summary: &DiscussionSummary,
    ctx: &SummaryContext<'_>,
) -> Vec<SummaryViolation> {
    let mut out = Vec::new();
    if summary.schema != DISCUSSION_SUMMARY_SCHEMA {
        out.push(violation(
            SUMMARY_INVALID,
            "schema",
            format!("schema is not {DISCUSSION_SUMMARY_SCHEMA}"),
        ));
    }
    if summary.thread_id != ctx.thread_id {
        out.push(violation(
            SUMMARY_OTHER_THREAD,
            "thread_id",
            "the summary is of another thread",
        ));
    }
    if summary.project_id.as_deref() != ctx.project_id {
        out.push(violation(
            SUMMARY_OTHER_THREAD,
            "project_id",
            "the summary's project is not the thread's",
        ));
    }
    if summary.through_seq == 0 {
        out.push(violation(
            SUMMARY_INVALID,
            "through_seq",
            "through_seq is 0",
        ));
    }
    if summary.body.trim().is_empty() {
        out.push(violation(SUMMARY_INVALID, "body", "the body is empty"));
    } else if summary.body.len() > DISCUSSION_SUMMARY_BODY_MAX_BYTES {
        out.push(violation(
            SUMMARY_LIMIT,
            "body",
            format!(
                "the body is {} bytes, over {DISCUSSION_SUMMARY_BODY_MAX_BYTES}",
                summary.body.len()
            ),
        ));
    }
    if summary.citations.is_empty() {
        out.push(violation(
            SUMMARY_SOURCE_MISSING,
            "citations",
            "the summary cites no message",
        ));
    } else if summary.citations.len() > DISCUSSION_SUMMARY_CITATIONS_MAX {
        out.push(violation(
            SUMMARY_LIMIT,
            "citations",
            format!(
                "{} citations, over {DISCUSSION_SUMMARY_CITATIONS_MAX}",
                summary.citations.len()
            ),
        ));
        return out;
    }
    for (i, c) in summary.citations.iter().enumerate() {
        let now = ctx
            .cited
            .iter()
            .find(|m| m.thread_id == c.thread_id && m.message_id == c.message_id);
        let path = format!("citations[{i}]");
        if let Err(refusal) = verify_citation(ctx.thread_id, c, now) {
            out.push(violation(
                refusal.code(),
                path,
                "the citation cannot be attached",
            ));
        } else if let Some(m) = now.filter(|m| m.thread_id == summary.thread_id)
            && m.seq > summary.through_seq
        {
            out.push(violation(
                SUMMARY_INVALID,
                path,
                format!(
                    "cites seq {} after through_seq {}",
                    m.seq, summary.through_seq
                ),
            ));
        }
    }
    out
}
