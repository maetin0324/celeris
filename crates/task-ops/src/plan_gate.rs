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
    // ADR-0079 付記「R6-1」D5: 子を作る unit だけを数える（`adopt` の unit と done の unit は除く。検証の
    // `max_child_tasks_per_plan` と同じ `creates_child`。R5b-prep 付記 8.）。以前は `kind == Task` をすべて数え、
    // BenchFS の v2 で `near_limit:max_child_tasks_per_plan:10/6`（採用 6 を含む）と出た。
    let done_keys: std::collections::BTreeSet<&str> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done)
        .map(|u| u.key.as_str())
        .collect();
    let child_task_units = spec
        .units
        .iter()
        .filter(|u| u.creates_child() && !done_keys.contains(u.key.as_str()))
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

/// D8（Phase R3b、R5b-prep で task-dispatch から移した）: 承認を挟まずに進める root の計画の報告（`kind: progress`、
/// bad_news でないので複製も Discord も無い。U-R3）を 1 件だけ残す。報告するノードは担当（組織にあるとき）、無ければ
/// 秘書。組織が無い DB では書かない（`None`）。planner の計画（dispatcher）と人の計画（`PUT`）が同じ関数を使う。
pub fn record_plan_notice(
    store: &dyn TaskStore,
    task: &Task,
    headline: &str,
    body: &str,
    now: time::OffsetDateTime,
) -> Result<Option<task_core::report::Report>, task_core::StoreError> {
    let org = store.org_list()?;
    let node = task
        .assignee
        .as_deref()
        .and_then(|a| org.iter().find(|n| n.id == a))
        .or_else(|| org.iter().find(|n| n.kind == task_core::OrgKind::Secretary));
    let Some(node) = node else {
        return Ok(None);
    };
    let level = task_core::report::level_of(&org, &node.id);
    let report = task_core::report::report_for_plan_notice(
        &node.id,
        level,
        task.project_id,
        task.id,
        headline,
        body,
        now,
    );
    store.report_append(&report)?;
    Ok(Some(report))
}

/// ADR-0079 D8 / 付記「R5b-prep」: 人が書いた root の計画（origin human、`PUT /tasks/{id}/execution-plan`）の報告の
/// 見出しと本文。人の計画は、書いた人がその場で承認したものとして `PlanGate` を挟まない。承認が要る形
/// （決定・`review: human`・上限に近い）だったなら、その理由を本文に残す（決定的）。
pub fn human_plan_notice(
    spec: &task_core::ExecutionPlanSpec,
    by: &str,
    reasons: &[String],
) -> (String, String) {
    let (headline, body) = plan_notice(spec);
    let rest = body.split_once('\n').map(|(_, r)| r).unwrap_or("");
    let mut head = format!(
        "人（{by}）が書いた root の計画（origin human）を採用しました。書いた人の承認とみなし、計画の承認（ADR-0079 D8）は挟みません。\n"
    );
    if !reasons.is_empty() {
        head.push_str(&format!(
            "planner の計画なら承認を求める条件: {}\n",
            describe_reasons(reasons)
        ));
    }
    (headline, format!("{head}{rest}"))
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

/// ADR-0079 付記「R6-1」D1: 有効な計画の版の承認の状態（PlanGate を通ったか）。events から決定的に導く
/// （列を足さない。migration なし）。dispatcher はこの値が [`PlanGateState::Pending`] の版の unit を起こさない
/// （leaf の run・子 task の生成・段階の統合のどれも）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanGateState {
    /// `PlanApprovalRequested{plan_id}` があり、その後に `plan_approved` の遷移が無い（承認待ち、または承認待ちの
    /// まま人が `replan` を求めた = 次の版が採用され承認されるまで、この版の unit は止めたまま）。
    Pending,
    /// PlanGate を通った（`plan_approved`）か、承認の要らない planner の計画（理由が無く `PlanApprovalRequested`
    /// を積まなかった版。子の計画・/1・/2・木が無効な task も同じ）。
    Approved,
    /// 人が書いた計画（origin human。`PUT /tasks/{id}/execution-plan`）。書いた人の承認とみなす（R5b-prep 付記 3.）。
    Skipped,
}

