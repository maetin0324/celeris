//! ADR 2026-10-29-phase-integration-auto-repair: 段の統合（origin `phase:<統合 WU の key>`）の依頼。
//! 依頼で止まった統合 WU の特定と再開（受信箱の回答と sweep が共有する）、自動の解消を経ていない古い依頼を
//! 自動の解消へ戻す sweep（D3）を持つ。判定は store だけで、LLM は呼ばない。
use task_core::{
    Event, Status, StoreError, TaskId, TaskStore, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow,
    WorkUnitStatus,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::OpsError;

/// 段の統合の依頼の origin の接頭辞（`phase:<統合 WU の key>`）。
pub const PHASE_ORIGIN_PREFIX: &str = "phase:";
/// D3: sweep が古い依頼に記録する回答（受信箱の「再試行する」と同じ値）。
pub const RETRY_ANSWER: &str = "retry";
/// D3: sweep の回答に添える説明。
pub const AUTO_REPAIR_RETRY_NOTE: &str = "自動の解消へ回す: 段の統合の衝突は merge の repair WU が解く（ADR 2026-10-29-phase-integration-auto-repair）。統合をやり直す";

/// origin `phase:<key>` の依頼で止まっている統合 WU（task が blocked、WU が blocked(question・continuation cap)）。
/// 止まっていなければ `None`（別の経路で再開・完了した。回答の記録だけ行えばよい）。
pub fn blocked_integration_unit(
    store: &dyn TaskStore,
    task_id: TaskId,
    origin: &str,
) -> Result<Option<WorkUnitRow>, StoreError> {
    let Some(key) = origin.strip_prefix(PHASE_ORIGIN_PREFIX) else {
        return Ok(None);
    };
    let Some(task) = store.get(task_id)? else {
        return Ok(None);
    };
    if task.status != Status::Blocked {
        return Ok(None);
    }
    Ok(store.work_units_for(task_id)?.into_iter().find(|unit| {
        unit.key == key
            && unit.kind == WorkUnitKind::Integrate
            && unit.status == WorkUnitStatus::Blocked
            && matches!(
                unit.blocked_reason,
                Some(WorkUnitBlockedReason::Question)
                    | Some(WorkUnitBlockedReason::ContinuationCap)
            )
    }))
}

/// 止まった統合 WU を pending に戻し、task を回答（`Trigger::Answer`）で再開する。次の tick が統合をやり直す。
pub fn resume_integration_unit(
    store: &dyn TaskStore,
    task_id: TaskId,
    mut unit: WorkUnitRow,
    note: String,
    by: &str,
) -> Result<(), OpsError> {
    let previous = unit.status;
    unit.status = WorkUnitStatus::Pending;
    unit.blocked_reason = None;
    unit.updated_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| OpsError::Validation(format!("timestamp encoding failed: {e}")))?;
    store.work_unit_transition(
        task_id,
        unit.clone(),
        Event::WorkUnitTransitioned {
            work_unit_id: unit.id,
            key: unit.key,
            from: previous,
            to: WorkUnitStatus::Pending,
            reason: "integration_answer".to_string(),
            run_id: None,
        },
    )?;
    crate::gate::answer(store, task_id, note, by, Some(Status::Blocked))?;
    Ok(())
}

/// D3: 段 `phase` で merge の repair WU を 1 度でも作ったか（作っていれば依頼は自動の解消が尽きた後のもの）。
fn auto_repaired(units: &[WorkUnitRow], phase: &str) -> bool {
    let prefix = format!("merge-{phase}-");
    units
        .iter()
        .any(|u| u.kind == WorkUnitKind::Repair && u.key.starts_with(&prefix))
}

/// D3: 自動の解消（merge の repair WU）を経ずに人に回した段の統合の未回答の依頼（この ADR の前に記録したもの）を
/// `retry` で閉じ、統合 WU を再開する。やり直した統合は衝突を repair WU に解かせる。戻り値は再開した
/// `(task, request id)`。自動の解消が尽きて人に回した依頼（同じ段に repair WU がある）は触らない。
pub fn retry_unrepaired_requests(store: &dyn TaskStore) -> Result<Vec<(TaskId, String)>, OpsError> {
    let mut resumed = Vec::new();
    for open in super::open_requests(store)? {
        if !open.origin.starts_with(PHASE_ORIGIN_PREFIX) {
            continue;
        }
        let Some(unit) = blocked_integration_unit(store, open.task_id, &open.origin)? else {
            continue;
        };
        let units = store.work_units_for(open.task_id)?;
        if auto_repaired(&units, unit.phase.as_deref().unwrap_or_default()) {
            continue;
        }
        if !store.integration_request_answer(
            open.task_id,
            &open.request_id,
            RETRY_ANSWER,
            Some(AUTO_REPAIR_RETRY_NOTE),
            task_core::DAEMON_ACTOR,
        )? {
            continue;
        }
        resume_integration_unit(
            store,
            open.task_id,
            unit,
            AUTO_REPAIR_RETRY_NOTE.to_string(),
            task_core::DAEMON_ACTOR,
        )?;
        resumed.push((open.task_id, open.request_id));
    }
    Ok(resumed)
}
