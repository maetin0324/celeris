//! ADR 2026-10-30-cos-plan-conforms-approve: the routine category (f) `plan_conforms`.
//!
//! CoS judges whether a plan waiting for its approval has nothing out of the ordinary (it does the
//! task's objective, changes neither objective nor acceptance) and which of its stages bill, spend
//! node-hours, publish, destroy or change production. The API checks what the store can decide:
//! the answer is the recommended `approve` of a plan gate, the basis names the plan version that
//! waits, the task has no open decision, every sensitive stage CoS declared runs only after an
//! earlier `review: human` stage or inside a human approval, and the task has not adopted more than
//! [`PLAN_CHURN_MAX_VERSIONS`] plan versions within [`PLAN_CHURN_WINDOW`].

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use task_core::store::SqliteStore;
use task_core::{DecisionStatus, ExecutionPlanRow, StageReview, TaskStore};
use task_ops::human_inbox::InboxKind;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{
    BASIS_INVALID, BASIS_PLAN_CHURN, BASIS_PLAN_UNGUARDED, BasisTarget, DelegatedBasis, FrameError,
    FrameKind, approved_frame, problem,
};
use crate::problem::{ApiProblem, store_problem};

/// The option key of a plan gate that runs the plan as written.
pub(crate) const APPROVE_OPTION: &str = "approve";
/// D2 (4): how many plan versions the task may adopt within [`PLAN_CHURN_WINDOW`] before CoS stops
/// approving them (the current version counts).
pub(crate) const PLAN_CHURN_MAX_VERSIONS: usize = 2;
/// D2 (4): the window of the churn count.
pub(crate) const PLAN_CHURN_WINDOW: time::Duration = time::Duration::hours(24);
const MAX_GUARDED_STAGES: usize = 32;

/// D2: what guards a sensitive stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StageGuardKind {
    /// An earlier stage of the same plan with `review: human` (the person checks before it runs).
    ReviewHuman,
    /// A standing rule a person set.
    StandingRule,
    /// A decision a person answered.
    Decision,
    /// An approval a person granted.
    Approval,
}

/// D2: a stage that bills, spends node-hours, publishes, destroys or changes production, and its
/// guard.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuardedStage {
    /// The stage key of the plan.
    pub stage: String,
    pub guard: StageGuardKind,
    /// The `review: human` stage key, or the standing rule / decision / approval id.
    pub id: String,
}

/// D2: the plan version CoS read and its sensitive stages (empty when none).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasisPlan {
    /// The id of the plan version waiting for approval (`PlanApprovalRequested.plan_id`).
    pub plan_id: String,
    #[serde(default)]
    pub guarded_stages: Vec<GuardedStage>,
}

/// The stage keys of `plan` in order (/3 `stages`, /1・/2 `phases`) with their review.
fn stages(plan: &ExecutionPlanRow) -> Vec<(&str, StageReview)> {
    if plan.spec.stages.is_empty() {
        plan.spec
            .phases
            .iter()
            .map(|p| (p.key.as_str(), StageReview::None))
            .collect()
    } else {
        plan.spec
            .stages
            .iter()
            .map(|s| (s.key.as_str(), s.review))
            .collect()
    }
}

/// D2 (2): every declared sensitive stage is a stage of the plan behind its guard.
fn check_guards(
    store: &SqliteStore,
    plan: &ExecutionPlanRow,
    guarded: &[GuardedStage],
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| problem(BASIS_INVALID, detail);
    let unguarded =
        |detail: String| problem(BASIS_PLAN_UNGUARDED, format!("{detail}; escalate it"));
    if guarded.len() > MAX_GUARDED_STAGES {
        return Err(invalid(format!(
            "basis.plan.guarded_stages must list at most {MAX_GUARDED_STAGES} stages"
        )));
    }
    let order = stages(plan);
    let mut seen = HashSet::new();
    for g in guarded {
        let key = g.stage.trim();
        let Some(at) = order.iter().position(|(k, _)| *k == key) else {
            return Err(invalid(format!(
                "basis.plan.guarded_stages: stage {key:?} is not a stage of plan {}",
                plan.id
            )));
        };
        if !seen.insert(key) {
            return Err(invalid(format!(
                "basis.plan.guarded_stages: stage {key} is listed twice"
            )));
        }
        let id = g.id.trim();
        let frame_kind = match g.guard {
            StageGuardKind::ReviewHuman => {
                let reviewed_before = order[..at]
                    .iter()
                    .any(|(k, review)| *k == id && *review == StageReview::Human);
                if !reviewed_before {
                    return Err(unguarded(format!(
                        "stage {key} needs an earlier stage with review: human; {id:?} is not one"
                    )));
                }
                continue;
            }
            StageGuardKind::StandingRule => FrameKind::StandingRule,
            StageGuardKind::Decision => FrameKind::Decision,
            StageGuardKind::Approval => FrameKind::Approval,
        };
        approved_frame(store, frame_kind, id).map_err(|why| match why {
            FrameError::Store(p) => p,
            FrameError::Invalid(detail) => unguarded(format!("stage {key}: {detail}")),
        })?;
    }
    Ok(())
}

