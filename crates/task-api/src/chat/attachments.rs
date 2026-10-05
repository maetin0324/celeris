//! Upload and download routes are filled by the api-attach work unit.
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
}
