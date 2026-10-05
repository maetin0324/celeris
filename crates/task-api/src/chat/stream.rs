//! SSE routes are filled by the api-sse work unit.
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
}