impl PlanGateState {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanGateState::Pending => "pending",
            PlanGateState::Approved => "approved",
            PlanGateState::Skipped => "skipped",
        }
    }

    /// この版の unit を起こしてよいか。
    pub fn allows_dispatch(self) -> bool {
        !matches!(self, PlanGateState::Pending)
    }
}

/// ADR-0079 付記「R6-1」D1: `plan`（ふつうは有効な版）の承認の状態。`events` はその task の events（seq 昇順）。
pub fn plan_gate_state(plan: &ExecutionPlanRow, events: &[(u64, Event)]) -> PlanGateState {
    if plan.origin == task_core::PlanOrigin::Human {
        return PlanGateState::Skipped;
    }
    let requested_at = events.iter().rev().find_map(|(seq, e)| match e {
        Event::PlanApprovalRequested { plan_id, .. } if *plan_id == plan.id => Some(*seq),
        _ => None,
    });
    let Some(requested_at) = requested_at else {
        return PlanGateState::Approved;
    };
    let approved = events.iter().any(|(seq, e)| {
        *seq > requested_at
            && matches!(e, Event::Transitioned { reason, .. }
                if reason == PhaseResumeMode::PlanApprove.name())
    });
    if approved {
        PlanGateState::Approved
    } else {
        PlanGateState::Pending
    }
}

/// ADR-0079 付記「R6-1」D3: replan の上限を使い切った後の失敗を人に聞く質問（`WorkerQuestion`）の頭。回答は
/// 人の replan の依頼として扱う（[`answered_replan_exhausted`]。上限に数えない）。
pub const REPLAN_EXHAUSTED_QUESTION_PREFIX: &str = "replan の上限を使い切りました";

fn is_exhausted_question(e: &Event) -> Option<bool> {
    match e {
        Event::QuestionRaised { text, .. } => {
            Some(text.starts_with(REPLAN_EXHAUSTED_QUESTION_PREFIX))
        }
        Event::WorkerFinished { outcome, role, .. }
            if !crate::derive::is_reviewer(*role) && outcome.starts_with("question: ") =>
        {
            Some(
                outcome
                    .trim_start_matches("question: ")
                    .starts_with(REPLAN_EXHAUSTED_QUESTION_PREFIX),
            )
        }
        _ => None,
    }
}

/// ADR-0079 付記「R6-1」D3: 直前の遷移が人の回答（`answer`）で、その回答が
/// [`REPLAN_EXHAUSTED_QUESTION_PREFIX`] の質問へのもの（質問の後に run も計画の採用も無い）か。
pub fn answered_replan_exhausted(events: &[(u64, Event)]) -> bool {
    let mut iter = events.iter().rev();
    // 直前の遷移が `answer`。
    loop {
        match iter.next() {
            Some((_, Event::Transitioned { reason, .. })) => {
                if reason != "answer" {
                    return false;
                }
                break;
            }
            // 回答の後に計画が採用された（回答を受けた replan は済んだ）。
            Some((_, Event::ExecutionPlanned { .. })) => return false,
            Some(_) => continue,
            None => return false,
        }
    }
    for (_, e) in iter {
        if let Some(found) = is_exhausted_question(e) {
            return found;
        }
        if matches!(
            e,
            Event::ExecutionPlanned { .. }
                | Event::Transitioned {
                    to: Status::Running,
                    ..
                }
        ) {
            return false;
        }
    }
    false
}

