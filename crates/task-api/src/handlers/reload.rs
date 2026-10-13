//! Daemon configuration reload endpoint.

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};

use crate::cos::operations::{Effect, effect_result};
use crate::middleware::{require_active, require_admin};
use crate::problem::ApiProblem;
use crate::state::ApiState;
use crate::types::ReloadResult;

use super::{ApiResult, json_response, no_query};

pub(super) async fn reload(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let result = effect_result(reload_effect(&state).await)?;
    Ok(json_response(StatusCode::OK, &result))
}

/// The daemon side of `POST /reload` (shared with CoS `daemon.reload`).
pub(crate) async fn reload_effect(state: &ApiState) -> Effect<ReloadResult> {
    if let Err(problem) = require_active(state) {
        return Ok(Err(problem));
    }
    let Some(admin_tx) = state.inner.admin_tx.clone() else {
        return Ok(Err(ApiProblem::internal(
            "daemon administration is unavailable",
        )));
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if admin_tx
        .send(crate::admin::AdminRequest::Reload { reply: reply_tx })
        .await
        .is_err()
    {
        return Ok(Err(ApiProblem::internal(
            "celeris is not accepting admin requests",
        )));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
        Ok(Ok(Ok(()))) => Ok(Ok(ReloadResult { reloaded: true })),
        Ok(Ok(Err(message))) => Ok(Err(ApiProblem::bad_request(format!(
            "invalid config: {message}"
        )))),
        Ok(Err(_)) => Err(ApiProblem::internal("celeris dropped the reload request")),
        Err(_) => Err(ApiProblem::internal("reload timed out")),
    }
}
