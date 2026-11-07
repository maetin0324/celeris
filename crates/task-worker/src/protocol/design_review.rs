//! ADR 2026-11-07-health-review-integration D3.5: 採用の前の設計 review の wire 型。
//!
//! 設計 review の run は reviewer run で、入力は `context.review.design`（[`DesignReviewRequest`]）、出力は
//! `review.json` の `design`（[`DesignVerdictOut`]）。どちらも追加の欄で、省略した JSON は従来の review と
//! 同じに読める（`PROTOCOL_VERSION` は 4 のまま）。finding の種別は final review の health verdict と同じ
//! 固定 7 種（[`HealthFindingOut`]）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::design_review::{ReviewPolicy, RiskAssessment};
use task_core::execution_plan::DesignBrief;

use super::HealthFindingOut;

/// D3.5: `ReviewRequest.design`。risk が `high`・`unknown` の計画の採用前に dispatcher が作る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DesignReviewRequest {
    /// 計画の設計 brief（人の計画など brief の無い計画では省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<DesignBrief>,
    /// 計画の要約（rationale・unit の key と題・write path）。
    pub plan_summary: String,
    /// risk の判定と理由（`task_core::design_review::assess_risk`）。
    pub risk: RiskAssessment,
    /// 判定に使った base 側の review 方針（読めなかった `unknown` では省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_policy: Option<ReviewPolicy>,
}

/// D3.5: `ReviewOutput.design`。`pass = false` なら planner を replan で起こし、`findings` を planner の
/// prior に渡す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DesignVerdictOut {
    pub pass: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<HealthFindingOut>,
}

#[cfg(test)]
mod tests;
