//! Phase 1 model routing catalog. Only explicitly selected public fields cross this boundary.

use std::sync::Arc;

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::Serialize;
use task_core::Tier;
use task_core::model_router::{
    policy::{RoutingMode, RoutingPolicy},
    profiles::{Billing, ContextLimits, QualityIndex, TokenPricing},
};

use crate::handlers::{ApiResult, json_response, no_query};
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// Capability support is nullable because legacy configuration may not specify it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CatalogCapabilitiesView {
    pub tools: Option<bool>,
    pub structured_output: Option<bool>,
    pub vision: Option<bool>,
    pub streaming: Option<bool>,
    pub reasoning_efforts: Option<Vec<String>>,
}

/// Catalog model metadata. Unknown capability support is null.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CatalogModelView {
    pub id: String,
    pub revision: String,
    pub family: String,
    pub capabilities: CatalogCapabilitiesView,
    pub context_limits: ContextLimits,
    pub quality: Option<Vec<QualityIndex>>,
    pub pricing: Option<TokenPricing>,
}

/// A deployment is a source/model pairing; credentials and endpoint URLs are omitted.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CatalogDeploymentView {
    pub id: String,
    pub source_ref: String,
    pub model_profile_id: String,
    pub upstream_model: String,
    pub billing: Billing,
    pub allowed_lanes: Vec<Tier>,
    pub price_override: Option<TokenPricing>,
}

/// Credential-free snapshot returned by `GET /llm/routing/catalog`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct RoutingCatalogView {
    pub catalog_version: String,
    pub mode: RoutingMode,
    pub models: Vec<CatalogModelView>,
    pub deployments: Vec<CatalogDeploymentView>,
    pub policies: Vec<RoutingPolicy>,
    pub warnings: Vec<String>,
}

/// The daemon owns config and reload; the API reads one immutable snapshot per request.
pub trait RoutingCatalogReader: Send + Sync + 'static {
    fn view(&self) -> RoutingCatalogView;
}

pub type SharedRoutingCatalogReader = Arc<dyn RoutingCatalogReader>;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route("/api/v1/llm/routing/catalog", axum::routing::get(catalog))
}

pub(crate) async fn catalog(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let reader = state
        .inner
        .routing_catalog
        .as_ref()
        .ok_or_else(ApiProblem::llm_proxy_unavailable)?;
    Ok(json_response(StatusCode::OK, &reader.view()))
}
