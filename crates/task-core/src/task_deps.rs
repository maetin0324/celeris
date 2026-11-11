//! ADR 2026-11-11-unit-task-dependencies: /3 の計画の unit の `task:<task id>` 依存（木の外の task の完了・取り込みを待つ）。
//!
//! 書き方の検証（D4 の [`task_dependency_errors`]）と、満たされたかの判断（D2・D3・D5 の [`resolve_task_dependencies`]）の
//! 純粋関数だけを置く（I/O・LLM なし）。事実（依存先の状態・M10 の取り込みの cause・木・待つ辺）を読むのは
//! `task_ops::unit_task_deps`、unit を `pending → ready` に上げるのは dispatcher の `resolve_child_dependencies`。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution_plan::{
    EXECUTION_PLAN_SCHEMA_V3, ExecutionPlanSpec, PlanValidationError, WorkUnitRow, WorkUnitStatus,
};
use crate::model::{Status, TaskId};
use crate::org::ProjectId;
use crate::wait::{IntegrationWaitCause, dependency_done};

/// D1: unit の `depends_on` で木の外の task を指す接頭辞（`task:<task id>`）。
pub const TASK_DEP_PREFIX: &str = "task:";

/// `task:` で始まる依存か（id の形は問わない。形は [`task_dependency_id`]）。
pub fn is_task_dependency(dep: &str) -> bool {
    dep.starts_with(TASK_DEP_PREFIX)
}

/// `task:<id>` の id。接頭辞が無い・id が ULID でなければ `None`。
pub fn task_dependency_id(dep: &str) -> Option<TaskId> {
    dep.strip_prefix(TASK_DEP_PREFIX)?.parse().ok()
}

/// `id` を指す依存の文字列（`task:<id>`。unit の `depends_on` と解決の `met` の要素）。
pub fn task_dependency_key(id: TaskId) -> String {
    format!("{TASK_DEP_PREFIX}{id}")
}

/// /3 の計画の unit が書いた `task:` 依存（unit key, task id）。形の悪いものは飛ばす（形は `validate_v3` が拒む）。
pub fn plan_task_dependencies(spec: &ExecutionPlanSpec) -> Vec<(String, TaskId)> {
    if spec.schema != EXECUTION_PLAN_SCHEMA_V3 {
        return Vec::new();
    }
    spec.units
        .iter()
        .flat_map(|u| {
            u.depends_on
                .iter()
                .filter_map(|d| task_dependency_id(d))
                .map(|id| (u.key.clone(), id))
        })
        .collect()
}

/// unit の行が待つ `task:` 依存の id（形の悪いものは飛ばす）。
pub fn unit_task_dependencies(unit: &WorkUnitRow) -> impl Iterator<Item = TaskId> + '_ {
    unit.depends_on.iter().filter_map(|d| task_dependency_id(d))
}

/// D4 の検証に使う DB の事実（`task_ops::unit_task_deps::plan_errors` が集める）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskDependencyContext {
    /// 計画を持つ task の案件。
    pub project: Option<ProjectId>,
    /// 計画を持つ task の木（root とその子孫。自分・祖先・子孫を含む）。
    pub own_tree: BTreeSet<TaskId>,
    /// 見つかった依存先 → その案件。見つからない id は入らない。
    pub targets: BTreeMap<TaskId, Option<ProjectId>>,
    /// 依存先から辿れる「待つ」辺（task → その task が待つ task。`depends_on` と有効な計画の unit の `task:` 依存）。
    pub waits_on: BTreeMap<TaskId, Vec<TaskId>>,
}

/// D4: 計画の `task:` 依存の検証（存在・自分の木の外・同じ案件・循環なし）。依存ごとに最初に当たった理由を 1 件返す。
pub fn task_dependency_errors(
    spec: &ExecutionPlanSpec,
    ctx: &TaskDependencyContext,
) -> Vec<PlanValidationError> {
    let mut errors = Vec::new();
    for (key, task_id) in plan_task_dependencies(spec) {
        let Some(project) = ctx.targets.get(&task_id) else {
            errors.push(PlanValidationError::TaskDependencyNotFound { key, task_id });
            continue;
        };
        if ctx.own_tree.contains(&task_id) {
            errors.push(PlanValidationError::TaskDependencyInOwnTree { key, task_id });
            continue;
        }
        if ctx.project.is_none() || *project != ctx.project {
            errors.push(PlanValidationError::TaskDependencyOtherProject { key, task_id });
            continue;
        }
        if reaches(&ctx.waits_on, task_id, &ctx.own_tree) {
            errors.push(PlanValidationError::TaskDependencyCycle { key, task_id });
        }
    }
    errors
}

/// `start` から待つ辺を辿って `targets` のどれかに着くか（訪問済みは辿らない）。
fn reaches(
    edges: &BTreeMap<TaskId, Vec<TaskId>>,
    start: TaskId,
    targets: &BTreeSet<TaskId>,
) -> bool {
    let mut seen = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        for next in edges.get(&id).into_iter().flatten() {
            if targets.contains(next) {
                return true;
            }
            if seen.insert(*next) {
                queue.push_back(*next);
            }
        }
    }
    false
}

