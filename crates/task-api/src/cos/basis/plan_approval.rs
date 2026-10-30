//! ADR 2026-10-30-cos-plan-approval: the routine category (f) `plan_approval`.
//!
//! CoS reads the plan waiting for approval (`blocked(awaiting_plan_approval)`, a replanned version
//! included) and judges that it changes neither objective nor acceptance, stays inside the cost a
//! person approved, and touches no production, external publication, destructive operation or
//! secret. The API checks what the store can decide: the answer is `approve` to a plan gate, the
//! refs name the waiting version and the version before it, CoS lists every item it checked, the
//! task has no human check, the gate is not raised for near tree limits, no daemon check failed
//! (`plan_invalid` / `limit` / `leaf_too_large` decisions) and nobody edited objective or acceptance
//! since the previous version, and replans have not gone on past [`PLAN_VERSION_MAX`] versions or
//! [`PLAN_APPROVAL_MAX`] CoS approvals. Approving does not answer the plan's decisions nor skip its
//! `review: human` stages: they stay human waits.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use task_core::decision::{DecisionKind, DecisionStatus};
use task_core::store::SqliteStore;
use task_core::{Check, Event, TaskStore};
use task_ops::human_inbox::InboxKind;

use super::{
    BASIS_INVALID, BASIS_PLAN_LIMIT, BASIS_PLAN_NEEDS_HUMAN, BasisCategory, BasisRefKind,
    BasisTarget, DelegatedBasis, problem, reason_prefix,
};
use crate::problem::{ApiProblem, store_problem};

/// The option key of a plan gate that adopts the plan as written.
pub(crate) const APPROVE_OPTION: &str = "approve";
/// D3: how many plan versions of the same task CoS may approve on `plan_approval`.
pub(crate) const PLAN_APPROVAL_MAX: usize = 3;
/// D3: the highest plan version CoS may approve (the first plan and three replans).
pub(crate) const PLAN_VERSION_MAX: u32 = 4;
const MAX_CHANGES_CHARS: usize = 1000;

/// D2: what CoS checked in the plan before approving it. `basis.plan.checked` lists every item.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanCheck {
    /// The plan keeps the task's objective and acceptance (no change asked of the person).
    ObjectiveAcceptance,
    /// Billing, paid trials and node-hours stay inside a frame a person approved.
    CostFrame,
    /// No change to the production daemon, systemd units, releases or production config.
    Production,
    /// No external publication, external push or sending outside.
    ExternalPublish,
    /// No irreversible or destructive operation.
    Destructive,
    /// No secret, permission or security policy change.
    SecretsPermissions,
    /// The plan's decisions and `review: human` stages stay human waits.
    HumanSteps,
}

impl PlanCheck {
    pub const ALL: [PlanCheck; 7] = [
        PlanCheck::ObjectiveAcceptance,
        PlanCheck::CostFrame,
        PlanCheck::Production,
        PlanCheck::ExternalPublish,
        PlanCheck::Destructive,
        PlanCheck::SecretsPermissions,
        PlanCheck::HumanSteps,
    ];

    fn as_str(self) -> &'static str {
        match self {
            PlanCheck::ObjectiveAcceptance => "objective_acceptance",
            PlanCheck::CostFrame => "cost_frame",
            PlanCheck::Production => "production",
            PlanCheck::ExternalPublish => "external_publish",
            PlanCheck::Destructive => "destructive",
            PlanCheck::SecretsPermissions => "secrets_permissions",
            PlanCheck::HumanSteps => "human_steps",
        }
    }
}

/// D2 (f): what CoS read in the plan. The plan ids themselves are `refs` of kind `plan`.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasisPlan {
    /// What changed from the previous version (for the first plan: what the plan does).
    pub changes: String,
    /// The items CoS checked; must list all of them.
    pub checked: Vec<PlanCheck>,
}

/// The plan versions of a task in event order: `(seq, plan_id, version)`.
fn planned(events: &[(u64, Event)]) -> Vec<(u64, &str, u32)> {
    events
        .iter()
        .filter_map(|(seq, e)| match e {
            Event::ExecutionPlanned {
                plan_id, version, ..
            } => Some((*seq, plan_id.as_str(), *version)),
            _ => None,
        })
        .collect()
}

/// A daemon decision that says a check of the plan or the tree failed.
fn failed_check(kind: DecisionKind) -> bool {
    !matches!(kind, DecisionKind::Choice)
}

