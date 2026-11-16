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
            harness_id: None,
            status: current.status,
            attempts: current.attempts,
            lease: current.lease.clone(),
            ..a.task.clone()
        };
        Self::update_task_tx(tx, &merged)?;
        Self::append_event_tx(tx, a.task.id, &a.event)?;
        Ok(())
    }

    /// ADR-0079 D3（Phase R2a）: 木の節点（`root_id` の task 自身と、`tasks.root_id = root_id` の子孫）。
    /// `created_at` 昇順（同順位は rowid）。木の上限の数え上げ（`task_ops::tree::tree_counters`）に使う。
    /// root 自身は `tree` を持たない（`root_id` 列は NULL。R1b 付記 6.）ので id で拾う。
    pub fn tree_tasks(&self, root_id: TaskId) -> Result<Vec<Task>, StoreError> {
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

    /// ADR-0079 §7 R1b: 終わっていない kind task の unit（`pending` / `ready` / `running` /
    /// `needs_continuation` / `blocked`）を持つ task の id（重複なし、昇順）。dispatcher の tick が
    /// 子 task の生成と状態の写しの照合に使う（木が無ければ空で、何もしない）。
    pub fn tasks_with_open_task_units(&self) -> Result<Vec<TaskId>, StoreError> {
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

    /// ADR-0079 D4 (4)（Phase R1b）: kind task の unit から子 task を作る 1 トランザクション。
    /// 親が終端でなく、unit の行（`unit.id`）が今も `ready` で `child_task_id` を持たないことを確かめ、
    /// 子を挿入して `Event::Created{origin: plan_unit}` を積み、unit の行を `unit`（呼び出し側が
    /// `running`・`child_task_id` を書いたもの）に差し替え、親の events に `parent_events`
    /// （`WorkUnitTransitioned` と `ChildTaskCreated`）を積む。条件に合わなければ何も書かず `Ok(false)`。
    ///
    /// ADR-0079 D9（Phase R2b）: `replaces = Some(prev)` は基盤の失敗の作り直し（同じ unit から新しい子）。
    /// unit の行が `running` で `child_task_id = prev` であることを確かめる（`ready`・子なしの代わりに）。
    pub fn tree_child_create(
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
                 needs_decisions_json, rebase_pending FROM work_units WHERE id = ?1 AND task_id = ?2",
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
                created_by: Some(crate::DAEMON_ACTOR.to_string()),
            },
        )?;
        Self::update_work_unit_tx(&tx, &unit)?;
        for ev in &parent_events {
            Self::append_event_tx(&tx, parent_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// ADR-0079 D15（Phase R5b-prep）: 人の計画（origin human）の採用と、その計画の unit の `adopt` による既存の
    /// task の採用を 1 トランザクションで書く。`execution_plan_adopt` と同じことをした後に `after_events`
    /// （`WorkUnitTransitioned`・`ChildAdopted`・`UnitGateOverridden`・`DecisionRequested` など。`work_units` の行は
    /// 呼び出し側がこれらを当てた後の値で渡す）を積み、`adoptions` の各 task を書き換える。採用する task が今も
    /// `expect_status` で木に属していない（`tree` が無い）ことを確かめ、違えば何も書かずに `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    pub fn execution_plan_adopt_tree(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError> {
        self.write_tx(|tx| {
            Self::execution_plan_adopt_tree_tx(
                tx,
                task_id,
                plan,
                work_units,
                extra_events,
                event,
                after_events,
                adoptions,
            )
        })
    }

    /// Apply plan adoption within a caller-owned transaction (including CoS audit writes).
    #[allow(clippy::too_many_arguments)]
    pub fn execution_plan_adopt_tree_tx(
        tx: &Connection,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError> {
        for a in &adoptions {
            if !Self::tree_adoption_ok_tx(tx, a)? {
                return Ok(false);
            }
        }
        Self::adopt_plan_tx(tx, task_id, plan, work_units, extra_events, event)?;
        for ev in &after_events {
            Self::append_event_tx(tx, task_id, ev)?;
        }
        for a in &adoptions {
            Self::apply_tree_adoption_tx(tx, a)?;
        }
        Ok(true)
    }

    /// ADR-0079 D15（Phase R5b-prep）: 採用済みの計画の kind task の unit（`unit_id`）に既存の task を後から採用する
    /// 1 トランザクション（`POST /tasks/{id}/tree/adopt`）。計画を持つ task が終端でなく、unit の行が今も
    /// `expect_unit_status` の kind task で `child_task_id` を持たず、採用する task が `expect_status` で `tree` を
    /// 持たないことを確かめる。`updated` の行を書き換え、計画を持つ task に `events` を積み、採用する task を書き換える。
    /// 条件に合わなければ何も書かず `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    pub fn tree_adopt_apply(
        &self,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError> {
        self.write_tx(|tx| {
            Self::tree_adopt_apply_tx(
                tx,
                owner_id,
                unit_id,
                expect_unit_status,
                updated,
                events,
                adoption,
            )
        })
    }

    /// Apply a late tree adoption within a caller-owned transaction.
    pub fn tree_adopt_apply_tx(
        tx: &Connection,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError> {
        let Some(owner) = Self::get_locked(tx, owner_id)? else {
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
                 needs_decisions_json, rebase_pending FROM work_units WHERE id = ?1 AND task_id = ?2",
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
        if !Self::tree_adoption_ok_tx(tx, &adoption)? {
            return Ok(false);
        }
        for wu in &updated {
            Self::update_work_unit_tx(tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(tx, owner_id, ev)?;
        }
        Self::apply_tree_adoption_tx(tx, &adoption)?;
        Ok(true)
    }
}
