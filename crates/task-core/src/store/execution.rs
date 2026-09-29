use rusqlite::{Connection, TransactionBehavior, params};

use crate::execution_plan::{
    ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus, RunIndexRole, RunIndexStatus,
    RunRow, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus,
};
use crate::model::{Event, TaskId};
use crate::transition::{Outcome, Trigger};

use super::{SqliteStore, StoreError};

impl SqliteStore {
    /// `execution_plan_adopt` / `execution_plan_adopt_delegating` の共通部分（tx の中で計画・WU・events を書く）。
    pub(super) fn adopt_plan_tx(
        tx: &Connection,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError> {
        let existing: i64 = tx.query_row(
            "SELECT COUNT(*) FROM execution_plans WHERE task_id = ?1 AND status = 'active'",
            params![task_id.to_string()],
            |r| r.get(0),
        )?;
        if existing > 0 {
            return Err(StoreError::InUse {
                kind: "execution_plan",
                id: task_id.to_string(),
                detail: "task already has an active execution plan (replan is Phase E4)"
                    .to_string(),
            });
        }
        tx.execute(
            "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, status, \
             json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                plan.id,
                plan.task_id,
                plan.version,
                plan.origin.as_str(),
                plan.planner_run_id,
                plan.status.as_str(),
                serde_json::to_string(&plan.spec)?,
                plan.created_at,
                plan.superseded_at,
            ],
        )?;
        for wu in &work_units {
            Self::insert_work_unit_tx(tx, wu)?;
        }
        for ev in &extra_events {
            Self::append_event_tx(tx, task_id, ev)?;
        }
        Self::append_event_tx(tx, task_id, &event)?;
        Ok(())
    }

    pub(super) fn repair_apply(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(plan) = &new_plan {
            tx.execute(
                "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
                 status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    plan.id,
                    plan.task_id,
                    plan.version,
                    plan.origin.as_str(),
                    plan.planner_run_id,
                    plan.status.as_str(),
                    serde_json::to_string(&plan.spec)?,
                    plan.created_at,
                    plan.superseded_at
                ],
            )?;
        }
        for wu in &work_units {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        let outcome = Self::apply_transition_tx(&tx, task_id, trigger, extra_events)?;
        tx.commit()?;
        Ok(outcome)
    }

