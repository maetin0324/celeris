use super::stage::RoutingStage;
use crate::llm_route::{Candidate, CandidateKey, ExclusionReason};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteAction {
    Shadow,
    Enforce,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteDecision {
    pub candidates: Vec<Candidate>,
    pub equivalent: Vec<CandidateKey>,
    pub exclusions: Vec<(CandidateKey, ExclusionReason)>,
    pub selected: Candidate,
    pub action: RouteAction,
    pub propensity: f64,
    pub stage: RoutingStage,
    pub evidence_version: Option<String>,
    pub source: String,
}
