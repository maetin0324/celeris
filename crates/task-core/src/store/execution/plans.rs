use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::execution_plan::{
    ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus, WorkUnitRow,
};
use crate::model::{Event, Status, Task, TaskId};
use crate::transition::Trigger;

use crate::store::{SqliteStore, StoreError};

impl SqliteStore {
    /// `execution_plan_adopt` / `execution_plan_adopt_delegating` の共通部分（tx の中で計画・WU・events を書く）。
    pub(in crate::store) fn adopt_plan_tx(
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

    pub(in crate::store) fn row_to_execution_plan(
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

    // ---- ADR-0072 D5（Phase E2）: 実行層の派生索引（execution_plans / work_units / runs）----
    //
    // 正本は `events`（`Event::ExecutionPlanned` / `WorkUnitTransitioned` / `WorkerStarted` /
    // `WorkerFinished` / `CheckpointSaved`）。この 3 表は対応する Event と同じトランザクションで
    // 書く。`task_ops::execution::rebuild_from_events` で events から作り直して一致を確かめられる。
    /// D14: 新しい計画を採用する（`Event::ExecutionPlanned` と `execution_plans` / `work_units` の
    /// 行を同じトランザクションで書く）。E2 は新規のみ: そのタスクに既に `active` な計画があれば
    /// `StoreError::InUse`（replan は E4。`PlanOrigin::Repair` 等で明示的に旧版を `superseded` にした
    /// 上で採用する経路は別に用意する）。
    /// `extra_events`（ADR-0074 D2.1（Phase F3 途中確認）: `Event::PausePointsResolved` 用）は
    /// `event`（`ExecutionPlanned`）より先に、同じトランザクションで書く（`execution_plan_replan` と
    /// 同じ規律）。
    pub fn execution_plan_adopt(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        tx.commit()?;
        Ok(())
    }

    /// ADR-0074 D3.7（Phase F4b (f)）: `execution_plan_adopt` と同じことを行い、**同じトランザクションで**
    /// planner の `children` から作った子 Task（`parent_id = task_id`、draft なら `Accept`）を挿入し、
    /// `Event::Delegated{run_id: <planner run>, task_ids}` を親に残す（`delegate_children` と同じ形）。
    #[allow(clippy::too_many_arguments)]
    pub fn execution_plan_adopt_delegating(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        let mut ids = Vec::with_capacity(children.len());
        for child in &children {
            if child.parent_id != Some(task_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to task {task_id}",
                    child.id
                )));
            }
            Self::insert_tx(&tx, child)?;
            Self::append_event_tx(
                &tx,
                child.id,
                &Event::Created {
                    task: Box::new(child.clone()),
                    origin: None,
                    created_by: Some(crate::DAEMON_ACTOR.to_string()),
                },
            )?;
            if child.status == Status::Draft {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
            ids.push(child.id);
        }
        if !ids.is_empty() {
            Self::append_event_tx(
                &tx,
                task_id,
                &Event::Delegated {
                    run_id: run_id.to_string(),
                    task_ids: ids.clone(),
                },
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// そのタスクの `active` な計画（無ければ `None`）。
    pub fn execution_plan_active(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ExecutionPlanRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, task_id, version, origin, planner_run_id, status, json, created_at, \
                 superseded_at FROM execution_plans WHERE task_id = ?1 AND status = 'active'",
                params![task_id.to_string()],
                Self::row_to_execution_plan,
            )
            .optional()?
            .transpose()
        })
    }

    /// そのタスクの計画（`version` 昇順。履歴を含む）。
    pub fn execution_plan_list(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<ExecutionPlanRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, version, origin, planner_run_id, status, json, created_at, \
                 superseded_at FROM execution_plans WHERE task_id = ?1 ORDER BY version ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_execution_plan)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    /// ADR-0072 D5/D17（Phase E4b 項目5）: 同上。`task_id` の `execution_plans` 行（版の履歴）を
    /// 渡した集合でまるごと置き換える。`task_ops::replay::check_and_apply_execution` が
    /// `rebuild_execution_plans`（`Event::ExecutionPlanned` からの再構築。replan で複数版になった
    /// 履歴を含む）の結果を書き戻すのに使う。
    pub fn execution_plans_replace(
        &self,
        task_id: TaskId,
        rows: Vec<ExecutionPlanRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM execution_plans WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for plan in &rows {
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
                    plan.superseded_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
