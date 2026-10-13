//! Read-only view of declared LLM harnesses and their selection metadata.
use std::sync::Arc;

use axum::extract::{Path, RawQuery, State};

use crate::handlers::{ApiResult, json_response};
use crate::problem::ApiProblem;
use crate::state::ApiState;
use crate::types::HarnessView;

pub trait HarnessesReader: Send + Sync + 'static {
    fn harnesses(&self) -> Vec<HarnessView>;
}

pub type SharedHarnessesReader = Arc<dyn HarnessesReader>;

async fn list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    let Some(reader) = state.inner.harnesses.clone() else {
        return Err(ApiProblem::harnesses_unavailable());
    };
    Ok(json_response(
        axum::http::StatusCode::OK,
        &reader.harnesses(),
    ))
}

async fn detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    let Some(reader) = state.inner.harnesses.clone() else {
        return Err(ApiProblem::harnesses_unavailable());
    };
    let item = reader
        .harnesses()
        .into_iter()
        .find(|h| h.id == id)
        .ok_or_else(ApiProblem::not_found)?;
    Ok(json_response(axum::http::StatusCode::OK, &item))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::get;
    axum::Router::new()
        .route("/api/v1/harnesses", get(list))
        .route("/api/v1/harnesses/{harness_id}", get(detail))
}
