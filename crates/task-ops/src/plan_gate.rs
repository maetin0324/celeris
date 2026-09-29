//! ADR-0079 D8（Phase R3b）: root の計画の承認（`blocked(awaiting_plan_approval)`）の判定と、人の 3 つの操作
//! （`approve` / `replan` / `withdraw`）。途中確認（ADR-0074 D2、`crate::phase_gate`）と同じ形で、状態の変更は
//! `TaskStore::apply_transition*` だけで行う。承認の要否の材料（[`approval_facts`]）は store の読み取りだけ。
//! LLM は使わない（DESIGN 原則 1）。
//!
//! - approve: `PhaseResume{PlanApprove}`（reason `plan_approved`、attempts 不変）。`note` は次の run の `answers` に
//!   「計画の承認（ADR-0079 D8）」として渡る。
//! - replan: `note` 必須。`PhaseResume{PlanReplan}`（reason `plan_replan`）と、R3a の `plan_invalid` → replan と同じ
//!   `ExecutionHintSet{replan: true, note}`（次の dispatch で replan の planner run。`replan_reason` に note）。
//! - withdraw: 既存の `Cancel`（subtree に連鎖。ADR-0074 D2.4 の withdraw と同じ）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    DecisionStatus, Event, ExecutionMode, ExecutionPlanRow, PhaseResumeMode, PlanApprovalFacts,
    Status, Task, TaskId, TaskStore, Trigger, WorkUnitBlockedReason, WorkUnitKind, WorkUnitStatus,
};

use crate::error::OpsError;
use crate::gate::TransitionResult;

/// `Trigger::PlanGate` の `Transitioned.reason`。
pub const AWAITING_PLAN_APPROVAL: &str = "awaiting_plan_approval";

/// 承認の note を次の run に渡す `Event::Answered.question`。
pub const PLAN_APPROVAL_QUESTION: &str = "計画の承認（ADR-0079 D8）";

/// D8: root の計画の承認を待っているか（`Blocked` で、直前の遷移の reason が `awaiting_plan_approval`）。
pub fn is_awaiting_plan_approval(task: &Task, events: &[(u64, Event)]) -> bool {
    task.status == Status::Blocked
        && crate::phase_gate::last_transition_reason(events) == Some(AWAITING_PLAN_APPROVAL)
}

/// 人の操作だけで再開する止まり方（途中確認 `awaiting_human` と計画の承認 `awaiting_plan_approval`）か。
/// どちらも質問ではない（`Answer` を受けない・`QuestionBlocked` を鳴らさない・受信箱の `questions` に出さない）。
pub fn is_human_gate(task: &Task, events: &[(u64, Event)]) -> bool {
    crate::phase_gate::is_awaiting_human(task, events) || is_awaiting_plan_approval(task, events)
}

/// 承認待ちの 1 件分（受信箱・通知・GUI の材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanApprovalInfo {
    /// `Transitioned{reason: "awaiting_plan_approval"}` の seq。
    pub transition_seq: u64,
    pub plan_id: String,
    /// `PlanApprovalRequested.reasons`（`decisions:…` / `review_human:…` / `near_limit:…`）。
    pub reasons: Vec<String>,
}

/// D8: 承認待ちなら、直近の `PlanApprovalRequested` と止めた遷移の seq。承認待ちでなければ `None`。
pub fn latest_plan_approval(task: &Task, events: &[(u64, Event)]) -> Option<PlanApprovalInfo> {
    if !is_awaiting_plan_approval(task, events) {
        return None;
    }
    let transition_seq = events.iter().rev().find_map(|(seq, e)| match e {
        Event::Transitioned { reason, .. } if reason == AWAITING_PLAN_APPROVAL => Some(*seq),
        _ => None,
    })?;
    let (plan_id, reasons) = events.iter().rev().find_map(|(_, e)| match e {
        Event::PlanApprovalRequested { plan_id, reasons } => {
            Some((plan_id.clone(), reasons.clone()))
        }
        _ => None,
    })?;
    Some(PlanApprovalInfo {
        transition_seq,
        plan_id,
        reasons,
    })
}

/// D8: `POST /tasks/{id}/execution/plan-gate` の `action`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanGateAction {
    /// 計画のとおり進める（`PhaseResume{PlanApprove}`）。
    Approve,
    /// note の指示で planner に計画を書き直させる（`note` 必須）。
    Replan,
    /// この task（と subtree）を取り下げる（既存の `Cancel`）。
    Withdraw,
}

impl PlanGateAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanGateAction::Approve => "approve",
            PlanGateAction::Replan => "replan",
            PlanGateAction::Withdraw => "withdraw",
        }
    }
}

