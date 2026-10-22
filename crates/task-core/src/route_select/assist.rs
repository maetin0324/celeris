//! decision-assisted の lane 層（ADR 2026-10-15-mlsys-min-routing-policy D1/D2/D4、配線は wire-mode）。
//! 純粋関数のみ。分類は別段が作った結果（`ClassificationEntry`）を読むだけで、Jev・LLM は呼ばない。
//!
//! 結果は常に「既存の lane 以上」: 分類も `fixed_lane` も既存の lane を下げない（cheap への引き下げ権限は無い）。
//! 明示 lane・許可と天井・利用可否は分類に勝つ。
use super::lane::{
    ClassificationHint, DEFAULT_MIN_CONFIDENCE, LaneBasis, LaneChoice, LaneFacts, LaneLimits,
    RoutingMode, clamp_to_limits, classification_lane, decide_lane, fixed_lane, tier_rank,
};
use crate::model::Tier;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Jev 実 API 統合が検証済みか。fixed へ戻ったこととは別の欄。live smoke を人が承認して通すまで常に false。
pub const REAL_API_VERIFIED: bool = false;

/// 分類段の結果。`fell_back_to_fixed` の `reason` は理由名だけ（値・応答本文は含めない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClassifierStatus {
    JevOk,
    FellBackToFixed { reason: String },
    NotAttempted,
}

/// 分類段が board に置く 1 件。`hint` は `JevOk` のときだけ持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationEntry {
    pub status: ClassifierStatus,
    pub hint: Option<ClassificationHint>,
    /// Jev 応答の `model` 欄の値そのまま（応答が名乗る model 名）。設定した model 名ではなく、
    /// 応答と独立に観測した snapshot でもない（後者は `LaneAssistRecord::observed_snapshot`）。
    pub model_snapshot: Option<String>,
    /// Jev の応答が返した費用（USD）。返らなければ `None`（0 扱いしない）。
    pub cost_usd: Option<f64>,
}

impl ClassificationEntry {
    pub fn fell_back(reason: impl Into<String>) -> Self {
        Self {
            status: ClassifierStatus::FellBackToFixed {
                reason: reason.into(),
            },
            hint: None,
            model_snapshot: None,
            cost_usd: None,
        }
    }
}

/// `celeris.route-decision/1` に載せる lane 層の記録（`decision_assisted` のときだけ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LaneAssistRecord {
    pub mode: String,
    pub classifier_status: ClassifierStatus,
    /// Jev 実 API 統合の検証済みフラグ。`classifier_status` とは独立（既定 false）。
    pub real_api_verified: bool,
    /// 既存方策（assist 前）の lane。
    pub base_lane: String,
    /// 分類・fixed 規則が提案した lane（`replan` を含む）。無ければ None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_lane: Option<String>,
    /// fixed 規則だけの lane（`replan` を含む）。規則が既存方策へ委ねるとき None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_lane: Option<String>,
    /// Jev 単独の提案 lane（fixed 規則と合成する前。使える提案が無ければ None）。confidence は成功確率ではない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_lane: Option<String>,
    /// Jev の判定（specificity: `specified` | `needs_implementation` | `needs_investigation_design` |
    /// `insufficient_info`）。分類が成功した（`jev_ok`）ときだけ。`jev_lane` が無い理由（`specified`
    /// なのか確信不足なのか）はこの欄と `jev_confidence` で分かる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_specificity: Option<String>,
    /// Jev の分類の確信（成功確率ではない）。応答が返さなければ None で、そのとき `jev_confidence_missing` が true。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_confidence: Option<f64>,
    /// 分類は成功したが確信が欠測。0 や `DEFAULT_MIN_CONFIDENCE` 未満とは別（欠測は提案を採らない）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub jev_confidence_missing: bool,
    /// Jev 応答が名乗った model（`ClassificationEntry::model_snapshot`）。応答が無い・欠測は None。
    /// 設定の model 名では埋めない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    /// 応答 model 名とは独立に観測した版（response header 等）。観測経路が無ければ None
    /// （`response_model` から複製しない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_snapshot: Option<String>,
    /// この分類の呼び出し ID（`decision_log::jev_call_id`）。キャッシュ再利用で複数 run が同じ分類を使うと
    /// 同じ値を共有する（消費行の鍵と同一）。分類を呼ばなかった・未採番は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// 実際に使う lane（許可・天井・利用可否で制約した後。制約後 lane）。
    pub final_lane: String,
    /// `fixed_rule` | `classification_raise` | `fixed_fallback`。
    pub basis: String,
    /// 提案を採らなかった理由: `explicit`（人・System の lane）| `ceiling` | `availability`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overridden_by: Option<String>,
}

