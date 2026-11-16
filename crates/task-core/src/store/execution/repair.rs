use rusqlite::{TransactionBehavior, params};
use time::OffsetDateTime;

use crate::execution_plan::{ExecutionPlanRow, WorkUnitRow};
use crate::model::{Event, TaskId};
use crate::transition::{Outcome, Trigger};

use crate::store::{SqliteStore, StoreError, format_rfc3339};

impl SqliteStore {
    pub(in crate::store) fn repair_apply(
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

    /// ADR-0072 D16（Phase E4）: reviewer repair の採用。`Trigger::ReviewRepair`
    /// （`Reviewing → Ready`、attempts 据え置き）と、`new_plan`（`Some` のときだけ挿入。atomic な
    /// Task で暗黙の WorkUnit を初めて実体化するときの `execution_plans` 行、D5）・`work_units`
    /// （repair WU。atomic なら `main`（done）も含む）・`extra_events`（`WorkerFinished{role:
    /// reviewer}` / `ReviewVerdict` / repair WU の `WorkUnitTransitioned` 等）を同じトランザクションで
    /// 書く。呼び出し側（`dispatcher.rs`）が D16 の上限（`max_repairs`/`max_repairs_per_class`）を
    /// 検査済みであることが前提（ここでは検査しない）。
    pub fn review_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.repair_apply(
            task_id,
            Trigger::ReviewRepair,
            extra_events,
            new_plan,
            work_units,
        )
    }

    /// 配送の局所修復。Reopen と repair WU を同一トランザクションで保存する。
    pub fn delivery_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.repair_apply(task_id, Trigger::Reopen, extra_events, new_plan, work_units)
    }

    /// ADR-0072 D17（Phase E4）: replan の採用。旧 `active` な計画を `superseded` にし、新しい版
    /// （`new_plan.version = old.version + 1`）を挿入する。`updated_work_units` は既存行の書き換え
    /// （spec が変わった未完了 WU、または `superseded` にする削除された WU）、`new_work_units` は
    /// 新しい key の追加。`extra_events`（削除された WU の `WorkUnitTransitioned` 等）→
    /// `plan_event`（`Event::ExecutionPlanned{supersedes: Some(old_plan_id), ..}`）の順で同じ
    /// トランザクションに追記する。旧版が `active` でなければ `StoreError::InUse`
    /// （並行 replan の検出）。
    #[allow(clippy::too_many_arguments)]
    pub fn execution_plan_replan(
        &self,
        task_id: TaskId,
        old_plan_id: String,
        new_plan: ExecutionPlanRow,
        updated_work_units: Vec<WorkUnitRow>,
        new_work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        plan_event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::execution_plan_replan_tx(
            &tx,
            task_id,
            old_plan_id,
            new_plan,
            updated_work_units,
            new_work_units,
            extra_events,
            plan_event,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// `execution_plan_replan` の本体（呼び出し側の transaction の中で書く。ADR 2026-10-09 D5 の CoS 監査経路）。
    #[allow(clippy::too_many_arguments)]
    pub fn execution_plan_replan_tx(
        tx: &rusqlite::Connection,
        task_id: TaskId,
        old_plan_id: String,
        new_plan: ExecutionPlanRow,
        updated_work_units: Vec<WorkUnitRow>,
        new_work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        plan_event: Event,
    ) -> Result<(), StoreError> {
        let superseded_at = format_rfc3339(OffsetDateTime::now_utc())?;
        let n = tx.execute(
            "UPDATE execution_plans SET status = 'superseded', superseded_at = ?1 \
             WHERE id = ?2 AND task_id = ?3 AND status = 'active'",
            params![superseded_at, old_plan_id, task_id.to_string()],
        )?;
        if n == 0 {
            return Err(StoreError::InUse {
                kind: "execution_plan",
                id: old_plan_id,
                detail: "no active execution plan to replan (changed concurrently?)".to_string(),
            });
        }
        tx.execute(
            "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
             status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                new_plan.id,
                new_plan.task_id,
                new_plan.version,
                new_plan.origin.as_str(),
                new_plan.planner_run_id,
                new_plan.status.as_str(),
                serde_json::to_string(&new_plan.spec)?,
                new_plan.created_at,
                new_plan.superseded_at,
            ],
        )?;
        for wu in &updated_work_units {
            Self::update_work_unit_tx(tx, wu)?;
        }
        for wu in &new_work_units {
            Self::insert_work_unit_tx(tx, wu)?;
        }
        for ev in &extra_events {
            Self::append_event_tx(tx, task_id, ev)?;
        }
        Self::append_event_tx(tx, task_id, &plan_event)?;
        Ok(())
    }
}
