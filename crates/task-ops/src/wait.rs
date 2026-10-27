//! ADR 2026-10-11-task-wait-reasons D3〜D5: 一覧・詳細・木の応答に載せる task の待ちの理由（`waits`）を組む。
//!
//! DB の理由は task-core の `db_waits`（`TaskStore::db_waits_for`）と `plan_waits`、人の gate（D12）は events を
//! 読む [`human_gate_waits`]、dispatcher のメモリの理由は `DaemonSnapshot.task_waits`（[`MemoryWaits`]）から合わせる。
//! どれも決定的（LLM なし）で、dispatcher と同じ関数を呼ぶ。画面用の別の推測は作らない。

use std::collections::HashMap;

use task_core::{
    ChildFacts, DecisionKind, DecisionStatus, Event, ExecutionPlanRow, HumanGateKind, StageReview,
    Status, Task, TaskId, TaskStore, TaskWait, WorkUnitRow,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::OpsError;
use crate::daemon::DaemonSnapshot;

/// dispatcher のメモリの見送り理由（task ごとに 1 件。`DaemonSnapshot.task_waits` の写し）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryWaits(HashMap<TaskId, TaskWait>);

impl MemoryWaits {
    /// D4: snapshot が古すぎない（`last_tick_at` から `tick_ms × 10` 以内）ときだけ使う。古ければ空（推測しない）。
    pub fn from_snapshot(snapshot: Option<&DaemonSnapshot>, now: OffsetDateTime) -> Self {
        let Some(s) = snapshot else {
            return Self::default();
        };
        let Ok(last) = OffsetDateTime::parse(&s.last_tick_at, &Rfc3339) else {
            return Self::default();
        };
        let window =
            time::Duration::milliseconds(s.tick_ms.saturating_mul(10).min(i64::MAX as u64) as i64);
        if now - last > window {
            return Self::default();
        }
        Self(
            s.task_waits
                .iter()
                .map(|w| (w.task_id, w.wait.clone()))
                .collect(),
        )
    }

    /// dispatcher が自分のメモリの理由をそのまま渡す（ADR 2026-10-27-project-stagnation-inbox D1 の `task_unscheduled`）。
    pub fn from_waits(waits: impl IntoIterator<Item = (TaskId, TaskWait)>) -> Self {
        Self(waits.into_iter().collect())
    }

    pub fn get(&self, id: TaskId) -> Option<&TaskWait> {
        self.0.get(&id)
    }
}

/// `ready && waits が空`（D5。web は再計算しない）。
pub fn is_runnable(task: &Task, waits: &[TaskWait]) -> bool {
    task.status == Status::Ready && waits.is_empty()
}

/// D12: 人の gate（計画の承認・段の `review: human`・途中確認）。dispatcher の `human_gate_hold` もこの結果が
/// 空でないかで unit を止める（判定は 1 か所）。`plan` はその task の有効な計画、`events` はその task の events。
/// - 有効な計画の版が PlanGate を通っていない（`PlanGateState::Pending`）か、計画の承認待ちで止まっている
///   → `PlanApproval`。
/// - 段の後で止まっている（`phase_gate::is_awaiting_human`）→ 直近の `PhaseReported.phase` が /3 の計画の段で
///   `review: human` なら `StageReview{stage: 段 key}`、そうでなければ（/2 の `pause_after` など）
///   `PhaseCheckpoint{stage: phase}`。どちらも既存の事実だけで決め、推測しない。
pub fn human_gate_waits(
    task: &Task,
    plan: Option<&ExecutionPlanRow>,
    events: &[(u64, Event)],
) -> Vec<TaskWait> {
    let plan_pending =
        plan.is_some_and(|p| !crate::plan_gate::plan_gate_state(p, events).allows_dispatch());
    if plan_pending || crate::plan_gate::is_awaiting_plan_approval(task, events) {
        return vec![TaskWait::HumanGate {
            gate: HumanGateKind::PlanApproval,
            stage: None,
        }];
    }
    if !crate::phase_gate::is_awaiting_human(task, events) {
        return Vec::new();
    }
    let phase = events.iter().rev().find_map(|(_, e)| match e {
        Event::PhaseReported { report, .. } => Some(report.phase.clone()),
        _ => None,
    });
    let stage_review = phase.as_deref().is_some_and(|key| {
        plan.is_some_and(|p| {
            p.spec.schema == task_core::EXECUTION_PLAN_SCHEMA_V3
                && p.spec
                    .stages
                    .iter()
                    .any(|s| s.key == key && s.review == StageReview::Human)
        })
    });
    let gate = if stage_review {
        HumanGateKind::StageReview
    } else {
        HumanGateKind::PhaseCheckpoint
    };
    vec![TaskWait::HumanGate { gate, stage: phase }]
}

/// D11: 節点の未回答の `needed_before: self` の決定の key（dispatcher の `decision_self_hold` と同じ条件）。
fn open_self_decision_keys(rows: &[task_core::DecisionRow], task_id: TaskId) -> Vec<String> {
    rows.iter()
        .filter(|d| {
            d.task_id == task_id
                && d.status == DecisionStatus::Open
                && d.kind != DecisionKind::Limit
                && d.needed_before
                    .iter()
                    .any(|n| n == task_core::decision::NEEDED_BEFORE_SELF)
        })
        .map(|d| d.key.clone())
        .collect()
}

/// 計画の子 task の事実（`child_task_id` と `child-<key>` のラベルを持つ子）。
fn child_facts(
    store: &dyn TaskStore,
    task: &Task,
    units: &[WorkUnitRow],
    children_by_parent: &HashMap<TaskId, Vec<&Task>>,
) -> Result<Vec<ChildFacts>, OpsError> {
    let mut out: Vec<ChildFacts> = children_by_parent
        .get(&task.id)
        .map(|v| {
            v.iter()
                .map(|c| ChildFacts {
                    task_id: c.id,
                    status: c.status,
                    labels: c.labels.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    for u in units {
        let Some(id) = u
            .child_task_id
            .as_deref()
            .and_then(|s| s.parse::<TaskId>().ok())
        else {
            continue;
        };
        if out.iter().any(|c| c.task_id == id) {
            continue;
        }
        if let Some(c) = store.get(id)? {
            out.push(ChildFacts {
                task_id: c.id,
                status: c.status,
                labels: c.labels.clone(),
            });
        }
    }
    Ok(out)
}

/// 計画の待ち（D7〜D11・D13）。`units` はその task の `work_units_for`。
fn plan_waits_of(
    store: &dyn TaskStore,
    task: &Task,
    units: &[WorkUnitRow],
    children_by_parent: &HashMap<TaskId, Vec<&Task>>,
    decisions: &mut HashMap<TaskId, Vec<task_core::DecisionRow>>,
) -> Result<Vec<TaskWait>, OpsError> {
    let root = task_core::tree::root_id_of(task);
    if let std::collections::hash_map::Entry::Vacant(e) = decisions.entry(root) {
        e.insert(store.decisions_list(Some(root))?);
    }
    let keys = decisions
        .get(&root)
        .map(|rows| open_self_decision_keys(rows, task.id))
        .unwrap_or_default();
    if units.is_empty() && keys.is_empty() {
        return Ok(Vec::new());
    }
    let children = child_facts(store, task, units, children_by_parent)?;
    Ok(task_core::plan_waits(units, &children, &keys))
}

/// D3/D5: task ごとの `waits`（DB → 計画 → 人の gate → メモリの順。終端の task は空）。
/// `all_tasks` は子の事実を引くための全件（一覧で既に読んだ `list(None)`）。
pub fn task_waits(
    store: &dyn TaskStore,
    tasks: &[Task],
    all_tasks: &[Task],
    memory: &MemoryWaits,
) -> Result<HashMap<TaskId, Vec<TaskWait>>, OpsError> {
    let live: Vec<Task> = tasks
        .iter()
        .filter(|t| !t.status.is_terminal())
        .cloned()
        .collect();
    let mut out: HashMap<TaskId, Vec<TaskWait>> =
        tasks.iter().map(|t| (t.id, Vec::new())).collect();
    if live.is_empty() {
        return Ok(out);
    }
    let mut db = store.db_waits_for(&live)?;
    let mut children_by_parent: HashMap<TaskId, Vec<&Task>> = HashMap::new();
    for t in all_tasks {
        if let Some(p) = task_core::wait::parent_of(t) {
            children_by_parent.entry(p).or_default().push(t);
        }
    }
    let mut decisions = HashMap::new();
    for task in &live {
        let mut waits = db.remove(&task.id).unwrap_or_default();
        let units = store.work_units_for(task.id)?;
        waits.extend(plan_waits_of(
            store,
            task,
            &units,
            &children_by_parent,
            &mut decisions,
        )?);
        let plan = store.execution_plan_active(task.id)?;
        waits.extend(human_gate_waits(
            task,
            plan.as_ref(),
            &store.events_for(task.id)?,
        ));
        if let Some(w) = memory.get(task.id)
            && !waits.contains(w)
        {
            waits.push(w.clone());
        }
        out.insert(task.id, waits);
    }
    Ok(out)
}

/// 1 件分（詳細・木の節点）。
pub fn task_waits_one(
    store: &dyn TaskStore,
    task: &Task,
    memory: &MemoryWaits,
) -> Result<Vec<TaskWait>, OpsError> {
    if task.status.is_terminal() {
        return Ok(Vec::new());
    }
    let children: Vec<Task> = store
        .list_page(
            &task_core::ListFilter {
                parent_id: Some(task.id),
                ..Default::default()
            },
            task_core::ListOrder::CreatedDesc,
            None,
            10_000,
        )?
        .items;
    let mut all = children;
    all.push(task.clone());
    Ok(task_waits(store, std::slice::from_ref(task), &all, memory)?
        .remove(&task.id)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests;
