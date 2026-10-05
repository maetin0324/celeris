//! Phase 1 catalog adapter. It does not change the legacy execution selector.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use task_core::Tier;
use task_core::model_router::{
    policy::{RoutingMode, RoutingPolicy, Weights},
    profiles::{Billing, Capabilities, ContextLimits, DeploymentProfile, ModelProfile, Support},
};

use super::{Config, ConfigError};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelRoutingConfig {
    pub mode: Option<RoutingMode>,
    pub models: Vec<ModelEntry>,
    pub deployments: Vec<DeploymentEntry>,
    pub policies: std::collections::HashMap<Tier, PolicyEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub provenance: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Capabilities>,
    #[serde(default)]
    pub context_limits: Option<ContextLimits>,
    #[serde(default)]
    pub quality: Option<Vec<task_core::model_router::profiles::QualityIndex>>,
    #[serde(default)]
    pub pricing: Option<task_core::model_router::profiles::TokenPricing>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentEntry {
    pub id: String,
    pub source_ref: String,
    pub model_profile_id: String,
    pub upstream_model: String,
    #[serde(default)]
    pub allowed_lanes: Option<Vec<Tier>>,
    #[serde(default)]
    pub billing: Option<Billing>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub trust_zone: Option<String>,
    #[serde(default)]
    pub external_network: Option<bool>,
    #[serde(default)]
    pub retains_data: Option<bool>,
    #[serde(default)]
    pub resource_group_id: Option<String>,
    #[serde(default)]
    pub concurrency_limit: Option<u32>,
    #[serde(default)]
    pub rpm_limit: Option<u32>,
    #[serde(default)]
    pub tpm_limit: Option<u32>,
    #[serde(default)]
    pub price_override: Option<task_core::model_router::profiles::TokenPricing>,
    #[serde(default)]
    pub adapter_constraints: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyEntry {
    pub version: Option<String>,
    pub objective: Option<task_core::model_router::policy::Objective>,
    pub min_quality: Option<f64>,
    pub weights: Option<Weights>,
    pub normalization: Option<task_core::model_router::policy::Normalization>,
    pub constraints: Option<task_core::model_router::policy::Constraints>,
    pub fallback: Option<bool>,
    pub escalation: Option<bool>,
    pub local_preference: Option<bool>,
    pub prefer_free: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingCatalog {
    pub mode: RoutingMode,
    pub models: Vec<ModelProfile>,
    pub deployments: Vec<DeploymentProfile>,
    pub policies: Vec<RoutingPolicy>,
    pub warnings: Vec<String>,
}

fn unknown_model(id: &str, provenance: &str) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        revision: "unknown".into(),
        family: "unknown".into(),
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
        provenance: provenance.into(),
    }
}

fn deployment(
    id: String,
    source_ref: String,
    model: String,
    upstream_model: String,
    lanes: Vec<Tier>,
    order: usize,
) -> DeploymentProfile {
    DeploymentProfile {
        id,
        source_ref,
        model_profile_id: model,
        upstream_model,
        adapter_constraints: vec![],
        billing: Billing::Subscription,
        host: None,
        region: None,
        trust_zone: None,
        external_network: true,
        retains_data: None,
        allowed_lanes: lanes,
        resource_group_id: None,
        concurrency_limit: None,
        rpm_limit: None,
        tpm_limit: None,
        price_override: None,
        config_order: order,
    }
}

fn source_name(source: &task_core::LlmSourceRef) -> String {
    match source {
        task_core::LlmSourceRef::OpenaiCompatible(id) => format!("openai_compatible:{id}"),
        other => other.as_str().into(),
    }
}

fn lane_name(lane: Tier) -> &'static str {
    match lane {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

impl Config {
    /// Builds a complete, credential-free snapshot before it can replace a running one.
    pub fn routing_catalog(&self) -> Result<RoutingCatalog, ConfigError> {
        let mode = self.model_routing.mode.unwrap_or(RoutingMode::Legacy);
        if mode == RoutingMode::Enforce {
            return Err(ConfigError::Invalid(
                "model_routing.mode=enforce requires Phase 2".into(),
            ));
        }
        let mut models = BTreeMap::<String, ModelProfile>::new();
        let mut deployments = BTreeMap::<String, DeploymentProfile>::new();
        let mut warnings = BTreeSet::<String>::new();
        let mut order = 0;
        // Reuse the proxy adapter's identity and lane contract. In particular it keeps
        // source-qualified model IDs so equal wire aliases do not imply shared identity.
        let proxy_catalog = llm_proxy::legacy_catalog::normalize_legacy_config(&self.llm_proxy);
        for model in proxy_catalog.models {
            models.insert(model.id.clone(), model);
        }
        for dep in proxy_catalog.deployments {
            let family = dep.model_profile_id.split(':').nth(1).unwrap_or("unknown");
            let lane = dep.allowed_lanes[0];
            warnings.insert(format!(
                "llm_proxy.models.{family}.{} -> model_routing.models/deployments (legacy)",
                lane_name(lane)
            ));
            order = order.max(dep.config_order + 1);
            deployments.insert(dep.id.clone(), dep);
        }
        for provider in &self.providers {
            let Some(resolved) = self.provider_llm_source(&provider.id) else {
                continue;
            };
            let source = source_name(&resolved.source);
            if !provider.model.is_empty()
                && !provider.model.contains("celeris/")
                && !matches!(
                    resolved.source,
                    task_core::LlmSourceRef::None | task_core::LlmSourceRef::Unknown
                )
            {
                let wire = provider.model.clone();
                models
                    .entry(wire.clone())
                    .or_insert_with(|| unknown_model(&wire, "providers.model"));
                let dep_id = format!("provider:{}", provider.id);
                let lanes = self
                    .provider_specs()
                    .into_iter()
                    .find(|s| s.id == provider.id)
                    .map(|s| s.tiers)
                    .unwrap_or_default();
                deployments.insert(
                    dep_id.clone(),
                    deployment(dep_id, source.clone(), wire.clone(), wire, lanes, order),
                );
                order += 1;
            }
            for lane in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
                if let Some(binding) = provider.tier_models.get(&lane) {
                    if binding.unavailable_reason.is_some() {
                        continue;
                    }
                    let wire = binding.model_id.as_deref().unwrap_or(&binding.name);
                    if wire.is_empty() {
                        continue;
                    }
                    models
                        .entry(wire.into())
                        .or_insert_with(|| unknown_model(wire, "providers.tier_models"));
                    let dep_id = format!("provider:{}/{}", provider.id, lane_name(lane));
                    deployments.insert(
                        dep_id.clone(),
                        deployment(
                            dep_id,
                            source.clone(),
                            wire.into(),
                            wire.into(),
                            vec![lane],
                            order,
                        ),
                    );
                    order += 1;
                    warnings.insert(format!(
                        "providers.{}.tier_models.{} -> model_routing.deployments (legacy)",
                        provider.id,
                        lane_name(lane)
                    ));
                }
            }
            if provider.llm_source.is_some() {
                warnings.insert(format!(
                    "providers.{}.llm_source -> model_routing.deployments.source_ref (legacy)",
                    provider.id
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for entry in &self.model_routing.models {
            if entry.id.trim().is_empty() || !seen.insert(entry.id.clone()) {
                return Err(ConfigError::Invalid(
                    "model_routing.models: empty or duplicate id".into(),
                ));
            }
            if models.contains_key(&entry.id) {
                warnings.insert(format!("model_routing.models.{} overrides llm_proxy.models/providers.tier_models -> model_routing.models", entry.id));
            }
            let mut model = models
                .get(&entry.id)
                .cloned()
                .unwrap_or_else(|| unknown_model(&entry.id, "model_routing.models"));
            if let Some(v) = &entry.revision {
                model.revision = v.clone();
            }
            if let Some(v) = &entry.family {
                model.family = v.clone();
            }
            if let Some(v) = &entry.provenance {
                model.provenance = v.clone();
            }
            if let Some(v) = &entry.capabilities {
                model.capabilities = v.clone();
            }
            if let Some(v) = &entry.context_limits {
                model.context_limits = v.clone();
            }
            if let Some(v) = &entry.quality {
                model.quality = v.clone();
            }
            if let Some(v) = &entry.pricing {
                model.pricing = Some(v.clone());
            }
            models.insert(entry.id.clone(), model);
        }
        for model in models.values() {
            if model
                .quality
                .iter()
                .any(|q| !q.index.is_finite() || !(0.0..=1.0).contains(&q.index))
            {
                return Err(ConfigError::Invalid(format!(
                    "model_routing.models.{}: quality index must be finite and in [0,1]",
                    model.id
                )));
            }
            if let Some(pricing) = &model.pricing {
                for value in [
                    pricing.input_usd_per_million,
                    pricing.cached_input_usd_per_million,
                    pricing.output_usd_per_million,
                ]
                .into_iter()
                .flatten()
                {
                    if !value.is_finite() || value < 0.0 {
                        return Err(ConfigError::Invalid(format!(
                            "model_routing.models.{}: pricing must be finite and nonnegative",
                            model.id
                        )));
                    }
                }
            }
        }
        seen.clear();
        for entry in &self.model_routing.deployments {
            if entry.id.trim().is_empty() || !seen.insert(entry.id.clone()) {
                return Err(ConfigError::Invalid(
                    "model_routing.deployments: empty or duplicate id".into(),
                ));
            }
            if !models.contains_key(&entry.model_profile_id) {
                return Err(ConfigError::Invalid(format!(
                    "model_routing.deployments.{}: unknown model_profile_id",
                    entry.id
                )));
            }
            let known_source = match entry.source_ref.as_str() {
                "claude-oauth" | "claude_oauth" => self.llm_proxy.sources.claude_oauth.is_some(),
                "codex-oauth" | "codex_oauth" => self.llm_proxy.sources.codex_oauth.is_some(),
                s if s.starts_with("openai-compatible:") || s.starts_with("openai_compatible:") => {
                    self.llm_proxy
                        .sources
                        .openai_compatible
                        .iter()
                        .any(|source| {
                            s == format!("openai-compatible:{}", source.id)
                                || s == format!("openai_compatible:{}", source.id)
                        })
                }
                s if s.starts_with("provider:") => self
                    .providers
                    .iter()
                    .any(|p| s == format!("provider:{}", p.id)),
                _ => false,
            };
            if !known_source {
                return Err(ConfigError::Invalid(format!(
                    "model_routing.deployments.{}: unknown source_ref",
                    entry.id
                )));
            }
            if let Some(old) = deployments.get(&entry.id) {
                if old.source_ref != entry.source_ref
                    || old.model_profile_id != entry.model_profile_id
                    || old.upstream_model != entry.upstream_model
                {
                    return Err(ConfigError::Invalid(format!(
                        "model_routing.deployments.{}: legacy identity conflicts",
                        entry.id
                    )));
                }
                warnings.insert(format!("model_routing.deployments.{} overrides llm_proxy.models/providers.tier_models -> model_routing.deployments", entry.id));
            }
            let mut dep = deployments.get(&entry.id).cloned().unwrap_or_else(|| {
                deployment(
                    entry.id.clone(),
                    entry.source_ref.clone(),
                    entry.model_profile_id.clone(),
                    entry.upstream_model.clone(),
                    vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
                    order,
                )
            });
            if let Some(lanes) = &entry.allowed_lanes {
                if lanes.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "model_routing.deployments.{}: allowed_lanes is empty",
                        entry.id
                    )));
                }
                dep.allowed_lanes = lanes.clone();
            }
            if (entry.model_profile_id.to_ascii_lowercase().contains("qwen")
                || entry.source_ref.to_ascii_lowercase().contains("qwen")
                || models
                    .get(&entry.model_profile_id)
                    .is_some_and(|m| m.family.eq_ignore_ascii_case("qwen")))
                && dep.allowed_lanes.iter().any(|l| *l != Tier::Cheap)
            {
                return Err(ConfigError::Invalid(format!(
                    "model_routing.deployments.{}: Qwen is cheap only",
                    entry.id
                )));
            }
            if let Some(v) = entry.billing {
                dep.billing = v;
            }
            dep.host = entry.host.clone();
            dep.region = entry.region.clone();
            dep.trust_zone = entry.trust_zone.clone();
            if let Some(v) = entry.external_network {
                dep.external_network = v;
            }
            dep.retains_data = entry.retains_data;
            dep.resource_group_id = entry.resource_group_id.clone();
            dep.concurrency_limit = entry.concurrency_limit;
            dep.rpm_limit = entry.rpm_limit;
            dep.tpm_limit = entry.tpm_limit;
            dep.price_override = entry.price_override.clone();
            dep.adapter_constraints = entry.adapter_constraints.clone();
            deployments.insert(entry.id.clone(), dep);
            order += 1;
        }
        let mut policies = Vec::new();
        for lane in [Tier::Frontier, Tier::Standard, Tier::Cheap] {
            let mut policy = RoutingPolicy::defaults(lane, mode);
            policy.prefer_free = self.llm_proxy.prefer_free;
            if let Some(row) = self.model_routing.policies.get(&lane) {
                if let Some(v) = &row.version {
                    policy.version = v.clone();
                }
                if let Some(v) = row.objective {
                    policy.objective = v;
                }
                if let Some(v) = row.min_quality {
                    policy.min_quality = v;
                }
                if let Some(v) = &row.weights {
                    policy.weights = v.clone();
                }
                if let Some(v) = &row.normalization {
                    policy.normalization = v.clone();
                }
                if let Some(v) = &row.constraints {
                    policy.constraints = v.clone();
                }
                if let Some(v) = row.fallback {
                    policy.fallback = v;
                }
                if let Some(v) = row.escalation {
                    policy.escalation = v;
                }
                if let Some(v) = row.local_preference {
                    policy.local_preference = v;
                }
                if let Some(v) = row.prefer_free {
                    policy.prefer_free = v;
                }
                warnings.insert(format!("model_routing.policies.{lane:?} overrides llm_proxy.prefer_free -> model_routing.policies (legacy)"));
            }
            policy.validate().map_err(|reason| {
                ConfigError::Invalid(format!("model_routing.policies.{lane:?}: {reason}"))
            })?;
            if let Some(ref ids) = policy.constraints.allowed_deployments {
                for id in ids {
                    if !deployments.contains_key(id) {
                        return Err(ConfigError::Invalid(format!(
                            "model_routing.policies.{}: unknown deployment reference",
                            lane_name(lane)
                        )));
                    }
                }
            }
            if let Some(ref ids) = policy.constraints.allowed_sources {
                for id in ids {
                    if !deployments.values().any(|d| d.source_ref == *id) {
                        return Err(ConfigError::Invalid(format!(
                            "model_routing.policies.{}: unknown source reference",
                            lane_name(lane)
                        )));
                    }
                }
            }
            policies.push(policy);
        }
        Ok(RoutingCatalog {
            mode,
            models: models.into_values().collect(),
            deployments: deployments.into_values().collect(),
            policies,
            warnings: warnings.into_iter().collect(),
        })
    }
}
