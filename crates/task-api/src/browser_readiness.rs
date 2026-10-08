//! D5: daemon-owned, read-only browser diagnostics. Probes run off the API executor.
use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};

use crate::{problem::ApiProblem, state::ApiState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessItem {
    pub status: String,
    pub check: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserReadiness {
    pub items: Vec<ReadinessItem>,
}

impl BrowserReadiness {
    pub fn push(&mut self, status: &str, check: &str, detail: impl Into<String>) {
        self.items.push(ReadinessItem {
            status: status.into(),
            check: check.into(),
            detail: detail.into(),
        });
    }

    pub fn has_missing(&self) -> bool {
        self.items.iter().any(|item| item.status == "NG")
    }
}

/// celeris supplies the process configuration and probes; task-api does not depend on worker.
pub type ReadinessProbe = Arc<dyn Fn(&task_core::SqliteStore) -> BrowserReadiness + Send + Sync>;

async fn readiness(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<BrowserReadiness>, ApiProblem> {
    crate::middleware::require_admin(&state, &headers)?;
    let probe = state.browser_readiness.clone().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "browser_readiness_unavailable",
            "daemon readiness probe is unavailable",
        )
    })?;
    state.blocking(move |store| Ok(Json(probe(store)))).await
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route("/api/v1/browser/readiness", axum::routing::get(readiness))
}
