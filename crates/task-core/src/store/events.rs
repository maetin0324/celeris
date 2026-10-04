use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::BTreeMap;
use time::OffsetDateTime;

use crate::execution_plan::{RunIndexRole, RunIndexStatus};
use crate::model::{Event, Status, TaskId};

use super::query::{u64_to_i64, usize_to_i64};
use super::{EventRow, SqliteStore, StoreError, TaskWithEvents, format_rfc3339, status_str};

/// ADR-0121 付記: migration 0037 の部分 index `idx_events_delivery_skipped` の WHERE 式と
/// 字句まで同じにすること（でなければ SQLite がこの index を使わず events 全件を scan する）。
pub(super) const DELIVERY_SKIPPED_PREDICATE: &str =
    "json_extract(json,'$.type')='delivery_skipped'";

/// `latest_delivery_skipped_rows_impl` の問い合わせそのもの。試験の EXPLAIN QUERY PLAN と
/// 同じ文字列を使うため、ここに切り出す。
pub(super) fn latest_delivery_skipped_sql() -> String {
    format!(
        "SELECT e.id, e.task_id, e.seq, e.ts, e.json FROM events e \
         JOIN (SELECT task_id, MAX(seq) AS seq FROM events \
               WHERE {DELIVERY_SKIPPED_PREDICATE} GROUP BY task_id) latest \
           ON latest.task_id = e.task_id AND latest.seq = e.seq \
         ORDER BY e.id ASC"
    )
}

use crate::integration_request::{DELIVERY_ORIGIN, SUPERSEDED_ANSWER, TASK_TERMINAL_ANSWER};

/// migration 0047 と字句まで同じ式を使う。
pub(super) const INTEGRATION_REQUEST_PREDICATE: &str =
    "json_extract(json,'$.type') IN ('integration_requested', 'integration_answered')";

pub(super) fn integration_request_rows_sql() -> String {
    format!(
        "SELECT id, task_id, seq, ts, json FROM events \
         WHERE {INTEGRATION_REQUEST_PREDICATE} \
         ORDER BY task_id, seq ASC"
    )
}

fn integration_request_rows_for_task_sql() -> String {
    format!(
        "SELECT id, task_id, seq, ts, json FROM events \
         WHERE {INTEGRATION_REQUEST_PREDICATE} AND task_id = ?1 \
         ORDER BY task_id, seq ASC"
    )
}

fn fold_open_integration_requests(rows: Vec<EventRow>) -> Vec<EventRow> {
    let mut open = BTreeMap::<(TaskId, String), EventRow>::new();
    for row in rows {
        let key = match &row.event {
            Event::IntegrationRequested { request, .. } => {
                (row.task_id, request.id_for(row.task_id))
            }
            Event::IntegrationAnswered { request_id, .. } => {
                open.remove(&(row.task_id, request_id.clone()));
                continue;
            }
            _ => continue,
        };
        // If an older writer appended the same pair again, the latest request wins.
        open.insert(key, row);
    }
    let mut rows: Vec<_> = open.into_values().collect();
    rows.sort_by_key(|row| row.id);
    rows
}

impl SqliteStore {
    fn integration_request_rows_tx(
        conn: &Connection,
        task_id: Option<TaskId>,
    ) -> Result<Vec<EventRow>, StoreError> {
        let sql = if task_id.is_some() {
            integration_request_rows_for_task_sql()
        } else {
            integration_request_rows_sql()
        };
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = if let Some(id) = task_id {
            stmt.query(params![id.to_string()])?
        } else {
            stmt.query([])?
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let task_id: String = row.get(1)?;
            let seq: i64 = row.get(2)?;
            let ts: String = row.get(3)?;
            let json: String = row.get(4)?;
            out.push(EventRow {
                id: id as u64,
                task_id: Self::parse_id(&task_id)?,
                seq: seq as u64,
                ts,
                event: serde_json::from_str(&json)?,
            });
        }
        Ok(out)
    }

