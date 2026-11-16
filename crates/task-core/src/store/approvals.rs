//! 遷移と判断の適用（`SqliteStore` 本体。module map は `mod.rs` 冒頭）と、lease を照合してから書く遷移（lease の回収）。

use rusqlite::TransactionBehavior;

use crate::execution_plan::WorkUnitRow;
use crate::model::{Event, Lease, Status, Task, TaskId};
use crate::transition::{Outcome, Trigger};

use super::{SqliteStore, StoreError};

impl SqliteStore {
    /// `apply_transition_with_events` の一般形（ADR 2026-11-14-phase-integration-finish-one-write D1）。`work_units` の
    /// `(書き込む行, その WorkUnitTransitioned)` を順に書いてから `trigger` を適用し、全体を 1 つの Immediate
    /// transaction にする（段の統合の完了: 統合 WU の done・次の工程の WU の ready・`PhaseIntegrated`・Task の遷移）。
    /// 遷移が通らなければ（`InvalidTransition` など）何も書かない。
    /// `SqliteStore::apply_transition_with_work_units` の本体（`apply_transition_with_events` は `work_units` が空の形）。
    /// 既存の tx 関数 `update_work_unit_tx`・`append_event_tx`・`apply_transition_tx` を 1 つの transaction で組むだけ。
    pub fn apply_transition_with_work_units(
        &self,
        task_id: TaskId,
        work_units: Vec<(WorkUnitRow, Event)>,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (row, event) in &work_units {
            Self::update_work_unit_tx(&tx, row)?;
            Self::append_event_tx(&tx, task_id, event)?;
        }
        let outcome = Self::apply_transition_tx(&tx, task_id, trigger, extra_events)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// ADR 2026-11-15-lease-reclaim-compare-and-release D1・D2: lease の回収の書き込み口。1 つの Immediate transaction で
    /// Task を読み、`status` が `Running` で `lease` が `expected`（`worker_run_id` と `expires_at` の両方）と等しいときだけ
    /// `apply_transition_with_events` と同じ書き込み（遷移・`Transitioned`・`extra_events`）をして `Some(outcome)` を返す。
    /// Task が無い・running でない・lease が違う（別の接続が `renew_lease` で延ばした場合を含む）なら何も書かずに `None`。
    /// 遷移が無効なら `StoreError::InvalidTransition` を返し、何も書かない。
    /// `SqliteStore::apply_transition_if_lease` の本体（ADR 2026-11-15-lease-reclaim-compare-and-release D1・D2）。照合と
    /// `apply_transition_tx` を同じ Immediate transaction で行う。合わなければ commit せずに `None`（何も書かない）。
    pub fn apply_transition_if_lease(
        &self,
        task_id: TaskId,
        expected: &Lease,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Option<Outcome>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let held = Self::get_locked(&tx, task_id)?.is_some_and(|task| {
            task.status == Status::Running && task.lease.as_ref() == Some(expected)
        });
        if !held {
            return Ok(None);
        }
        let outcome = Self::apply_transition_tx(&tx, task_id, trigger, extra_events)?;
        tx.commit()?;
        Ok(Some(outcome))
    }

    /// ADR-0007 D3: Plan の子タスク群を挿入（`Created`、`accept_children` なら続けて `Accept`）し、
    /// 親に `ReviewPass` を適用して `verdict_events` を追記する。全体が 1 トランザクション。
    pub fn complete_plan(
        &self,
        plan_id: TaskId,
        verdict_events: Vec<Event>,
        children: Vec<Task>,
        accept_children: bool,
    ) -> Result<Outcome, StoreError> {
        // ADR-0007 D3: 子の insert + Created (+ Accept) と親の ReviewPass を単一トランザクションで行う。
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for child in &children {
            if child.parent_id != Some(plan_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to plan {plan_id}",
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
            if accept_children {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
        }
        let outcome = Self::apply_transition_tx(&tx, plan_id, Trigger::ReviewPass, verdict_events)?;
        tx.commit()?;
        Ok(outcome)
    }
}
