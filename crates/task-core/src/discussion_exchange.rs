//! ADR 2026-11-22-external-llm-discussion-exchange D1.3・D1.4・D2・D4.2・D4.3: the discussion snapshot an external LLM
//! (ChatGPT・Muse・any MCP client) may read, its Markdown / JSON exchange, and the check of a summary or quote a person
//! (or an MCP client's propose) brings in.
//!
//! - A snapshot holds only what a person shared ([`ExchangeShared`] in `chat_events`; the share in effect is derived on
//!   every read by [`active_shares`], never stored) and the calling client's own proposals. A message retracted or
//!   edited after the share, a document whose citations are not `current`, and a confidential document the person did
//!   not share with `include_confidential` are left out ([`OmitReason`]). Secrets are masked just before output.
//! - The rules (R3・R7・R8・R9・R10・R11・R13 and the mask) are the pure functions of [`crate::strategy::envelope`]; this
//!   module only builds the envelope and calls them (rule 2: one implementation).
//! - The store side ([`store`]) reads and writes through the existing chat functions (`chat_message_post_as_also`,
//!   `cited_message_conn`, `chat_events`); no table, column or migration is added.
//!
//! Kept outside [`crate::chat`] (repo health `size.responsibility-growth`); it depends on `chat` one way.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::chat::{ChatCitation, ChatMessageRole};
use crate::project_proposal::sha256_hex;
use crate::strategy::envelope::{
    self, BODY_MAX_BYTES, EnvelopeContext, MCP_PROPOSE_CONSENT_SCOPE, OutboundShare,
};
use crate::strategy::{
    Actor, ActorKind, ApprovalRecord, Confidence, Consent, DecisionState, Source, SourceKind,
    SourceRevision,
};

mod format;
mod store;

pub use format::{
    envelope_from_json, envelope_from_markdown, envelope_to_json, envelope_to_markdown,
    snapshot_from_json, snapshot_from_markdown, snapshot_to_json, snapshot_to_markdown,
};
pub use store::{ExchangeError, ExchangeTarget, Received, SnapshotQuery};

/// D1.4: the schema of a snapshot.
pub const SNAPSHOT_SCHEMA: &str = "celeris.discussion-snapshot/1";
/// D4.2: the schema of one envelope (the Markdown front matter's `schema`).
pub const ENVELOPE_SCHEMA: &str = "celeris.discussion-envelope/1";
/// D1.3: the longest Markdown / JSON document brought in, in UTF-8 bytes.
pub const DOCUMENT_MAX_BYTES: usize = 32_768;
/// D1.3: at most this many references.
pub const REFERENCES_MAX: usize = 32;
/// D1.3: the longest reference `url`, in bytes.
pub const REFERENCE_URL_MAX_BYTES: usize = 2_048;
/// D1.3: the longest reference `note`, in characters.
pub const REFERENCE_NOTE_MAX_CHARS: usize = 500;
/// D1.3: the longest idempotency key, in characters.
pub const IDEMPOTENCY_KEY_MAX_CHARS: usize = 128;
/// D1.3: the longest `source_label`, in characters.
pub const SOURCE_LABEL_MAX_CHARS: usize = 64;
/// D1.3: a share names 1 to this many messages.
pub const SHARE_MESSAGES_MAX: usize = 200;
/// D2.1: a share expires at most this many days after it was made.
pub const SHARE_DAYS_MAX: i64 = 30;
/// D1.4: at most this many items per snapshot.
pub const SNAPSHOT_ITEMS_MAX: usize = 200;
/// D1.4: the default number of items per snapshot.
pub const SNAPSHOT_ITEMS_DEFAULT: usize = 50;
/// D3・D4.2: the key of `chat_messages.metadata_json` holding the envelope of a received message (without `body`).
pub const METADATA_KEY: &str = "exchange";

/// D2.1: what the person shared: excerpts they picked, or a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareScope {
    SelectedExcerpt,
    Summary,
}

impl ShareScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelectedExcerpt => "selected_excerpt",
            Self::Summary => "summary",
        }
    }
}

