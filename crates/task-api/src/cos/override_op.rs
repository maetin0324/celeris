//! Human-only correction of an applied CoS operation.
use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde::Deserialize;
use task_core::chat::OverrideAction;
use time::OffsetDateTime;

use super::{cos_bearer_secret, cos_problem};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::ApiProblem;
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route(
        "/api/v1/cos/operations/{o}/override",
        post(override_operation),
    )
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode {
    Revoke,
    Return,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverrideBody {
    pub action: OverrideMode,
    pub reason: String,
}

async fn override_operation(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    // The general admin helper also accepts a verified CoS bearer. This route is human-only.
    if cos_bearer_secret(&headers).is_some() {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "human_credential_required",
            "only a human credential may override an operation",
        ));
    }
    require_admin(&state, &headers)?;
    let request: OverrideBody = read_json(body, false).await?;
    let action = match request.action {
        OverrideMode::Revoke => OverrideAction::Revoke,
        OverrideMode::Return => OverrideAction::Return,
    };
    if request.reason.trim().is_empty() {
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "reason must not be empty",
        ));
    }
    let result = state
        .blocking(move |store| {
            store
                .cos_operation_override_at(&id, action, &request.reason, OffsetDateTime::now_utc())
                .map_err(cos_problem)
        })
        .await?;
    state.chat.events.notify_waiters();
    Ok(json_response(
        StatusCode::OK,
        &serde_json::json!({
            "operation_id":result.operation_id,
            "state":result.state,
            "action":match result.action { OverrideAction::Revoke=>"revoke",OverrideAction::Return=>"return" },
            "new_revision":result.new_revision,
            "new_wait_id":result.new_wait_id,
            "remediation_task_id":result.remediation_task_id,
            "paused_task_ids":result.paused_task_ids,
        }),
    ))
}
