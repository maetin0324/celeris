//! ADR 2026-11-22-external-llm-discussion-exchange D4.1・D4.2: the wire types of the exchange routes
//! (`crate::chat::exchange`), registered in `ApiV1Schema`. A leaf: it depends on `task_core` only, so the schema root
//! and the chat routes both point at it without a new edge inside the module cycle (repo health `deps.cycle`).
//! `task_core::discussion_exchange`'s request types have no JSON Schema; each wire type here builds one with an
//! exhaustive struct literal, so a field added there fails to compile here.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use task_core::chat::ChatMessage;
use task_core::discussion_exchange::{
    ExchangeShareRequest, ImportFormat, ImportRequest, Received, ShareScope,
};

/// D2.1: what the person shares (`selected_excerpt`: excerpts they picked; `summary`: a summary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatExchangeShareScope {
    SelectedExcerpt,
    Summary,
}

/// D4.1: `POST /chat/threads/{t}/exchange/shares`. The wire form of
/// `task_core::discussion_exchange::ExchangeShareRequest` (that type has no JSON Schema); [`From`] builds it with an
/// exhaustive literal, so a field added there fails to compile here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatExchangeShareRequest {
    /// 1–200 distinct message ids of the thread, shared at their current revision and body.
    pub message_ids: Vec<String>,
    pub scope: ChatExchangeShareScope,
    /// A confidential or `outbound=false` strategy document carried by a shared message may go out (D2.3).
    #[serde(default)]
    pub include_confidential: bool,
    /// RFC 3339; after now and at most 30 days later.
    pub expires_at: String,
}

impl From<ChatExchangeShareRequest> for ExchangeShareRequest {
    fn from(w: ChatExchangeShareRequest) -> Self {
        Self {
            message_ids: w.message_ids,
            scope: match w.scope {
                ChatExchangeShareScope::SelectedExcerpt => ShareScope::SelectedExcerpt,
                ChatExchangeShareScope::Summary => ShareScope::Summary,
            },
            include_confidential: w.include_confidential,
            expires_at: w.expires_at,
        }
    }
}

/// D4.1: the answer of a share (201) and of its revocation (200).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatExchangeShareResponse {
    /// The `chat_events` id of the `exchange_shared` record (the `share_id` of the export and the revocation).
    pub share_id: i64,
    pub revoked: bool,
}

/// D4.2: how the imported text is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatExchangeImportFormat {
    Json,
    Markdown,
}

/// D4.1・D4.2: `POST /chat/threads/{t}/exchange/imports`. `envelope` (one `celeris.discussion-envelope/1` object
/// without `actor`・`consent`・`approval`) for `json`; `content` (front matter + body) for `markdown`. The wire form
/// of `task_core::discussion_exchange::ImportRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChatExchangeImportRequest {
    pub format: ChatExchangeImportFormat,
    /// The idempotency key (D4.3): the same key and body return the message already received.
    pub client_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl From<ChatExchangeImportRequest> for ImportRequest {
    fn from(w: ChatExchangeImportRequest) -> Self {
        Self {
            format: match w.format {
                ChatExchangeImportFormat::Json => ImportFormat::Json,
                ChatExchangeImportFormat::Markdown => ImportFormat::Markdown,
            },
            client_message_id: w.client_message_id,
            envelope: w.envelope,
            content: w.content,
        }
    }
}

/// D4.2・D4.3: the received message (`role=user`, `discussion_mode=discuss`, author the caller). 201 when new,
/// 200 with `duplicate: true` when the same key and body were received before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatExchangeImportResponse {
    pub message: ChatMessage,
    pub project_id: String,
    /// `sha256:<hex>` of the received body.
    pub content_hash: String,
    pub duplicate: bool,
}

impl From<Received> for ChatExchangeImportResponse {
    fn from(r: Received) -> Self {
        Self {
            message: r.message,
            project_id: r.project_id,
            content_hash: r.content_hash,
            duplicate: r.duplicate,
        }
    }
}
