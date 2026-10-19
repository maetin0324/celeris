//! MLSys 最小実装の lane 規則（ADR 2026-10-15-mlsys-min-routing-policy D2〜D5）。
//! 純粋関数のみ。dispatcher・store・LLM は呼ばない。
use crate::llm_route::{CandidateKey, HarnessId, Registry};
use crate::model::Tier;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_POLICY_VERSION: &str = "mlsys-min-1";
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.6;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    #[default]
    Fixed,
    DecisionAssisted,
}

/// 構造化事実の 3 値。`Unknown` は `Yes` として扱わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fact {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkClass {
    Routine,
    UndecidedDesign,
    HardInvestigation,
    HighCostReview,
    Unspecified,
}

/// コード側が決める構造化事実（Jev に聞き直さない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneFacts {
    pub class: WorkClass,
    pub procedure_clear: Fact,
    pub machine_checkable: Fact,
    pub reversible: Fact,
    pub failure_impact_small: Fact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneChoice {
    Lane(Tier),
    Replan,
}

pub fn tier_rank(tier: Tier) -> u8 {
    match tier {
        Tier::Cheap => 0,
        Tier::Standard => 1,
        Tier::Frontier => 2,
    }
}

impl LaneChoice {
    fn rank(self) -> u8 {
        match self {
            LaneChoice::Lane(t) => tier_rank(t),
            LaneChoice::Replan => 3,
        }
    }
}

/// fixed 方策。`None` は既存方策へ保守的に委ねる。
pub fn fixed_lane(facts: &LaneFacts) -> Option<LaneChoice> {
    match facts.class {
        WorkClass::UndecidedDesign => Some(LaneChoice::Replan),
        WorkClass::HardInvestigation | WorkClass::HighCostReview => {
            Some(LaneChoice::Lane(Tier::Frontier))
        }
        WorkClass::Unspecified => None,
        WorkClass::Routine => {
            let conds = [
                facts.procedure_clear,
                facts.machine_checkable,
                facts.reversible,
                facts.failure_impact_small,
            ];
            if conds.contains(&Fact::No) {
                Some(LaneChoice::Lane(Tier::Standard))
            } else if conds.contains(&Fact::Unknown) {
                None
            } else {
                Some(LaneChoice::Lane(Tier::Cheap))
            }
        }
    }
}

