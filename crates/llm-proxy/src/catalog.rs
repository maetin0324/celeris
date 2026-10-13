//! Catalog view consumed by the proxy and its shadow estimators.
//!
//! Celeris constructs this from its validated routing catalog. The proxy never
//! derives catalog entries from `[llm_proxy]` or role assignments.

use task_core::Tier;
use task_core::model_router::policy::RoutingPolicy;
use task_core::model_router::profiles::{DeploymentProfile, ModelProfile};

#[derive(Debug, Clone, Default)]
pub struct ProxyCatalog {
    pub models: Vec<ModelProfile>,
    pub deployments: Vec<DeploymentProfile>,
    pub policies: Vec<RoutingPolicy>,
    pub warnings: Vec<String>,
}

impl ProxyCatalog {
    pub fn deployment(&self, source_ref: &str, tier: Tier) -> Option<&DeploymentProfile> {
        self.deployments
            .iter()
            .find(|d| d.source_ref == source_ref && d.allowed_lanes.contains(&tier))
    }
}