pub(super) fn validate(
    store: &SqliteStore,
    basis: &DelegatedBasis,
    target: &BasisTarget,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| problem(BASIS_INVALID, detail);
    let needs_human = |detail: String| problem(BASIS_PLAN_NEEDS_HUMAN, detail);
    if target.kind != Some(InboxKind::PlanGate) || target.option.as_deref() != Some(APPROVE_OPTION)
    {
        return Err(invalid(format!(
            "plan_approval answers {APPROVE_OPTION} to a plan gate (replan and withdraw stay with a person)"
        )));
    }
    let Some(plan) = &basis.plan else {
        return Err(invalid("plan_approval needs basis.plan".into()));
    };
    let changes = plan.changes.trim();
    if changes.is_empty() || changes.chars().count() > MAX_CHANGES_CHARS {
        return Err(invalid(format!(
            "basis.plan.changes must be 1..={MAX_CHANGES_CHARS} characters"
        )));
    }
    let checked: BTreeSet<PlanCheck> = plan.checked.iter().copied().collect();
    let missing: Vec<&str> = PlanCheck::ALL
        .iter()
        .filter(|c| !checked.contains(c))
        .map(|c| c.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(invalid(format!(
            "basis.plan.checked must list every item; missing {}",
            missing.join(", ")
        )));
    }
    let Some(task_id) = target.task_id else {
        return Err(invalid(
            "plan_approval needs the task of the plan gate".into(),
        ));
    };
    let Some(task) = store.get(task_id).map_err(store_problem)? else {
        return Err(invalid(format!("task {task_id} does not exist")));
    };
    // D3: a human check is the person's judgment of the result; the plan to reach it is theirs too.
    if task
        .acceptance
        .iter()
        .any(|c| matches!(c.check, Check::Human))
    {
        return Err(crate::cos::inbox::human_required_problem(
            "the task has a human check",
        ));
    }
    let events = store.events_for(task_id).map_err(store_problem)?;
    let Some(info) = task_ops::plan_gate::latest_plan_approval(&task, &events) else {
        return Err(invalid(format!(
            "task {task_id} is not awaiting a plan approval"
        )));
    };
    let versions = planned(&events);
    let Some(pos) = versions.iter().rposition(|(_, id, _)| *id == info.plan_id) else {
        return Err(invalid(format!(
            "plan {} has no adopted version in task {task_id}",
            info.plan_id
        )));
    };
    let (_, _, version) = versions[pos];
    let previous = versions[..pos]
        .iter()
        .rev()
        .find(|(_, id, _)| *id != info.plan_id);
    let expected: BTreeSet<&str> = std::iter::once(info.plan_id.as_str())
        .chain(previous.map(|(_, id, _)| *id))
        .collect();
    let cited: BTreeSet<&str> = basis
        .refs
        .iter()
        .filter(|r| r.kind == BasisRefKind::Plan)
        .map(|r| r.id.trim())
        .collect();
    if cited != expected {
        return Err(invalid(format!(
            "plan_approval needs plan refs naming the waiting version and the one before it ({})",
            expected.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    if let Some(near) = info.reasons.iter().find(|r| r.starts_with("near_limit:")) {
        return Err(needs_human(format!(
            "the plan gate was raised for a tree limit ({near}); escalate it"
        )));
    }
    // Since the previous version (or ever, for the first plan): no daemon check failed.
    let since = previous.map_or(0, |(seq, _, _)| *seq);
    let check_failed = events.iter().find_map(|(seq, e)| match e {
        Event::DecisionRequested { decision } if *seq > since && failed_check(decision.kind) => {
            Some(decision.kind.as_str())
        }
        _ => None,
    });
    let open_failed = store
        .decisions_list(Some(task_core::tree::root_id_of(&task)))
        .map_err(store_problem)?
        .into_iter()
        .find(|d| d.task_id == task_id && d.status == DecisionStatus::Open && failed_check(d.kind))
        .map(|d| d.kind.as_str());
    if let Some(kind) = check_failed.or(open_failed) {
        return Err(needs_human(format!(
            "a check of the plan or the tree failed ({kind} decision); escalate it"
        )));
    }
    if previous.is_some() {
        let edited = events.iter().any(|(seq, e)| {
            *seq > since
                && matches!(e, Event::Edited { fields, .. }
                    if fields.iter().any(|f| f == "objective" || f == "acceptance"))
        });
        if edited {
            return Err(needs_human(
                "the objective or acceptance was edited since the previous plan; escalate it"
                    .into(),
            ));
        }
    }
    if version > PLAN_VERSION_MAX {
        return Err(problem(
            BASIS_PLAN_LIMIT,
            format!(
                "plan version {version} of task {task_id} is past the replan limit \
                 (version {PLAN_VERSION_MAX}); escalate it"
            ),
        ));
    }
    let prefix = reason_prefix(&DelegatedBasis {
        category: BasisCategory::PlanApproval,
        refs: Vec::new(),
        frame: None,
        failures: None,
        plan: None,
    });
    let approved = events
        .iter()
        .filter(|(_, e)| {
            matches!(e, Event::CosOperation { state, target_kind, reason, .. }
                if state == "applied" && target_kind == "task" && reason.starts_with(&prefix))
        })
        .count();
    if approved >= PLAN_APPROVAL_MAX {
        return Err(problem(
            BASIS_PLAN_LIMIT,
            format!(
                "CoS already approved {approved} plans of task {task_id} on plan_approval (limit \
                 {PLAN_APPROVAL_MAX}); escalate it"
            ),
        ));
    }
    Ok(())
}
