//! 遷移と判断の適用（`TaskStore` 本体。module map は `mod.rs` 冒頭）。

use rusqlite::{TransactionBehavior, params};
use time::OffsetDateTime;

use crate::model::{Event, Status, Task, TaskId};
use crate::org::{MilestoneId, MilestoneStatus};
use crate::transition::{Outcome, Trigger};

use super::{ProjectPlanApply, SqliteStore, StoreError, format_rfc3339};

impl SqliteStore {
    pub(super) fn apply_transition_with_events_impl(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = Self::apply_transition_tx(&tx, task_id, trigger, extra_events)?;
        tx.commit()?;
        Ok(outcome)
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

    pub(super) fn project_plan_decide_apply_impl(
        &self,
        plan_task_id: TaskId,
        milestones: &[MilestoneId],
        milestone_status: MilestoneStatus,
        tasks: &[TaskId],
        trigger: Trigger,
        decided_event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        for id in milestones {
            let affected = tx.execute(
                "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![milestone_status.as_str(), now, id.to_string()],
            )?;
            if affected != 1 {
                return Err(StoreError::Invalid(format!("milestone not found: {id}")));
            }
        }
        for id in tasks {
            let Some(task) = Self::get_locked(&tx, *id)? else {
                return Err(StoreError::Invalid(format!("task not found: {id}")));
            };
            // `Trigger::Cancel` は後続へカスケードする（ADR-0010 D2）ので、先に処理した兄弟の
            // cancel で既に終端になったものは飛ばす。
            if task.status != Status::Draft {
                continue;
            }
            Self::apply_transition_tx(&tx, *id, trigger.clone(), vec![])?;
        }
        Self::append_event_tx(&tx, plan_task_id, &decided_event)?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn project_plan_apply_impl(
        &self,
        apply: &ProjectPlanApply,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        for m in &apply.milestones {
            let affected = tx.execute(
                "UPDATE milestones SET status = ?1, updated_at = ?2, \
                 title = COALESCE(?3, title), description = COALESCE(?4, description) WHERE id = ?5",
                params![
                    m.status.as_str(),
                    now,
                    m.title.as_deref(),
                    m.description.as_deref(),
                    m.id.to_string()
                ],
            )?;
            if affected != 1 {
                return Err(StoreError::Invalid(format!(
                    "milestone not found: {}",
                    m.id
                )));
            }
        }
        for task in &apply.task_updates {
            let Some(current) = Self::get_locked(&tx, task.id)? else {
                return Err(StoreError::Invalid(format!("task not found: {}", task.id)));
            };
            if !matches!(current.status, Status::Draft | Status::Ready) || current.lease.is_some() {
                return Err(StoreError::Invalid(format!(
                    "task {} was already dispatched ({:?}); it cannot be modified",
                    task.id, current.status
                )));
            }
            let merged = Task {
                status: current.status,
                attempts: current.attempts,
                lease: current.lease.clone(),
                ..task.clone()
            };
            Self::update_task_tx(&tx, &merged)?;
            Self::append_event_tx(
                &tx,
                task.id,
                &Event::Edited {
                    fields: vec!["project_plan".to_string()],
                    by: "project-plan".to_string(),
                },
            )?;
        }
        for (id, trigger) in &apply.transitions {
            let Some(task) = Self::get_locked(&tx, *id)? else {
                return Err(StoreError::Invalid(format!("task not found: {id}")));
            };
            let applicable = match trigger {
                Trigger::Accept => task.status == Status::Draft,
                _ => !task.status.is_terminal(),
            };
            if !applicable {
                continue;
            }
            Self::apply_transition_tx(&tx, *id, trigger.clone(), vec![])?;
        }
        Self::append_event_tx(&tx, apply.plan_task_id, &apply.decided_event)?;
        tx.commit()?;
        Ok(())
    }
}
