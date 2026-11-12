//! ADR 2026-10-28-cos-routine-delegated-answers D2 and ADR 2026-11-12-cos-triage-scope D3: the basis
//! of a CoS answer that no human instructed.
//!
//! A CoS answer to a wait without `instructed_by` names its category and evidence. `task_scope` is
//! CoS's own judgment inside the task's purpose and needs a ground (task, ADR, grant, a person's
//! words, …); the other categories are the shapes with extra deterministic checks (D3). This module checks only what
//! the API can decide from the request and the store: shape, the referenced approval, the
//! arithmetic of the frame, the repeated failure and the once-only raise. The judgment that the
//! cited evidence says what it claims stays with the worker and the reviewer of the audit.

use std::collections::HashSet;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use task_core::approval::{ApprovalId, ApprovalStore, Decision};
use task_core::decision::DecisionStatus;
use task_core::store::SqliteStore;
use task_core::{Event, TaskId, TaskStore};
use task_ops::human_inbox::InboxKind;

use crate::problem::{ApiProblem, store_problem};

mod job_wait_extend;
pub(crate) use job_wait_extend::grant_used;
mod plan_conforms;
mod review_rework;
pub use plan_conforms::{BasisPlan, GuardedStage, StageGuardKind, StageRiskKind};
pub(crate) use review_rework::REOPEN_OPTION;

/// The option key of a run-time tree limit decision that adds one allowance (ADR-0079 D7).
pub const RAISE_ONCE_OPTION: &str = "raise-once";
const MAX_REFS: usize = 20;
const MAX_REF_CHARS: usize = 300;
const MAX_FAILURE_CHARS: usize = 500;

/// ADR 2026-11-12-cos-triage-scope D3: what kind of judgment a CoS answer without a human
/// instruction is. Anything outside the human-reserved list (D1) may be answered as `task_scope`;
/// the other categories are answer shapes with their own deterministic checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BasisCategory {
    /// (a) Continue inside a resource / budget / node-hour frame a person approved.
    WithinApprovedFrame,
    /// (b) Raise a run-time tree limit (replan, retry, …) once, with progress and a new failure.
    RaiseOnce,
    /// CoS's judgment inside the task's objective and acceptance, grounded on the task, an ADR, a
    /// grant or a person's earlier instruction (ADR 2026-11-12-cos-triage-scope D3).
    TaskScope,
    /// (e) Reopen a task the reviewer failed for a technical defect (ADR 2026-10-29-cos-review-rework-reopen).
    ReviewRework,
    /// (f) Approve a plan gate whose plan has nothing out of the ordinary (ADR 2026-10-30-cos-plan-conforms-approve).
    PlanConforms,
    /// (g) Extend the wait of cluster jobs still queued at the wait limit (ADR 2026-10-30-cos-job-wait-extend).
    JobWaitExtend,
}

impl BasisCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            BasisCategory::WithinApprovedFrame => "within_approved_frame",
            BasisCategory::RaiseOnce => "raise_once",
            BasisCategory::TaskScope => "task_scope",
            BasisCategory::ReviewRework => "review_rework",
            BasisCategory::PlanConforms => "plan_conforms",
            BasisCategory::JobWaitExtend => "job_wait_extend",
        }
    }
}

/// D2 `refs[].kind`: what a piece of evidence is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BasisRefKind {
    StandingRule,
    Decision,
    Approval,
    Usage,
    Ledger,
    Commit,
    Artifact,
    Progress,
    Adr,
    Doc,
    Message,
    /// The reviewer run whose verdict failed the task (the id in `reviewer(<id>): …`).
    Review,
    /// ADR 2026-11-04-project-controller A7: a live grant of the authority ledger. It is evidence
    /// only: it does not lift `human_required` and does not stand in for the category's checks.
    AuthorityGrant,
    /// ADR 2026-11-12-cos-triage-scope D3: the task whose objective and acceptance CoS read.
    Task,
}

