//! ADR 2026-10-22-dynamic-config: config.toml の欄の閲覧・変更・履歴（`/config/fields`・`/config/events`）。
//! 検証・書き戻し・稼働中の差し替えは celeris（`AdminRequest::Config*`）が行い、履歴の読みだけが API の store。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};

use task_core::TaskStore;

use crate::admin::{AdminRequest, ConfigAdminError};
use crate::middleware::{require_active, require_admin};
use crate::problem::ApiProblem;
use crate::state::ApiState;
use crate::types::{ConfigEvent, ConfigEventList, ConfigPatchBody};

use super::{ApiResult, json_response, no_query, read_json, rfc3339};

/// 履歴の 1 回の応答の件数（新しい順）。
const EVENTS_LIMIT: usize = 200;

fn problem(error: ConfigAdminError) -> ApiProblem {
    match error {
        ConfigAdminError::Invalid(detail) => {
            ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, "config_invalid", detail)
        }
        ConfigAdminError::RestartRequired(detail) => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "config_restart_required",
            detail,
        ),
        ConfigAdminError::Internal(detail) => ApiProblem::internal(detail),
    }
}

async fn ask<T>(
    state: &ApiState,
    build: impl FnOnce(tokio::sync::oneshot::Sender<Result<T, ConfigAdminError>>) -> AdminRequest,
) -> Result<T, ApiProblem> {
    require_active(state)?;
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Err(ApiProblem::internal("daemon administration is unavailable"));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx.send(build(reply_tx)).await.is_err() {
        return Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
        Ok(Ok(result)) => result.map_err(problem),
        Ok(Err(_)) => Err(ApiProblem::internal("celeris dropped the config request")),
        Err(_) => Err(ApiProblem::internal("config request timed out")),
    }
}

/// `GET /config/fields`。
pub(super) async fn get_fields(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let view = ask(&state, |reply| AdminRequest::ConfigFields { reply }).await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// `PATCH /config/fields`: 検証に落ちたら 422 で何も変えない。
pub(super) async fn patch_fields(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let patch: ConfigPatchBody = read_json(body, false).await?;
    if patch.changes.is_empty() {
        return Err(ApiProblem::bad_request("changes must not be empty"));
    }
    let result = ask(&state, |reply| AdminRequest::ConfigUpdate {
        changes: patch.changes,
        actor: "admin".to_string(),
        reply,
    })
    .await?;
    Ok(json_response(StatusCode::OK, &result))
}

/// `GET /config/events`。
pub(super) async fn get_events(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let rows = state
        .blocking(|store| {
            store
                .config_events_recent(EVENTS_LIMIT)
                .map_err(crate::problem::store_problem)
        })
        .await?;
    let items = rows
        .into_iter()
        .map(|row| ConfigEvent {
            id: row.id,
            ts: rfc3339(row.ts),
            actor: row.actor,
            key: row.key,
            class: row.class,
            apply_unit: row.apply_unit,
            before: Some(row.before).filter(|v| !v.is_null()),
            after: Some(row.after).filter(|v| !v.is_null()),
        })
        .collect();
    Ok(json_response(StatusCode::OK, &ConfigEventList { items }))
}
