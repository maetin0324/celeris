use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use crate::execution_plan::{RunIndexRole, RunIndexStatus, RunRow};
use crate::model::TaskId;

use crate::store::query::parse_status;
use crate::store::{
    ExecutionMetricsLatestRun, ExecutionMetricsTaskRow, SqliteStore, StoreError, format_rfc3339,
    parse_rfc3339,
};

impl SqliteStore {
    /// ADR-0072 D5（Phase E2/E2b）: `runs` へ 1 行挿入する共通ロジック（`run_index_start` と
    /// `runs_replace` の両方から呼ぶ）。`tx` はトランザクションでも素の `Connection` でもよい
    /// （`Transaction: Deref<Target = Connection>`）。
    pub(in crate::store) fn insert_run_row_tx(
        tx: &Connection,
        row: &RunRow,
    ) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, adapter, \
             model, account, session_id, checkpoint_json, usage_json, metrics_json, \
             started_at, finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                row.run_id,
                row.task_id,
                row.work_unit_id,
                row.role.as_str(),
                row.seq,
                row.status.as_str(),
                row.adapter,
                row.model,
                row.account,
                row.session_id,
                row.checkpoint
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.usage.as_ref().map(serde_json::to_string).transpose()?,
                row.metrics
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.started_at,
                row.finished_at,
            ],
        )?;
        Ok(())
    }

    pub(in crate::store) fn row_to_run(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<RunRow, StoreError>> {
        let run_id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let work_unit_id: Option<String> = row.get(2)?;
        let role_s: String = row.get(3)?;
        let seq: u32 = row.get(4)?;
        let status_s: String = row.get(5)?;
        let adapter: Option<String> = row.get(6)?;
        let model: Option<String> = row.get(7)?;
        let account: Option<String> = row.get(8)?;
        let session_id: Option<String> = row.get(9)?;
        let checkpoint_json: Option<String> = row.get(10)?;
        let usage_json: Option<String> = row.get(11)?;
        let metrics_json: Option<String> = row.get(12)?;
        let started_at: String = row.get(13)?;
        let finished_at: Option<String> = row.get(14)?;
        Ok((|| -> Result<RunRow, StoreError> {
            let Some(role) = RunIndexRole::parse(&role_s) else {
                return Err(StoreError::Invalid(format!("invalid runs.role: {role_s}")));
            };
            let Some(status) = RunIndexStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid runs.status: {status_s}"
                )));
            };
            let checkpoint = checkpoint_json
                .map(|j| serde_json::from_str(&j))
                .transpose()?;
            let usage = usage_json.map(|j| serde_json::from_str(&j)).transpose()?;
            let metrics = metrics_json.map(|j| serde_json::from_str(&j)).transpose()?;
            Ok(RunRow {
                run_id,
                task_id,
                work_unit_id,
                role,
                seq,
                status,
                adapter,
                model,
                account,
                session_id,
                checkpoint,
                usage,
                metrics,
                started_at,
                finished_at,
            })
        })())
    }

    pub(in crate::store) fn run_index_start_impl(&self, row: RunRow) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::insert_run_row_tx(&conn, &row)
    }

    pub(in crate::store) fn run_index_finish_impl(
        &self,
        run_id: &str,
        status: RunIndexStatus,
        checkpoint: Option<crate::execution::Checkpoint>,
        usage: Option<crate::model::Usage>,
        metrics: Option<crate::model::RunMetrics>,
        finished_at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE runs SET status = ?1, checkpoint_json = ?2, usage_json = ?3, \
             metrics_json = ?4, finished_at = ?5 WHERE run_id = ?6",
            params![
                status.as_str(),
                checkpoint.as_ref().map(serde_json::to_string).transpose()?,
                usage.as_ref().map(serde_json::to_string).transpose()?,
                metrics.as_ref().map(serde_json::to_string).transpose()?,
                format_rfc3339(finished_at)?,
                run_id,
            ],
        )?;
        Ok(n > 0)
    }

    pub(in crate::store) fn run_index_get_impl(
        &self,
        run_id: &str,
    ) -> Result<Option<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE run_id = ?1",
                params![run_id],
                Self::row_to_run,
            )
            .optional()?
            .transpose()
        })
    }

    pub(in crate::store) fn runs_for_task_impl(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE task_id = ?1 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn runs_running_impl(&self) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE status = 'running' ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map([], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn close_runs_of_terminal_tasks_impl(
        &self,
    ) -> Result<Vec<(TaskId, String)>, StoreError> {
        let mut conn = self.lock()?;
        let stale: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT t.id, t.status FROM runs r JOIN tasks t ON t.id = r.task_id \
                 WHERE r.status = 'running' AND t.status IN ('done', 'failed', 'cancelled')",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        let mut closed = Vec::new();
        for (id, status) in stale {
            let task_id = Self::parse_id(&id)?;
            let status = parse_status(&status)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let runs = Self::close_open_runs_tx(&tx, task_id, status)?;
            tx.commit()?;
            closed.extend(runs.into_iter().map(|r| (task_id, r)));
        }
        Ok(closed)
    }

    pub(in crate::store) fn runs_for_work_unit_impl(
        &self,
        work_unit_id: &str,
    ) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE work_unit_id = ?1 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![work_unit_id], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn execution_metrics_task_rows_impl(
        &self,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ExecutionMetricsTaskRow>, StoreError> {
        let since_text = since.map(format_rfc3339).transpose()?;
        self.with_read_conn(|conn| {
            // Aggregate each child table before joining: raw joins multiply counts.
            let mut stmt = conn.prepare(
                "WITH eligible AS MATERIALIZED (\
                   SELECT id, status, genre, assignee, json, updated_at FROM tasks \
                   WHERE (?1 IS NULL OR julianday(updated_at) >= julianday(?1) - 1.0 / 86400000)), \
                 wu AS (\
                   SELECT w.task_id, SUM(CASE WHEN w.kind = 'repair' THEN 1 ELSE 0 END) repairs, \
                          SUM(w.continuations) continuations, SUM(w.retries) retries \
                   FROM work_units w JOIN eligible t ON t.id = w.task_id GROUP BY w.task_id), \
                 plans AS (\
                   SELECT p.task_id, SUM(CASE WHEN p.version > 1 THEN 1 ELSE 0 END) replans \
                   FROM execution_plans p JOIN eligible t ON t.id = p.task_id GROUP BY p.task_id), \
                 run_counts AS (\
                   SELECT r.task_id, COUNT(*) runs_count, \
                          SUM(CASE WHEN r.status = 'budget_exhausted' THEN 1 ELSE 0 END) budget_exhausted_runs \
                   FROM runs r JOIN eligible t ON t.id = r.task_id GROUP BY r.task_id) \
                 SELECT t.id, t.status, t.genre, t.assignee, json_extract(t.json, '$.routing'), \
                        COALESCE(wu.repairs, 0), COALESCE(plans.replans, 0), \
                        COALESCE(wu.continuations, 0), COALESCE(wu.retries, 0), \
                        COALESCE(run_counts.runs_count, 0), COALESCE(run_counts.budget_exhausted_runs, 0), \
                        latest.run_id, latest.role, latest.status, latest.adapter, latest.model, \
                        latest.metrics_json, latest.usage_json, \
                        t.updated_at, \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') IN \
                          ('execution_planned', 'work_unit_transitioned', 'worker_started', \
                           'worker_finished', 'transitioned', 'routing_decided') LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'worker_finished' AND \
                          json_extract(e.json, '$.end.type') = 'budget_exhausted' LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'transitioned' AND \
                          json_extract(e.json, '$.reason') IN \
                          ('continue', 'work_unit_retry', 'worker_error') LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'quota_estimated' LIMIT 1) \
                 FROM eligible t \
                 LEFT JOIN wu ON wu.task_id = t.id \
                 LEFT JOIN plans ON plans.task_id = t.id \
                 LEFT JOIN run_counts ON run_counts.task_id = t.id \
                 LEFT JOIN runs latest ON latest.run_id = (\
                   SELECT r.run_id FROM runs r WHERE r.task_id = t.id \
                   ORDER BY r.started_at DESC, r.run_id DESC LIMIT 1) \
                 ORDER BY t.id",
            )?;
            let rows = stmt.query_map(params![since_text], |row| {
                Ok((
                    row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?, row.get::<_, u32>(5)?,
                    row.get::<_, u32>(6)?, row.get::<_, u32>(7)?,
                    row.get::<_, u32>(8)?, row.get::<_, u32>(9)?,
                    row.get::<_, u32>(10)?, row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?, row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?, row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?, row.get::<_, Option<String>>(17)?,
                    row.get::<_, String>(18)?, row.get::<_, bool>(19)?, row.get::<_, bool>(20)?,
                    row.get::<_, bool>(21)?, row.get::<_, bool>(22)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, status, genre, assignee, routing_json, repairs, replans,
                    continuations, retries, runs_count, budget_exhausted_runs, run_id,
                    role, run_status, adapter, model, metrics_json, usage_json, updated_at,
                    has_execution_events, has_budget_events, has_transition_metrics,
                    has_quota_events) = row?;
                // SQLite julianday has millisecond resolution. Keep a 1 ms candidate margin in
                // SQL, then apply the original OffsetDateTime comparison exactly here.
                if let Some(since) = since && parse_rfc3339(&updated_at)? < since {
                    continue;
                }
                let latest_run = if let Some(run_id) = run_id {
                    let role = role.and_then(|s| RunIndexRole::parse(&s)).ok_or_else(|| {
                        StoreError::Invalid(format!("invalid latest runs.role for {run_id}"))
                    })?;
                    let status = run_status
                        .and_then(|s| RunIndexStatus::parse(&s))
                        .ok_or_else(|| StoreError::Invalid(format!("invalid latest runs.status for {run_id}")))?;
                    Some(ExecutionMetricsLatestRun {
                        run_id, role, status, adapter, model, metrics_json, usage_json,
                    })
                } else {
                    None
                };
                out.push(ExecutionMetricsTaskRow {
                    task_id: id.parse().map_err(|_| StoreError::Invalid(format!("invalid tasks.id: {id}")))?,
                    status: parse_status(&status)?, genre, assignee, routing_json,
                    repairs, replans, continuations, retries, runs_count,
                    budget_exhausted_runs, latest_run, has_execution_events, has_budget_events,
                    has_transition_metrics, has_quota_events,
                });
            }
            Ok(out)
        })
    }

    pub(in crate::store) fn runs_replace_impl(
        &self,
        task_id: TaskId,
        rows: Vec<RunRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM runs WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for r in &rows {
            Self::insert_run_row_tx(&tx, r)?;
        }
        tx.commit()?;
        Ok(())
    }
}