pub struct AssistInput<'a> {
    pub mode: RoutingMode,
    /// 人・System が lane を明示した（policy が決めない）。
    pub explicit: bool,
    /// 既存方策（ADR-0069、天井込み）が決めた lane。
    pub base: Tier,
    pub facts: LaneFacts,
    pub entry: Option<&'a ClassificationEntry>,
    pub limits: &'a LaneLimits,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssistOutcome {
    pub lane: Tier,
    /// `Fixed` では None（従来と同じ記録）。
    pub record: Option<LaneAssistRecord>,
}

fn name(t: Tier) -> &'static str {
    match t {
        Tier::Cheap => "cheap",
        Tier::Standard => "standard",
        Tier::Frontier => "frontier",
    }
}

fn specificity_name(s: super::lane::SpecificityHint) -> &'static str {
    use super::lane::SpecificityHint as S;
    match s {
        S::Specified => "specified",
        S::NeedsImplementation => "needs_implementation",
        S::NeedsInvestigationDesign => "needs_investigation_design",
        S::InsufficientInfo => "insufficient_info",
    }
}

fn basis_name(b: LaneBasis) -> &'static str {
    match b {
        LaneBasis::FixedRule => "fixed_rule",
        LaneBasis::ClassificationRaise => "classification_raise",
        LaneBasis::FixedFallback => "fixed_fallback",
    }
}

/// `available(lane)` は、その lane で route が 1 つでも通るか（コード側の利用可否）。
pub fn assist_lane(input: &AssistInput<'_>, available: impl Fn(Tier) -> bool) -> AssistOutcome {
    if input.mode == RoutingMode::Fixed {
        return AssistOutcome {
            lane: input.base,
            record: None,
        };
    }
    let status = input
        .entry
        .map_or(ClassifierStatus::NotAttempted, |e| e.status.clone());
    let lane_name = |c: LaneChoice| match c {
        LaneChoice::Lane(t) => name(t).to_owned(),
        LaneChoice::Replan => "replan".to_owned(),
    };
    let fixed = fixed_lane(&input.facts).map(lane_name);
    let jev = classification_lane(
        input.entry.and_then(|e| e.hint.as_ref()),
        DEFAULT_MIN_CONFIDENCE,
    )
    .map(lane_name);
    let hint = input.entry.and_then(|e| e.hint.as_ref());
    let response_model = input.entry.and_then(|e| e.model_snapshot.clone());
    let make = |lane: Tier,
                proposed: Option<LaneChoice>,
                basis: LaneBasis,
                overridden_by: Option<&str>| AssistOutcome {
        lane,
        record: Some(LaneAssistRecord {
            mode: "decision_assisted".into(),
            classifier_status: status.clone(),
            real_api_verified: REAL_API_VERIFIED,
            base_lane: name(input.base).into(),
            proposed_lane: proposed.map(lane_name),
            fixed_lane: fixed.clone(),
            jev_lane: jev.clone(),
            jev_specificity: hint.map(|h| specificity_name(h.specificity).to_owned()),
            jev_confidence: hint.and_then(|h| h.confidence),
            jev_confidence_missing: hint.is_some_and(|h| h.confidence.is_none()),
            response_model: response_model.clone(),
            observed_snapshot: None,
            call_id: None,
            final_lane: name(lane).into(),
            basis: basis_name(basis).into(),
            overridden_by: overridden_by.map(str::to_owned),
        }),
    };
    if input.explicit {
        return make(input.base, None, LaneBasis::FixedFallback, Some("explicit"));
    }
    let decided = decide_lane(
        RoutingMode::DecisionAssisted,
        &input.facts,
        hint,
        DEFAULT_MIN_CONFIDENCE,
    );
    let Some(choice) = decided.choice else {
        return make(input.base, None, decided.basis, None);
    };
    // 結果は既存 lane 以上（下げない）。replan は frontier を要求する（実際の計画直しは別段）。
    let want = match choice {
        LaneChoice::Lane(t) => t,
        LaneChoice::Replan => Tier::Frontier,
    };
    if tier_rank(want) <= tier_rank(input.base) {
        // lane は既存 lane のまま。分類が fixed 規則を上回っても、結果を動かしていないので引き上げの帰属にしない。
        let basis = match decided.basis {
            LaneBasis::ClassificationRaise => LaneBasis::FixedRule,
            b => b,
        };
        return make(input.base, Some(choice), basis, None);
    }
    match clamp_to_limits(LaneChoice::Lane(want), input.limits) {
        LaneChoice::Lane(t) if tier_rank(t) > tier_rank(input.base) => {
            if available(t) {
                make(t, Some(choice), decided.basis, None)
            } else {
                make(
                    input.base,
                    Some(choice),
                    decided.basis,
                    Some("availability"),
                )
            }
        }
        _ => make(input.base, Some(choice), decided.basis, Some("ceiling")),
    }
}