/// D2 (4): the plan versions the task adopted within the window before `now`. A version whose time
/// does not parse counts (conservative).
fn recent_versions(plans: &[ExecutionPlanRow], now: OffsetDateTime) -> usize {
    plans
        .iter()
        .filter(|p| {
            OffsetDateTime::parse(&p.created_at, &Rfc3339)
                .map_or(true, |at| now - at < PLAN_CHURN_WINDOW)
        })
        .count()
}

pub(super) fn validate(
    store: &SqliteStore,
    basis: &DelegatedBasis,
    target: &BasisTarget,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| problem(BASIS_INVALID, detail);
    // D2 (5): only the recommended approve; replan and withdraw change the plan.
    if target.kind != Some(InboxKind::PlanGate)
        || target.option.as_deref() != Some(APPROVE_OPTION)
        || target.recommended.as_deref() != Some(APPROVE_OPTION)
    {
        return Err(invalid(format!(
            "plan_conforms answers the recommended {APPROVE_OPTION} to a plan gate (replan and \
             withdraw change the plan; escalate them)"
        )));
    }
    let Some(claimed) = &basis.plan else {
        return Err(invalid("plan_conforms needs basis.plan".into()));
    };
    let Some(task_id) = target.task_id else {
        return Err(invalid(
            "plan_conforms needs the task of the plan gate".into(),
        ));
    };
    let Some(task) = store.get(task_id).map_err(store_problem)? else {
        return Err(invalid(format!("task {task_id} does not exist")));
    };
    let events = store.events_for(task_id).map_err(store_problem)?;
    let Some(waiting) = task_ops::plan_gate::latest_plan_approval(&task, &events) else {
        return Err(invalid(format!(
            "task {task_id} is not waiting for a plan approval"
        )));
    };
    if claimed.plan_id.trim() != waiting.plan_id {
        return Err(invalid(format!(
            "basis.plan.plan_id {} is not the plan waiting for approval ({}); read the current version",
            claimed.plan_id, waiting.plan_id
        )));
    }
    let plans = store.execution_plan_list(task_id).map_err(store_problem)?;
    let Some(plan) = plans.iter().find(|p| p.id == waiting.plan_id) else {
        return Err(invalid(format!(
            "plan {} of task {task_id} is not in the store",
            waiting.plan_id
        )));
    };
    // D2 (1): a decision of the plan is a choice a person (or another routine answer) makes first.
    let root = task_core::tree::root_id_of(&task);
    let open: Vec<String> = store
        .decisions_list(Some(root))
        .map_err(store_problem)?
        .into_iter()
        .filter(|d| d.task_id == task_id && d.status == DecisionStatus::Open)
        .map(|d| d.key)
        .collect();
    if !open.is_empty() {
        return Err(invalid(format!(
            "task {task_id} has open decisions ({}); answer or escalate them before the plan",
            open.join(", ")
        )));
    }
    check_guards(store, plan, &claimed.guarded_stages)?;
    let recent = recent_versions(&plans, OffsetDateTime::now_utc());
    if recent > PLAN_CHURN_MAX_VERSIONS {
        return Err(problem(
            BASIS_PLAN_CHURN,
            format!(
                "task {task_id} adopted {recent} plan versions within {} hours (limit \
                 {PLAN_CHURN_MAX_VERSIONS}); escalate it",
                PLAN_CHURN_WINDOW.whole_hours()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, created_at: &str) -> ExecutionPlanRow {
        ExecutionPlanRow {
            id: id.into(),
            task_id: task_core::TaskId::new().to_string(),
            version: 1,
            origin: task_core::PlanOrigin::Planner,
            planner_run_id: None,
            status: task_core::PlanStatus::Active,
            spec: task_core::ExecutionPlanSpec {
                stages: Vec::new(),
                units: Vec::new(),
                decisions: Vec::new(),
                schema: task_core::EXECUTION_PLAN_SCHEMA_V3.to_string(),
                rationale: String::new(),
                work_units: Vec::new(),
                phases: Vec::new(),
                children: Vec::new(),
            },
            created_at: created_at.into(),
            superseded_at: None,
        }
    }

    #[test]
    fn recent_versions_counts_the_window_and_unparsable_times() {
        let now = OffsetDateTime::parse("2026-10-30T12:00:00Z", &Rfc3339).expect("now");
        let plans = [
            row("old", "2026-10-29T11:59:59Z"),
            row("fresh", "2026-10-29T12:00:01Z"),
            row("now", "2026-10-30T12:00:00Z"),
            row("broken", "yesterday"),
        ];
        assert_eq!(recent_versions(&plans, now), 3);
    }
}
