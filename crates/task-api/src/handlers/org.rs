//! 組織のノードの一覧と編集（`GET/POST /org`、`PATCH/DELETE /org/{id}`。ADR-0033 D1）。
//!
//! 編集の route は plan と transaction の中の書き込みを `crate::org_write` に任せ、認可・本文の読み取り・
//! response だけを持つ（ADR 2026-11-18-org-write-shared-use-case D2）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use serde_json::Value;

use crate::middleware::require_admin;
use crate::org_write;
use crate::problem::store_problem;
use crate::state::ApiState;
use crate::types::{OrgCreateBody, OrgList};

use crate::http_io::{ApiResult, Params, json_response, no_query, read_json};

pub(super) async fn patch_browser_settings(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let raw: Value = read_json(body, true).await?;
    let genres = state.inner.genres.clone();
    let node = state
        .blocking(move |store| {
            let write = org_write::plan_browser_settings(store, &genres, &id, raw)?;
            store.write_tx(|tx| org_write::apply(tx, &write, "admin"))
        })
        .await?
        .into_node()?;
    Ok(json_response(StatusCode::OK, &node))
}

pub(super) async fn org_list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let (items, lead_sessions) = state
        .blocking(|store| {
            let items = store.org_list().map_err(store_problem)?;
            // ADR-0054 D3（Phase 68）: 部門長（`OrgKind::Department`）の継続セッションがあれば拾う
            // （無いノードは含めない。CoS はここに出さない。
            // `SqliteStore` を薄く読むだけ）。
            let mut lead_sessions = Vec::new();
            for node in items
                .iter()
                .filter(|n| n.kind == task_core::OrgKind::Department)
            {
                if let Some(session) = store
                    .node_session_active(&node.id, task_core::SessionKind::Lead, None)
                    .map_err(store_problem)?
                {
                    lead_sessions.push(crate::types::NodeSessionSummary {
                        node_id: session.node_id,
                        turns: session.turns,
                        approx_tokens: session.approx_tokens,
                        last_used_at: session.last_used_at,
                    });
                }
            }
            Ok((items, lead_sessions))
        })
        .await?;
    // ADR-0046 D1: 継いだ後の実効 profile も一緒に返す（計算は純粋関数。DB には保存しない）。
    let effective_profiles = items
        .iter()
        .map(|n| task_core::resolve_profile(&items, &n.id))
        .collect();
    Ok(json_response(
        StatusCode::OK,
        &OrgList {
            items,
            effective_profiles,
            lead_sessions,
        },
    ))
}

pub(super) async fn create_org_node(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let create: OrgCreateBody = read_json(body, false).await?;
    let genres = state.inner.genres.clone();
    let node = state
        .blocking(move |store| {
            let write = org_write::plan_create(store, &genres, create)?;
            store.write_tx(|tx| org_write::apply(tx, &write, "admin"))
        })
        .await?
        .into_node()?;
    tracing::info!(who = "admin", op = "org_create", org_id = %node.id, "admin: org node created");
    let mut response = json_response(StatusCode::CREATED, &node);
    if let Ok(location) = HeaderValue::from_str(&format!("/api/v1/org/{}", node.id)) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

pub(super) async fn patch_org_node(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let raw: Value = read_json(body, true).await?;
    let genres = state.inner.genres.clone();
    let node = state
        .blocking(move |store| {
            let write = org_write::plan_patch(store, &genres, &id, raw)?;
            store.write_tx(|tx| org_write::apply(tx, &write, "admin"))
        })
        .await?
        .into_node()?;
    tracing::info!(who = "admin", op = "org_patch", org_id = %node.id, "admin: org node updated");
    Ok(json_response(StatusCode::OK, &node))
}

/// `DELETE /org/{id}`: 204 whether or not the row was still there at the write (404 only from the plan).
pub(super) async fn delete_org_node(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    state
        .blocking(move |store| {
            let write = org_write::plan_delete(store, &id)?;
            store.write_tx(|tx| org_write::apply(tx, &write, "admin"))?;
            tracing::info!(who = "admin", op = "org_delete", org_id = %id, "admin: org node deleted");
            Ok(())
        })
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
