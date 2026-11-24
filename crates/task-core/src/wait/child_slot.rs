//! ADR 2026-11-24-paused-children-free-child-slots: 同時の子 task の枠（`max_parallel_child_tasks`。ADR-0079 D3）。
//!
//! 枠を占める子の規則（D1）と子 task の作成の候補（D1）、枠の待ち（D3）をここ 1 か所に置き、dispatcher の子の生成
//! （`reconcile_tree_units`）・`plan_waits`・生存確認の事実（`task_ops::tree::node_liveness_facts`）が同じ関数を呼ぶ。純粋関数。

use std::collections::BTreeSet;

use crate::execution_plan::{WorkUnitKind, WorkUnitRow, WorkUnitStatus};
use crate::model::TaskId;

/// D1: 同時の子 task の枠を占める kind task の unit（seq 順）。`running` の kind task の unit のうち、子 task 自身が一時停止
/// されていない（`self_paused` に子の id が無い）もの。祖先の一時停止で止まっている子は `self_paused` に入れない（数える）。
pub fn child_slot_holders<'a>(
    units: &'a [WorkUnitRow],
    self_paused: &BTreeSet<TaskId>,
) -> Vec<&'a WorkUnitRow> {
    let mut out: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.kind == WorkUnitKind::Task && u.status == WorkUnitStatus::Running)
        .filter(|u| {
            !u.child_task_id
                .as_deref()
                .and_then(|s| s.parse::<TaskId>().ok())
                .is_some_and(|id| self_paused.contains(&id))
        })
        .collect();
    out.sort_by_key(|u| u.seq);
    out
}

/// D1: 子 task の作成の候補（seq 順）。現在の段（終端でない行のうち seq 最小の行の段。`runnable_work_units` と同じ）の
/// `ready` の kind task の unit で、子がまだ無いもの。計画の `adopt`（既存の task の採用を待つ unit。ADR-0079 D15）は計画の
/// spec にしか無いので、dispatcher が作成の直前に見送る（枠は使わない）。
pub fn child_creation_candidates(units: &[WorkUnitRow]) -> Vec<&WorkUnitRow> {
    let current_stage = units
        .iter()
        .filter(|u| !u.status.is_terminal())
        .min_by_key(|u| u.seq)
        .and_then(|u| u.phase.clone());
    let mut out: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| {
            u.kind == WorkUnitKind::Task
                && u.status == WorkUnitStatus::Ready
                && u.child_task_id.is_none()
                && u.phase == current_stage
        })
        .collect();
    out.sort_by_key(|u| u.seq);
    out
}

/// D3: 枠の待ち（[`child_slot_wait`] の結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSlotWait {
    /// 枠を待つ unit の key（[`child_creation_candidates`] の順）。
    pub unit_keys: Vec<String>,
    /// 枠を占める子 task（[`child_slot_holders`] の子。id 順）。
    pub task_ids: Vec<TaskId>,
    /// 実効の上限（1 未満は 1。dispatcher と同じ）。
    pub limit: usize,
}

/// D3: 子 task の作成の候補が 1 件以上あり、枠を占める子の数が上限（`limit`、1 未満は 1）以上なら `Some`。空きがあれば
/// 次の tick で子が作られるので `None`。
pub fn child_slot_wait(
    units: &[WorkUnitRow],
    self_paused: &BTreeSet<TaskId>,
    limit: usize,
) -> Option<ChildSlotWait> {
    let limit = limit.max(1);
    let candidates = child_creation_candidates(units);
    if candidates.is_empty() {
        return None;
    }
    let holders = child_slot_holders(units, self_paused);
    if holders.len() < limit {
        return None;
    }
    let task_ids: BTreeSet<TaskId> = holders
        .iter()
        .filter_map(|u| u.child_task_id.as_deref()?.parse::<TaskId>().ok())
        .collect();
    Some(ChildSlotWait {
        unit_keys: candidates.into_iter().map(|u| u.key.clone()).collect(),
        task_ids: task_ids.into_iter().collect(),
        limit,
    })
}

#[cfg(test)]
mod tests;
