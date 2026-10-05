use super::{context::RoutingContext, profiles::ModelProfile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod sidecar;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorDescriptor {
    pub id: String,
    pub version: String,
    pub maturity: String,
    pub external_dependencies: Vec<String>,
    pub needs_network: bool,
    pub needs_prompt: bool,
    pub supported_domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityEstimate {
    pub index: Option<f64>,
    pub confidence: Option<f64>,
    pub reasons: Vec<String>,
    pub feature_version: String,
}

pub trait QualityEstimator: Send + Sync {
    fn descriptor(&self) -> EstimatorDescriptor;
    fn estimate(&self, model: &ModelProfile, context: &RoutingContext) -> QualityEstimate;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicEstimator;

impl QualityEstimator for HeuristicEstimator {
    fn descriptor(&self) -> EstimatorDescriptor {
        EstimatorDescriptor {
            id: "heuristic".into(),
            version: "1".into(),
            maturity: "initial".into(),
            external_dependencies: vec![],
            needs_network: false,
            needs_prompt: false,
            supported_domains: vec!["general".into()],
        }
    }
    fn estimate(&self, model: &ModelProfile, context: &RoutingContext) -> QualityEstimate {
        let domain = context.task_kind.as_deref().unwrap_or("general");
        let selected = model
            .quality
            .iter()
            .find(|q| q.domain == domain)
            .or_else(|| model.quality.iter().find(|q| q.domain == "general"));
        let index = selected
            .map(|q| q.index)
            .filter(|q| q.is_finite() && (0.0..=1.0).contains(q));
        QualityEstimate {
            index,
            confidence: None,
            reasons: vec![if selected.is_some() {
                format!("profile_quality:{domain}")
            } else {
                "quality_unknown".into()
            }],
            feature_version: "heuristic-1".into(),
        }
    }
}
