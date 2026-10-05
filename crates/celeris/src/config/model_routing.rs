//! Phase 1 catalog adapter. It does not change the legacy execution selector.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use task_core::Tier;
use task_core::model_router::{
    cost::SelfHostRates,
    policy::{FreshnessPolicy, RoutingMode, RoutingPolicy, Weights},
    profiles::{Billing, Capabilities, ContextLimits, DeploymentProfile, ModelProfile, Support},
    shadow::{ShadowAllowlist, ShadowPolicy, ShadowTarget},
};

use super::{Config, ConfigError};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModelRoutingConfig {
    pub mode: Option<RoutingMode>,
    pub models: Vec<ModelEntry>,
    pub deployments: Vec<DeploymentEntry>,
    pub policies: std::collections::HashMap<Tier, PolicyEntry>,
    /// Phase 2: `[model_routing.estimator] kind`。enforce は `kind = "heuristic"` の明示だけを受け入れる。
    pub estimator: EstimatorEntry,
    /// Phase 2: enforce を適用する経路。`standalone`（context の無い proxy 要求）と
    /// `server`（サーバ全体の制約で足りる経路）だけ。未設定は両方。
    pub enforce_routes: Option<Vec<String>>,
    /// Phase 2: 観測の鮮度（秒）。未設定は ADR の既定 300 秒。
    pub observation_ttl_seconds: Option<f64>,
    /// Phase 2: subscription 窓 id（`five_hour`・`seven_day`・`monthly` 等）ごとの reserve_value。
    pub subscription_windows: BTreeMap<String, SubscriptionWindowEntry>,
    /// Phase 2: self-host の resource group（同時数と費用係数）。
    pub resource_groups: Vec<ResourceGroupEntry>,
    /// Phase 2: proxy の同一要求内 fallback の分類別上限と breaker。
    pub retry: RetryEntry,
    /// Phase 3: context 長に積む余裕 token 数。未指定は従来の context 既定値。
    pub context_safety_margin: Option<u64>,
    /// Phase 3: run 間 escalation の品質失敗閾値と試行上限。
    pub escalation: EscalationEntry,
    /// Phase 4: decision shadow と、明示された場合だけの実行 shadow。
    pub shadow: ShadowEntry,
    /// Phase 2 の state・cost・retry の検証済み設定（`Config::load` が埋める。dispatcher と proxy に配る）。
    /// `Config` 本体でなくここに持つのは、reload で `model_routing` と一緒に原子的に差し替えるため。
    #[serde(skip)]
    pub runtime: Option<std::sync::Arc<RoutingRuntime>>,
    /// Phase 5: daemon が llm-proxy に配線した estimator sidecar の差し替え（proxy 無しは `None`）。
    /// reload は新しい `estimator.sidecar` をこれに適用し、これ自体は古い値から引き継ぐ。
    #[serde(skip)]
    pub estimator_sidecar_control:
        Option<std::sync::Arc<crate::daemon::routing_sidecar::EstimatorSidecarControl>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EstimatorEntry {
    pub kind: Option<String>,
    pub sidecar: SidecarEntry,
}

/// Phase 5 の比較 estimator。primary は常に heuristic のまま。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SidecarEntry {
    pub enabled: bool,
    pub shadow_only: bool,
    pub endpoint: Option<String>,
    pub protocol_version: u32,
    pub estimator_id: Option<String>,
    pub estimator_version: Option<String>,
    pub timeout_ms: u64,
    pub max_inflight: u32,
    pub max_payload_bytes: usize,
    pub send_prompt: bool,
    /// Endpoint と response に宣言された外部依存先の許可 host。完全一致。
    pub network_allowlist: Vec<String>,
    pub allowlist: ShadowAllowlist,
    pub prompt_allowlist: ShadowAllowlist,
    pub daily_max_requests: Option<u64>,
}

impl Default for SidecarEntry {
    fn default() -> Self {
        Self {
            enabled: false,
            shadow_only: true,
            endpoint: None,
            protocol_version: 1,
            estimator_id: None,
            estimator_version: None,
            timeout_ms: 1_000,
            max_inflight: 4,
            max_payload_bytes: 65_536,
            send_prompt: false,
            network_allowlist: Vec::new(),
            allowlist: ShadowAllowlist::default(),
            prompt_allowlist: ShadowAllowlist::default(),
            daily_max_requests: None,
        }
    }
}