/// D2.1 / D4.1: `POST /chat/threads/{t}/exchange/shares`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeShareRequest {
    pub message_ids: Vec<String>,
    pub scope: ShareScope,
    #[serde(default)]
    pub include_confidential: bool,
    /// RFC 3339; after now and at most [`SHARE_DAYS_MAX`] days later.
    pub expires_at: String,
}

/// One shared message at the revision the person saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedRevision {
    pub message_id: String,
    pub revision: u64,
    /// SHA-256 (64 lowercase hex) of that revision's body (the chat citation's `sha256`).
    pub sha256: String,
}

/// D2.1: the `exchange_shared` record appended to `chat_events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeShared {
    /// `human:<name>` of the caller.
    pub by: String,
    pub message_ids: Vec<String>,
    pub revisions: Vec<SharedRevision>,
    pub scope: ShareScope,
    pub include_confidential: bool,
    pub expires_at: String,
}

/// D2.1: the `exchange_share_revoked` record appended to `chat_events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeShareRevoked {
    pub by: String,
    pub share_event_id: i64,
}

/// One share record read back from `chat_events` (id, time, record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareEvent {
    Shared {
        id: i64,
        at: String,
        record: ExchangeShared,
    },
    Revoked(ExchangeShareRevoked),
}

/// A share in effect (derived by [`active_shares`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveShare {
    pub share_id: i64,
    pub granted_at: String,
    pub record: ExchangeShared,
}

fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// D2.1: the shares in effect at `now` — shared, not revoked, not expired (an unreadable `expires_at` counts as
/// expired). Derived from the append-only records on every call.
pub fn active_shares(events: &[ShareEvent], now: OffsetDateTime) -> Vec<ActiveShare> {
    let revoked: BTreeSet<i64> = events
        .iter()
        .filter_map(|e| match e {
            ShareEvent::Revoked(r) => Some(r.share_event_id),
            ShareEvent::Shared { .. } => None,
        })
        .collect();
    events
        .iter()
        .filter_map(|e| match e {
            ShareEvent::Shared { id, at, record }
                if !revoked.contains(id)
                    && parse_ts(&record.expires_at).is_some_and(|x| x > now) =>
            {
                Some(ActiveShare {
                    share_id: *id,
                    granted_at: at.clone(),
                    record: record.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

/// D1.4: what an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Message,
    Summary,
    Strategy,
    Proposal,
}

/// D1.4: one item of a snapshot (the envelope fields of D1.3 plus where it came from).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeItem {
    pub kind: ItemKind,
    pub message_id: String,
    pub seq: u64,
    pub revision: u64,
    /// The message at the shared revision; `sha256` is the original body's (D2.4).
    pub citation: ChatCitation,
    /// The workspace path of a summary / strategy file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub source: Source,
    pub actor: Actor,
    pub timestamp: String,
    /// `sha256:<hex>` of `body` as sent (after the mask).
    pub content_hash: String,
    pub consent: Consent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<i64>,
    pub references: Vec<Value>,
    pub confidence: Confidence,
    pub decision_state: DecisionState,
    pub body: String,
    /// How many secrets [`envelope::mask_secrets`] replaced.
    pub masked: usize,
}

/// D2.2・D2.3: why a shared message or file is not in the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmitReason {
    Retracted,
    EditedSinceShare,
    CitationStale,
    Confidential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Omitted {
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub reason: OmitReason,
}

/// D1.4: `celeris.discussion-snapshot/1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscussionSnapshot {
    pub schema: String,
    pub project_id: String,
    pub thread_id: String,
    pub source_revision: SourceRevision,
    pub generated_at: String,
    pub items: Vec<ExchangeItem>,
    pub omitted: Vec<Omitted>,
    pub next_after_seq: u64,
}

/// A chat message as it is now (read by [`store`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFacts {
    pub message_id: String,
    pub seq: u64,
    pub revision: u64,
    /// SHA-256 (64 lowercase hex) of the current text.
    pub sha256: String,
    pub retracted: bool,
    pub role: ChatMessageRole,
    pub author: Option<String>,
    pub text: String,
    pub created_at: String,
    /// `metadata_json.exchange` of a received message (D3・D4.2).
    pub exchange: Option<Value>,
}

/// A summary (`discussion/summary/*.json`) or strategy (`strategy/*/v*.json`) file attached to a CoS reply, read by
/// the caller (which has the workspace). It is shared when its carrying message is.
#[derive(Debug, Clone, PartialEq)]
pub struct ExchangeDocument {
    /// The CoS reply the file is attached to.
    pub message_id: String,
    /// [`ItemKind::Summary`] or [`ItemKind::Strategy`].
    pub kind: ItemKind,
    pub path: String,
    /// The text sent (the summary body, or the strategy document's JSON).
    pub body: String,
    /// `acl.classification = confidential` or `acl.outbound = false`.
    pub confidential: bool,
    /// The chat messages the file cites (all must be `current`, D2.2).
    pub citations: Vec<ChatCitation>,
    pub references: Vec<Value>,
    pub confidence: Confidence,
    /// The recorded state; only `discussion`・`proposed` are used. `approved` is derived from `approval` (D1.4).
    pub decision_state: DecisionState,
    /// The decision approving this text (child 2 D2.2), looked up by the caller.
    pub approval: Option<ApprovalRecord>,
}

/// What [`build_snapshot`] reads (gathered by [`store`] in one read transaction).
#[derive(Debug, Clone)]
pub struct SnapshotInput {
    pub project_id: String,
    pub thread_id: String,
    pub source_revision: SourceRevision,
    pub generated_at: String,
    pub shares: Vec<ActiveShare>,
    /// The shared messages, the documents' carrying messages and the viewer's proposals, by id.
    pub messages: BTreeMap<String, MessageFacts>,
    /// `(document, every citation current)`.
    pub documents: Vec<(ExchangeDocument, bool)>,
    /// `mcp:<client_id>` (its own proposals are included) or `human:<name>` (export: shares only).
    pub viewer: String,
    pub after_seq: u64,
    pub limit: usize,
}

fn content_hash(body: &str) -> String {
    format!("sha256:{}", sha256_hex(body))
}

/// The recorded state of an external text can only stay at `discussion`・`proposed` (R11); anything else reads as
/// `proposed`, never as approved.
fn recorded_state(state: Option<DecisionState>) -> DecisionState {
    match state {
        Some(DecisionState::Discussion) => DecisionState::Discussion,
        _ => DecisionState::Proposed,
    }
}

fn exchange_field<T: serde::de::DeserializeOwned>(exchange: &Value, key: &str) -> Option<T> {
    exchange
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// The source / actor / references / confidence / state of a message: from its received envelope if it has one,
/// otherwise from the chat record (a person's or the CoS's words in the thread).
fn message_envelope(
    thread_id: &str,
    m: &MessageFacts,
) -> (Source, Actor, Vec<Value>, Confidence, DecisionState) {
    if let Some(x) = &m.exchange {
        let source = exchange_field(x, "source").unwrap_or(Source {
            kind: SourceKind::McpClient,
            label: "unknown".into(),
            thread_id: None,
        });
        let actor = exchange_field(x, "actor").unwrap_or(Actor {
            kind: ActorKind::Mcp,
            id: m.author.clone().unwrap_or_default(),
        });
        return (
            source,
            actor,
            exchange_field(x, "references").unwrap_or_default(),
            exchange_field(x, "confidence").unwrap_or(Confidence::Estimated),
            recorded_state(exchange_field(x, "decision_state")),
        );
    }
    let actor = match (&m.role, &m.author) {
        (ChatMessageRole::User, Some(a)) if crate::is_human_principal(a) => Actor {
            kind: ActorKind::Human,
            id: a.clone(),
        },
        (ChatMessageRole::User, Some(a)) if a.starts_with("mcp:") => Actor {
            kind: ActorKind::Mcp,
            id: a.clone(),
        },
        _ => Actor {
            kind: ActorKind::Cos,
            id: format!("cos:{thread_id}"),
        },
    };
    let kind = if actor.kind == ActorKind::Mcp {
        SourceKind::McpClient
    } else {
        SourceKind::CelerisCosThread
    };
    let label = if kind == SourceKind::McpClient {
        actor.id.trim_start_matches("mcp:").to_owned()
    } else {
        "celeris".to_owned()
    };
    (
        Source {
            kind,
            label,
            thread_id: Some(thread_id.to_owned()),
        },
        actor,
        Vec::new(),
        // D1.4: a person's or the CoS's words are not a verified claim.
        Confidence::Estimated,
        DecisionState::Discussion,
    )
}

fn share_consent(share: &ActiveShare) -> Consent {
    Consent {
        granted_by: share.record.by.clone(),
        scope: share.record.scope.as_str().to_owned(),
        granted_at: share.granted_at.clone(),
    }
}

/// D1.4・D2: the snapshot of `input` — the shared messages still at the shared revision, their files whose citations
/// are current and that are not confidential (unless shared with `include_confidential` or approved by a person), and
/// the viewer's own proposals; masked, ordered by seq, after `after_seq`, at most `limit` (≤ [`SNAPSHOT_ITEMS_MAX`]).
pub fn build_snapshot(input: &SnapshotInput) -> DiscussionSnapshot {
    let thread_id = input.thread_id.as_str();
    // The latest share covering each message at its current revision.
    let mut covered: BTreeMap<&str, &ActiveShare> = BTreeMap::new();
    let mut stale: BTreeMap<&str, OmitReason> = BTreeMap::new();
    for share in &input.shares {
        for rev in &share.record.revisions {
            let Some(m) = input.messages.get(&rev.message_id) else {
                continue;
            };
            if m.retracted {
                stale.insert(&rev.message_id, OmitReason::Retracted);
            } else if m.revision == rev.revision && m.sha256 == rev.sha256 {
                covered.insert(&rev.message_id, share);
            } else {
                stale.insert(&rev.message_id, OmitReason::EditedSinceShare);
            }
        }
    }
    let mut items = Vec::new();
    let mut omitted: Vec<Omitted> = stale
        .iter()
        .filter(|(id, _)| !covered.contains_key(*id))
        .map(|(id, reason)| Omitted {
            message_id: (*id).to_owned(),
            path: None,
            reason: *reason,
        })
        .collect();
    let item = |m: &MessageFacts, kind: ItemKind, consent: Consent, share_id: Option<i64>| {
        let (source, actor, references, confidence, decision_state) =
            message_envelope(thread_id, m);
        let masked = envelope::mask_secrets(&m.text);
        ExchangeItem {
            kind,
            message_id: m.message_id.clone(),
            seq: m.seq,
            revision: m.revision,
            citation: ChatCitation {
                thread_id: thread_id.to_owned(),
                message_id: m.message_id.clone(),
                revision: m.revision,
                sha256: m.sha256.clone(),
            },
            path: None,
            source,
            actor,
            timestamp: m.created_at.clone(),
            content_hash: content_hash(&masked.text),
            consent,
            share_id,
            references,
            confidence,
            decision_state,
            body: masked.text,
            masked: masked.masked,
        }
    };
    let own = |m: &MessageFacts| {
        input.viewer.starts_with("mcp:")
            && m.exchange.is_some()
            && m.author.as_deref() == Some(input.viewer.as_str())
    };
    for (id, share) in &covered {
        let Some(m) = input.messages.get(*id) else {
            continue;
        };
        let kind = if own(m) {
            ItemKind::Proposal
        } else {
            ItemKind::Message
        };
        items.push(item(m, kind, share_consent(share), Some(share.share_id)));
    }
    for m in input.messages.values() {
        if own(m) && !m.retracted && !covered.contains_key(m.message_id.as_str()) {
            let consent = exchange_field(m.exchange.as_ref().unwrap_or(&Value::Null), "consent")
                .unwrap_or_default();
            items.push(item(m, ItemKind::Proposal, consent, None));
        }
    }
    for (doc, citations_current) in &input.documents {
        let (Some(share), Some(m)) = (
            covered.get(doc.message_id.as_str()),
            input.messages.get(&doc.message_id),
        ) else {
            continue;
        };
        let omit = |reason| Omitted {
            message_id: doc.message_id.clone(),
            path: Some(doc.path.clone()),
            reason,
        };
        if !citations_current {
            omitted.push(omit(OmitReason::CitationStale));
            continue;
        }
        let masked = envelope::mask_secrets(&doc.body);
        let hash = content_hash(&masked.text);
        let outbound = OutboundShare {
            by: share.record.by.clone(),
            sha256s: vec![hash.clone()],
            include_confidential: share.record.include_confidential,
        };
        if doc.confidential
            && !envelope::outbound_allowed(&hash, Some(&outbound), doc.approval.as_ref())
        {
            omitted.push(omit(OmitReason::Confidential));
            continue;
        }
        let (source, actor, ..) = message_envelope(thread_id, m);
        let decision_state = if envelope::human_approved(doc.approval.as_ref(), &hash) {
            DecisionState::Approved
        } else {
            recorded_state(Some(doc.decision_state))
        };
        items.push(ExchangeItem {
            kind: doc.kind,
            message_id: doc.message_id.clone(),
            seq: m.seq,
            revision: m.revision,
            citation: ChatCitation {
                thread_id: thread_id.to_owned(),
                message_id: m.message_id.clone(),
                revision: m.revision,
                sha256: m.sha256.clone(),
            },
            path: Some(doc.path.clone()),
            source,
            actor,
            timestamp: m.created_at.clone(),
            content_hash: hash,
            consent: share_consent(share),
            share_id: Some(share.share_id),
            references: doc.references.clone(),
            confidence: doc.confidence,
            decision_state,
            body: masked.text,
            masked: masked.masked,
        });
    }
    items.retain(|i| i.seq > input.after_seq);
    items.sort_by(|a, b| (a.seq, a.kind, &a.path).cmp(&(b.seq, b.kind, &b.path)));
    items.truncate(input.limit.clamp(1, SNAPSHOT_ITEMS_MAX));
    omitted.sort_by(|a, b| (&a.message_id, &a.path).cmp(&(&b.message_id, &b.path)));
    DiscussionSnapshot {
        schema: SNAPSHOT_SCHEMA.to_owned(),
        project_id: input.project_id.clone(),
        thread_id: input.thread_id.clone(),
        source_revision: input.source_revision.clone(),
        generated_at: input.generated_at.clone(),
        next_after_seq: items.last().map_or(input.after_seq, |i| i.seq),
        items,
        omitted,
    }
}

/// D4.2: one portable envelope (the JSON `envelope` / the Markdown front matter + body). On the way in, `actor`,
/// `consent` and `direction` must be absent (Celeris fills them; [`check_inbound`]); `approval` is not a field at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub project_id: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,
    pub source_revision: SourceRevision,
    pub timestamp: String,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<Consent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<ChatCitation>,
    #[serde(default)]
    pub references: Vec<Value>,
    pub confidence: Confidence,
    pub decision_state: DecisionState,
    pub body: String,
}

impl ExchangeEnvelope {
    /// D4.1: the outbound envelope of one snapshot item (`direction: outbound`).
    pub fn outbound(snapshot: &DiscussionSnapshot, item: &ExchangeItem) -> Self {
        Self {
            schema: Some(ENVELOPE_SCHEMA.to_owned()),
            project_id: snapshot.project_id.clone(),
            source: item.source.clone(),
            actor: Some(item.actor.clone()),
            source_revision: snapshot.source_revision.clone(),
            timestamp: item.timestamp.clone(),
            content_hash: item.content_hash.clone(),
            consent: Some(item.consent.clone()),
            direction: Some("outbound".into()),
            citation: Some(item.citation.clone()),
            references: item.references.clone(),
            confidence: item.confidence,
            decision_state: item.decision_state,
            body: item.body.clone(),
        }
    }
}

/// D4.2: how the imported text is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFormat {
    Json,
    Markdown,
}

/// D4.1・D4.2: `POST /chat/threads/{t}/exchange/imports`. `envelope` for `json`, `content` (the whole Markdown) for
/// `markdown`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub format: ImportFormat,
    pub client_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// D1.6: an MCP client's `discussion_propose` (the target thread is resolved by the caller).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposeRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub idempotency_key: String,
    pub source_label: String,
    pub body: String,
    #[serde(default)]
    pub references: Vec<Value>,
    pub confidence: Confidence,
    pub decision_state: DecisionState,
}

