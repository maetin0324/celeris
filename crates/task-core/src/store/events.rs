use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use crate::execution_plan::RunIndexStatus;
use crate::model::{Event, TaskId};

use super::{SqliteStore, StoreError, format_rfc3339};

impl SqliteStore {
    pub(crate) fn append_event_tx(
        conn: &Connection,
        task_id: TaskId,
        event: &Event,
    ) -> Result<u64, StoreError> {
        let next_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        let json = serde_json::to_string(event)?;
        conn.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
            params![task_id.to_string(), next_seq, ts, json],
        )?;
        Self::close_run_row_for_event_tx(conn, event, &ts)?;
        Self::apply_tree_event_tx(conn, task_id, event, &ts)?;
        Ok(next_seq as u64)
    }

    /// ADR-0079 D4 (4) / D7 / D15（Phase R1a）: 木の Event の派生の書き込み（Event と同じトランザクション）。
    /// `ChildTaskCreated` / `ChildAdopted` → `work_units.child_task_id`、`DecisionRequested` /
    /// `DecisionAnswered` / `DecisionWithdrawn` → `decisions`。畳み込みは `task_core::decision::DecisionRow`
    /// の関数を通すので、`task_ops::replay::rebuild_decisions` の再構築と同じ行になる。
    pub(super) fn apply_tree_event_tx(
        conn: &Connection,
        task_id: TaskId,
        event: &Event,
        ts: &str,
    ) -> Result<(), StoreError> {
        match event {
            Event::ChildTaskCreated {
                unit_key,
                child_task_id,
                ..
            }
            | Event::ChildAdopted {
                unit_key,
                child_task_id,
                ..
            } => {
                conn.execute(
                    "UPDATE work_units SET child_task_id = ?1 WHERE task_id = ?2 AND key = ?3",
                    params![child_task_id.to_string(), task_id.to_string(), unit_key],
                )?;
            }
            Event::DecisionRequested { decision } => {
                let row = crate::decision::DecisionRow::from_request(task_id, decision, ts);
                Self::insert_decision_tx(conn, &row)?;
            }
            Event::DecisionAnswered {
                id,
                option,
                note,
                by,
            } => {
                if let Some(mut row) = Self::decision_get_tx(conn, id)? {
                    row.apply_answer(option, note.as_deref(), by, ts);
                    Self::update_decision_tx(conn, &row)?;
                }
            }
            Event::DecisionWithdrawn { id, reason } => {
                if let Some(mut row) = Self::decision_get_tx(conn, id)? {
                    row.apply_withdrawal(reason);
                    Self::update_decision_tx(conn, &row)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn insert_decision_tx(
        conn: &Connection,
        row: &crate::decision::DecisionRow,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO decisions (id, root_id, task_id, key, kind, status, needed_before_json, \
             json, created_at, answered_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                row.id,
                row.root_id.to_string(),
                row.task_id.to_string(),
                row.key,
                row.kind.as_str(),
                row.status.as_str(),
                serde_json::to_string(&row.needed_before)?,
                serde_json::to_string(&row.request)?,
                row.created_at,
                row.answered_at,
            ],
        )?;
        Ok(())
    }

    pub(super) fn update_decision_tx(
        conn: &Connection,
        row: &crate::decision::DecisionRow,
    ) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE decisions SET status = ?1, json = ?2, answered_at = ?3 WHERE id = ?4",
            params![
                row.status.as_str(),
                serde_json::to_string(&row.request)?,
                row.answered_at,
                row.id,
            ],
        )?;
        Ok(())
    }

    pub(super) const DECISION_COLUMNS: &'static str = "id, root_id, task_id, key, kind, status, \
        needed_before_json, json, created_at, answered_at";

    pub(super) fn decision_get_tx(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        let sql = format!(
            "SELECT {} FROM decisions WHERE id = ?1",
            Self::DECISION_COLUMNS
        );
        conn.query_row(&sql, params![id], Self::row_to_decision)
            .optional()?
            .transpose()
    }

    pub(super) fn row_to_decision(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<crate::decision::DecisionRow, StoreError>> {
        let id: String = row.get(0)?;
        let root_id: String = row.get(1)?;
        let task_id: String = row.get(2)?;
        let key: String = row.get(3)?;
        let kind_s: String = row.get(4)?;
        let status_s: String = row.get(5)?;
        let needed_before_json: String = row.get(6)?;
        let json: String = row.get(7)?;
        let created_at: String = row.get(8)?;
        let answered_at: Option<String> = row.get(9)?;
        Ok((|| -> Result<crate::decision::DecisionRow, StoreError> {
            let Some(kind) = crate::decision::DecisionKind::parse(&kind_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid decisions.kind: {kind_s}"
                )));
            };
            let Some(status) = crate::decision::DecisionStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid decisions.status: {status_s}"
                )));
            };
            Ok(crate::decision::DecisionRow {
                id,
                root_id: Self::parse_id(&root_id)?,
                task_id: Self::parse_id(&task_id)?,
                key,
                kind,
                status,
                needed_before: serde_json::from_str(&needed_before_json)?,
                request: serde_json::from_str(&json)?,
                created_at,
                answered_at,
            })
        })())
    }

    /// Phase F5-fix3: `WorkerFinished` を書いたら、その run の `runs` 行がまだ `running` なら同じ
    /// トランザクションで終端にする（status は `end` から、無ければ `harness_error`。`finished_at` は
    /// event の ts）。`task_ops::replay::rebuild_work_units_and_runs` と同じ規則なので `replay --check` と
    /// 食い違わない。既に `run_index_finish` で閉じた行（checkpoint 等を持つ）には触れない。
    /// dogfood 4 回目、lease 失効の requeue（`reclaim_expired_leases`）は `WorkerFinished` だけを書き、
    /// `runs` 行は `running` のまま残っていた。どの経路で run を終えても行が閉じるよう、ここで保証する。
    pub(super) fn close_run_row_for_event_tx(
        conn: &Connection,
        event: &Event,
        ts: &str,
    ) -> Result<(), StoreError> {
        let Event::WorkerFinished {
            run_id,
            usage,
            metrics,
            end,
            ..
        } = event
        else {
            return Ok(());
        };
        let status = end
            .map(RunIndexStatus::from_run_end)
            .unwrap_or(RunIndexStatus::HarnessError);
        conn.execute(
            "UPDATE runs SET status = ?1, usage_json = COALESCE(?2, usage_json), \
             metrics_json = COALESCE(?3, metrics_json), finished_at = ?4 \
             WHERE run_id = ?5 AND status = 'running'",
            params![
                status.as_str(),
                usage.as_ref().map(serde_json::to_string).transpose()?,
                metrics.as_ref().map(serde_json::to_string).transpose()?,
                ts,
                run_id,
            ],
        )?;
        Ok(())
    }
}