    pub(super) fn open_integration_requests_impl(&self) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(fold_open_integration_requests(
                Self::integration_request_rows_tx(conn, None)?,
            ))
        })
    }

    pub(super) fn integration_request_record_impl(
        &self,
        task_id: TaskId,
        request: &crate::integration_request::IntegrationRequest,
        origin: &str,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let open =
            fold_open_integration_requests(Self::integration_request_rows_tx(&tx, Some(task_id))?);
        if open.iter().any(|row| {
            matches!(&row.event, Event::IntegrationRequested { request: existing, .. }
                if existing.source_sha == request.source_sha && existing.target_sha == request.target_sha)
        }) {
            return Ok(false);
        }
        // 同じ発生元の古い組（両端の head が動いた後の依頼）はこの依頼に置き換わった。受信箱に二つ出さない。
        let new_id = request.id_for(task_id);
        for row in &open {
            let Event::IntegrationRequested {
                request: existing,
                origin: existing_origin,
            } = &row.event
            else {
                continue;
            };
            if existing_origin != origin {
                continue;
            }
            Self::append_event_tx(
                &tx,
                task_id,
                &Event::IntegrationAnswered {
                    request_id: existing.id_for(task_id),
                    answer: SUPERSEDED_ANSWER.to_owned(),
                    note: Some(format!("新しい依頼 {new_id} に置き換わった")),
                },
            )?;
        }
        Self::append_event_tx(
            &tx,
            task_id,
            &Event::IntegrationRequested {
                request: Box::new(request.clone()),
                origin: origin.to_owned(),
            },
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn integration_request_answer_impl(
        &self,
        task_id: TaskId,
        request_id: &str,
        answer: &str,
        note: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let open =
            fold_open_integration_requests(Self::integration_request_rows_tx(&tx, Some(task_id))?);
        let is_open = open.iter().any(|row| {
            matches!(&row.event, Event::IntegrationRequested { request, .. }
                if request.id_for(task_id) == request_id)
        });
        if !is_open {
            return Ok(false);
        }
        Self::append_event_tx(
            &tx,
            task_id,
            &Event::IntegrationAnswered {
                request_id: request_id.to_owned(),
                answer: answer.to_owned(),
                note: note.map(str::to_owned),
            },
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn integration_requests_close_impl(
        &self,
        task_id: TaskId,
        origin: &str,
        answer: &str,
        note: Option<&str>,
    ) -> Result<Vec<String>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let open =
            fold_open_integration_requests(Self::integration_request_rows_tx(&tx, Some(task_id))?);
        let mut closed = Vec::new();
        for row in &open {
            let Event::IntegrationRequested {
                request,
                origin: existing_origin,
            } = &row.event
            else {
                continue;
            };
            if existing_origin != origin {
                continue;
            }
            let request_id = request.id_for(task_id);
            Self::append_event_tx(
                &tx,
                task_id,
                &Event::IntegrationAnswered {
                    request_id: request_id.clone(),
                    answer: answer.to_owned(),
                    note: note.map(str::to_owned),
                },
            )?;
            closed.push(request_id);
        }
        if !closed.is_empty() {
            tx.commit()?;
        }
        Ok(closed)
    }

    fn close_terminal_integration_request_tx(
        conn: &Connection,
        task_id: TaskId,
        request_id: String,
    ) -> Result<(), StoreError> {
        Self::append_event_tx(
            conn,
            task_id,
            &Event::IntegrationAnswered {
                request_id,
                answer: TASK_TERMINAL_ANSWER.to_owned(),
                note: None,
            },
        )?;
        Ok(())
    }

    /// 終端遷移と同じトランザクションで、配送（origin `delivery`）以外の未回答依頼だけを閉じる。
    /// 配送の依頼は done の task の上で人の判断を待つ正当な依頼なので残す。
    pub(super) fn close_open_integration_requests_tx(
        conn: &Connection,
        task_id: TaskId,
    ) -> Result<(), StoreError> {
        for row in
            fold_open_integration_requests(Self::integration_request_rows_tx(conn, Some(task_id))?)
        {
            if let Event::IntegrationRequested { request, origin } = row.event
                && origin != DELIVERY_ORIGIN
            {
                Self::close_terminal_integration_request_tx(
                    conn,
                    task_id,
                    request.id_for(task_id),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn close_integration_requests_of_terminal_tasks_impl(
        &self,
    ) -> Result<Vec<(TaskId, String)>, StoreError> {
        let mut conn = self.lock()?;
        // 読み取りから追記まで同じ書き込みトランザクション。並行する回答や再開と競合しない。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let open = fold_open_integration_requests(Self::integration_request_rows_tx(&tx, None)?);
        let mut closed = Vec::new();
        for row in open {
            let terminal: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ?1 AND status IN ('done', 'cancelled', 'failed'))",
                params![row.task_id.to_string()],
                |r| r.get(0),
            )?;
            if terminal
                && let Event::IntegrationRequested { request, origin } = row.event
                && origin != DELIVERY_ORIGIN
            {
                let request_id = request.id_for(row.task_id);
                Self::close_terminal_integration_request_tx(&tx, row.task_id, request_id.clone())?;
                closed.push((row.task_id, request_id));
            }
        }
        tx.commit()?;
        Ok(closed)
    }

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
        crate::cluster_job::apply_event_tx(conn, task_id, event, &ts)?;
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

    /// ADR-0079 付記「R6-1」D4: `task_id` の `runs` 索引の `running` の行を、`WorkerFinished{end: Cancelled,
    /// outcome: "interrupted: …"}` を積んで閉じる（索引は events の派生なので、replay が同じ行を作れるよう行の
    /// 書き換えではなく event で閉じる。`append_event_tx` が `close_run_row_for_event_tx` で行を更新する）。
    /// 閉じた run の id を返す。終端への遷移（`apply_transition_tx`）と、終端の task の行の照合
    /// （[`TaskStore::close_runs_of_terminal_tasks`](super::TaskStore::close_runs_of_terminal_tasks)）が使う。
    pub(super) fn close_open_runs_tx(
        conn: &Connection,
        task_id: TaskId,
        status: Status,
    ) -> Result<Vec<String>, StoreError> {
        let open: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT run_id, role FROM runs WHERE task_id = ?1 AND status = 'running' \
                 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        let mut closed = Vec::new();
        for (run_id, role) in open {
            let role = match RunIndexRole::parse(&role) {
                Some(RunIndexRole::Reviewer) => Some(crate::model::RunRole::Reviewer),
                Some(RunIndexRole::Planner) => Some(crate::model::RunRole::Planner),
                _ => None,
            };
            let finished = Event::WorkerFinished {
                run_id: run_id.clone(),
                outcome: format!(
                    "interrupted: the task reached {} while this run was still open (runs index closed, ADR-0079 R6-1)",
                    status_str(status)
                ),
                usage: None,
                role,
                metrics: None,
                end: Some(crate::execution::RunEnd::Cancelled),
            };
            Self::append_event_tx(conn, task_id, &finished)?;
            closed.push(run_id);
        }
        Ok(closed)
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

    pub(super) fn append_event_impl(
        &self,
        task_id: TaskId,
        event: &Event,
    ) -> Result<u64, StoreError> {
        let mut conn = self.lock()?;
        // seq の採番（SELECT）と INSERT を 1 つの IMMEDIATE トランザクションにする。別接続（celerisctl / API）が同じタスクに
        // 追記しても seq が衝突せず、書き込みロックは busy_timeout で待つ（Phase 9 監査）。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq = Self::append_event_tx(&tx, task_id, event)?;
        tx.commit()?;
        Ok(seq)
    }

    pub(super) fn events_for_impl(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT seq, json FROM events WHERE task_id = ?1 ORDER BY seq ASC")?;
            let rows = stmt.query_map(params![task_id.to_string()], |row| {
                let seq: i64 = row.get(0)?;
                let json: String = row.get(1)?;
                Ok((seq, json))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (seq, json) = row?;
                let event: Event = serde_json::from_str(&json)?;
                events.push((seq as u64, event));
            }
            Ok(events)
        })
    }

    pub(super) fn tasks_with_events_impl(&self) -> Result<Vec<TaskWithEvents>, StoreError> {
        self.with_read_conn(|conn| {
            // 1 つの読み取りトランザクション（WAL のスナップショット）で tasks と events を読む。
            let tx = conn.unchecked_transaction()?;
            let mut tasks = Vec::new();
            {
                let mut stmt = tx.prepare("SELECT json FROM tasks")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for row in rows {
                    tasks.push(Self::row_to_task(row?)?);
                }
            }
            let mut out = Vec::with_capacity(tasks.len());
            {
                let mut stmt =
                    tx.prepare("SELECT seq, json FROM events WHERE task_id = ?1 ORDER BY seq ASC")?;
                for task in tasks {
                    let rows = stmt.query_map(params![task.id.to_string()], |row| {
                        let seq: i64 = row.get(0)?;
                        let json: String = row.get(1)?;
                        Ok((seq, json))
                    })?;
                    let mut events = Vec::new();
                    for row in rows {
                        let (seq, json) = row?;
                        let event: Event = serde_json::from_str(&json)?;
                        events.push((seq as u64, event));
                    }
                    out.push((task, events));
                }
            }
            tx.finish()?;
            Ok(out)
        })
    }

    pub(super) fn events_for_with_global_ids_impl(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<(u64, Event)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, json FROM events WHERE task_id = ?1 ORDER BY id ASC")?;
            let rows = stmt.query_map(params![task_id.to_string()], |row| {
                let id: i64 = row.get(0)?;
                let json: String = row.get(1)?;
                Ok((id, json))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (id, json) = row?;
                let event: Event = serde_json::from_str(&json)?;
                events.push((id as u64, event));
            }
            Ok(events)
        })
    }

    pub(super) fn events_since_impl(
        &self,
        after_id: u64,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, seq, ts, json FROM events WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
            )?;
            let rows =
                stmt.query_map(params![u64_to_i64(after_id), usize_to_i64(limit)], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, task_id, seq, ts, json) = row?;
                let task_id = Self::parse_id(&task_id)?;
                let event: Event = serde_json::from_str(&json)?;
                out.push(EventRow {
                    id: id as u64,
                    task_id,
                    seq: seq as u64,
                    ts,
                    event,
                });
            }
            Ok(out)
        })
    }

    pub(super) fn latest_event_id_impl(&self) -> Result<u64, StoreError> {
        self.with_read_conn(|conn| {
            let id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
                row.get(0)
            })?;
            Ok(id as u64)
        })
    }

    pub(super) fn event_rows_for_impl(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, seq, ts, json FROM events WHERE task_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            )?;
            let after: i64 = after_seq.map(u64_to_i64).unwrap_or(-1);
            let rows = stmt.query_map(
                params![task_id.to_string(), after, usize_to_i64(limit)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )?;
            let mut out = Vec::new();
            for row in rows {
                let (id, seq, ts, json) = row?;
                let event: Event = serde_json::from_str(&json)?;
                out.push(EventRow {
                    id: id as u64,
                    task_id,
                    seq: seq as u64,
                    ts,
                    event,
                });
            }
            Ok(out)
        })
    }

    pub(super) fn latest_delivery_skipped_rows_impl(&self) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&latest_delivery_skipped_sql())?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, task_id, seq, ts, json) = row?;
                out.push(EventRow {
                    id: id as u64,
                    task_id: Self::parse_id(&task_id)?,
                    seq: seq as u64,
                    ts,
                    event: serde_json::from_str(&json)?,
                });
            }
            Ok(out)
        })
    }
}