/// D3: the refs that ground a `task_scope` judgment (the others are evidence, not a ground).
pub(crate) const TASK_SCOPE_GROUNDS: &[BasisRefKind] = &[
    BasisRefKind::Task,
    BasisRefKind::Adr,
    BasisRefKind::Doc,
    BasisRefKind::Progress,
    BasisRefKind::Decision,
    BasisRefKind::StandingRule,
    BasisRefKind::Approval,
    BasisRefKind::Message,
    BasisRefKind::AuthorityGrant,
];

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasisRef {
    pub kind: BasisRefKind,
    /// The id, commit hash, repository-relative `.md` path or the API path read (with its time).
    pub id: String,
}

/// D2 `frame.kind`: the human approval that set the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    StandingRule,
    Decision,
    Approval,
}

/// D1 (a): the approved frame and the usage CoS checked against it.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasisFrame {
    pub kind: FrameKind,
    pub id: String,
    /// What is counted (`tokens`, `usd`, `node_hours`, …).
    pub unit: String,
    pub limit: f64,
    pub used: f64,
    pub requested: f64,
}

/// D1 (b): the failure before the previous allowance and the current one.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasisFailures {
    pub previous: String,
    pub current: String,
}

/// D2: the basis of a routine answer (`resolve` with outcome=answer, or a wait-answering
/// `/cos/operations` request without `instructed_by`).
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegatedBasis {
    pub category: BasisCategory,
    pub refs: Vec<BasisRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<BasisFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failures: Option<BasisFailures>,
    /// (f) `plan_conforms` only: the plan version CoS read and the guard of each sensitive stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<BasisPlan>,
}

/// What the answer targets, as far as the API knows it.
#[derive(Debug, Clone, Default)]
pub(crate) struct BasisTarget {
    pub kind: Option<InboxKind>,
    pub recommended: Option<String>,
    pub task_id: Option<TaskId>,
    /// The chosen option key (`None` for a free-text answer).
    pub option: Option<String>,
    pub note: Option<String>,
    /// The approval an authorization answer decides (`authorization-<id>`, `/approvals/<id>/decide`).
    pub approval_id: Option<String>,
}

pub(crate) const BASIS_MISSING: &str = "cos_basis_missing";
pub(crate) const BASIS_INVALID: &str = "cos_basis_invalid";
pub(crate) const BASIS_OVER_FRAME: &str = "cos_basis_over_frame";
pub(crate) const BASIS_REPEATED_FAILURE: &str = "cos_basis_repeated_failure";
pub(crate) const BASIS_RAISE_ONCE_USED: &str = "cos_basis_raise_once_used";
pub(crate) const BASIS_REWORK_LIMIT: &str = "cos_basis_rework_limit";
pub(crate) const BASIS_PLAN_UNGUARDED: &str = "cos_basis_plan_unguarded";
pub(crate) const BASIS_PLAN_CHURN: &str = "cos_basis_plan_churn";
pub(crate) const BASIS_PLAN_NEEDS_HUMAN: &str = "cos_basis_plan_needs_human";
pub(crate) const BASIS_JOB_NOT_QUEUED: &str = "cos_basis_job_not_queued";
pub(crate) const BASIS_EXTEND_LIMIT: &str = "cos_basis_extend_limit";
pub(crate) const BASIS_AUTHORITY_GRANT: &str = "cos_basis_authority_grant_invalid";
/// ADR 2026-11-12-cos-triage-scope D1/D2: the wait is one a person keeps.
pub(crate) const RESERVED_FOR_HUMAN: &str = "cos_reserved_for_human";

/// D1: refuse answering (or routing to the operator) a wait the deterministic match reserved for a person.
pub(crate) fn reserved_for_human(reserved: task_ops::triage_match::Reserved) -> ApiProblem {
    problem(
        RESERVED_FOR_HUMAN,
        format!(
            "this wait is reserved for a person ({}); escalate it with to=human",
            reserved.as_str()
        ),
    )
}

fn problem(code: &'static str, detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, code, detail)
}

pub(crate) fn missing() -> ApiProblem {
    problem(
        BASIS_MISSING,
        "a CoS answer without instructed_by needs basis (task_scope with a ground, or within_approved_frame, \
         raise_once, review_rework, plan_conforms or job_wait_extend with its evidence); otherwise escalate",
    )
}