    // ---- ADR-0072 D5（Phase E2）: execution_plans / work_units / runs の行の変換・書き込み ----
    pub(super) fn insert_work_unit_tx(tx: &Connection, wu: &WorkUnitRow) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO work_units (id, task_id, plan_id, key, seq, kind, status, \
             blocked_reason, depends_on_json, runs, continuations, retries, last_run_id, \
             last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
             lease_expires_at, branch, base_commit, head_commit, integrated_commit, \
             child_task_id, needs_decisions_json) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
             ?21,?22,?23,?24,?25,?26)",
            params![
                wu.id,
                wu.task_id,
                wu.plan_id,
                wu.key,
                wu.seq,
                wu.kind.as_str(),
                wu.status.as_str(),
                wu.blocked_reason.map(|r| r.as_str()),
                serde_json::to_string(&wu.depends_on)?,
                wu.runs,
                wu.continuations,
                wu.retries,
                wu.last_run_id,
                wu.last_checkpoint_run_id,
                serde_json::to_string(&wu.spec)?,
                wu.created_at,
                wu.updated_at,
                wu.phase,
                wu.lease_run_id,
                wu.lease_expires_at,
                wu.branch,
                wu.base_commit,
                wu.head_commit,
                wu.integrated_commit,
                wu.child_task_id,
                serde_json::to_string(&wu.needs_decisions)?,
            ],
        )?;
        Ok(())
    }

    /// ADR-0072 D5（Phase E2/E2b）: `runs` へ 1 行挿入する共通ロジック（`run_index_start` と
    /// `runs_replace` の両方から呼ぶ）。`tx` はトランザクションでも素の `Connection` でもよい
    /// （`Transaction: Deref<Target = Connection>`）。
    pub(super) fn insert_run_row_tx(tx: &Connection, row: &RunRow) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, adapter, \
             model, account, session_id, checkpoint_json, usage_json, metrics_json, \
             started_at, finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                row.run_id,
                row.task_id,
                row.work_unit_id,
                row.role.as_str(),
                row.seq,
                row.status.as_str(),
                row.adapter,
                row.model,
                row.account,
                row.session_id,
                row.checkpoint
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.usage.as_ref().map(serde_json::to_string).transpose()?,
                row.metrics
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.started_at,
                row.finished_at,
            ],
        )?;
        Ok(())
    }

    pub(super) fn update_work_unit_tx(tx: &Connection, wu: &WorkUnitRow) -> Result<(), StoreError> {
        tx.execute(
            "UPDATE work_units SET plan_id = ?1, status = ?2, blocked_reason = ?3, \
             depends_on_json = ?4, runs = ?5, continuations = ?6, retries = ?7, \
             last_run_id = ?8, last_checkpoint_run_id = ?9, json = ?10, updated_at = ?11, \
             phase = ?13, lease_run_id = ?14, lease_expires_at = ?15, branch = ?16, \
             base_commit = ?17, head_commit = ?18, integrated_commit = ?19, \
             child_task_id = ?20, needs_decisions_json = ?21 \
             WHERE id = ?12",
            params![
                wu.plan_id,
                wu.status.as_str(),
                wu.blocked_reason.map(|r| r.as_str()),
                serde_json::to_string(&wu.depends_on)?,
                wu.runs,
                wu.continuations,
                wu.retries,
                wu.last_run_id,
                wu.last_checkpoint_run_id,
                serde_json::to_string(&wu.spec)?,
                wu.updated_at,
                wu.id,
                wu.phase,
                wu.lease_run_id,
                wu.lease_expires_at,
                wu.branch,
                wu.base_commit,
                wu.head_commit,
                wu.integrated_commit,
                wu.child_task_id,
                serde_json::to_string(&wu.needs_decisions)?,
            ],
        )?;
        Ok(())
    }

    pub(super) fn row_to_execution_plan(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<ExecutionPlanRow, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let version: u32 = row.get(2)?;
        let origin_s: String = row.get(3)?;
        let planner_run_id: Option<String> = row.get(4)?;
        let status_s: String = row.get(5)?;
        let json: String = row.get(6)?;
        let created_at: String = row.get(7)?;
        let superseded_at: Option<String> = row.get(8)?;
        Ok((|| -> Result<ExecutionPlanRow, StoreError> {
            let Some(origin) = PlanOrigin::parse(&origin_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid execution_plans.origin: {origin_s}"
                )));
            };
            let Some(status) = PlanStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid execution_plans.status: {status_s}"
                )));
            };
            let spec: ExecutionPlanSpec = serde_json::from_str(&json)?;
            Ok(ExecutionPlanRow {
                id,
                task_id,
                version,
                origin,
                planner_run_id,
                status,
                spec,
                created_at,
                superseded_at,
            })
        })())
    }

    pub(super) fn row_to_work_unit(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<WorkUnitRow, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let plan_id: String = row.get(2)?;
        let key: String = row.get(3)?;
        let seq: u32 = row.get(4)?;
        let kind_s: String = row.get(5)?;
        let status_s: String = row.get(6)?;
        let blocked_reason_s: Option<String> = row.get(7)?;
        let depends_on_json: String = row.get(8)?;
        let runs: u32 = row.get(9)?;
        let continuations: u32 = row.get(10)?;
        let retries: u32 = row.get(11)?;
        let last_run_id: Option<String> = row.get(12)?;
        let last_checkpoint_run_id: Option<String> = row.get(13)?;
        let json: String = row.get(14)?;
        let created_at: String = row.get(15)?;
        let updated_at: String = row.get(16)?;
        let phase: Option<String> = row.get(17)?;
        let lease_run_id: Option<String> = row.get(18)?;
        let lease_expires_at: Option<String> = row.get(19)?;
        let branch: Option<String> = row.get(20)?;
        let base_commit: Option<String> = row.get(21)?;
        let head_commit: Option<String> = row.get(22)?;
        let integrated_commit: Option<String> = row.get(23)?;
        // ADR-0079（migration 0031）。
        let child_task_id: Option<String> = row.get(24)?;
        let needs_decisions_json: String = row.get(25)?;
        Ok((|| -> Result<WorkUnitRow, StoreError> {
            let Some(kind) = WorkUnitKind::parse(&kind_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid work_units.kind: {kind_s}"
                )));
            };
            let Some(status) = WorkUnitStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid work_units.status: {status_s}"
                )));
            };
            let blocked_reason = blocked_reason_s
                .map(|s| {
                    WorkUnitBlockedReason::parse(&s).ok_or_else(|| {
                        StoreError::Invalid(format!("invalid work_units.blocked_reason: {s}"))
                    })
                })
                .transpose()?;
            let depends_on: Vec<String> = serde_json::from_str(&depends_on_json)?;
            let spec: WorkUnitSpec = serde_json::from_str(&json)?;
            let needs_decisions: Vec<String> = serde_json::from_str(&needs_decisions_json)?;
            Ok(WorkUnitRow {
                id,
                task_id,
                plan_id,
                key,
                seq,
                kind,
                status,
                blocked_reason,
                depends_on,
                runs,
                continuations,
                retries,
                last_run_id,
                last_checkpoint_run_id,
                spec,
                created_at,
                updated_at,
                phase,
                lease_run_id,
                lease_expires_at,
                branch,
                base_commit,
                head_commit,
                integrated_commit,
                child_task_id,
                needs_decisions,
            })
        })())
    }

    pub(super) fn row_to_run(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<RunRow, StoreError>> {
        let run_id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let work_unit_id: Option<String> = row.get(2)?;
        let role_s: String = row.get(3)?;
        let seq: u32 = row.get(4)?;
        let status_s: String = row.get(5)?;
        let adapter: Option<String> = row.get(6)?;
        let model: Option<String> = row.get(7)?;
        let account: Option<String> = row.get(8)?;
        let session_id: Option<String> = row.get(9)?;
        let checkpoint_json: Option<String> = row.get(10)?;
        let usage_json: Option<String> = row.get(11)?;
        let metrics_json: Option<String> = row.get(12)?;
        let started_at: String = row.get(13)?;
        let finished_at: Option<String> = row.get(14)?;
        Ok((|| -> Result<RunRow, StoreError> {
            let Some(role) = RunIndexRole::parse(&role_s) else {
                return Err(StoreError::Invalid(format!("invalid runs.role: {role_s}")));
            };
            let Some(status) = RunIndexStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid runs.status: {status_s}"
                )));
            };
            let checkpoint = checkpoint_json
                .map(|j| serde_json::from_str(&j))
                .transpose()?;
            let usage = usage_json.map(|j| serde_json::from_str(&j)).transpose()?;
            let metrics = metrics_json.map(|j| serde_json::from_str(&j)).transpose()?;
            Ok(RunRow {
                run_id,
                task_id,
                work_unit_id,
                role,
                seq,
                status,
                adapter,
                model,
                account,
                session_id,
                checkpoint,
                usage,
                metrics,
                started_at,
                finished_at,
            })
        })())
    }
}
