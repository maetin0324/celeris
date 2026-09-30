use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::execution_plan::{ExecutionPlanRow, WorkUnitRow, WorkUnitStatus};
use crate::model::{Event, Task, TaskId};

use crate::store::{SqliteStore, StoreError, TreeAdoption};

impl SqliteStore {
    /// ADR-0079 D15（Phase R5b-prep）: 採用する task が今も `expect_status` で、木に属していない（`tree` が無い）か。
    pub(in crate::store) fn tree_adoption_ok_tx(
        tx: &Connection,
        a: &TreeAdoption,
    ) -> Result<bool, StoreError> {
        Ok(Self::get_locked(tx, a.task.id)?
            .is_some_and(|current| current.status == a.expect_status && current.tree.is_none()))
    }

    /// ADR-0079 D15: 採用する task の `json` と絞り込みの列（`root_id`・`parent_id`）を書き、その task に event を積む。
    /// 状態・attempts・lease は読み直した値のまま（状態機械は通らない）。
    pub(in crate::store) fn apply_tree_adoption_tx(
        tx: &Connection,
        a: &TreeAdoption,
    ) -> Result<(), StoreError> {
        let Some(current) = Self::get_locked(tx, a.task.id)? else {
            return Err(StoreError::Invalid(format!(
                "task not found: {}",
                a.task.id
            )));
        };
        let merged = Task {
            status: current.status,
            attempts: current.attempts,
            lease: current.lease.clone(),
            ..a.task.clone()
        };
        Self::update_task_tx(tx, &merged)?;
        Self::append_event_tx(tx, a.task.id, &a.event)?;
        Ok(())
    }

    pub(in crate::store) fn tree_tasks_impl(
        &self,
        root_id: TaskId,
    ) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE id = ?1 OR root_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![root_id.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn tasks_with_open_task_units_impl(
        &self,
    ) -> Result<Vec<TaskId>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT task_id FROM work_units WHERE kind = 'task' AND status IN \
                 ('pending', 'ready', 'running', 'needs_continuation', 'blocked') ORDER BY task_id",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
            ids.iter().map(|s| Self::parse_id(s)).collect()
        })
    }

    pub(in crate::store) fn tree_child_create_impl(
        &self,
        parent_id: TaskId,
        child: &Task,
        unit: WorkUnitRow,
        parent_events: Vec<Event>,
        replaces: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(parent) = Self::get_locked(&tx, parent_id)? else {
            return Ok(false);
        };
        if parent.status.is_terminal() || child.parent_id != Some(parent_id) {
            return Ok(false);
        }
        let current = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![unit.id, parent_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?;
        let Some(current) = current else {
            return Ok(false);
        };
        let expected = match replaces {
            None => current.status == WorkUnitStatus::Ready && current.child_task_id.is_none(),
            Some(prev) => {
                current.status == WorkUnitStatus::Running
                    && current.child_task_id.as_deref() == Some(prev)
            }
        };
        if !expected || current.kind != crate::execution_plan::WorkUnitKind::Task {
            return Ok(false);
        }
        Self::insert_tx(&tx, child)?;
        Self::append_event_tx(
            &tx,
            child.id,
            &Event::Created {
                task: Box::new(child.clone()),
                origin: Some(crate::model::CreatedOrigin::PlanUnit),
            },
        )?;
        Self::update_work_unit_tx(&tx, &unit)?;
        for ev in &parent_events {
            Self::append_event_tx(&tx, parent_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::store) fn execution_plan_adopt_tree_impl(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for a in &adoptions {
            if !Self::tree_adoption_ok_tx(&tx, a)? {
                return Ok(false);
            }
        }
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        for ev in &after_events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        for a in &adoptions {
            Self::apply_tree_adoption_tx(&tx, a)?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub(in crate::store) fn tree_adopt_apply_impl(
        &self,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(owner) = Self::get_locked(&tx, owner_id)? else {
            return Ok(false);
        };
        if owner.status.is_terminal() {
            return Ok(false);
        }
        let current = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![unit_id, owner_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?;
        let Some(current) = current else {
            return Ok(false);
        };
        if current.status != expect_unit_status
            || current.child_task_id.is_some()
            || current.kind != crate::execution_plan::WorkUnitKind::Task
        {
            return Ok(false);
        }
        if !Self::tree_adoption_ok_tx(&tx, &adoption)? {
            return Ok(false);
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, owner_id, ev)?;
        }
        Self::apply_tree_adoption_tx(&tx, &adoption)?;
        tx.commit()?;
        Ok(true)
    }
}
