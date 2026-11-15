//! 遷移と判断の適用（`TaskStore` 本体。module map は `mod.rs` 冒頭）と、lease を照合してから書く遷移（lease の回収）。

use rusqlite::TransactionBehavior;

use crate::execution_plan::WorkUnitRow;
use crate::model::{Event, Lease, Status, Task, TaskId};
use crate::transition::{Outcome, Trigger};

use super::{SqliteStore, StoreError};

impl SqliteStore {
    /// `TaskStore::apply_transition_with_work_units` の本体（`apply_transition_with_events` は `work_units` が空の形）。
    /// 既存の tx 関数 `update_work_unit_tx`・`append_event_tx`・`apply_transition_tx` を 1 つの transaction で組むだけ。
    pub(super) fn apply_transition_with_work_units_impl(
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

    /// `TaskStore::apply_transition_if_lease` の本体（ADR 2026-11-15-lease-reclaim-compare-and-release D1・D2）。照合と
    /// `apply_transition_tx` を同じ Immediate transaction で行う。合わなければ commit せずに `None`（何も書かない）。
    pub(super) fn apply_transition_if_lease_impl(
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

    pub(super) fn complete_plan_impl(
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
