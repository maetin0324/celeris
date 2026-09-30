//! Root plan approval policy.

use super::*;

// ---- ADR-0079 D8（Phase R3b）: root の計画の承認の要否 ----

/// D8: 見込みの leaf の数で、子 task 1 つを leaf いくつと見るか（「leaf + 子 task × 既定 4」）。
pub const CHILD_LEAF_ESTIMATE: u64 = 4;

/// D8（Phase R3b）: root の計画の承認の要否の材料（呼び出し側が store の読み取りで集める。
/// `task_ops::plan_gate::approval_facts`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanApprovalFacts {
    /// この節点の未回答の決定の key（計画の決定と、採用時に daemon が出した limit / leaf_too_large）。
    pub open_decisions: Vec<String>,
    /// `review: human` で、まだ済んでいない unit を持つ段階の key（replan の版では済んだ段階を数えない）。
    pub review_human_stages: Vec<String>,
    /// 計画の段階の数。
    pub stages: usize,
    /// 最も多い段階の unit の数（統合 WU・repair を除く）。
    pub max_units_in_stage: usize,
    /// 計画の kind task の unit の数。
    pub child_task_units: usize,
    /// 木の生涯の leaf の見込み（作った leaf + 決定を待つ leaf + 終わっていない子 task × [`CHILD_LEAF_ESTIMATE`]）。
    pub estimated_leaves: u64,
    /// 木の run の見込み（ここまでの run + これから走る leaf + 終わっていない子 task × (planner 1 + leaf の見込み)）。
    pub estimated_runs: u64,
}

/// D8: 承認の要否と理由（理由は機械の読める短い文字列。`Event::PlanApprovalRequested.reasons`）:
/// `decisions:<key>,…` / `review_human:<stage>` / `near_limit:<設定名>:<値>/<上限>`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanApproval {
    pub required: bool,
    pub reasons: Vec<String>,
}

/// D8: `value` が `max × permille / 1000` 以上か（整数だけで比べる。`max = 0` は「近くない」）。
pub fn near_limit(value: u64, max: u64, permille: u32) -> bool {
    max > 0 && value.saturating_mul(1000) >= max.saturating_mul(u64::from(permille))
}

/// D8（Phase R3b）: `approval_required = has_decisions || has_review_human_stage || near_limits`（純粋関数、
/// 決定的）。`near_limits` は段階数・段階あたりの unit 数・子 task 数・見込みの leaf 数・木の run の見込みの
/// どれかが上限 × `approval_near_limit_permille` 以上。
pub fn plan_approval(facts: &PlanApprovalFacts, limits: &TreeLimits) -> PlanApproval {
    let mut reasons = Vec::new();
    if !facts.open_decisions.is_empty() {
        reasons.push(format!("decisions:{}", facts.open_decisions.join(",")));
    }
    for stage in &facts.review_human_stages {
        reasons.push(format!("review_human:{stage}"));
    }
    let permille = limits.approval_near_limit_permille;
    for (name, value, max) in [
        ("max_stages", facts.stages as u64, limits.max_stages as u64),
        (
            "max_units_per_stage",
            facts.max_units_in_stage as u64,
            limits.max_units_per_stage as u64,
        ),
        (
            "max_child_tasks_per_plan",
            facts.child_task_units as u64,
            limits.max_child_tasks_per_plan as u64,
        ),
        (
            "max_tree_leaves",
            facts.estimated_leaves,
            u64::from(limits.max_tree_leaves),
        ),
        (
            "max_tree_runs",
            facts.estimated_runs,
            u64::from(limits.max_tree_runs),
        ),
    ] {
        if near_limit(value, max, permille) {
            reasons.push(format!("near_limit:{name}:{value}/{max}"));
        }
    }
    PlanApproval {
        required: !reasons.is_empty(),
        reasons,
    }
}
