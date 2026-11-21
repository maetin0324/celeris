//! ADR 2026-11-07-health-review-integration D3.5・ADR 2026-11-17-boundary-change-risk-review 付記 (i): 設計 review の
//! wire 型。
//!
//! 入力の [`DesignReviewRequest`] は 2 か所で使う。採用前の計画の設計 review の run は `context.review.design`、
//! final review の run は差分の設計の節として `context.review.diff_design`。出力はどちらも `review.json` の
//! `design`（[`DesignVerdictOut`]）。どれも追加の欄で、省略した JSON は従来の review と同じに読める
//! （`PROTOCOL_VERSION` は上げない）。finding の種別は final review の health verdict と同じ固定 7 種（[`HealthFindingOut`]）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::design_review::{ReviewPolicy, RiskAssessment};
use task_core::execution_plan::DesignBrief;

use super::{HealthFindingOut, OwnerRow};

/// D3.5: `ReviewRequest.design`（risk が `high`・`unknown` の計画の採用前に dispatcher が作る。`plan_summary` は
/// 計画の要約）と、ADR 2026-11-17-boundary-change-risk-review 付記 (i) の `ReviewRequest.diff_design`（final review の
/// 差分が設計 review の覆わない高 risk の (class, path) の組に触れたとき dispatcher が作る。`plan_summary` は差分の
/// stat、`risk` は覆われない組の理由）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DesignReviewRequest {
    /// 計画の設計 brief（人の計画など brief の無い計画では省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<DesignBrief>,
    /// 判定の対象の要約。`design` では計画の要約（rationale・unit の key と題・write path）、`diff_design` では
    /// 差分の stat。
    pub plan_summary: String,
    /// risk の判定と理由（`task_core::design_review::assess_risk`）。
    pub risk: RiskAssessment,
    /// 判定に使った base 側の review 方針（読めなかった `unknown` では省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_policy: Option<ReviewPolicy>,
    /// ADR 2026-11-17-boundary-change-risk-review D4: base 側の owner 地図のうち、risk の理由の path と計画の write path を
    /// 含む表の行（dispatcher が `health_review::input::owner_rows` で引く。上限 40 件）。`design` だけが持つ。
    /// `diff_design` では空で、reviewer は同じ request の `health.repos[].owner_rows` を使う。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owner_rows: Vec<OwnerRow>,
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
