//! ADR 2026-10-29-phase-integration-auto-repair: 段の統合の衝突を解く merge の repair WU
//! （key `merge-<phase>-<unit>`、Task の worktree で走る）の数え方。失敗したときの扱い（WU を退かせて統合を
//! やり直す）は worker 完了の判断 `run_settlement::settle_work_unit` にある（ADR 2026-11-11 dispatcher-state-ownership D3）。

use task_core::{WorkUnitKind, WorkUnitRow};

/// D2: 同じ計画の段 `phase` で WU `unit` の merge の衝突に作った repair WU
/// （key が `merge-<phase>-<unit>` か `merge-<phase>-<unit>-<n>`）。状態を問わず数える。
pub(in crate::dispatcher) fn merge_repairs_of<'a>(
    units: &'a [WorkUnitRow],
    plan_id: &str,
    phase: &str,
    unit: &str,
) -> Vec<&'a WorkUnitRow> {
    let base = format!("merge-{phase}-{unit}");
    units
        .iter()
        .filter(|u| u.kind == WorkUnitKind::Repair && u.plan_id == plan_id)
        .filter(|u| {
            u.key == base
                || u.key.strip_prefix(&base).is_some_and(|rest| {
                    rest.strip_prefix('-')
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
        })
        .collect()
}