/// jev-classifier の `Specificity` と同じ語彙の狭い写し（task-core は jev-classifier に依存しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecificityHint {
    Specified,
    NeedsImplementation,
    NeedsInvestigationDesign,
    InsufficientInfo,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassificationHint {
    pub specificity: SpecificityHint,
    /// 分類の確信（LLM の成功確率ではない）。
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneBasis {
    FixedRule,
    /// 分類が fixed の結果を引き上げた／fixed が不明のとき提案した。
    ClassificationRaise,
    /// decision-assisted だが分類が使えず fixed と同じ結果になった。
    FixedFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneDecision {
    pub choice: Option<LaneChoice>,
    pub basis: LaneBasis,
}

/// 分類 hint 単独が示す lane（fixed 規則と無関係。`Specified` は提案なし）。
fn hint_proposal(h: &ClassificationHint) -> Option<LaneChoice> {
    match h.specificity {
        SpecificityHint::Specified => None,
        SpecificityHint::NeedsImplementation => Some(LaneChoice::Lane(Tier::Standard)),
        SpecificityHint::NeedsInvestigationDesign => Some(LaneChoice::Lane(Tier::Frontier)),
        SpecificityHint::InsufficientInfo => Some(LaneChoice::Replan),
    }
}

/// Jev（分類）単独の提案 lane。confidence が `min_confidence` 未満・欠測の hint は使わない（`decide_lane` と同じ門）。
/// fixed 規則との合成・clamp の前の値で、`None` は「使える提案が無い」。
pub fn classification_lane(
    hint: Option<&ClassificationHint>,
    min_confidence: f64,
) -> Option<LaneChoice> {
    hint.filter(|h| h.confidence.is_some_and(|c| c >= min_confidence))
        .and_then(hint_proposal)
}

pub fn decide_lane(
    mode: RoutingMode,
    facts: &LaneFacts,
    hint: Option<&ClassificationHint>,
    min_confidence: f64,
) -> LaneDecision {
    let base = fixed_lane(facts);
    if mode == RoutingMode::Fixed {
        return LaneDecision {
            choice: base,
            basis: LaneBasis::FixedRule,
        };
    }
    let usable = hint.filter(|h| h.confidence.is_some_and(|c| c >= min_confidence));
    let Some(h) = usable else {
        return LaneDecision {
            choice: base,
            basis: LaneBasis::FixedFallback,
        };
    };
    let proposal = hint_proposal(h);
    // 引き上げのみ。`NeedsImplementation` は fixed が不明（None）のとき何も作らない。
    match (base, proposal) {
        (Some(b), Some(p)) if p.rank() > b.rank() => LaneDecision {
            choice: Some(p),
            basis: LaneBasis::ClassificationRaise,
        },
        (None, Some(p)) if p.rank() > tier_rank(Tier::Standard) => LaneDecision {
            choice: Some(p),
            basis: LaneBasis::ClassificationRaise,
        },
        _ => LaneDecision {
            choice: base,
            basis: LaneBasis::FixedRule,
        },
    }
}

/// 許可（`allowed_tiers`、空は制限なし）と天井（`max_lane`）の外の lane を返さない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaneLimits {
    pub allowed: Vec<Tier>,
    pub max_lane: Option<Tier>,
}

impl LaneLimits {
    pub fn permits(&self, tier: Tier) -> bool {
        (self.allowed.is_empty() || self.allowed.contains(&tier))
            && self
                .max_lane
                .is_none_or(|m| tier_rank(tier) <= tier_rank(m))
    }
}

/// 要求 lane より弱くならない最小の許可 lane。無ければ `Replan`。
pub fn clamp_to_limits(choice: LaneChoice, limits: &LaneLimits) -> LaneChoice {
    let LaneChoice::Lane(want) = choice else {
        return choice;
    };
    [Tier::Cheap, Tier::Standard, Tier::Frontier]
        .into_iter()
        .filter(|t| tier_rank(*t) >= tier_rank(want) && limits.permits(*t))
        .min_by_key(|t| tier_rank(*t))
        .map_or(LaneChoice::Replan, LaneChoice::Lane)
}

fn lane_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

/// lane → model/source 対応表（版付き）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanePolicyTable {
    pub policy_version: String,
    pub lanes: BTreeMap<String, Vec<CandidateKey>>,
}

impl LanePolicyTable {
    /// 現行設定（`Registry` の lane 対応）を初期値として写す。
    pub fn from_registry(policy_version: &str, registry: &Registry, harness: &HarnessId) -> Self {
        let lanes = [Tier::Frontier, Tier::Standard, Tier::Cheap]
            .into_iter()
            .map(|t| {
                (
                    lane_name(t).to_owned(),
                    registry.route_defaults_for(harness, lane_name(t)).to_vec(),
                )
            })
            .collect();
        Self {
            policy_version: policy_version.to_owned(),
            lanes,
        }
    }

    pub fn candidates(&self, tier: Tier) -> &[CandidateKey] {
        self.lanes
            .get(lane_name(tier))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// 決定 record に載せる lane 判断（版つき）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaneRecord {
    pub mode: RoutingMode,
    pub policy_version: String,
    pub lane: Option<LaneChoice>,
    pub basis: LaneBasis,
}

pub fn lane_record(mode: RoutingMode, table: &LanePolicyTable, d: &LaneDecision) -> LaneRecord {
    LaneRecord {
        mode,
        policy_version: table.policy_version.clone(),
        lane: d.choice,
        basis: d.basis,
    }
}