/// D8: `POST /tasks/{id}/execution/plan-gate` の本文（ADR-0074 D2.4 の phase-gate と同じ形。`decision` は
/// `action` の別名として受け付ける）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanGateRequest {
    #[serde(alias = "decision")]
    pub action: PlanGateAction,
    /// `approve` では任意（次の run に「計画の承認」として渡す）。`replan` では必須（planner への指示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// note の上限（決定の回答の note と同じ 2,000 文字）。
pub const PLAN_GATE_NOTE_MAX_CHARS: usize = 2000;

/// D8: 人（API は `human`、MCP は `mcp:<client_id>`）の計画の承認への応答を適用する。
///
/// - 無い task は `NotFound`（404）、承認待ちでなければ `InvalidState`（409）。
/// - `replan` で `note` が空・note が長すぎるときは `Validation`（422）。状態は変えない。
pub fn plan_gate(
    store: &dyn TaskStore,
    id: TaskId,
    action: PlanGateAction,
    note: Option<String>,
    by: &str,
) -> Result<TransitionResult, OpsError> {
    let task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    let events = store.events_for(id)?;
    if !is_awaiting_plan_approval(&task, &events) {
        return Err(OpsError::InvalidState {
            id,
            context: format!(
                "status={:?}, reason={}",
                task.status,
                crate::phase_gate::last_transition_reason(&events).unwrap_or("-")
            ),
            action: format!(
                "resumed from a plan approval ({}); only tasks awaiting_plan_approval accept it",
                action.as_str()
            ),
        });
    }
    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if note
        .as_deref()
        .is_some_and(|n| n.chars().count() > PLAN_GATE_NOTE_MAX_CHARS)
    {
        return Err(OpsError::Validation(format!(
            "note: must be at most {PLAN_GATE_NOTE_MAX_CHARS} characters"
        )));
    }
    let plan_id = latest_plan_approval(&task, &events)
        .map(|i| i.plan_id)
        .unwrap_or_default();
    let audit = |what: &str| {
        Event::worker_progress(
            String::new(),
            format!(
                "{PLAN_APPROVAL_QUESTION}: {what}（{by}、計画 {plan_id}）{}",
                note.as_deref()
                    .map(|n| format!(": {n}"))
                    .unwrap_or_default()
            ),
        )
    };
    let (trigger, extra) = match action {
        PlanGateAction::Withdraw => {
            store.append_event(id, &audit("取り下げ"))?;
            return crate::gate::cancel(store, id, Some(Status::Blocked));
        }
        PlanGateAction::Approve => {
            let mut extra = vec![audit("承認")];
            if let Some(n) = &note {
                extra.push(Event::Answered {
                    question: PLAN_APPROVAL_QUESTION.to_string(),
                    answer: n.clone(),
                });
            }
            (
                Trigger::PhaseResume {
                    mode: PhaseResumeMode::PlanApprove,
                },
                extra,
            )
        }
        PlanGateAction::Replan => {
            let Some(n) = note.clone() else {
                return Err(OpsError::Validation(
                    "note: replan requires a non-empty note (the human instruction for the planner)"
                        .to_string(),
                ));
            };
            let previous = task.routing.as_ref().and_then(|r| r.execution_hint);
            (
                Trigger::PhaseResume {
                    mode: PhaseResumeMode::PlanReplan,
                },
                vec![
                    audit("replan"),
                    Event::ExecutionHintSet {
                        mode: ExecutionMode::Compound,
                        previous,
                        previous_decision: None,
                        source: format!("{by} (plan-gate)"),
                        note: Some(format!("計画の承認で replan: {n}")),
                        replan: true,
                    },
                ],
            )
        }
    };
    let from = task.status;
    let outcome = store.apply_transition_with_events(id, trigger, extra)?;
    Ok(TransitionResult {
        id,
        from,
        to: outcome.next,
        reason: outcome.reason.to_string(),
        cascaded: Vec::new(),
    })
}

