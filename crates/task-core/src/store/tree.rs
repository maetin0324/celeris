use rusqlite::Connection;

use crate::model::Task;

use super::{SqliteStore, StoreError, TreeAdoption};

impl SqliteStore {
    /// ADR-0079 D15（Phase R5b-prep）: 採用する task が今も `expect_status` で、木に属していない（`tree` が無い）か。
    pub(super) fn tree_adoption_ok_tx(
        tx: &Connection,
        a: &TreeAdoption,
    ) -> Result<bool, StoreError> {
        Ok(Self::get_locked(tx, a.task.id)?
            .is_some_and(|current| current.status == a.expect_status && current.tree.is_none()))
    }

    /// ADR-0079 D15: 採用する task の `json` と絞り込みの列（`root_id`・`parent_id`）を書き、その task に event を積む。
    /// 状態・attempts・lease は読み直した値のまま（状態機械は通らない）。
    pub(super) fn apply_tree_adoption_tx(
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
}
