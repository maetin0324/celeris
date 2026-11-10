use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;

use crate::message::is_conversation;
use crate::model::{Event, Status, Task, TaskId, TaskKind};
use crate::transition::{
    DependencyCancel, Outcome, StateView, Trigger, dependency_reopen_trigger, dependents_reopened,
    transition,
};

use super::{SqliteStore, StoreError, format_rfc3339, kind_str, status_str};

impl SqliteStore {
    /// The current status of a task read on `tx` (`None` when it does not exist), for cascades
    /// that re-check each task inside one caller-owned transaction.
    pub fn task_status_tx(tx: &Connection, task_id: TaskId) -> Result<Option<Status>, StoreError> {
        Ok(Self::get_locked(tx, task_id)?.map(|task| task.status))
    }

    /// `apply_transition_with_events` の本体（ADR-0004 D1 / ADR-0005 D4）。`tx` 内で任意のトリガーを
    /// 検証し、tasks の更新と Event::Transitioned (+ extra_events) の追記を行う。commit は呼び出し側。
    pub fn apply_transition_tx(
        tx: &Connection,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        // ADR-0004 D1 / ADR-0005 D4: 任意のトリガーを検証し、tasks の更新と
        // Event::Transitioned (+ extra_events) の追記を単一トランザクションで行う。

        let Some(mut task) = Self::get_locked(tx, task_id)? else {
            return Err(StoreError::Invalid(format!("task not found: {task_id}")));
        };

        let view = StateView {
            kind: task.kind,
            status: task.status,
            attempts: task.attempts,
            max_retries: task.budget.max_retries,
        };
        let outcome = transition(&view, &trigger)?;
        // ADR-0074 付記（2026-10-05）: 統合開始後の古い idle 判定で工程 lease を捨てない。
        // 人の Interrupt / Cancel は従来どおり許す（孤立統合は dispatcher が照合する）。
        if matches!(
            trigger,
            Trigger::Continue {
                why: crate::ContinueWhy::Advance
            }
        ) && tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM work_units WHERE task_id = ?1 \
                 AND kind = 'integrate' AND phase IS NOT NULL AND status = 'running')",
            params![task_id.to_string()],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(StoreError::InvalidTransition(
                crate::transition::InvalidTransition {
                    status: view.status,
                    kind: view.kind,
                    trigger: "integration_running",
                },
            ));
        }
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

        // 遷移の event と extra_events は `append_event_tx`（seq は同じ transaction の続き番号、派生の行も書く）で追記する。
        let transitioned = Event::Transitioned {
            from: view.status,
            to: outcome.next,
            reason: outcome.reason.to_string(),
        };
        for event in std::iter::once(transitioned).chain(extra_events) {
            Self::append_event_tx(tx, task_id, &event)?;
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
            // ADR-0080 D4: 終端（cancel・連鎖の中止を含む）になったら、未解決の browser wait を
            // `cancelled` に閉じる（未消費の承認も同時に失効する）。
            crate::browser_wait::cancel_open_for_task_tx(tx, task_id, now)?;
            // ADR-0090 D6: クラスタ job の wait も `cancelled` に閉じる（job は qdel しない）。
            crate::cluster_job::cancel_open_for_task_tx(tx, task_id)?;
            // ADR-0079 付記「R6-1」D4: 終端になったら、この task の `runs` 索引の `running` の行を同じトランザクションで
            // 閉じる（本番: 失敗した task 01M3PBAVFAYPDWMQMDBXPTE2V8 の reviewer run が `running` のまま残った）。
            Self::close_open_runs_tx(tx, task_id, outcome.next)?;
            // 終端 task に人の判断待ちを残さない。統合成功とは区別して task_terminal で閉じる。
            Self::close_open_integration_requests_tx(tx, task_id)?;
        }

        Self::cascade_after_transition_tx(tx, &task, view.status, outcome.next)?;

        Ok(outcome)
    }

    /// 終端化に伴う伝播（ADR-0010 D2）。呼び出し元と同一トランザクションで、再帰的に行う。
    /// 1. `Approval` が failed/cancelled → 終端でない直接の子を `Cancel`（ADR-0008 D1 を reject 以外にも拡張）
    /// 2. `Approval` 以外が終端 → 終端でない直接の `Approval` 子を `Cancel`（P-37）
    /// 3. failed/cancelled → 終端でない後続（`depends_on` に含むタスク）を `DependencyFailed`（P-9、推移的）
    /// 4. 終端 → 終端でない（reopen・rereview・後続の復帰）→ 3 で cancel された後続を `DependencyReopened` で戻す
    ///    （ADR 2026-11-08-health-enforcement-loop D7。判定は `transition::dependency_reopen_trigger`。戻した後続の
    ///    遷移もここを通るので推移的。戻すのは新しい `Transitioned` の追記だけで、cancel の event は変えない）。
    ///    retry の後継を持つ後続は戻さない（D7.2。後継は `retry_successors_tx` が引き、`retry_task_tx` の付け替えと
    ///    同じ述語・同じ入力で判定する）。戻した後続（restored）と、述語が `Kept` を返した後続（kept）を、この task の
    ///    events に `Event::DependentsReopened` として同じ transaction で追記する（D7.3。書くか・何を書くかは
    ///    `transition::dependents_reopened`。直前の記録は `last_event_tx` で 1 件だけ読む）
    pub(super) fn cascade_after_transition_tx(
        tx: &Connection,
        task: &Task,
        from: Status,
        to: Status,
    ) -> Result<(), StoreError> {
        if from.is_terminal() && !to.is_terminal() {
            let dependents = Self::dependents_of_tx(tx, task.id, false)?;
            let successors = retry_successors_tx(tx, &dependents)?;
            let mut verdicts = Vec::new();
            for dependent in dependents {
                let Some(dep) = Self::get_locked(tx, dependent)? else {
                    continue;
                };
                let last = Self::last_transitioned_tx(tx, dependent)?;
                let others = (dep.depends_on.iter().filter(|d| **d != task.id))
                    .map(|d| Ok((*d, Self::task_status_tx(tx, *d)?)))
                    .collect::<Result<Vec<_>, StoreError>>()?;
                let successor = successors.get(&dependent).copied();
                let verdict =
                    dependency_reopen_trigger(dep.status, last.as_ref(), successor, &others);
                if let DependencyCancel::Restore(trigger) = &verdict {
                    Self::apply_transition_tx(tx, dependent, trigger.clone(), vec![])?;
                }
                verdicts.push((dependent, dep.depends_on, verdict));
            }
            let previous = last_event_tx(tx, task.id, "dependents_reopened")?;
            if let Some(event) = dependents_reopened(from, verdicts, previous.as_ref()) {
                Self::append_event_tx(tx, task.id, &event)?;
            }
        }
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
            for dependent in Self::dependents_of_tx(tx, task.id, true)? {
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

    /// `depends_on` に `dep_id` を含むタスク（`non_terminal` なら終端でないものだけ）。JSON 列を `LIKE` で絞ってから
    /// 型で確認する。依存の cascade（失敗の伝播と reopen での復帰）と Phase 31（やり直しの付け替え）が使う。
    pub(super) fn dependents_of_tx(
        tx: &Connection,
        dep_id: TaskId,
        non_terminal: bool,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT json FROM tasks WHERE json LIKE ?1 AND (?2 = 0 OR {})",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![format!("%{dep_id}%"), non_terminal], |row| {
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

    /// `task_id` の最後の `Event::Transitioned` の `(from, reason)`。無ければ `None`。依存の cascade と Phase 31
    /// （`transition::cancelled_by_dependency`）、browser の前提 gate の判定が使う。読み出しは `last_event_tx`。
    pub(crate) fn last_transitioned_tx(
        tx: &Connection,
        task_id: TaskId,
    ) -> Result<Option<(Status, String)>, StoreError> {
        match last_event_tx(tx, task_id, "transitioned")? {
            Some(Event::Transitioned { from, reason, .. }) => Ok(Some((from, reason))),
            _ => Ok(None),
        }
    }
}

/// `task_id` の events のうち、種類 `kind`（`Event` の JSON の `type`。例 `transitioned`・`retried`）の最新の 1 件。
/// 無ければ `None`。1 つの task の最新の 1 件を種類で読む SQL はここだけで、`UNIQUE(task_id, seq)` の index を
/// seq の降順に辿り、ほかの task の events は読まない。`instr` で比べるので、`kind` の `_` は LIKE の wildcard に
/// ならない。JSON の中の入れ子の `type` に当たった行が最新なら、呼び出し側の variant の照合で `None` になる（旧い
/// `last_transitioned_tx` の LIKE と同じ）。
pub(super) fn last_event_tx(
    tx: &Connection,
    task_id: TaskId,
    kind: &str,
) -> Result<Option<Event>, StoreError> {
    let json: Option<String> = tx
        .query_row(
            "SELECT json FROM events WHERE task_id = ?1 AND instr(json, ?2) > 0 \
             ORDER BY seq DESC LIMIT 1",
            params![task_id.to_string(), format!("\"type\":\"{kind}\"")],
            |row| row.get(0),
        )
        .optional()?;
    Ok(json.map(|json| serde_json::from_str(&json)).transpose()?)
}

/// ADR 2026-11-08-health-enforcement-loop D7.2（P-9）: 依存先の後続 `dependents`（`SqliteStore::dependents_of_tx`
/// の結果）から、retry の元の task → 後継 の対応を作る。retry（`plan_retry`）は `depends_on` を写すので、依存先の
/// 後続 B の後継 B' も同じ依存先の後続に入る。後続ごとに最新の `Event::Retried` を `last_event_tx` で 1 件読み、
/// `from` → その後続 を返す（同じ元に後継が複数あれば新しい id）。人が後継の `depends_on` から依存先を外すと、後継は
/// `dependents` に入らず、元の後続は後継を持たない扱いになる。`retry_task_tx` の付け替えと
/// `cascade_after_transition_tx` の復帰が、この結果を `transition::cancelled_by_dependency` の入力にする。
pub(super) fn retry_successors_tx(
    tx: &Connection,
    dependents: &[TaskId],
) -> Result<HashMap<TaskId, TaskId>, StoreError> {
    let mut successors: HashMap<TaskId, TaskId> = HashMap::new();
    for &id in dependents {
        if let Some(Event::Retried { from, .. }) = last_event_tx(tx, id, "retried")? {
            let newest = successors.get(&from).map_or(id, |s| (*s).max(id));
            successors.insert(from, newest);
        }
    }
    Ok(successors)
}
