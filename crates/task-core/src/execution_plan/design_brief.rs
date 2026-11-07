//! ADR 2026-11-07-health-review-integration D3.2・D3.3: 設計 brief の形の検証と、高 risk の計画での必須判定。
//!
//! 純粋関数だけを置く。どの write path が高 risk の class に合うかは `design_review::high_risk_classes` が決める。
//! 計画から write path を集める関数と計画 1 つの risk の判定（`design_review::assess_risk` を呼ぶ）もここに置く。

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

/// `DesignBriefMissing` の文（`PlanValidationError` の `Display`）。
pub(super) fn missing_message(classes: &[String]) -> String {
    format!(
        "design_brief is required: the plan writes high-risk path classes {} (write root_cause, owner, options, new_* and removals)",
        classes.join(", ")
    )
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

/// D3.3: 計画の write path。全 unit（/3 の `units`）の `expected_write_paths` と leaf の `context.paths`、
/// /1・/2 の `work_units` の `context.paths` の和（重複を除き昇順）。
pub fn plan_write_paths(spec: &ExecutionPlanSpec) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for unit in &spec.units {
        paths.extend(unit.expected_write_paths.iter().flatten().cloned());
        if !unit.is_task() {
            paths.extend(unit.context.paths.iter().cloned());
        }
    }
    for wu in &spec.work_units {
        paths.extend(wu.context.paths.iter().cloned());
    }
    paths.into_iter().collect()
}

/// D3.3・D3.5: 計画 1 つの risk（[`plan_write_paths`] と計画の `design_brief` で `design_review::assess_risk` を呼ぶ）。
pub fn assess_plan_risk(
    policy: &crate::design_review::ReviewPolicyState,
    spec: &ExecutionPlanSpec,
) -> crate::design_review::RiskAssessment {
    crate::design_review::assess_risk(policy, &plan_write_paths(spec), spec.design_brief.as_ref())
}

/// D3.5: `Event::DesignReviewRequested.plan_sha256`・`Event::DesignReviewed.plan_sha256` の値（計画の JSON の
/// SHA-256 の 16 進）。設計 review の待ちは同じ値の 2 つの event の有無だけで表す（I9）。
pub fn plan_sha256(spec: &ExecutionPlanSpec) -> String {
    crate::project_proposal::sha256_hex(&serde_json::to_string(spec).unwrap_or_default())
}

#[cfg(test)]
#[path = "design_brief_tests.rs"]
mod tests;