/// ADR-0079 付記「R6-1」D3: `[execution] max_replans` に数える replan の数（events から）。2 版目以降の
/// `ExecutionPlanned` のうち、人が起こしたものを**数えない**:
/// - origin human の版（`PUT /tasks/{id}/execution-plan` の人の replan）。
/// - 人の依頼の後に planner が書いた版: `ExecutionHintSet{replan: true}`（decompose・計画の承認の replan・決定への
///   replan の回答）、途中確認の `phase_replan`・計画の承認の `plan_replan` の遷移、replan の上限を使い切った後の
///   質問への回答（[`answered_replan_exhausted`] と同じ形）の後に、次に採用された版。
///
/// 以前は `execution_plan_list().len() - 1`（版の数 − 1）で、人の replan も上限に数え、上限を使い切った後の人の
/// replan の依頼は黙って捨てられていた（R6-1 D3）。
pub fn counted_replans(events: &[(u64, Event)]) -> u32 {
    let mut counted = 0u32;
    let mut human_requested = false;
    let mut exhausted_question_open = false;
    for (_, e) in events {
        if let Some(found) = is_exhausted_question(e) {
            exhausted_question_open = found;
            continue;
        }
        match e {
            Event::ExecutionHintSet { replan: true, .. } => human_requested = true,
            Event::Transitioned { reason, .. } => {
                if reason == PhaseResumeMode::PlanReplan.name()
                    || reason == PhaseResumeMode::Replan.name()
                    || (reason == "answer" && exhausted_question_open)
                {
                    human_requested = true;
                }
                if reason == "answer" {
                    exhausted_question_open = false;
                }
            }
            Event::ExecutionPlanned {
                version, origin, ..
            } => {
                if *version > 1 && *origin != task_core::PlanOrigin::Human && !human_requested {
                    counted += 1;
                }
                human_requested = false;
            }
            _ => {}
        }
    }
    counted
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

    fn empty_spec() -> task_core::ExecutionPlanSpec {
        task_core::ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA_V3.to_string(),
            rationale: String::new(),
            work_units: Vec::new(),
            phases: Vec::new(),
            children: Vec::new(),
        }
    }

    fn plan_row(id: &str, version: u32, origin: task_core::PlanOrigin) -> ExecutionPlanRow {
        ExecutionPlanRow {
            id: id.to_string(),
            task_id: TaskId::new().to_string(),
            version,
            origin,
            planner_run_id: None,
            status: task_core::PlanStatus::Active,
            spec: empty_spec(),
            created_at: "2026-09-30T00:00:00Z".to_string(),
            superseded_at: None,
        }
    }

    fn planned(id: &str, version: u32, origin: task_core::PlanOrigin) -> Event {
        Event::ExecutionPlanned {
            plan_id: id.to_string(),
            version,
            origin,
            supersedes: None,
            reason: None,
            plan: Box::new(empty_spec()),
        }
    }

    fn transitioned(from: Status, to: Status, reason: &str) -> Event {
        Event::Transitioned {
            from,
            to,
            reason: reason.to_string(),
        }
    }

    fn seq(events: Vec<Event>) -> Vec<(u64, Event)> {
        events
            .into_iter()
            .enumerate()
            .map(|(i, e)| (i as u64, e))
            .collect()
    }

    fn hint_replan() -> Event {
        Event::ExecutionHintSet {
            mode: ExecutionMode::Compound,
            previous: None,
            previous_decision: None,
            source: "human (plan-gate)".to_string(),
            note: Some("n".to_string()),
            replan: true,
        }
    }

    /// ADR-0079 付記「R6-1」D1: 承認の状態は events から決まる。人の計画は skipped、`PlanApprovalRequested` の無い
    /// planner の計画は approved、要求の後に `plan_approved` が無ければ pending（承認待ちのまま人が replan を求めても
    /// pending のまま）、`plan_approved` の後は approved。別の版の要求は関係しない。
    #[test]
    fn plan_gate_state_is_derived_from_events() {
        use task_core::PlanOrigin;
        let v1 = plan_row("p1", 1, PlanOrigin::Planner);
        assert_eq!(
            plan_gate_state(&plan_row("h", 1, PlanOrigin::Human), &[]),
            PlanGateState::Skipped
        );
        assert_eq!(
            plan_gate_state(&v1, &seq(vec![planned("p1", 1, PlanOrigin::Planner)])),
            PlanGateState::Approved
        );
        let requested = vec![
            planned("p1", 1, PlanOrigin::Planner),
            transitioned(Status::Running, Status::Blocked, AWAITING_PLAN_APPROVAL),
            Event::PlanApprovalRequested {
                plan_id: "p1".to_string(),
                reasons: vec!["review_human:s1".to_string()],
            },
        ];
        assert_eq!(
            plan_gate_state(&v1, &seq(requested.clone())),
            PlanGateState::Pending
        );
        let mut replan = requested.clone();
        replan.push(transitioned(
            Status::Blocked,
            Status::Ready,
            PhaseResumeMode::PlanReplan.name(),
        ));
        replan.push(hint_replan());
        assert_eq!(plan_gate_state(&v1, &seq(replan)), PlanGateState::Pending);
        assert!(!PlanGateState::Pending.allows_dispatch());
        let mut approved = requested;
        approved.push(transitioned(
            Status::Blocked,
            Status::Ready,
            PhaseResumeMode::PlanApprove.name(),
        ));
        assert_eq!(
            plan_gate_state(&v1, &seq(approved.clone())),
            PlanGateState::Approved
        );
        // v2 の要求は v1 の状態を変えない。v2 は自分の要求で pending。
        approved.push(planned("p2", 2, PlanOrigin::Planner));
        approved.push(Event::PlanApprovalRequested {
            plan_id: "p2".to_string(),
            reasons: vec![],
        });
        let events = seq(approved);
        assert_eq!(plan_gate_state(&v1, &events), PlanGateState::Approved);
        assert_eq!(
            plan_gate_state(&plan_row("p2", 2, PlanOrigin::Planner), &events),
            PlanGateState::Pending
        );
    }

    /// ADR-0079 付記「R6-1」D3: `max_replans` に数えるのは、人が起こしていない planner / repair の 2 版目以降だけ。
    #[test]
    fn human_origin_replans_are_not_counted() {
        use task_core::PlanOrigin;
        let events = seq(vec![
            planned("p1", 1, PlanOrigin::Planner),
            // daemon の replan（WU の失敗）: 数える。
            transitioned(Status::Running, Status::Ready, "replan"),
            planned("p2", 2, PlanOrigin::Planner),
            // 人の PUT の replan: 数えない。
            planned("p3", 3, PlanOrigin::Human),
            // 計画の承認の replan: 数えない。
            transitioned(
                Status::Blocked,
                Status::Ready,
                PhaseResumeMode::PlanReplan.name(),
            ),
            hint_replan(),
            transitioned(Status::Ready, Status::Running, "dispatch"),
            planned("p4", 4, PlanOrigin::Planner),
            // 途中確認の replan: 数えない。
            transitioned(
                Status::Blocked,
                Status::Ready,
                PhaseResumeMode::Replan.name(),
            ),
            planned("p5", 5, PlanOrigin::Planner),
            // 上限を使い切った後の質問への回答: 数えない。
            Event::QuestionRaised {
                run_id: "r".to_string(),
                text: format!("{REPLAN_EXHAUSTED_QUESTION_PREFIX}（1/1 回）: work unit b failed"),
            },
            transitioned(Status::Running, Status::Blocked, "worker_question"),
            transitioned(Status::Blocked, Status::Ready, "answer"),
        ]);
        assert!(answered_replan_exhausted(&events));
        let mut events = events;
        events.push((100, planned("p6", 6, PlanOrigin::Planner)));
        assert!(!answered_replan_exhausted(&events));
        // 普通の質問への回答の後の planner の版は数える。
        events.push((
            101,
            Event::QuestionRaised {
                run_id: "r".to_string(),
                text: "どちらの API を使いますか".to_string(),
            },
        ));
        events.push((102, transitioned(Status::Blocked, Status::Ready, "answer")));
        assert!(!answered_replan_exhausted(&events));
        events.push((103, transitioned(Status::Running, Status::Ready, "replan")));
        events.push((104, planned("p7", 7, PlanOrigin::Planner)));
        assert_eq!(counted_replans(&events), 2, "p2 and p7 only");
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