impl SidecarEntry {
    fn allowed_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            || self
                .network_allowlist
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(host))
    }

    /// response の依存先も送信先と同じ許可集合に制限する。
    pub fn allows_dependencies(&self, hosts: &[String]) -> bool {
        hosts.iter().all(|host| self.allowed_host(host))
    }

    pub fn allows_prompt(&self, target: &ShadowTarget) -> bool {
        self.send_prompt && self.prompt_allowlist.matches(target)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |reason: &str| {
            ConfigError::Invalid(format!("model_routing.estimator.sidecar.{reason}"))
        };
        if !self.shadow_only {
            return Err(invalid(
                "shadow_only must be true (heuristic remains primary)",
            ));
        }
        if self.protocol_version != 1 {
            return Err(invalid("protocol_version must be 1"));
        }
        if self.timeout_ms == 0 || self.max_inflight == 0 || self.max_payload_bytes == 0 {
            return Err(invalid(
                "timeout_ms, max_inflight and max_payload_bytes must be positive",
            ));
        }
        for host in &self.network_allowlist {
            if host.is_empty() || host.trim() != host || host.contains(['/', ':', '@', '*', ' ']) {
                return Err(invalid("network_allowlist must contain host names only"));
            }
        }
        if let Some(endpoint) = &self.endpoint {
            let url = reqwest::Url::parse(endpoint)
                .map_err(|_| invalid("endpoint must be a valid URL"))?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || !url.host_str().is_some_and(|host| self.allowed_host(host))
            {
                return Err(invalid(
                    "endpoint must use HTTP(S) and a loopback or network_allowlist host",
                ));
            }
        }
        if self.send_prompt && !allowlist_complete(&self.prompt_allowlist) {
            return Err(invalid(
                "send_prompt requires every prompt_allowlist dimension",
            ));
        }
        if self.enabled {
            if self.endpoint.is_none() {
                return Err(invalid("enabled requires endpoint"));
            }
            if !self
                .estimator_id
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
                || !self
                    .estimator_version
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty())
            {
                return Err(invalid(
                    "enabled requires estimator_id and estimator_version",
                ));
            }
            if !allowlist_complete(&self.allowlist)
                || self.daily_max_requests.is_none_or(|n| n == 0)
            {
                return Err(invalid(
                    "enabled requires allowlist and positive daily_max_requests",
                ));
            }
        }
        Ok(())
    }
}

