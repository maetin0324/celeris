//! `POST /api/v1/cos/operations` and `GET /api/v1/cos/operations/{o}` (ADR 2026-10-05 D2/D3).
//! The ops-api WorkUnit adds the routes here; `super::routes` already merges this router.

use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
}
