use rusqlite::{TransactionBehavior, params};

use crate::execution_plan::WorkUnitRow;
use crate::model::{Event, TaskId};

use crate::store::{SqliteStore, StoreError};

impl SqliteStore {
    /// ADR-0079 D7（Phase R1a / migration 0031）: 決定の要求の行（`created_at` 昇順、同順位は id）。
    /// `root_id` を渡せばその木だけ。行は `Event::DecisionRequested` / `DecisionAnswered` /
    /// `DecisionWithdrawn` の追記と同じトランザクションで store が書く（派生）。
    pub fn decisions_list(
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

    /// ADR-0079 D7: 1 件（無ければ `None`）。
    pub fn decision_get(
        &self,
        id: &str,
    ) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| Self::decision_get_tx(conn, id))
    }

    /// ADR-0079 D15: `decisions` の全行を渡した集合でまるごと置き換える（`events` は変えない）。
    /// replay の再構築（`task_ops::replay::rebuild_decisions`）を書き戻すのに使う。
    pub fn decisions_replace(
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

    /// ADR-0079 D7（Phase R3a）: 決定への回答・取り下げ・revise の 1 トランザクション。`decisions` の行
    /// `decision_id` の状態が今も `expect` であることを同じトランザクションで確かめ（違えば何も書かず
    /// `Ok(false)` = API の 409）、`task_id`（決定を出した節点）の WU の行 `updated` を書き換え、`events`
    /// （`DecisionAnswered` / `DecisionWithdrawn`・`WorkUnitTransitioned` など）を積む。
    pub fn decision_resolve_apply(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let applied =
            Self::decision_resolve_apply_tx(&tx, task_id, decision_id, expect, updated, events)?;
        if applied {
            tx.commit()?;
        }
        Ok(applied)
    }

    /// `decision_resolve_apply` inside a caller's transaction (ADR 2026-10-05 D3). `false`
    /// when the decision is no longer in `expect`; nothing is written then.
    pub fn decision_resolve_apply_tx(
        tx: &rusqlite::Connection,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError> {
        Self::decision_resolve_apply_tx_at(
            tx,
            task_id,
            decision_id,
            expect,
            updated,
            events,
            time::OffsetDateTime::now_utc(),
        )
    }

    /// Clock-injected decision application; events and derived rows share one timestamp.
    pub fn decision_resolve_apply_tx_at(
        tx: &rusqlite::Connection,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        now: time::OffsetDateTime,
    ) -> Result<bool, StoreError> {
        match Self::decision_get_tx(tx, decision_id)? {
            Some(row) if row.status == expect && row.task_id == task_id => {}
            _ => return Ok(false),
        }
        for wu in &updated {
            Self::update_work_unit_tx(tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx_at(tx, task_id, ev, now)?;
        }
        Ok(true)
    }
}
