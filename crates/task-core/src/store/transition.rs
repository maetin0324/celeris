use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use crate::message::is_conversation;
use crate::model::{Event, Status, Task, TaskId, TaskKind};
use crate::transition::{Outcome, StateView, Trigger, transition};

use super::{SqliteStore, StoreError, format_rfc3339, kind_str, status_str};

impl SqliteStore {
    /// `apply_transition_with_events` の本体（ADR-0004 D1 / ADR-0005 D4）。`tx` 内で任意のトリガーを
    /// 検証し、tasks の更新と Event::Transitioned (+ extra_events) の追記を行う。commit は呼び出し側。
    pub(crate) fn apply_transition_tx(
        tx: &Connection,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        // ADR-0004 D1 / ADR-0005 D4: 任意のトリガーを検証し、tasks の更新と
        // Event::Transitioned (+ extra_events) の追記を単一トランザクションで行う。

        let json: Option<String> = tx
            .query_row(
                "SELECT json FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let json = match json {
            Some(j) => j,
            None => return Err(StoreError::Invalid(format!("task not found: {task_id}"))),
        };
        let mut task = Self::row_to_task(json)?;

        let view = StateView {
            kind: task.kind,
            status: task.status,
            attempts: task.attempts,
            max_retries: task.budget.max_retries,
        };
        let outcome = transition(&view, &trigger)?;
        // ADR-0080 D4: browser の wait が未解決の間は、一般の回答・途中確認の再開で `ready` に戻さない
        // （解除は専用の browser 操作〈`BrowserResume` / `BrowserFail`〉だけ）。
        if matches!(trigger, Trigger::Answer | Trigger::PhaseResume { .. })
            && crate::browser_wait::has_pending_wait_tx(tx, task_id)?
        {
            return Err(StoreError::InvalidTransition(
                crate::transition::InvalidTransition {
                    status: view.status,
                    kind: view.kind,
                    trigger: "browser_wait_pending",
                },
            ));
        }
        // ADR-0090 D2: クラスタ job の wait が `waiting` の間は、一般の回答・途中確認の再開で `ready` に戻さない
        // （再開は daemon の poll〈`ClusterJobResume`〉、上限切れの後は人の回答）。
        if matches!(trigger, Trigger::Answer | Trigger::PhaseResume { .. })
            && view.status == Status::Blocked
            && crate::cluster_job::has_waiting_tx(tx, task_id)?
        {
            return Err(StoreError::InvalidTransition(
                crate::transition::InvalidTransition {
                    status: view.status,
                    kind: view.kind,
                    trigger: "cluster_job_wait_pending",
                },
            ));
        }

        let now = OffsetDateTime::now_utc();
        // ADR-0002 D1: running から出る全遷移でリースを解放する。
        let leaving_running = view.status == Status::Running && outcome.next != Status::Running;
        task.status = outcome.next;
        task.attempts = outcome.attempts;
        task.updated_at = now;
        if leaving_running {
            task.lease = None;
        }

        let new_json = serde_json::to_string(&task)?;
        let updated_at_str = format_rfc3339(task.updated_at)?;
        if leaving_running {
            tx.execute(
                "UPDATE tasks SET status = ?1, lease_worker_run_id = NULL, \
                 lease_expires_at = NULL, json = ?2, title = ?3, updated_at = ?4 WHERE id = ?5",
                params![
                    status_str(task.status),
                    new_json,
                    task.title,
                    updated_at_str,
                    task_id.to_string()
                ],
            )?;
        } else {
            tx.execute(
                "UPDATE tasks SET status = ?1, json = ?2, title = ?3, updated_at = ?4 WHERE id = ?5",
                params![status_str(task.status), new_json, task.title, updated_at_str, task_id.to_string()],
            )?;
        }

        let mut next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;

        let transitioned = Event::Transitioned {
            from: view.status,
            to: outcome.next,
            reason: outcome.reason.to_string(),
        };
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        tx.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                task_id.to_string(),
                next_seq,
                ts,
                serde_json::to_string(&transitioned)?
            ],
        )?;
        next_seq += 1;

        for event in extra_events {
            let ts = format_rfc3339(OffsetDateTime::now_utc())?;
            tx.execute(
                "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    task_id.to_string(),
                    next_seq,
                    ts,
                    serde_json::to_string(&event)?
                ],
            )?;
            Self::close_run_row_for_event_tx(tx, &event, &ts)?;
            Self::apply_tree_event_tx(tx, task_id, &event, &ts)?;
            crate::cluster_job::apply_event_tx(tx, task_id, &event, &ts)?;
            next_seq += 1;
        }

        // Phase F7（ADR-0033 D5 追記 2026-09-28）: 終端になったら、このタスクの未決の認可の要求を
        // 同じトランザクションで `withdrawn` に閉じる（答える相手がいない要求を一覧・バッジに残さない）。
        // 伝播（子・後続の cancel）もこの関数を通るので、子の分も同じく閉じる。
        if !view.status.is_terminal() && outcome.next.is_terminal() {
            crate::approval::withdraw_pending_for_task_tx(
                tx,
                task_id,
                outcome.next,
                crate::approval::WITHDRAWN_BY_TRANSITION,
                now,
            )?;
        }
        // ADR-0080 D4: 終端（cancel・連鎖の中止を含む）になったら、未解決の browser wait を
        // `cancelled` に閉じる（未消費の承認も同時に失効する）。
        if !view.status.is_terminal() && outcome.next.is_terminal() {
            crate::browser_wait::cancel_open_for_task_tx(tx, task_id, now)?;
            // ADR-0090 D6: クラスタ job の wait も `cancelled` に閉じる（job は qdel しない）。
            crate::cluster_job::cancel_open_for_task_tx(tx, task_id)?;
        }
        // ADR-0079 付記「R6-1」D4: 終端になったら、この task の `runs` 索引の `running` の行を同じトランザクションで
        // 閉じる（本番: 失敗した task 01M3PBAVFAYPDWMQMDBXPTE2V8 の reviewer run が `running` のまま残った）。
        if !view.status.is_terminal() && outcome.next.is_terminal() {
            Self::close_open_runs_tx(tx, task_id, outcome.next)?;
        }

        Self::cascade_after_transition_tx(tx, &task, view.status, outcome.next)?;

        Ok(outcome)
    }

    /// 終端化に伴う伝播（ADR-0010 D2）。呼び出し元と同一トランザクションで、再帰的に行う。
    /// 1. `Approval` が failed/cancelled → 終端でない直接の子を `Cancel`（ADR-0008 D1 を reject 以外にも拡張）
    /// 2. `Approval` 以外が終端 → 終端でない直接の `Approval` 子を `Cancel`（P-37）
    /// 3. failed/cancelled → 終端でない後続（`depends_on` に含むタスク）を `DependencyFailed`（P-9、推移的）
    pub(super) fn cascade_after_transition_tx(
        tx: &Connection,
        task: &Task,
        from: Status,
        to: Status,
    ) -> Result<(), StoreError> {
        if from.is_terminal() || !to.is_terminal() {
            return Ok(());
        }
        let unsuccessful = matches!(to, Status::Failed | Status::Cancelled);
        if task.kind == TaskKind::Approval {
            if unsuccessful {
                for child in Self::non_terminal_children_tx(tx, task.id, None)? {
                    Self::transition_if_non_terminal_tx(tx, child, Trigger::Cancel)?;
                }
            }
        } else {
            for child in Self::non_terminal_children_tx(tx, task.id, Some(TaskKind::Approval))? {
                Self::transition_if_non_terminal_tx(tx, child, Trigger::Cancel)?;
            }
        }
        // ADR-0079 §7 R1b（D4・D16）: 木の親が中止・失敗で終わったら、親の計画の unit から作った子 task
        // （`root_id` を持つ `parent_id = 親` の task）を同じトランザクションで中止する。子の遷移がさらに
        // 孫へ連鎖する（subtree 全体）。走っている run は dispatcher の `abort_stale_runs` が止める。
        if unsuccessful {
            for child in Self::non_terminal_tree_children_tx(tx, task.id)? {
                Self::transition_if_non_terminal_tx(tx, child, Trigger::ParentCancelled)?;
            }
        }
        if unsuccessful {
            for dependent in Self::non_terminal_dependents_tx(tx, task.id)? {
                // P-78（ADR-0033 D4 / Phase 28）: 対話タスクは `DependencyFailed` の対象から外す
                // （直列化の順番待ちだけなので、前の対話タスクの失敗を理由に次を `cancelled` にしない。
                // `ready_tasks` 側が「終端に達していれば進めてよい」を見る）。
                if let Some(dep_task) = Self::get_locked(tx, dependent)?
                    && is_conversation(&dep_task)
                {
                    continue;
                }
                Self::transition_if_non_terminal_tx(tx, dependent, Trigger::DependencyFailed)?;
            }
        }
        Ok(())
    }

    /// 伝播の途中で既に終端になったタスク（例: 子でもあり後続でもある）は飛ばす。
    pub(super) fn transition_if_non_terminal_tx(
        tx: &Connection,
        id: TaskId,
        trigger: Trigger,
    ) -> Result<(), StoreError> {
        match Self::get_locked(tx, id)? {
            Some(t) if !t.status.is_terminal() => {
                Self::apply_transition_tx(tx, id, trigger, vec![])?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// `parent_id` の直接の子のうち終端でないもの（`kind` 指定があればその kind だけ）。
    pub(super) fn non_terminal_children_tx(
        tx: &Connection,
        parent_id: TaskId,
        kind: Option<TaskKind>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT id FROM tasks WHERE parent_id = ?1 AND {} AND (?2 IS NULL OR kind = ?2)",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![parent_id.to_string(), kind.map(kind_str)], |row| {
            row.get::<_, String>(0)
        })?;
        let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
        ids.iter().map(|s| Self::parse_id(s)).collect()
    }

    /// ADR-0079（Phase R1b）: `parent_id` の木の子（`root_id` を持つ = 計画の unit から作った子）のうち
    /// 終端でないもの。
    pub(super) fn non_terminal_tree_children_tx(
        tx: &Connection,
        parent_id: TaskId,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT id FROM tasks WHERE parent_id = ?1 AND root_id IS NOT NULL AND {} \
             ORDER BY created_at ASC, rowid ASC",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![parent_id.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
        ids.iter().map(|s| Self::parse_id(s)).collect()
    }

    /// `depends_on` に `dep_id` を含む、終端でないタスク。JSON 列を `LIKE` で絞ってから型で確認する。
    pub(super) fn non_terminal_dependents_tx(
        tx: &Connection,
        dep_id: TaskId,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT json FROM tasks WHERE {} AND json LIKE ?1",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![format!("%{dep_id}%")], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let t = Self::row_to_task(row?)?;
            if t.id != dep_id && t.depends_on.contains(&dep_id) {
                out.push(t.id);
            }
        }
        Ok(out)
    }

    /// `depends_on` に `dep_id` を含むタスク（状態を問わない）。Phase 31（やり直し）が使う。
    pub(super) fn dependents_of_tx(
        tx: &Connection,
        dep_id: TaskId,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut stmt = tx.prepare("SELECT json FROM tasks WHERE json LIKE ?1")?;
        let rows = stmt.query_map(params![format!("%{dep_id}%")], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let t = Self::row_to_task(row?)?;
            if t.id != dep_id && t.depends_on.contains(&dep_id) {
                out.push(t.id);
            }
        }
        Ok(out)
    }

    /// `task_id` の最後の `Event::Transitioned` の `reason`。無ければ `None`。Phase 31 が「`cancelled` が
    /// `dependency_failed` 由来か」を見分けるのに使う。
    pub(super) fn last_transitioned_reason_tx(
        tx: &Connection,
        task_id: TaskId,
    ) -> Result<Option<String>, StoreError> {
        let mut stmt = tx.prepare(
            "SELECT json FROM events WHERE task_id = ?1 AND json LIKE '%\"type\":\"transitioned\"%' ORDER BY seq DESC LIMIT 1",
        )?;
        let json: Option<String> = stmt
            .query_row(params![task_id.to_string()], |row| row.get(0))
            .optional()?;
        let Some(json) = json else {
            return Ok(None);
        };
        let event: Event = serde_json::from_str(&json)?;
        match event {
            Event::Transitioned { reason, .. } => Ok(Some(reason)),
            _ => Ok(None),
        }
    }
}
