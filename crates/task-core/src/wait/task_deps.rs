//! ADR 2026-11-11-unit-task-dependencies D2・D3・D5・D6: 計画の unit の `task:<task id>` 依存が満たされたかの判断（純粋関数）。
//!
//! 事実（依存先の状態と M10 の取り込みの cause）を読むのは `task_ops::unit_task_deps`。満たされた依存は dispatcher が
//! `newly_ready_with` の `external_done` に足して unit を上げ、待ちは `task_ops::wait` が M11 の理由にする。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{IntegrationWaitCause, TaskWait, dependency_done};
use crate::execution_plan::{
    WorkUnitRow, WorkUnitStatus, task_dependency_key, unit_task_dependencies,
};
use crate::model::{Status, TaskId};

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
    /// D5: まだ満たされない最初の依存先（task_id 順）を M11 の待ちの理由にする。
    pub fn first_wait(&self) -> Option<TaskWait> {
        self.waits.first().map(|w| TaskWait::TaskDependency {
            task_id: w.task_id,
            cause: w.cause,
            detail: w.detail.clone(),
            unit_keys: w.unit_keys.clone(),
        })
    }

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
