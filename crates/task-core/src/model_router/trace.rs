use super::{estimator::QualityEstimate, policy::RoutingMode};
use crate::Tier;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CandidateTrace {
    pub model_profile_id: String,
    pub deployment_id: String,
    pub eligible_provider_ids: Vec<String>,
    pub excluded_reasons: Vec<String>,
    pub quality: Option<QualityEstimate>,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<f64>,
    pub pressure: Option<f64>,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingTraceV1 {
    pub decision_id: String,
    pub parent_decision_id: Option<String>,
    pub task_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub run_id: Option<String>,
    pub request_id: Option<String>,
    pub stage: String,
    pub mode: RoutingMode,
    pub policy_version: String,
    pub catalog_version: String,
    pub feature_version: String,
    pub estimator_version: String,
    pub snapshot_id: String,
    pub observed_at: Option<String>,
    pub requested_lane: Tier,
    pub selected_lane: Option<Tier>,
    pub candidates: Vec<CandidateTrace>,
    pub selected: Option<String>,
    pub fallback_order: Vec<String>,
    pub reasons: Vec<String>,
}