/// The audit reason of an accepted CoS answer (ADR 2026-11-12-cos-triage-scope D3). The policy
/// version and the cited grants are kept in the operation record (`policy_version`, `payload.basis`).
pub(crate) fn reason_prefix(basis: &DelegatedBasis) -> String {
    format!("CoS の判断（{}）", basis.category.as_str())
}

fn md_path(id: &str, prefixes: &[&str]) -> bool {
    id.ends_with(".md")
        && !id.starts_with('/')
        && !id.split('/').any(|part| part == ".." || part.is_empty())
        && prefixes.iter().any(|p| id.starts_with(p))
}

fn check_ref(r: &BasisRef) -> Result<(), String> {
    let id = r.id.trim();
    if id.is_empty() || id.chars().count() > MAX_REF_CHARS {
        return Err(format!(
            "basis.refs[].id must be 1..={MAX_REF_CHARS} characters"
        ));
    }
    let ok = match r.kind {
        BasisRefKind::Commit => {
            (7..=40).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit())
        }
        BasisRefKind::Adr => md_path(id, &["agent-docs/adr/"]),
        BasisRefKind::Doc => md_path(id, &["docs/", "agent-docs/"]),
        BasisRefKind::Progress => md_path(id, &["agent-docs/progress/"]),
        BasisRefKind::Task => id.parse::<TaskId>().is_ok(),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "basis.refs[] {:?} {id:?} is malformed (commit: 7..=40 hex; adr: agent-docs/adr/*.md; \
             doc: docs/ or agent-docs/ *.md; progress: agent-docs/progress/*.md; task: a task id)",
            r.kind
        ))
    }
}

fn has_ref(basis: &DelegatedBasis, kinds: &[BasisRefKind]) -> bool {
    basis.refs.iter().any(|r| kinds.contains(&r.kind))
}

/// Collapse whitespace so a reworded spacing does not count as a new failure.
fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A7: a cited grant must exist and still be live (not revoked, not expired, budget left, source readable).
fn live_grant(store: &SqliteStore, id: &str) -> Result<(), ApiProblem> {
    let dead = |why: &str| {
        problem(
            BASIS_AUTHORITY_GRANT,
            format!("grant {id} is not live: {why}"),
        )
    };
    let grant = store
        .authority_grant(id)
        .map_err(store_problem)?
        .ok_or_else(|| dead("not found"))?;
    let verdict = store.authority_check(
        &grant.project_id,
        grant.action_class.as_str(),
        &grant.scope,
        time::OffsetDateTime::now_utc(),
    );
    if verdict.allow {
        Ok(())
    } else {
        Err(dead(&verdict.reason))
    }
}