/// Who brings the text in and what the thread already holds (built by [`store`] from the authentication and the
/// thread; never from the document).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundContext {
    pub project_id: String,
    pub thread_id: String,
    /// `human:<name>` (import) or `mcp:<client_id>` (propose).
    pub caller: String,
    pub caller_scopes: Vec<String>,
    /// key → `content_hash` of the message already received with that key (D4.3).
    pub idempotency: BTreeMap<String, String>,
    pub source_revision: SourceRevision,
    pub now: String,
}

/// A text that passed [`check_inbound`]: the message to post and its `metadata_json.exchange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    pub client_message_id: String,
    /// The message author (`caller`).
    pub author: String,
    pub body: String,
    pub content_hash: String,
    /// The envelope without `body` (D3・D4.2).
    pub metadata: Value,
}

/// Why a text is refused or a share / snapshot cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct ExchangeRefusal {
    /// `exchange_invalid`・`exchange_too_long`・`exchange_self_reported`・`exchange_secret`・`exchange_forbidden`・
    /// `thread_project_mismatch`・`thread_kind_not_project`・`R7:idempotency_conflict`・`exchange_rules`.
    pub code: &'static str,
    /// The envelope rules broken (`R4:content_hash`, …), for `exchange_rules` and R7.
    pub rules: Vec<&'static str>,
    pub detail: String,
}