/// D8（Phase R3b）: 採用した root の計画の承認の要否の材料（store の読み取りだけ。判定は
/// `task_core::tree::plan_approval`）。
pub fn approval_facts(
    store: &dyn TaskStore,
    task: &Task,
    plan: &ExecutionPlanRow,
) -> Result<PlanApprovalFacts, OpsError> {
    let units: Vec<task_core::WorkUnitRow> = store
        .work_units_for(task.id)?
        .into_iter()
        .filter(|u| u.status.is_active())
        .collect();
    let root_id = task_core::tree::root_id_of(task);
    let open_decisions: Vec<String> = store
        .decisions_list(Some(root_id))?
        .into_iter()
        .filter(|d| d.task_id == task.id && d.status == DecisionStatus::Open)
        .map(|d| d.key)
        .collect();
    let spec = &plan.spec;
    let review_human_stages: Vec<String> = spec
        .stages
        .iter()
        .filter(|s| s.review == task_core::StageReview::Human)
        .filter(|s| {
            units.iter().any(|u| {
                u.phase.as_deref() == Some(s.key.as_str()) && u.status != WorkUnitStatus::Done
            })
        })
        .map(|s| s.key.clone())
        .collect();
    let max_units_in_stage = spec
        .stages
        .iter()
        .map(|s| spec.units.iter().filter(|u| u.stage == s.key).count())
        .max()
        .unwrap_or(0);
    let child_task_units = spec
        .units
        .iter()
        .filter(|u| u.kind == WorkUnitKind::Task)
        .count();
    let counters = crate::tree::tree_counters(store, root_id)?;
    let is_leaf = |u: &&task_core::WorkUnitRow| {
        !matches!(
            u.kind,
            WorkUnitKind::Task | WorkUnitKind::Integrate | WorkUnitKind::Repair
        )
    };
    let held_leaves = units
        .iter()
        .filter(is_leaf)
        .filter(|u| {
            u.status == WorkUnitStatus::Blocked
                && u.blocked_reason == Some(WorkUnitBlockedReason::Decision)
        })
        .count() as u64;
    let remaining_leaves = units
        .iter()
        .filter(is_leaf)
        .filter(|u| u.status != WorkUnitStatus::Done)
        .count() as u64;
    let open_children = units
        .iter()
        .filter(|u| u.kind == WorkUnitKind::Task && u.status != WorkUnitStatus::Done)
        .count() as u64;
    let per_child = task_core::tree::CHILD_LEAF_ESTIMATE;
    Ok(PlanApprovalFacts {
        open_decisions,
        review_human_stages,
        stages: spec.stages.len(),
        max_units_in_stage,
        child_task_units,
        estimated_leaves: u64::from(counters.leaves)
            .saturating_add(held_leaves)
            .saturating_add(open_children.saturating_mul(per_child)),
        estimated_runs: u64::from(counters.runs)
            .saturating_add(remaining_leaves)
            .saturating_add(open_children.saturating_mul(per_child + 1)),
    })
}

/// D8: 承認を挟まずに進めるときの報告（ADR-0034 の報告の流れ）の見出しと本文（決定的）。
pub fn plan_notice(spec: &task_core::ExecutionPlanSpec) -> (String, String) {
    let stages: Vec<String> = spec
        .stages
        .iter()
        .map(|s| {
            if s.title.trim().is_empty() {
                s.key.clone()
            } else {
                s.title.clone()
            }
        })
        .collect();
    let headline = format!("計画を採用して進めます: {}", stages.join(" → "));
    let mut body = String::from("承認の要らない root の計画（ADR-0079 D8）を採用しました。\n");
    for s in &spec.stages {
        body.push_str(&format!("\n段階 {}（{}）\n", s.key, s.title));
        for u in spec.units.iter().filter(|u| u.stage == s.key) {
            let kind = if u.kind == WorkUnitKind::Task {
                "子 task"
            } else {
                "leaf"
            };
            body.push_str(&format!("- {}: {}（{kind}）\n", u.key, u.title));
        }
    }
    (headline, body)
}

/// D8: 承認の理由（`PlanApprovalRequested.reasons`）の人が読む 1 行。
pub fn describe_reasons(reasons: &[String]) -> String {
    reasons
        .iter()
        .map(|r| {
            if let Some(keys) = r.strip_prefix("decisions:") {
                format!("決定を含む（{keys}）")
            } else if let Some(stage) = r.strip_prefix("review_human:") {
                format!("段階 {stage} の後に人の確認")
            } else if let Some(rest) = r.strip_prefix("near_limit:") {
                format!("上限に近い（{rest}）")
            } else {
                r.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("・")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_accepts_decision_as_an_alias_of_action() {
        let a: PlanGateRequest = serde_json::from_str(r#"{"action":"approve"}"#).expect("action");
        assert_eq!(a.action, PlanGateAction::Approve);
        let b: PlanGateRequest =
            serde_json::from_str(r#"{"decision":"replan","note":"n"}"#).expect("decision");
        assert_eq!(b.action, PlanGateAction::Replan);
        assert!(serde_json::from_str::<PlanGateRequest>(r#"{"action":"continue"}"#).is_err());
        assert!(serde_json::from_str::<PlanGateRequest>(r#"{"action":"approve","x":1}"#).is_err());
    }

    #[test]
    fn reasons_are_described_in_one_line() {
        assert_eq!(
            describe_reasons(&[
                "decisions:h1,h2".to_string(),
                "review_human:phase-2".to_string(),
                "near_limit:max_stages:4/5".to_string()
            ]),
            "決定を含む（h1,h2）・段階 phase-2 の後に人の確認・上限に近い（max_stages:4/5）"
        );
    }
}
