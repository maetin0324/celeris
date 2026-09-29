use std::time::Duration as StdDuration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use crate::execution_plan::{
    WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus,
};
use crate::model::{Event, Status, Task, TaskId};

use crate::store::query::usize_to_i64;
use crate::store::{SqliteStore, StoreError, format_rfc3339, status_str};

impl SqliteStore {
    pub(in crate::store) fn insert_work_unit_tx(
        tx: &Connection,
        wu: &WorkUnitRow,
    ) -> Result<(), StoreError> {
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

    pub(in crate::store) fn update_work_unit_tx(
        tx: &Connection,
        wu: &WorkUnitRow,
    ) -> Result<(), StoreError> {
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

    pub(in crate::store) fn row_to_work_unit(
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

    pub(in crate::store) fn work_units_for_impl(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<WorkUnitRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units \
                 WHERE task_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_work_unit)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn work_unit_get_impl(
        &self,
        id: &str,
    ) -> Result<Option<WorkUnitRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1",
                params![id],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()
        })
    }

    pub(in crate::store) fn work_unit_transition_impl(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::update_work_unit_tx(&tx, &updated)?;
        Self::append_event_tx(&tx, task_id, &event)?;
        tx.commit()?;
        Ok(())
    }

    pub(in crate::store) fn acquire_work_unit_lease_impl(
        &self,
        task_id: TaskId,
        work_unit_id: &str,
        run_id: &str,
        ttl: StdDuration,
        branch: Option<String>,
        base_commit: Option<String>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        if task.status != Status::Running {
            return Ok(false);
        }
        let Some(current) = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, \
                 child_task_id, needs_decisions_json \
                 FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![work_unit_id, task_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?
        else {
            return Ok(false);
        };
        if !matches!(
            current.status,
            WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
        ) {
            return Ok(false);
        }
        let now = OffsetDateTime::now_utc();
        let expires_at = now + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let mut updated = current.clone();
        updated.status = WorkUnitStatus::Running;
        updated.blocked_reason = None;
        updated.runs += 1;
        updated.last_run_id = Some(run_id.to_string());
        updated.updated_at = format_rfc3339(now)?;
        updated.lease_run_id = Some(run_id.to_string());
        updated.lease_expires_at = Some(format_rfc3339(expires_at)?);
        if branch.is_some() {
            updated.branch = branch;
        }
        if base_commit.is_some() {
            updated.base_commit = base_commit;
        }
        Self::update_work_unit_tx(&tx, &updated)?;
        Self::append_event_tx(
            &tx,
            task_id,
            &Event::WorkUnitTransitioned {
                work_unit_id: current.id.clone(),
                key: current.key.clone(),
                from: current.status,
                to: WorkUnitStatus::Running,
                reason: "dispatch".to_string(),
                run_id: Some(run_id.to_string()),
            },
        )?;
        // Task の lease の期限を延ばす（保持者はそのまま。短くはしない）。
        if let Some(lease) = task.lease.as_mut()
            && lease.expires_at < expires_at
        {
            lease.expires_at = expires_at;
            tx.execute(
                "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                params![
                    format_rfc3339(expires_at)?,
                    serde_json::to_string(&task)?,
                    task_id.to_string(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub(in crate::store) fn work_units_apply_impl(
        &self,
        task_id: TaskId,
        inserted: Vec<WorkUnitRow>,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for wu in &inserted {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(in crate::store) fn running_tasks_with_runnable_work_units_impl(
        &self,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.json FROM tasks t WHERE t.status = ?1 AND EXISTS ( \
                 SELECT 1 FROM work_units w WHERE w.task_id = t.id AND w.phase IS NOT NULL \
                 AND w.kind NOT IN ('integrate', 'task') AND w.status IN ('ready', 'needs_continuation')) \
                 ORDER BY t.created_at ASC LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![status_str(Status::Running), usize_to_i64(limit)],
                |row| row.get::<_, String>(0),
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn work_units_replace_impl(
        &self,
        task_id: TaskId,
        rows: Vec<WorkUnitRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM work_units WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for wu in &rows {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        tx.commit()?;
        Ok(())
    }
}
