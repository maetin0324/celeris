//! ADR 2026-11-07-health-review-integration D3.2・D3.3: 設計 brief の形の検証と、高 risk の計画での必須判定。
//!
//! 純粋関数だけを置く。どの write path が高 risk の class に合うかは review policy を読む呼び出し側
//! （dispatcher の risk 判定、`design_review::assess_risk`）が決め、ここには合った class の名前だけを渡す。

use super::*;

/// D3.2: brief の形の誤り（`validate_v3` が brief のあるときだけ呼ぶ）。`root_cause`・`owner` が空でない、
/// `options` が 2 件以上で `chosen = true` がちょうど 1 件、各要素の文字列が空でない。
pub(super) fn brief_shape_errors(brief: &DesignBrief) -> Vec<PlanValidationError> {
    let mut errors = Vec::new();
    let mut err = |field: &'static str, reason: String| {
        errors.push(PlanValidationError::DesignBrief { field, reason });
    };
    for (field, value) in [("root_cause", &brief.root_cause), ("owner", &brief.owner)] {
        if value.trim().is_empty() {
            err(field, "must not be empty".to_string());
        }
    }
    if brief.options.len() < 2 {
        err(
            "options",
            format!("needs at least 2 options, found {}", brief.options.len()),
        );
    }
    let chosen = brief.options.iter().filter(|o| o.chosen).count();
    if chosen != 1 {
        err(
            "options",
            format!("exactly one option must be chosen, found {chosen}"),
        );
    }
    for (i, o) in brief.options.iter().enumerate() {
        if o.name.trim().is_empty() || o.reason.trim().is_empty() {
            err(
                "options",
                format!("option {i}: name and reason must not be empty"),
            );
        }
    }
    for (field, items) in [
        ("reuse", &brief.reuse),
        ("new_state", &brief.new_state),
        ("new_branch", &brief.new_branch),
        ("new_fallback", &brief.new_fallback),
        ("new_port", &brief.new_port),
        ("removals", &brief.removals),
        ("invariants", &brief.invariants),
    ] {
        if let Some(i) = items.iter().position(|s| s.trim().is_empty()) {
            err(field, format!("item {i} must not be empty"));
        }
    }
    errors
}

/// D3.3: 高 risk の path class（`high_risk_classes`、呼び出し側が review policy から求めた名前）を変える
/// planner の計画に brief が無ければ `DesignBriefMissing` で差し戻す（既存の検証失敗の再試行と同じ経路）。
/// planner 以外の計画（人の計画 `PlanOrigin::Human` など）は brief を必須にしない（risk 判定が `brief_absent_human_plan` で `Low` にする）。
/// class が空（高 risk の path に触れない）なら brief は任意。
pub fn require_design_brief(
    spec: &ExecutionPlanSpec,
    origin: PlanOrigin,
    high_risk_classes: &[String],
) -> Result<(), PlanValidationError> {
    if high_risk_classes.is_empty() || spec.design_brief.is_some() || origin != PlanOrigin::Planner
    {
        return Ok(());
    }
    let classes: BTreeSet<String> = high_risk_classes.iter().cloned().collect();
    Err(PlanValidationError::DesignBriefMissing {
        classes: classes.into_iter().collect(),
    })
}

#[cfg(test)]
#[path = "design_brief_tests.rs"]
mod tests;
