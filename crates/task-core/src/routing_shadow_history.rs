//! Read-only wire types for historical routing shadow events.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SHADOW_RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowKind {
    Decision,
    Execution,
    Estimator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowStatus {
    Completed,
    Failed,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowReason {
    Off,
    NotAllowlisted,
    SampledOut,
    QueueFull,
    ConcurrencyLimit,
    CapExceeded,
    UnknownCost,
    Timeout,
    Privacy,
    ResourceGroupShared,
    PrimaryPressure,
    UpstreamError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShadowRecord {
    pub shadow_id: String,
    pub primary_decision_id: String,
    pub kind: ShadowKind,
    pub status: ShadowStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ShadowReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub policy_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cash_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
}
