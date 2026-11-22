//! ADR 2026-11-22-external-llm-discussion-exchange D4.1: the path for a client without MCP (ChatGPT on any plan,
//! Muse, a person's paste). A person shares messages of a project thread (`POST …/exchange/shares`), revokes a
//! share (`POST …/exchange/shares/{share_id}/revoke`), exports what is shared as JSON or Markdown
//! (`GET …/exchange/export`), and brings in one summary or quote they picked in an external LLM
//! (`POST …/exchange/imports`).
//!
//! - All four are for `Principal::Human` only: an operator, an MCP client, a CoS run and an unauthenticated caller
//!   are refused (403 `exchange_forbidden`; the middleware refuses MCP and CoS writes even earlier). The consent and
//!   the author are the caller (`caller.actor()`), never a field of the request.
//! - The rules (scope of the share, masking, R3・R7・R8・R9・R11, the input limits) are
//!   [`task_core::discussion_exchange`]'s; this module only reads the request, calls the store and writes the
//!   answer (rule 2: one implementation, shared with the MCP tools).
//! - Nothing here reads an external LLM's conversation or memory: the import is the one text the person pasted.
//! - The export passes no summary / strategy documents (`ExchangeDocument`): reading them from the CoS reply's
//!   attachments is not yet a shared task-core function (progress `rest-import.md`, 未解決).

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::routing::{get, post};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use task_core::chat::ChatMessage;
use task_core::discussion_exchange::{
    self as exchange, ExchangeError, ExchangeShareRequest, ExchangeTarget, ImportFormat,
    ImportRequest, Received, SNAPSHOT_ITEMS_DEFAULT, SNAPSHOT_ITEMS_MAX, ShareScope, SnapshotQuery,
};
use time::OffsetDateTime;

use super::{authorize, changed, chat_json, chat_problem, require_cos_enabled};
use crate::http_io::{ApiResult, Params, json_response, no_query};
use crate::principal::{Caller, Principal};
use crate::problem::ApiProblem;
use crate::query::QueryParams;
use crate::state::ApiState;

pub(super) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/chat/threads/{t}/exchange/shares", post(share))
        .route(
            "/api/v1/chat/threads/{t}/exchange/shares/{share_id}/revoke",
            post(revoke),
        )
        .route("/api/v1/chat/threads/{t}/exchange/export", get(export))
        .route("/api/v1/chat/threads/{t}/exchange/imports", post(import))
}

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

/// D4.1: the person (`human:<name>`) the exchange acts for, from the authentication only. Every other principal is
/// refused (403 `exchange_forbidden`).
fn person(caller: &Caller) -> Result<String, ApiProblem> {
    match &caller.0 {
        Principal::Human { .. } => Ok(caller.actor()),
        Principal::Operator { .. }
        | Principal::Mcp { .. }
        | Principal::Cos { .. }
        | Principal::Unauthenticated => Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            exchange::FORBIDDEN,
            format!(
                "{} cannot share, export or import discussion text; only a person can",
                caller.actor()
            ),
        )),
    }
}

/// A refusal keeps its code (`exchange_secret`, `R7:idempotency_conflict`, …) and status (403 / 409 / 422).
fn exchange_problem(error: ExchangeError) -> ApiProblem {
    match error {
        ExchangeError::Refused(r) => {
            let status =
                StatusCode::from_u16(r.http_status()).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
            let detail = if r.rules.is_empty() {
                r.detail.clone()
            } else {
                format!("{} ({})", r.detail, r.rules.join(", "))
            };
            ApiProblem::new(status, r.code, detail)
        }
        ExchangeError::Chat(e) => chat_problem(e),
    }
}

async fn share(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let by = person(&caller)?;
    authorize(&state, &headers)?;
    let req: ChatExchangeShareRequest = chat_json(body).await?;
    let req = ExchangeShareRequest::from(req);
    let share_id = state
        .blocking(move |store| {
            store
                .chat_exchange_share(&t, &by, &req, OffsetDateTime::now_utc())
                .map_err(exchange_problem)
        })
        .await?;
    Ok(json_response(
        StatusCode::CREATED,
        &ChatExchangeShareResponse {
            share_id,
            revoked: false,
        },
    ))
}

async fn revoke(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params((t, share_id)): Params<(String, i64)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let by = person(&caller)?;
    authorize(&state, &headers)?;
    state
        .blocking(move |store| {
            store
                .chat_exchange_share_revoke(&t, share_id, &by, OffsetDateTime::now_utc())
                .map_err(exchange_problem)
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        &ChatExchangeShareResponse {
            share_id,
            revoked: true,
        },
    ))
}

/// D4.1: `?format=json|markdown` (default `json`), `share_id` (one share; default all in effect), `after_seq`,
/// `limit` (default 50, at most 200). The body is the `celeris.discussion-snapshot/1` of D1.4 that the caller may
/// read now: only shared, unchanged messages, secrets masked. Reading writes nothing.
async fn export(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let viewer = person(&caller)?;
    authorize(&state, &headers)?;
    let params = QueryParams::parse(
        raw.as_deref(),
        &["format", "share_id", "after_seq", "limit"],
    )?;
    let markdown = match params.single("format")? {
        None | Some("json") => false,
        Some("markdown") => true,
        Some(other) => {
            return Err(ApiProblem::bad_request(format!(
                "format must be json or markdown, not {other:?}"
            )));
        }
    };
    let number = |key: &str| -> Result<Option<u64>, ApiProblem> {
        params
            .single(key)?
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| ApiProblem::bad_request(format!("{key} must be a number")))
            })
            .transpose()
    };
    let share_id = number("share_id")?
        .map(|v| i64::try_from(v).map_err(|_| ApiProblem::bad_request("share_id is out of range")))
        .transpose()?;
    let query = SnapshotQuery {
        target: ExchangeTarget {
            project_id: None,
            thread_id: Some(t),
        },
        after_seq: number("after_seq")?.unwrap_or(0),
        limit: Some(params.limit("limit", SNAPSHOT_ITEMS_DEFAULT, SNAPSHOT_ITEMS_MAX)?),
        share_id,
    };
    let snapshot = state
        .blocking(move |store| {
            store
                .chat_exchange_snapshot(&query, &viewer, &[], OffsetDateTime::now_utc())
                .map_err(exchange_problem)
        })
        .await?;
    let (text, content_type) = if markdown {
        (
            exchange::snapshot_to_markdown(&snapshot),
            "text/markdown; charset=utf-8",
        )
    } else {
        (exchange::snapshot_to_json(&snapshot), "application/json")
    };
    let mut response = axum::http::Response::new(Body::from(text));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn import(
    State(state): State<ApiState>,
    headers: HeaderMap,
    caller: Caller,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let by = person(&caller)?;
    authorize(&state, &headers)?;
    require_cos_enabled(&state)?;
    let req: ChatExchangeImportRequest = chat_json(body).await?;
    let req = ImportRequest::from(req);
    let received = state
        .blocking(move |store| {
            store
                .chat_exchange_import(&t, &by, &req, OffsetDateTime::now_utc())
                .map_err(exchange_problem)
        })
        .await?;
    let status = if received.duplicate {
        StatusCode::OK
    } else {
        changed(&state);
        StatusCode::CREATED
    };
    Ok(json_response(
        status,
        &ChatExchangeImportResponse::from(received),
    ))
}
