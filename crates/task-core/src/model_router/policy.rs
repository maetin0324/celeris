use crate::Tier;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Legacy,
    Shadow,
    Enforce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Objective {
    QualityFirst,
    Balanced,
    ResourceFirst,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Weights {
    pub quality: f64,
    pub cost: f64,
    pub latency: f64,
    pub pressure: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Normalization {
    pub cost_reference_usd: f64,
    pub latency_reference_ms: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Constraints {
    pub allowed_deployments: Option<Vec<String>>,
    pub allowed_sources: Option<Vec<String>>,
    pub external_network_allowed: Option<bool>,
    pub data_retention_allowed: Option<bool>,
    pub required_region: Option<String>,
    pub max_cost_usd: Option<f64>,
    pub max_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingPolicy {
    pub version: String,
    pub mode: RoutingMode,
    pub lane: Tier,
    pub objective: Objective,
    pub min_quality: f64,
    pub weights: Weights,
    pub normalization: Normalization,
    pub constraints: Constraints,
    pub fallback: bool,
    pub escalation: bool,
    pub local_preference: bool,
    pub prefer_free: bool,
}

impl RoutingPolicy {
    pub fn defaults(lane: Tier, mode: RoutingMode) -> Self {
        let (objective, min_quality, weights) = match lane {
            Tier::Frontier => (
                Objective::QualityFirst,
                0.85,
                Weights {
                    quality: 0.70,
                    cost: 0.10,
                    latency: 0.15,
                    pressure: 0.05,
                },
            ),
            Tier::Standard => (
                Objective::Balanced,
                0.65,
                Weights {
                    quality: 0.45,
                    cost: 0.25,
                    latency: 0.20,
                    pressure: 0.10,
                },
            ),
            Tier::Cheap => (
                Objective::ResourceFirst,
                0.40,
                Weights {
                    quality: 0.15,
                    cost: 0.45,
                    latency: 0.15,
                    pressure: 0.25,
                },
            ),
        };
        Self {
            version: "phase1-v1".into(),
            mode,
            lane,
            objective,
            min_quality,
            weights,
            normalization: Normalization {
                cost_reference_usd: 1.0,
                latency_reference_ms: 60_000.0,
            },
            constraints: Constraints::default(),
            fallback: true,
            escalation: false,
            local_preference: false,
            prefer_free: false,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        let w = &self.weights;
        let values = [w.quality, w.cost, w.latency, w.pressure];
        if values.iter().any(|v| !v.is_finite() || *v < 0.0)
            || (values.iter().sum::<f64>() - 1.0).abs() > 1e-9
        {
            return Err("weights must be finite, nonnegative and sum to one");
        }
        if !self.min_quality.is_finite() || !(0.0..=1.0).contains(&self.min_quality) {
            return Err("min_quality must be in [0,1]");
        }
        if !self.normalization.cost_reference_usd.is_finite()
            || self.normalization.cost_reference_usd <= 0.0
            || !self.normalization.latency_reference_ms.is_finite()
            || self.normalization.latency_reference_ms <= 0.0
        {
            return Err("normalization references must be finite and positive");
        }
        for limit in [
            self.constraints.max_cost_usd,
            self.constraints.max_latency_ms,
        ]
        .into_iter()
        .flatten()
        {
            if !limit.is_finite() || limit < 0.0 {
                return Err("constraint limits must be finite and nonnegative");
            }
        }
        Ok(())
    }
}

/// ADR §4 の観測の鮮度（TTL）。既定は 300 秒。config の観測 TTL で上書きする。
/// 観測時刻が無い・TTL を超えた・負（未来）の観測は「既知」ではなく unknown として扱う。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FreshnessPolicy {
    pub observation_ttl_seconds: f64,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            observation_ttl_seconds: 300.0,
        }
    }
}

impl FreshnessPolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.observation_ttl_seconds.is_finite() || self.observation_ttl_seconds <= 0.0 {
            return Err("observation ttl must be finite and positive");
        }
        Ok(())
    }
}

/// subscription 窓の既定の長さ（秒）。5h / weekly / monthly の 3 種。
pub const WINDOW_5H_SECONDS: f64 = 5.0 * 3600.0;
pub const WINDOW_WEEKLY_SECONDS: f64 = 7.0 * 24.0 * 3600.0;
pub const WINDOW_MONTHLY_SECONDS: f64 = 30.0 * 24.0 * 3600.0;

/// 窓 id から既定の窓長を引く。未知の id は None（窓長不明なので shadow は unknown になる）。
pub fn default_window_duration_seconds(window_id: &str) -> Option<f64> {
    match window_id {
        "five_hour" | "5h" => Some(WINDOW_5H_SECONDS),
        "seven_day" | "weekly" => Some(WINDOW_WEEKLY_SECONDS),
        "monthly" | "one_month" => Some(WINDOW_MONTHLY_SECONDS),
        _ => None,
    }
}
