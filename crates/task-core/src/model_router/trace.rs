use super::{
    cost::{CostEstimate, ScoreBreakdown},
    estimator::QualityEstimate,
    policy::{RoutingMode, Weights},
};
use crate::Tier;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// ADR §9（Phase 2）: 候補が落ちた理由の型。旧 event には無い（None）。
/// 既知の理由コード（optimizer・cost の `&'static str`）は [`ExcludedReason::from_code`] で写す。
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExcludedReason {
    /// 不合格の制約（lane・deployment・source・privacy・locality・adapter・capability・context・
    /// provider・cost・latency・model_identity など）。`name` は制約名。
    Constraint {
        name: String,
    },
    QuotaExhausted,
    Cooldown,
    Concurrency,
    RateLimit,
    HealthDown,
    CircuitOpen,
    Disabled,
    /// enforce で必須の値が未知（`context_unknown`・`quality_unknown` など）。`field` は欠けた値。
    UnknownRequired {
        field: String,
    },
    QualityInvalid,
    QualityBelowMin,
    InvalidEstimate,
    /// 上のどれにも当たらない理由コード（新しい版が足したコードを旧い読み手が落とさない）。
    Other {
        code: String,
    },
}

impl ExcludedReason {
    pub fn from_code(code: &str) -> Self {
        match code {
            "quota_exhausted" => Self::QuotaExhausted,
            "cooldown" => Self::Cooldown,
            "concurrency" => Self::Concurrency,
            "rate_limit" => Self::RateLimit,
            "health_down" => Self::HealthDown,
            "circuit_open" => Self::CircuitOpen,
            "disabled" => Self::Disabled,
            "quality_invalid" => Self::QualityInvalid,
            "quality_below_min" => Self::QualityBelowMin,
            "invalid_estimate" => Self::InvalidEstimate,
            "context_unknown" => Self::UnknownRequired {
                field: "context".into(),
            },
            "quality_unknown" => Self::UnknownRequired {
                field: "quality".into(),
            },
            "model_identity" | "lane" | "deployment" | "source" | "privacy" | "locality"
            | "adapter" | "capability" | "context" | "provider" | "cost" | "latency" => {
                Self::Constraint { name: code.into() }
            }
            other => Self::Other { code: other.into() },
        }
    }

    /// 理由コードの並びから代表の理由を決める。並び順に依らず決定的（型の順序で最小のもの）。
    pub fn primary<S: AsRef<str>>(codes: &[S]) -> Option<Self> {
        codes.iter().map(|c| Self::from_code(c.as_ref())).min()
    }
}

/// score の内訳（ADR §4: score = wq·Q − wc·C − wl·L − wp·P）。C/L/P の未知は 1 で `unknown` に残す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScoreTrace {
    pub q: f64,
    pub c: f64,
    pub l: f64,
    pub p: f64,
    pub wq: f64,
    pub wc: f64,
    pub wl: f64,
    pub wp: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
    pub score: f64,
}

impl ScoreTrace {
    pub fn new(b: &ScoreBreakdown, w: &Weights) -> Self {
        let mut unknown: Vec<String> = b.unknown.iter().map(|s| (*s).to_string()).collect();
        unknown.sort();
        unknown.dedup();
        Self {
            q: b.quality,
            c: b.cost_term,
            l: b.latency_term,
            p: b.pressure_term,
            wq: w.quality,
            wc: w.cost,
            wl: w.latency,
            wp: w.pressure,
            unknown,
            score: b.score,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Phase 2: 設定順（決定的な並びのキー）。旧 event には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_order: Option<usize>,
    /// Phase 2: 代表の除外理由。採用可能な候補・旧 event は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_reason: Option<ExcludedReason>,
    /// Phase 2: score の内訳。score を計算しなかった候補・旧 event は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_breakdown: Option<ScoreTrace>,
    /// Phase 2: API の実料金。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cash_usd: Option<f64>,
    /// Phase 2: subscription の shadow price。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_usd: Option<f64>,
    /// Phase 2: self-host の資源の機会費用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_usd: Option<f64>,
    /// Phase 2: cash + shadow + resource（どれかが未知なら None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_usd: Option<f64>,
}

impl CandidateTrace {
    /// cost.rs の見積もりを cash/shadow/resource/effective の欄に写す。
    pub fn with_cost(mut self, estimate: &CostEstimate) -> Self {
        self.cash_usd = estimate.cash_usd;
        self.shadow_usd = estimate.subscription_shadow_usd;
        self.resource_usd = estimate.self_host_resource_usd;
        self.effective_usd = estimate.effective_usd;
        self
    }

    /// 理由の並びを整え（重複を除いて整列）、代表の除外理由を決める。
    pub fn normalize(&mut self) {
        self.excluded_reasons.sort();
        self.excluded_reasons.dedup();
        if self.excluded_reason.is_none() {
            self.excluded_reason = ExcludedReason::primary(&self.excluded_reasons);
        }
        if let Some(b) = &mut self.score_breakdown {
            b.unknown.sort();
            b.unknown.dedup();
        }
    }

    /// 決定的な並びのキー: 設定順（未知は最後）→ deployment id → model profile id。
    fn order_key(&self) -> (bool, usize, &str, &str) {
        (
            self.config_order.is_none(),
            self.config_order.unwrap_or(usize::MAX),
            self.deployment_id.as_str(),
            self.model_profile_id.as_str(),
        )
    }
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
    /// Phase 2: 最終の source（provider / deployment）。旧 event には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Phase 2: 最終の model。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Phase 2: 最終の account（OAuth pool の account 等）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl RoutingTraceV1 {
    /// 候補を決定的キー（設定順・ID）で並べ、各候補の理由を整える。入力順に依らず同じ JSON になる。
    /// `fallback_order` は選択の順位なので並べ替えない。
    pub fn normalize(&mut self) {
        for c in &mut self.candidates {
            c.normalize();
        }
        self.candidates
            .sort_by(|a, b| a.order_key().cmp(&b.order_key()));
    }
}