fn allowlist_complete(list: &ShadowAllowlist) -> bool {
    !list.task_kinds.is_empty()
        && !list.roles.is_empty()
        && !list.lanes.is_empty()
        && !list.sources.is_empty()
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubscriptionWindowEntry {
    /// 窓を使い切る機会費用（USD）。未設定は unknown（shadow price も unknown）。
    pub reserve_value_usd: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGroupEntry {
    pub id: String,
    #[serde(default)]
    pub concurrency_limit: Option<u32>,
    #[serde(default)]
    pub usd_per_gpu_second: Option<f64>,
    #[serde(default)]
    pub usd_per_wait_second: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetryEntry {
    pub unauthorized: Option<u32>,
    pub rate_limited: Option<u32>,
    pub server: Option<u32>,
    pub network: Option<u32>,
    pub local: Option<u32>,
    pub client: Option<u32>,
    pub total_attempts: Option<u32>,
    pub deadline_secs: Option<i64>,
    pub breaker_failure_threshold: Option<u32>,
    pub breaker_open_secs: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EscalationEntry {
    pub quality_failures_per_lane: Option<u32>,
    pub max_total_attempts: Option<u32>,
}

/// TOML の候補 policy 指定と、task-core が共有する実行上限を分ける。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShadowEntry {
    pub candidate_policy: Option<String>,
    pub execute: bool,
    pub allowlist: ShadowAllowlist,
    pub sample_rate: f64,
    pub daily_max_requests: Option<u64>,
    pub daily_max_tokens: Option<u64>,
    pub daily_max_effective_usd: Option<f64>,
    pub max_concurrency: Option<u32>,
    pub max_queue_depth: Option<u32>,
    pub timeout_ms: Option<u64>,
}

impl ShadowEntry {
    pub fn policy(&self) -> ShadowPolicy {
        ShadowPolicy {
            execute: self.execute,
            allowlist: self.allowlist.clone(),
            sample_rate: self.sample_rate,
            daily_max_requests: self.daily_max_requests,
            daily_max_tokens: self.daily_max_tokens,
            daily_max_effective_usd: self.daily_max_effective_usd,
            max_concurrency: self.max_concurrency,
            max_queue_depth: self.max_queue_depth,
            timeout_ms: self.timeout_ms,
        }
    }
}

/// 検証済みの Phase 2 実行時設定。dispatcher と proxy に配る（未知は None のまま）。
#[derive(Debug, Clone, PartialEq)]
pub struct RoutingRuntime {
    pub mode: RoutingMode,
    pub enforce_routes: Vec<String>,
    pub freshness: FreshnessPolicy,
    pub window_reserves: BTreeMap<String, f64>,
    pub resource_groups: BTreeMap<String, ResourceGroupRates>,
    pub fallback: llm_proxy::fallback::FallbackSettings,
    pub context_safety_margin: Option<u64>,
    pub escalation: task_core::EscalationThresholds,
    pub shadow: ShadowPolicy,
    pub shadow_candidate_policy: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ResourceGroupRates {
    pub concurrency_limit: Option<u32>,
    pub rates: SelfHostRates,
}

/// enforce が受け入れる経路（ADR §10 Phase 2: standalone かサーバ全体の制約で足りる経路だけ）。
pub const ENFORCE_ROUTES: [&str; 2] = ["standalone", "server"];

impl RoutingRuntime {
    pub fn dispatch_settings(&self) -> task_dispatch::DispatchRoutingSettings {
        task_dispatch::DispatchRoutingSettings {
            mode: self.mode,
            constraints: Default::default(),
            freshness: self.freshness,
            window_reserves: self.window_reserves.clone(),
            context_safety_margin: self.context_safety_margin,
            escalation: self.escalation.clone(),
        }
    }

    /// resource group の費用係数。未登録・未設定は unknown（None）。
    pub fn self_host_rates(&self, resource_group_id: Option<&str>) -> SelfHostRates {
        resource_group_id
            .and_then(|id| self.resource_groups.get(id))
            .map(|g| g.rates)
            .unwrap_or_default()
    }

    /// proxy の予約表の上限。account 上限は呼び出し側（`max_concurrent_per_account`）。
    pub fn capacity_limits(
        &self,
        per_account: Option<u32>,
    ) -> llm_proxy::reservation::CapacityLimits {
        llm_proxy::reservation::CapacityLimits {
            per_account,
            resource_groups: self
                .resource_groups
                .iter()
                .filter_map(|(id, g)| g.concurrency_limit.map(|n| (id.clone(), n)))
                .collect(),
        }
    }
}

fn finite_nonneg(value: Option<f64>, key: &str) -> Result<(), ConfigError> {
    match value {
        Some(v) if !v.is_finite() || v < 0.0 => Err(ConfigError::Invalid(format!(
            "{key} must be finite and nonnegative"
        ))),
        _ => Ok(()),
    }
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
        self.validate_enforce_opt_in(mode)?;
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

impl Config {
    /// enforce は Phase 2 の heuristic の明示 opt-in と、standalone / サーバ全体の制約の経路だけ受け入れる。
    fn validate_enforce_opt_in(&self, mode: RoutingMode) -> Result<(), ConfigError> {
        let kind = self.model_routing.estimator.kind.as_deref();
        if let Some(kind) = kind
            && kind != "heuristic"
        {
            return Err(ConfigError::Invalid(format!(
                "model_routing.estimator.kind={kind}: only \"heuristic\" is supported (Phase 2)"
            )));
        }
        if mode == RoutingMode::Enforce && kind != Some("heuristic") {
            return Err(ConfigError::Invalid(
                "model_routing.mode=enforce requires the Phase 2 heuristic opt-in \
                 ([model_routing.estimator] kind = \"heuristic\")"
                    .into(),
            ));
        }
        if let Some(routes) = &self.model_routing.enforce_routes {
            if routes.is_empty() {
                return Err(ConfigError::Invalid(
                    "model_routing.enforce_routes is empty".into(),
                ));
            }
            for route in routes {
                if !ENFORCE_ROUTES.contains(&route.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "model_routing.enforce_routes.{route}: enforce is limited to standalone \
                         or server-wide constraints (Phase 2)"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Phase 2 の実行時設定（state・cost・retry）を検証して組む。未設定は unknown か ADR の既定。
    pub fn routing_runtime(&self) -> Result<RoutingRuntime, ConfigError> {
        let mr = &self.model_routing;
        mr.estimator.sidecar.validate()?;
        let shadow = mr.shadow.policy();
        shadow
            .validate()
            .map_err(|error| ConfigError::Invalid(error.to_string()))?;
        if shadow.execute
            && (shadow.allowlist.task_kinds.is_empty()
                || shadow.allowlist.roles.is_empty()
                || shadow.allowlist.lanes.is_empty()
                || shadow.allowlist.sources.is_empty())
        {
            return Err(ConfigError::Invalid(
                "model_routing.shadow.execute = true requires every allowlist dimension".into(),
            ));
        }
        if mr
            .shadow
            .candidate_policy
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ConfigError::Invalid(
                "model_routing.shadow.candidate_policy must not be empty".into(),
            ));
        }
        let mut escalation = task_core::EscalationThresholds::default();
        if let Some(value) = mr.escalation.quality_failures_per_lane {
            if value == 0 {
                return Err(ConfigError::Invalid(
                    "model_routing.escalation.quality_failures_per_lane must be positive".into(),
                ));
            }
            escalation.quality_failures_per_lane = value;
        }
        if let Some(value) = mr.escalation.max_total_attempts {
            if value == 0 {
                return Err(ConfigError::Invalid(
                    "model_routing.escalation.max_total_attempts must be positive".into(),
                ));
            }
            escalation.max_total_attempts = value;
        }
        let mode = mr.mode.unwrap_or(RoutingMode::Legacy);
        self.validate_enforce_opt_in(mode)?;
        let mut freshness = FreshnessPolicy::default();
        if let Some(ttl) = mr.observation_ttl_seconds {
            freshness.observation_ttl_seconds = ttl;
        }
        freshness.validate().map_err(|reason| {
            ConfigError::Invalid(format!("model_routing.observation_ttl_seconds: {reason}"))
        })?;
        let mut window_reserves = BTreeMap::new();
        for (id, w) in &mr.subscription_windows {
            if id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "model_routing.subscription_windows: empty window id".into(),
                ));
            }
            finite_nonneg(
                w.reserve_value_usd,
                &format!("model_routing.subscription_windows.{id}.reserve_value_usd"),
            )?;
            if let Some(v) = w.reserve_value_usd {
                window_reserves.insert(id.clone(), v);
            }
        }
        let mut resource_groups = BTreeMap::new();
        for g in &mr.resource_groups {
            if g.id.trim().is_empty() || resource_groups.contains_key(&g.id) {
                return Err(ConfigError::Invalid(
                    "model_routing.resource_groups: empty or duplicate id".into(),
                ));
            }
            if g.concurrency_limit == Some(0) {
                return Err(ConfigError::Invalid(format!(
                    "model_routing.resource_groups.{}: concurrency_limit must be positive",
                    g.id
                )));
            }
            finite_nonneg(
                g.usd_per_gpu_second,
                &format!("model_routing.resource_groups.{}.usd_per_gpu_second", g.id),
            )?;
            finite_nonneg(
                g.usd_per_wait_second,
                &format!("model_routing.resource_groups.{}.usd_per_wait_second", g.id),
            )?;
            resource_groups.insert(
                g.id.clone(),
                ResourceGroupRates {
                    concurrency_limit: g.concurrency_limit,
                    rates: SelfHostRates {
                        usd_per_gpu_second: g.usd_per_gpu_second,
                        usd_per_wait_second: g.usd_per_wait_second,
                    },
                },
            );
        }
        let mut fallback = llm_proxy::fallback::FallbackSettings::default();
        let r = &mr.retry;
        let limits = &mut fallback.limits;
        for (slot, value) in [
            (&mut limits.unauthorized, r.unauthorized),
            (&mut limits.rate_limited, r.rate_limited),
            (&mut limits.server, r.server),
            (&mut limits.network, r.network),
            (&mut limits.local, r.local),
            (&mut limits.client, r.client),
            (&mut limits.total_attempts, r.total_attempts),
            (
                &mut fallback.breaker.failure_threshold,
                r.breaker_failure_threshold,
            ),
        ] {
            if let Some(v) = value {
                *slot = v;
            }
        }
        if let Some(v) = r.deadline_secs {
            fallback.deadline_secs = v;
        }
        if let Some(v) = r.breaker_open_secs {
            fallback.breaker.open_secs = v;
        }
        // 4xx（client）の誤りを別候補へ投げると quota を無駄にする（p2-proxy-fallback の提案）。
        if fallback.limits.client != 0 {
            return Err(ConfigError::Invalid(
                "model_routing.retry.client must be 0 (client errors are not retried)".into(),
            ));
        }
        if fallback.limits.total_attempts == 0 {
            return Err(ConfigError::Invalid(
                "model_routing.retry.total_attempts must be at least 1".into(),
            ));
        }
        if fallback.deadline_secs <= 0 || fallback.breaker.open_secs < 0 {
            return Err(ConfigError::Invalid(
                "model_routing.retry: deadline_secs must be positive and breaker_open_secs nonnegative"
                    .into(),
            ));
        }
        Ok(RoutingRuntime {
            mode,
            enforce_routes: mr
                .enforce_routes
                .clone()
                .unwrap_or_else(|| ENFORCE_ROUTES.iter().map(|r| (*r).to_string()).collect()),
            freshness,
            window_reserves,
            resource_groups,
            fallback,
            context_safety_margin: mr.context_safety_margin,
            escalation,
            shadow,
            shadow_candidate_policy: mr.shadow.candidate_policy.clone(),
        })
    }
}
