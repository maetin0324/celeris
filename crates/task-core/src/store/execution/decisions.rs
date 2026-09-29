use rusqlite::{TransactionBehavior, params};

use crate::execution_plan::WorkUnitRow;
use crate::model::{Event, TaskId};

use crate::store::{SqliteStore, StoreError};

impl SqliteStore {
    pub(in crate::store) fn decisions_list_impl(
        &self,
        root_id: Option<TaskId>,
    ) -> Result<Vec<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut out = Vec::new();
            match root_id {
                Some(root) => {
                    let sql = format!(
                        "SELECT {} FROM decisions WHERE root_id = ?1 ORDER BY created_at, id",
                        Self::DECISION_COLUMNS
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![root.to_string()], Self::row_to_decision)?;
                    for row in rows {
                        out.push(row??);
                    }
                }
                None => {
                    let sql = format!(
                        "SELECT {} FROM decisions ORDER BY created_at, id",
                        Self::DECISION_COLUMNS
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], Self::row_to_decision)?;
                    for row in rows {
                        out.push(row??);
                    }
                }
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn decision_get_impl(
        &self,
        id: &str,
    ) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| Self::decision_get_tx(conn, id))
    }

    pub(in crate::store) fn decisions_replace_impl(
        &self,
        rows: Vec<crate::decision::DecisionRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM decisions", [])?;
        for row in &rows {
            Self::insert_decision_tx(&tx, row)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(in crate::store) fn decision_resolve_apply_impl(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match Self::decision_get_tx(&tx, decision_id)? {
            Some(row) if row.status == expect && row.task_id == task_id => {}
            _ => return Ok(false),
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }
}