/// D1/D2: check `basis` against the answer target. Every refusal is 422 with a `cos_basis_*` code.
pub(crate) fn validate(
    store: &SqliteStore,
    basis: &DelegatedBasis,
    target: &BasisTarget,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| problem(BASIS_INVALID, detail);
    if basis.refs.is_empty() || basis.refs.len() > MAX_REFS {
        return Err(invalid(format!(
            "basis.refs must list 1..={MAX_REFS} pieces of evidence"
        )));
    }
    let mut seen = HashSet::new();
    for r in &basis.refs {
        check_ref(r).map_err(&invalid)?;
        if !seen.insert((r.kind, r.id.trim())) {
            return Err(invalid(format!(
                "basis.refs[] {:?} {} is listed twice",
                r.kind, r.id
            )));
        }
        if r.kind == BasisRefKind::AuthorityGrant {
            live_grant(store, r.id.trim())?;
        }
    }
    let category = basis.category;
    if basis.frame.is_some() && category != BasisCategory::WithinApprovedFrame {
        return Err(invalid(
            "basis.frame belongs to within_approved_frame only".into(),
        ));
    }
    if basis.failures.is_some() && category != BasisCategory::RaiseOnce {
        return Err(invalid("basis.failures belongs to raise_once only".into()));
    }
    if basis.plan.is_some() && category != BasisCategory::PlanConforms {
        return Err(invalid("basis.plan belongs to plan_conforms only".into()));
    }
    let option = target.option.as_deref();
    // ADR 2026-10-30-cos-plan-conforms-approve D3: approving a plan takes the plan checks (or a
    // numeric frame); the gate always recommends approve, so task_scope would skip them.
    if target.kind == Some(InboxKind::PlanGate)
        && option == Some(plan_conforms::APPROVE_OPTION)
        && !matches!(
            category,
            BasisCategory::PlanConforms | BasisCategory::WithinApprovedFrame
        )
    {
        return Err(invalid(
            "approving a plan gate needs basis.category=plan_conforms (or within_approved_frame)"
                .into(),
        ));
    }
    if option == Some(review_rework::REOPEN_OPTION)
        && target.kind == Some(InboxKind::Failed)
        && category != BasisCategory::ReviewRework
    {
        return Err(invalid(
            "reopening a failed task needs basis.category=review_rework".into(),
        ));
    }
    // ADR 2026-11-12-cos-triage-scope D3: an authorization is a permission gate; only the frame and
    // the job wait limit answer it.
    if target.kind == Some(InboxKind::Authorization)
        && !matches!(
            category,
            BasisCategory::JobWaitExtend | BasisCategory::WithinApprovedFrame
        )
    {
        return Err(invalid(
            "answering an authorization needs basis.category=job_wait_extend or within_approved_frame"
                .into(),
        ));
    }
    if option == Some(RAISE_ONCE_OPTION) && category != BasisCategory::RaiseOnce {
        return Err(invalid(format!(
            "answering {RAISE_ONCE_OPTION} needs basis.category=raise_once"
        )));
    }
    let kind = target.kind;
    let kind_name = kind.map_or("unknown", InboxKind::as_str);
    match category {
        BasisCategory::WithinApprovedFrame => {
            if !matches!(
                kind,
                Some(
                    InboxKind::Decision
                        | InboxKind::Authorization
                        | InboxKind::PlanGate
                        | InboxKind::PhaseGate
                )
            ) {
                return Err(invalid(format!(
                    "within_approved_frame answers a decision, authorization or plan/phase gate, not {kind_name}"
                )));
            }
            plan_conforms::needs_human(store, target)?;
            let Some(frame) = &basis.frame else {
                return Err(invalid("within_approved_frame needs basis.frame".into()));
            };
            if !has_ref(basis, &[BasisRefKind::Usage, BasisRefKind::Ledger]) {
                return Err(invalid(
                    "within_approved_frame needs a usage or ledger ref (where the used amount was read)"
                        .into(),
                ));
            }
            if frame.unit.trim().is_empty() {
                return Err(invalid("basis.frame.unit must not be empty".into()));
            }
            let numbers = [frame.limit, frame.used, frame.requested];
            if numbers.iter().any(|n| !n.is_finite())
                || frame.limit <= 0.0
                || frame.used < 0.0
                || frame.requested <= 0.0
            {
                return Err(invalid(
                    "basis.frame needs finite limit > 0, used >= 0 and requested > 0".into(),
                ));
            }
            approved_frame(store, frame.kind, &frame.id).map_err(|why| match why {
                FrameError::Store(p) => p,
                FrameError::Invalid(detail) => invalid(detail),
            })?;
            if frame.used + frame.requested > frame.limit {
                return Err(problem(
                    BASIS_OVER_FRAME,
                    format!(
                        "used {} + requested {} exceeds the approved limit {} {}; escalate it",
                        frame.used, frame.requested, frame.limit, frame.unit
                    ),
                ));
            }
        }
        BasisCategory::RaiseOnce => {
            if kind != Some(InboxKind::Decision) || option != Some(RAISE_ONCE_OPTION) {
                return Err(invalid(format!(
                    "raise_once answers {RAISE_ONCE_OPTION} to a limit decision"
                )));
            }
            if !has_ref(basis, &[BasisRefKind::Commit, BasisRefKind::Artifact]) {
                return Err(invalid(
                    "raise_once needs a commit or artifact ref showing progress".into(),
                ));
            }
            let Some(failures) = &basis.failures else {
                return Err(invalid("raise_once needs basis.failures".into()));
            };
            let (previous, current) = (
                normalized(&failures.previous),
                normalized(&failures.current),
            );
            for (name, text) in [("previous", &previous), ("current", &current)] {
                if text.is_empty() || text.chars().count() > MAX_FAILURE_CHARS {
                    return Err(invalid(format!(
                        "basis.failures.{name} must be 1..={MAX_FAILURE_CHARS} characters"
                    )));
                }
            }
            if previous == current {
                return Err(problem(
                    BASIS_REPEATED_FAILURE,
                    "the current failure repeats the previous one; escalate it",
                ));
            }
            let Some(task_id) = target.task_id else {
                return Err(invalid("raise_once needs the task of the decision".into()));
            };
            let raised_before = store
                .events_for(task_id)
                .map_err(store_problem)?
                .iter()
                .any(|(_, e)| {
                    matches!(e, Event::DecisionAnswered { option, by, .. }
                        if option == RAISE_ONCE_OPTION && task_core::is_cos_actor(by))
                });
            if raised_before {
                return Err(problem(
                    BASIS_RAISE_ONCE_USED,
                    format!(
                        "CoS already answered {RAISE_ONCE_OPTION} for task {task_id}; escalate it"
                    ),
                ));
            }
        }
        BasisCategory::TaskScope => {
            if !has_ref(basis, TASK_SCOPE_GROUNDS) {
                return Err(invalid(
                    "task_scope needs a ground in refs (task, adr, doc, progress, decision, \
                     standing_rule, approval, message or authority_grant); without one, escalate"
                        .into(),
                ));
            }
            if kind == Some(InboxKind::Question)
                && option.is_none()
                && target.note.as_deref().is_none_or(|n| n.trim().is_empty())
            {
                return Err(invalid(
                    "a free-text answer to a worker question needs answer.note".into(),
                ));
            }
        }
        BasisCategory::ReviewRework => review_rework::validate(store, basis, target)?,
        BasisCategory::PlanConforms => plan_conforms::validate(store, basis, target)?,
        BasisCategory::JobWaitExtend => job_wait_extend::validate(store, basis, target)?,
    }
    Ok(())
}

