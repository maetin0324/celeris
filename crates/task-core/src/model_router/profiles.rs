use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Capabilities {
    pub tools: Support,
    pub structured_output: Support,
    pub vision: Support,
    pub streaming: Support,
    pub reasoning_efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextLimits {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityIndex {
    pub domain: String,
    pub index: f64,
    pub evaluation_version: String,
    pub samples: Option<u64>,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TokenPricing {
    pub input_usd_per_million: Option<f64>,
    pub cached_input_usd_per_million: Option<f64>,
    pub output_usd_per_million: Option<f64>,
    pub as_of: Option<String>,
    pub provenance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelProfile {
    pub id: String,
    pub revision: String,
    pub family: String,
    pub capabilities: Capabilities,
    pub context_limits: ContextLimits,
    pub quality: Vec<QualityIndex>,
    pub pricing: Option<TokenPricing>,
    pub provenance: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    Subscription,
    MeteredApi,
    SelfHosted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeploymentProfile {
    pub id: String,
    pub source_ref: String,
    pub model_profile_id: String,
    pub upstream_model: String,
    pub adapter_constraints: Vec<String>,
    pub billing: Billing,
    pub host: Option<String>,
    pub region: Option<String>,
    pub trust_zone: Option<String>,
    pub external_network: bool,
    pub retains_data: Option<bool>,
    pub allowed_lanes: Vec<crate::Tier>,
    pub resource_group_id: Option<String>,
    pub concurrency_limit: Option<u32>,
    pub rpm_limit: Option<u32>,
    pub tpm_limit: Option<u32>,
    pub price_override: Option<TokenPricing>,
    pub config_order: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reachability {
    Unknown,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QuotaWindow {
    pub account_id: String,
    pub window_id: String,
    pub unit: String,
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
    pub measured: bool,
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceState {
    pub deployment_id: String,
    pub observed_at: Option<String>,
    pub expires_at: Option<String>,
    pub enabled: bool,
    pub reachability: Reachability,
    pub cooldown: bool,
    pub circuit_open: bool,
    pub latency_ms: Option<f64>,
    pub in_use: Option<u32>,
    pub rpm_remaining: Option<u32>,
    pub tpm_remaining: Option<u32>,
    pub quota_remaining: Option<f64>,
    pub quota_windows: Vec<QuotaWindow>,
    pub capacity_scope: Option<String>,
    pub queue_depth: Option<u32>,
    pub queue_limit: Option<u32>,
    pub gpu_utilization: Option<f64>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub provenance: Option<String>,
}

impl SourceState {
    pub fn unobserved(deployment_id: impl Into<String>) -> Self {
        Self {
            deployment_id: deployment_id.into(),
            observed_at: None,
            expires_at: None,
            enabled: true,
            reachability: Reachability::Unknown,
            cooldown: false,
            circuit_open: false,
            latency_ms: None,
            in_use: None,
            rpm_remaining: None,
            tpm_remaining: None,
            quota_remaining: None,
            quota_windows: vec![],
            capacity_scope: None,
            queue_depth: None,
            queue_limit: None,
            gpu_utilization: None,
            vram_used: None,
            vram_total: None,
            provenance: None,
        }
    }
}
