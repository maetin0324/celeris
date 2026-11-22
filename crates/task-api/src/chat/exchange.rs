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
use task_core::discussion_exchange::{
    self as exchange, ExchangeError, ExchangeShareRequest, ExchangeTarget, ImportRequest,
    SNAPSHOT_ITEMS_DEFAULT, SNAPSHOT_ITEMS_MAX, SnapshotQuery,
};
use time::OffsetDateTime;

use super::{authorize, changed, chat_json, chat_problem, require_cos_enabled};
use crate::exchange_wire::{
    ChatExchangeImportRequest, ChatExchangeImportResponse, ChatExchangeShareRequest,
    ChatExchangeShareResponse,
};
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