enum FrameError {
    Store(ApiProblem),
    Invalid(String),
}

/// The frame (or a plan stage guard) must point at a human approval in the store.
fn approved_frame(store: &SqliteStore, kind: FrameKind, id: &str) -> Result<(), FrameError> {
    let id = id.trim();
    let store_err = |e| FrameError::Store(store_problem(e));
    match kind {
        FrameKind::StandingRule => {
            let rules = store.standing_rule_list(None).map_err(store_err)?;
            if rules.iter().any(|r| r.id.to_string() == id) {
                Ok(())
            } else {
                Err(FrameError::Invalid(format!(
                    "basis.frame: standing rule {id} does not exist"
                )))
            }
        }
        FrameKind::Decision => {
            let row = store.decision_get(id).map_err(store_err)?;
            let human = row.as_ref().is_some_and(|row| {
                row.status == DecisionStatus::Answered
                    && row
                        .request
                        .answer
                        .as_ref()
                        .is_some_and(|a| !task_core::is_cos_actor(&a.by))
            });
            if human {
                Ok(())
            } else {
                Err(FrameError::Invalid(format!(
                    "basis.frame: decision {id} is not answered by a person"
                )))
            }
        }
        FrameKind::Approval => {
            let decided = match id.parse::<ApprovalId>() {
                Ok(approval_id) => store
                    .approval_get(approval_id)
                    .map_err(store_err)?
                    .is_some_and(|a| {
                        matches!(a.decision, Some(Decision::Once | Decision::Standing))
                    }),
                Err(_) => false,
            };
            if decided {
                Ok(())
            } else {
                Err(FrameError::Invalid(format!(
                    "basis.frame: approval {id} is not granted"
                )))
            }
        }
    }
}