pub const INVALID: &str = "exchange_invalid";
pub const TOO_LONG: &str = "exchange_too_long";
pub const SELF_REPORTED: &str = "exchange_self_reported";
pub const SECRET: &str = "exchange_secret";
pub const FORBIDDEN: &str = "exchange_forbidden";
pub const THREAD_PROJECT_MISMATCH: &str = "thread_project_mismatch";
pub const THREAD_KIND_NOT_PROJECT: &str = "thread_kind_not_project";
pub const IDEMPOTENCY_CONFLICT: &str = "R7:idempotency_conflict";
pub const RULES: &str = "exchange_rules";

impl ExchangeRefusal {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            rules: Vec::new(),
            detail: detail.into(),
        }
    }

    /// 403 for `exchange_forbidden`, 409 for R7, 422 otherwise (MCP maps 422 to `-32602`).
    pub fn http_status(&self) -> u16 {
        match self.code {
            FORBIDDEN => 403,
            IDEMPOTENCY_CONFLICT => 409,
            _ => 422,
        }
    }
}

fn key_ok(key: &str) -> bool {
    (1..=IDEMPOTENCY_KEY_MAX_CHARS).contains(&key.chars().count())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// D1.3: `source_label` is 1–64 of `[a-z0-9._-]`.
pub fn source_label_ok(label: &str) -> bool {
    (1..=SOURCE_LABEL_MAX_CHARS).contains(&label.chars().count())
        && label.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// D1.3: at most [`REFERENCES_MAX`] references, each an object; `url` ≤ 2,048 bytes, `note` ≤ 500 characters.
pub fn references_ok(refs: &[Value]) -> Result<(), ExchangeRefusal> {
    if refs.len() > REFERENCES_MAX {
        return Err(ExchangeRefusal::new(
            TOO_LONG,
            format!("more than {REFERENCES_MAX} references"),
        ));
    }
    for (i, r) in refs.iter().enumerate() {
        let Some(obj) = r.as_object() else {
            return Err(ExchangeRefusal::new(
                INVALID,
                format!("references[{i}] is not an object"),
            ));
        };
        let len = |k: &str, chars: bool| {
            obj.get(k)
                .and_then(Value::as_str)
                .map_or(0, |s| if chars { s.chars().count() } else { s.len() })
        };
        if len("url", false) > REFERENCE_URL_MAX_BYTES
            || len("note", true) > REFERENCE_NOTE_MAX_CHARS
        {
            return Err(ExchangeRefusal::new(
                TOO_LONG,
                format!("references[{i}] url or note is too long"),
            ));
        }
    }
    Ok(())
}

/// D4.2: the fields Celeris fills; a document carrying any of them is refused (self-reported identity / approval).
const SELF_REPORTED_KEYS: [&str; 4] = ["actor", "consent", "approval", "direction"];

/// D4.2・D4.3: reads a person's import (`human_paste` / `markdown_import`), refuses what it may not carry, fills
/// `actor` / `consent` / `idempotency_key` from `ctx`, and runs [`envelope::validate_envelope`]. Too long is refused,
/// never truncated.
pub fn check_import(req: &ImportRequest, ctx: &InboundContext) -> Result<Inbound, ExchangeRefusal> {
    if !key_ok(&req.client_message_id) {
        return Err(ExchangeRefusal::new(
            INVALID,
            "client_message_id is 1-128 of [A-Za-z0-9._:-]",
        ));
    }
    let raw: Map<String, Value> = match (req.format, &req.envelope, &req.content) {
        (ImportFormat::Json, Some(env), None) => {
            if serde_json::to_string(env).map_or(usize::MAX, |s| s.len()) > DOCUMENT_MAX_BYTES {
                return Err(too_long_document());
            }
            env.as_object()
                .cloned()
                .ok_or_else(|| ExchangeRefusal::new(INVALID, "envelope is not an object"))?
        }
        (ImportFormat::Markdown, None, Some(content)) => {
            if content.len() > DOCUMENT_MAX_BYTES {
                return Err(too_long_document());
            }
            format::markdown_fields(content)?
        }
        _ => {
            return Err(ExchangeRefusal::new(
                INVALID,
                "json needs `envelope`, markdown needs `content` (not both)",
            ));
        }
    };
    if let Some(k) = SELF_REPORTED_KEYS.iter().find(|k| raw.contains_key(**k)) {
        return Err(ExchangeRefusal::new(
            SELF_REPORTED,
            format!("`{k}` is filled by Celeris, not by the document"),
        ));
    }
    let doc: ExchangeEnvelope = serde_json::from_value(Value::Object(raw))
        .map_err(|e| ExchangeRefusal::new(INVALID, e.to_string()))?;
    if doc.schema.as_deref().is_some_and(|s| s != ENVELOPE_SCHEMA)
        || (req.format == ImportFormat::Markdown && doc.schema.is_none())
    {
        return Err(ExchangeRefusal::new(
            INVALID,
            format!("schema is not {ENVELOPE_SCHEMA}"),
        ));
    }
    if !matches!(
        doc.source.kind,
        SourceKind::HumanPaste | SourceKind::MarkdownImport
    ) || !source_label_ok(&doc.source.label)
    {
        return Err(ExchangeRefusal::new(
            INVALID,
            "source.kind is human_paste or markdown_import and source.label is 1-64 of [a-z0-9._-]",
        ));
    }
    references_ok(&doc.references)?;
    let actor = Actor {
        kind: ActorKind::Human,
        id: ctx.caller.clone(),
    };
    let consent = Consent {
        granted_by: ctx.caller.clone(),
        scope: ShareScope::SelectedExcerpt.as_str().to_owned(),
        granted_at: ctx.now.clone(),
    };
    inbound(doc, actor, consent, &req.client_message_id, ctx)
}

/// D1.6: an MCP client's propose. Celeris fills `source` (`mcp_client`), `actor` (`mcp:<client_id>`), `consent`
/// (the client and `mcp_scope:discussion:propose`), `timestamp`, `content_hash` and `source_revision`; the client
/// needs `discussion:propose` (R8). The idempotency key is namespaced by the client (D3).
pub fn check_propose(
    req: &ProposeRequest,
    ctx: &InboundContext,
) -> Result<Inbound, ExchangeRefusal> {
    if !key_ok(&req.idempotency_key) || !source_label_ok(&req.source_label) {
        return Err(ExchangeRefusal::new(
            INVALID,
            "idempotency_key is 1-128 of [A-Za-z0-9._:-] and source_label 1-64 of [a-z0-9._-]",
        ));
    }
    references_ok(&req.references)?;
    let doc = ExchangeEnvelope {
        schema: None,
        project_id: ctx.project_id.clone(),
        source: Source {
            kind: SourceKind::McpClient,
            label: req.source_label.clone(),
            thread_id: Some(ctx.thread_id.clone()),
        },
        actor: None,
        source_revision: ctx.source_revision.clone(),
        timestamp: ctx.now.clone(),
        content_hash: content_hash(&req.body),
        consent: None,
        direction: None,
        citation: None,
        references: req.references.clone(),
        confidence: req.confidence,
        decision_state: req.decision_state,
        body: req.body.clone(),
    };
    let actor = Actor {
        kind: ActorKind::Mcp,
        id: ctx.caller.clone(),
    };
    let consent = Consent {
        granted_by: ctx.caller.clone(),
        scope: MCP_PROPOSE_CONSENT_SCOPE.to_owned(),
        granted_at: ctx.now.clone(),
    };
    let key = format!("{}:{}", ctx.caller, req.idempotency_key);
    inbound(doc, actor, consent, &key, ctx)
}

fn too_long_document() -> ExchangeRefusal {
    ExchangeRefusal::new(
        TOO_LONG,
        format!("the document is over {DOCUMENT_MAX_BYTES} bytes"),
    )
}

/// The shared tail: refuse a long body or a secret by name, then hand the filled envelope to
/// [`envelope::validate_envelope`] (R1–R13) with the caller's identity and the thread's keys.
fn inbound(
    mut doc: ExchangeEnvelope,
    actor: Actor,
    consent: Consent,
    key: &str,
    ctx: &InboundContext,
) -> Result<Inbound, ExchangeRefusal> {
    if doc.body.len() > BODY_MAX_BYTES {
        return Err(ExchangeRefusal::new(
            TOO_LONG,
            format!("body is over {BODY_MAX_BYTES} bytes"),
        ));
    }
    if envelope::contains_secret(&doc.body) {
        return Err(ExchangeRefusal::new(
            SECRET,
            "the body looks like it holds a secret; remove it and bring it in again",
        ));
    }
    doc.source.thread_id = Some(ctx.thread_id.clone());
    doc.actor = Some(actor);
    doc.consent = Some(consent);
    let mut full =
        serde_json::to_value(&doc).map_err(|e| ExchangeRefusal::new(INVALID, e.to_string()))?;
    full["idempotency_key"] = json!(key);
    let ectx = EnvelopeContext {
        project_id: ctx.project_id.clone(),
        idempotency: ctx.idempotency.clone(),
        caller: ctx.caller.clone(),
        caller_scopes: ctx.caller_scopes.clone(),
        approval: None,
        outbound_share: None,
    };
    let rules = envelope::validate_envelope(&full, &ectx);
    if !rules.is_empty() {
        let code = if rules == [IDEMPOTENCY_CONFLICT] {
            IDEMPOTENCY_CONFLICT
        } else {
            RULES
        };
        return Err(ExchangeRefusal {
            code,
            detail: rules.join(", "),
            rules,
        });
    }
    let body = doc.body.clone();
    let mut metadata = full;
    if let Some(obj) = metadata.as_object_mut() {
        obj.remove("body");
        obj.remove("schema");
    }
    Ok(Inbound {
        client_message_id: key.to_owned(),
        author: ctx.caller.clone(),
        content_hash: doc.content_hash,
        body,
        metadata,
    })
}

#[cfg(test)]
#[path = "discussion_exchange_tests.rs"]
mod tests;
