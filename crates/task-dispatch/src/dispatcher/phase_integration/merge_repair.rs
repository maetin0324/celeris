//! ADR 2026-10-29-phase-integration-auto-repair: 段の統合の衝突を解く merge の repair WU
//! （key `merge-<phase>-<unit>`、Task の worktree で走る）の数え方と、失敗したときの扱い。

use task_core::{WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitStatus};

use crate::execution_scheduler::WuDecision;

/// D2: 失敗した merge の repair WU を退かせたときの `WorkUnitTransitioned.reason`。
pub(in crate::dispatcher) const FAILED_REASON: &str = "merge_repair_failed";

/// merge の repair WU か（統合後の検査の repair `repair-<phase>-*` とは別）。
fn is_merge_repair(wu: &WorkUnitRow) -> bool {
    wu.kind == WorkUnitKind::Repair && wu.phase.is_some() && wu.key.starts_with("merge-")
}

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

/// D2: merge の repair WU が失敗した（再試行を使い切った・進まない・計画の問題を申告した）ら、replan せずに
/// その WU を `cancelled` にして退かせ、統合 WU を `pending` に戻す（task は `Continue{advance}`）。やり直した
/// 統合は同じ衝突でもう一度 repair WU を作り、上限（`MAX_MERGE_REPAIRS_PER_UNIT`）を超えたら理由を付けて
/// 統合の依頼にする（人の受信箱）。merge の repair WU でない・失敗でない決定は変えない。
pub(in crate::dispatcher) fn retire_if_failed(wu: &WorkUnitRow, decision: &mut WuDecision) {
    if !is_merge_repair(wu) || !matches!(decision.reason, "failed" | "limit" | "plan_issue") {
        return;
    }
    let previous = decision.reason;
    decision.updated.status = WorkUnitStatus::Cancelled;
    decision.updated.blocked_reason = None;
    decision.reason = FAILED_REASON;
    decision.trigger = task_core::Trigger::Continue {
        why: task_core::ContinueWhy::Advance,
    };
    decision.plan_complete = false;
    for row in decision
        .newly_blocked
        .iter_mut()
        .filter(|row| row.kind == WorkUnitKind::Integrate)
    {
        row.status = WorkUnitStatus::Pending;
        row.blocked_reason = None::<WorkUnitBlockedReason>;
        row.depends_on.retain(|d| d != &wu.key);
        row.spec.depends_on.retain(|d| d != &wu.key);
    }
    decision.outcome_override = Some(format!(
        "merge の repair WU {} が衝突を解けなかった（{previous}）。段の統合をやり直す（上限を超えたら統合の依頼として人に回す）",
        wu.key
    ));
}
