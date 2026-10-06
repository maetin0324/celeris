//! Legacy `[llm_proxy.models]` and `[llm_proxy.sources]` catalog normalization.
//! The old keys remain the authority in legacy mode. Missing capability and price
//! observations stay unknown; no source URL or credential is copied to the catalog.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::Tier;
use task_core::model_router::policy::{RoutingMode, RoutingPolicy};
use task_core::model_router::profiles::{
    Billing, Capabilities, ContextLimits, DeploymentProfile, ModelProfile, Support,
};

use crate::config::LlmProxyConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LegacyCatalog {
    pub models: Vec<ModelProfile>,
    pub deployments: Vec<DeploymentProfile>,
    pub policies: Vec<RoutingPolicy>,
}

impl LegacyCatalog {
    pub fn deployment(&self, source_ref: &str, tier: Tier) -> Option<&DeploymentProfile> {
        self.deployments
            .iter()
            .find(|d| d.source_ref == source_ref && d.allowed_lanes.contains(&tier))
    }
}

/// Derive model, deployment and policy values from the existing proxy config.
/// Source-qualified model IDs deliberately avoid merging aliases whose identity
/// and revision cannot be established from the legacy configuration.
pub fn normalize_legacy_config(config: &LlmProxyConfig) -> LegacyCatalog {
    let mut catalog = LegacyCatalog {
        models: Vec::new(),
        deployments: Vec::new(),
        policies: [Tier::Frontier, Tier::Standard, Tier::Cheap]
            .into_iter()
            .map(|lane| {
                let mut policy = RoutingPolicy::defaults(lane, RoutingMode::Legacy);
                policy.prefer_free = config.prefer_free;
                policy
            })
            .collect(),
    };
    let mut add = |source_ref: &str, family: &str, billing: Billing, tier: Tier, wire: &str| {
        let model_id = format!("legacy:{family}:{wire}");
        if !catalog.models.iter().any(|m| m.id == model_id) {
            catalog.models.push(ModelProfile {
                id: model_id.clone(),
                revision: "unknown".into(),
                family: family.into(),
                capabilities: Capabilities {
                    tools: Support::Unknown,
                    structured_output: Support::Unknown,
                    vision: Support::Unknown,
                    streaming: Support::Unknown,
                    reasoning_efforts: vec![],
                },
                context_limits: ContextLimits {
                    input: None,
                    output: None,
                    total: None,
                },
                quality: vec![],
                pricing: None,
                provenance: "llm_proxy.models (legacy)".into(),
            });
        }
        let order = catalog.deployments.len();
        catalog.deployments.push(DeploymentProfile {
            id: format!("legacy:{source_ref}:{tier:?}"),
            source_ref: source_ref.into(),
            model_profile_id: model_id,
            upstream_model: wire.into(),
            adapter_constraints: vec![],
            billing,
            host: None,
            region: None,
            trust_zone: None,
            external_network: true,
            retains_data: None,
            allowed_lanes: vec![tier],
            resource_group_id: None,
            concurrency_limit: None,
            rpm_limit: None,
            tpm_limit: None,
            price_override: None,
            config_order: order,
        });
    };
    for tier in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
        if config
            .sources
            .claude_oauth
            .as_ref()
            .is_some_and(|s| s.enabled)
            && let Some(wire) = config.models.claude.get(&tier)
        {
            add("claude-oauth", "claude", Billing::Subscription, tier, wire);
        }
        if config
            .sources
            .codex_oauth
            .as_ref()
            .is_some_and(|s| s.enabled)
            && let Some(wire) = config.models.gpt.get(&tier)
        {
            add("codex-oauth", "gpt", Billing::Subscription, tier, wire);
        }
        // The deployment lane is fixed here, independently of source or wire prefixes.
        if tier == Tier::Cheap
            && let Some(wire) = config.models.qwen.get(&Tier::Cheap)
        {
            for source in config
                .sources
                .openai_compatible
                .iter()
                .filter(|s| s.enabled)
            {
                add(
                    &format!("openai-compatible:{}", source.id),
                    "qwen",
                    Billing::SelfHosted,
                    tier,
                    wire,
                );
            }
        }
    }
    catalog
}