/// D5: unit の `task:` 依存がまだ満たされない理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskDependencyCause {
    /// 依存先が done でない（走っている・待っている・review 中など）。
    Unfinished,
    /// 依存先は done だが、成果が既定 branch に入っていない（M10 の `unmerged`）。
    Unmerged,
    /// delivery が取り込みを見送った（M10 の `delivery_skipped`）。
    DeliverySkipped,
    /// 人が取り込みを保留にした（M10 の `held`）。
    Held,
    /// 人が取り込まないと決めた（M10 の `discarded`）。
    Discarded,
    /// 依存先が failed。
    Failed,
    /// 依存先が cancelled。
    Cancelled,
}

impl TaskDependencyCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unfinished => "unfinished",
            Self::Unmerged => "unmerged",
            Self::DeliverySkipped => "delivery_skipped",
            Self::Held => "held",
            Self::Discarded => "discarded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// D3: 依存先がもう成果を出さない（人が依存先をやり直すか計画を作り直すまで解けない）。木の生存確認が理由なしに分け、
    /// 既存の `StallDetected`・受信箱の `stalled` で人に知らせる。
    pub fn is_ended(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled | Self::Discarded)
    }
}

impl From<IntegrationWaitCause> for TaskDependencyCause {
    fn from(cause: IntegrationWaitCause) -> Self {
        match cause {
            IntegrationWaitCause::Unmerged => Self::Unmerged,
            IntegrationWaitCause::DeliverySkipped => Self::DeliverySkipped,
            IntegrationWaitCause::Held => Self::Held,
            IntegrationWaitCause::Discarded => Self::Discarded,
        }
    }
}

/// D2: 依存先 1 件の事実。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependencyFact {
    pub task_id: TaskId,
    pub status: Status,
    /// 依存先が done で、その成果がまだ既定 branch に入っていない理由（M10 の
    /// `task_ops::dep_integration::dependency_integration` の結果。done でない・対象外・取り込み済みなら `None`）。
    pub integration: Option<(IntegrationWaitCause, String)>,
}

/// D5: まだ満たされない依存先 1 件と、それを待つ pending の unit。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependencyWait {
    pub task_id: TaskId,
    pub cause: TaskDependencyCause,
    pub detail: String,
    /// 待つ pending の unit の key（seq 順）。
    pub unit_keys: Vec<String>,
}

/// [`resolve_task_dependencies`] の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskDependencyResolution {
    /// 満たされた依存（`task:<id>`。`newly_ready_with` の `external_done` に足す）。
    pub met: BTreeSet<String>,
    /// まだ満たされない依存先（task_id 順）。
    pub waits: Vec<TaskDependencyWait>,
}

impl TaskDependencyResolution {
    /// unit `key` が待つ依存先の理由（D3 の終わった cause を先に。無ければ `None`）。
    pub fn cause_of(&self, key: &str) -> Option<TaskDependencyCause> {
        let mut causes = self
            .waits
            .iter()
            .filter(|w| w.unit_keys.iter().any(|k| k == key))
            .map(|w| w.cause);
        let first = causes.next()?;
        Some(causes.fold(first, |acc, c| if acc.is_ended() { acc } else { c }))
    }
}

/// D2・D3・D6: pending の unit の `task:` 依存を事実で解く。`ready` 以降の unit は見ない（始まった unit は止めない）。
/// 満たされる条件は task 全体の `depends_on` と同じ: 依存先が done（`wait::dependency_done`。`db_waits` D5 と同じ関数）で、
/// M10 の取り込みの理由（`integration`）が無いこと。事実の無い依存先は `unfinished` として待つ。
pub fn resolve_task_dependencies(
    units: &[WorkUnitRow],
    facts: &[TaskDependencyFact],
) -> TaskDependencyResolution {
    let mut pending: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .collect();
    pending.sort_by_key(|u| u.seq);
    let mut waiting: BTreeMap<TaskId, Vec<String>> = BTreeMap::new();
    for u in pending {
        for id in unit_task_dependencies(u) {
            let keys = waiting.entry(id).or_default();
            if !keys.contains(&u.key) {
                keys.push(u.key.clone());
            }
        }
    }
    let mut out = TaskDependencyResolution::default();
    for (task_id, unit_keys) in waiting {
        let state = match facts.iter().find(|f| f.task_id == task_id) {
            None => Some((
                TaskDependencyCause::Unfinished,
                format!("依存先 {task_id} が見つからない"),
            )),
            Some(f) => fact_state(f),
        };
        match state {
            None => {
                out.met.insert(task_dependency_key(task_id));
            }
            Some((cause, detail)) => out.waits.push(TaskDependencyWait {
                task_id,
                cause,
                detail,
                unit_keys,
            }),
        }
    }
    out
}

fn fact_state(f: &TaskDependencyFact) -> Option<(TaskDependencyCause, String)> {
    if !dependency_done(Some(f.status), false) {
        let cause = match f.status {
            Status::Failed => TaskDependencyCause::Failed,
            Status::Cancelled => TaskDependencyCause::Cancelled,
            _ => TaskDependencyCause::Unfinished,
        };
        let status = serde_json::to_value(f.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        return Some((cause, format!("依存先 {} は {status}", f.task_id)));
    }
    f.integration
        .as_ref()
        .map(|(cause, detail)| ((*cause).into(), detail.clone()))
}

#[cfg(test)]
mod tests;
