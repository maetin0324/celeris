use std::time::Duration as StdDuration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use time::OffsetDateTime;

use crate::message::is_conversation;
use crate::model::{Event, Status, Task, TaskId, TaskKind};
use crate::transition::{InvalidTransition, Trigger};

use super::query::{
    CursorPayload, decode_cursor, encode_cursor, filter_predicate, keyset_predicate, order_by_sql,
    parse_status, usize_to_i64,
};
use super::{
    ListFilter, ListOrder, Page, SqliteStore, StoreError, format_rfc3339, kind_str, status_str,
};

impl SqliteStore {
    pub(super) fn row_to_task(json: String) -> Result<Task, StoreError> {
        Ok(serde_json::from_str(&json)?)
    }

    /// 既にロック済みの connection を使ってタスクを取得する内部ヘルパー。
    /// `get()` が再度 Mutex をロックしないようにするために分離してある。
    pub(crate) fn get_locked(conn: &Connection, id: TaskId) -> Result<Option<Task>, StoreError> {
        let json: Option<String> = conn
            .query_row(
                "SELECT json FROM tasks WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(Self::row_to_task(j)?)),
            None => Ok(None),
        }
    }

    /// `insert` の本体（トランザクション内でも使えるよう `Connection` を受ける）。
    pub(super) fn insert_tx(conn: &Connection, task: &Task) -> Result<(), StoreError> {
        let json = serde_json::to_string(task)?;
        let created_at = format_rfc3339(task.created_at)?;
        let updated_at = format_rfc3339(task.updated_at)?;
        let (lease_worker_run_id, lease_expires_at) = match &task.lease {
            Some(lease) => (
                Some(lease.worker_run_id.clone()),
                Some(format_rfc3339(lease.expires_at)?),
            ),
            None => (None, None),
        };
        // ADR-0043 D2: `repos_json` は索引（正は `json` の中の `repos`）。空なら NULL。
        let repos_json = if task.repos.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&task.repos)?)
        };
        conn.execute(
            "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, \
             lease_worker_run_id, lease_expires_at, json, title, updated_at, objective, genre, \
             project_id, milestone_id, assignee, repos_json, labels_json, category, skills_json, mode, \
             root_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                task.id.to_string(),
                status_str(task.status),
                kind_str(task.kind),
                task.parent_id.map(|p| p.to_string()),
                task.priority,
                created_at,
                lease_worker_run_id,
                lease_expires_at,
                json,
                task.title,
                updated_at,
                // objective / genre は作成後に変わらないので、列を書くのは挿入時だけ（ADR-0014 D2, ADR-0027 D1）。
                task.objective,
                task.genre,
                // ADR-0033 D2: 案件・途中目標・担当も作成後に変わらないので、列を書くのは挿入時だけ。
                task.project_id.map(|p| p.to_string()),
                task.milestone_id.map(|m| m.to_string()),
                task.assignee.clone(),
                repos_json,
                // ADR-0044 D3（Phase 53）: ラベルと種類は `PATCH /tasks/{id}` で変わるので、
                // `update_task_tx` が同じ 2 列を書き直す。
                serde_json::to_string(&task.labels)?,
                task.category.as_str(),
                // ADR-0046 D2 / D4（Phase 59）: skills と mode も `PATCH /tasks/{id}` で変わるので、
                // `update_task_tx` が同じ 2 列を書き直す。
                serde_json::to_string(&task.skills)?,
                task.mode.as_str(),
                // ADR-0079 D4 (4)（migration 0031）: `Task.tree.root_id` の写し（木に属さない task は NULL）。
                task.tree.as_ref().map(|t| t.root_id.to_string()),
            ],
        )?;
        Ok(())
    }

    /// ADR-0044 D1（Phase 53）: `PATCH /tasks/{id}` の書き戻し。`json`（正本）と、絞り込みのための
    /// 写しの列を**全部**書き直す（挿入時にしか書いていなかった `objective` / `genre` / `project_id` /
    /// `milestone_id` / `assignee` も、編集で変わりうるのでここで揃える）。状態機械は通らない
    /// （`status` / `attempts` / `lease` は触らない）。
    pub(super) fn update_task_tx(tx: &Connection, task: &Task) -> Result<(), StoreError> {
        let json = serde_json::to_string(task)?;
        let updated_at = format_rfc3339(task.updated_at)?;
        tx.execute(
            "UPDATE tasks SET json = ?1, title = ?2, updated_at = ?3, objective = ?4, genre = ?5, \
             priority = ?6, parent_id = ?7, project_id = ?8, milestone_id = ?9, assignee = ?10, \
             labels_json = ?11, category = ?12, skills_json = ?13, mode = ?14, root_id = ?16 \
             WHERE id = ?15",
            params![
                json,
                task.title,
                updated_at,
                task.objective,
                task.genre,
                task.priority,
                task.parent_id.map(|p| p.to_string()),
                task.project_id.map(|p| p.to_string()),
                task.milestone_id.map(|m| m.to_string()),
                task.assignee.clone(),
                serde_json::to_string(&task.labels)?,
                task.category.as_str(),
                serde_json::to_string(&task.skills)?,
                task.mode.as_str(),
                task.id.to_string(),
                task.tree.as_ref().map(|t| t.root_id.to_string()),
            ],
        )?;
        Ok(())
    }

    /// `task` の `status` / `depends_on`（json 全体）/ `updated_at` を書き戻す（リースは変えない）。
    /// Phase 31 の張り替えが使う低レベルの書き込み（`transition()` を経由しない）。
    pub(super) fn rewrite_task_tx(tx: &Connection, task: &Task) -> Result<(), StoreError> {
        let json = serde_json::to_string(task)?;
        let updated_at = format_rfc3339(task.updated_at)?;
        tx.execute(
            "UPDATE tasks SET status = ?1, json = ?2, title = ?3, updated_at = ?4 WHERE id = ?5",
            params![
                status_str(task.status),
                json,
                task.title,
                updated_at,
                task.id.to_string()
            ],
        )?;
        Ok(())
    }

    pub(super) fn insert_impl(&self, task: &Task) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::insert_tx(&conn, task)
    }

    pub(super) fn get_impl(&self, id: TaskId) -> Result<Option<Task>, StoreError> {
        self.with_read_conn(|conn| Self::get_locked(conn, id))
    }

    pub(super) fn list_impl(&self, filter: Option<Status>) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut tasks = Vec::new();
            match filter {
                Some(status) => {
                    let mut stmt = conn.prepare("SELECT json FROM tasks WHERE status = ?1")?;
                    let rows =
                        stmt.query_map(params![status_str(status)], |row| row.get::<_, String>(0))?;
                    for row in rows {
                        tasks.push(Self::row_to_task(row?)?);
                    }
                }
                None => {
                    let mut stmt = conn.prepare("SELECT json FROM tasks")?;
                    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                    for row in rows {
                        tasks.push(Self::row_to_task(row?)?);
                    }
                }
            }
            Ok(tasks)
        })
    }

    pub(super) fn acquire_lease_impl(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        // ADR-0002 D2: 遷移（ready -> running, Trigger::Dispatch）の結果は
        // `Event::Transitioned` と同一トランザクションで追記する。D8: `Dispatch`
        // は `kind == Approval` では無効（Approval は running に入らない）。
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let current: Option<(String, String, String)> = tx
            .query_row(
                "SELECT status, kind, json FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let (status_col, kind_col, json) = match current {
            Some(v) => v,
            None => return Ok(false),
        };
        if status_col != status_str(Status::Ready) || kind_col == kind_str(TaskKind::Approval) {
            return Ok(false);
        }

        let mut task = Self::row_to_task(json)?;

        let dur = time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let now = OffsetDateTime::now_utc();
        let expires_at = now + dur;

        task.status = Status::Running;
        task.lease = Some(crate::model::Lease {
            worker_run_id: worker_run_id.to_string(),
            expires_at,
        });
        task.updated_at = now;

        let new_json = serde_json::to_string(&task)?;
        let expires_at_str = format_rfc3339(expires_at)?;
        let updated_at_str = format_rfc3339(now)?;

        let affected = tx.execute(
            "UPDATE tasks SET status = ?1, lease_worker_run_id = ?2, lease_expires_at = ?3, \
             json = ?4, updated_at = ?5 WHERE id = ?6 AND status = ?7",
            params![
                status_str(Status::Running),
                worker_run_id,
                expires_at_str,
                new_json,
                updated_at_str,
                task_id.to_string(),
                status_str(Status::Ready),
            ],
        )?;

        if affected != 1 {
            return Ok(false);
        }

        let event = Event::Transitioned {
            from: Status::Ready,
            to: Status::Running,
            reason: "dispatch".to_string(),
        };
        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        let event_json = serde_json::to_string(&event)?;
        tx.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
            params![task_id.to_string(), next_seq, ts, event_json],
        )?;

        tx.commit()?;
        Ok(true)
    }

    pub(super) fn release_lease_impl(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        // 読んだ json を書き戻すので、間に別接続（celerisctl / API）の書き込みが挟まらないよう IMMEDIATE で囲む（Phase 9 監査）。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let current: Option<(Option<String>, String)> = tx
            .query_row(
                "SELECT lease_worker_run_id, json FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| Ok((row.get(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        let (lease_worker_run_id, json) = match current {
            Some(v) => v,
            None => return Ok(()),
        };

        if lease_worker_run_id.as_deref() != Some(worker_run_id) {
            return Ok(());
        }

        let mut task = Self::row_to_task(json)?;
        task.lease = None;
        task.updated_at = OffsetDateTime::now_utc();
        let new_json = serde_json::to_string(&task)?;
        let updated_at_str = format_rfc3339(task.updated_at)?;

        tx.execute(
            "UPDATE tasks SET lease_worker_run_id = NULL, lease_expires_at = NULL, json = ?1, \
             updated_at = ?2 WHERE id = ?3 AND lease_worker_run_id = ?4",
            params![new_json, updated_at_str, task_id.to_string(), worker_run_id],
        )?;
        tx.commit()?;

        Ok(())
    }

    pub(super) fn halted_by_pause_impl(&self, task: &Task) -> Result<bool, StoreError> {
        if is_conversation(task) {
            return Ok(false);
        }
        self.with_read_conn(|conn| {
            let halted_projects = Self::halted_projects_locked(conn)?;
            if task
                .project_id
                .is_some_and(|p| halted_projects.contains(&p.to_string()))
            {
                return Ok(true);
            }
            Self::halted_by_ancestry_locked(conn, task, &halted_projects)
        })
    }

    pub(super) fn ready_tasks_impl(&self, limit: usize) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            // ADR-0044 D6（Phase 55）: **一時停止・中止・アーカイブされた案件／途中目標のタスクは
            // dispatch しない**（`ready` のまま。状態機械は触らない）。案件の支援 run（計画・レビュー・
            // まとめ・報告の圧縮）も同じ `tasks` の行なので、この 1 か所で全部が止まる。
            // **対話（`is_conversation`）だけは例外**: 人が「なぜ止めたのか」を秘書と話せなくなるため、
            // 止まっている案件でも対話は起こす（判断は下の Rust 側。`Task` を読まないと見分けられない）。
            let halted_projects = Self::halted_projects_locked(conn)?;
            // ADR-0079 D13（Phase R5a）: 途中目標は凍結（新しく paused / cancelled にする入口は無い）。既存の
            // paused / cancelled の途中目標に属する task の抑止だけは残す。
            let halted_milestones = Self::halted_milestones_locked(conn)?;
            // ADR-0074 D3.2 の途中目標の Go（`milestones_awaiting_go_locked`）は ADR-0079 D13（R5a）で廃止。

            // ADR-0010 D2（P-36）: dispatch されない Approval は取得件数を占有しないよう SQL 段階で除外する。
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE status = ?1 AND kind != ?2 ORDER BY priority DESC, created_at ASC",
            )?;
            let rows = stmt.query_map(
                params![status_str(Status::Ready), kind_str(TaskKind::Approval)],
                |row| row.get::<_, String>(0),
            )?;

            let mut result = Vec::new();
            for row in rows {
                let task = Self::row_to_task(row?)?;

                // ADR-0044 D6（Phase 55）: 止まっている案件・途中目標のタスクは見送る（対話は除く）。
                if !is_conversation(&task) {
                    let halted = task
                        .project_id
                        .is_some_and(|p| halted_projects.contains(&p.to_string()))
                        || task
                            .milestone_id
                            .is_some_and(|m| halted_milestones.contains(&m.to_string()));
                    if halted {
                        continue;
                    }
                    // ADR-0079 D13（Phase R5a）: subtree の一時停止。自分か祖先（`parent_id` /
                    // `tree.parent_unit` の鎖）が `paused_at` を持つか、祖先が止まっている案件に属するなら見送る。
                    if Self::halted_by_ancestry_locked(conn, &task, &halted_projects)? {
                        continue;
                    }
                }

                // P-78（ADR-0033 D4 / Phase 28）: 対話タスクの `depends_on` は返事を送った順に返すための
                // 直列化だけが目的で、前の対話タスクの成否には意味が無い。前の対話タスクが終端に達していれば
                // （`done` だけでなく `failed` / `cancelled` でも）次の対話タスクへ進めてよい。
                let is_conv = is_conversation(&task);
                let mut deps_done = true;
                for dep_id in &task.depends_on {
                    match Self::get_locked(conn, *dep_id)? {
                        Some(dep) if dep.status == Status::Done => {}
                        Some(dep) if is_conv && dep.status.is_terminal() => {}
                        _ => {
                            deps_done = false;
                            break;
                        }
                    }
                }
                if !deps_done {
                    continue;
                }

                if let Some(parent_id) = task.parent_id
                    && let Some(parent) = Self::get_locked(conn, parent_id)?
                    && parent.kind == TaskKind::Approval
                    && parent.status != Status::Done
                {
                    continue;
                }

                result.push(task);
                if result.len() >= limit {
                    break;
                }
            }

            Ok(result)
        })
    }

    pub(super) fn create_task_impl(
        &self,
        task: &Task,
        origin: Option<crate::model::CreatedOrigin>,
        extra_events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::create_task_tx(&tx, task, origin, extra_events)?;
        tx.commit()?;
        Ok(())
    }

    /// `create_task_impl` inside a caller's transaction (ADR 2026-10-05 D3: a CoS operation writes
    /// the task, `cos_operations` and the audit events in one transaction).
    pub fn create_task_tx(
        tx: &Connection,
        task: &Task,
        origin: Option<crate::model::CreatedOrigin>,
        extra_events: Vec<Event>,
    ) -> Result<(), StoreError> {
        Self::insert_tx(tx, task)?;
        Self::append_event_tx(
            tx,
            task.id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin,
            },
        )?;
        for event in &extra_events {
            Self::append_event_tx(tx, task.id, event)?;
        }
        Ok(())
    }

    pub(super) fn delegate_children_impl(
        &self,
        parent_id: TaskId,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if Self::get_locked(&tx, parent_id)?.is_none() {
            return Err(StoreError::Invalid(format!("task not found: {parent_id}")));
        }
        let mut ids = Vec::with_capacity(children.len());
        for child in &children {
            if child.parent_id != Some(parent_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to task {parent_id}",
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
            if child.status == Status::Draft {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
            ids.push(child.id);
        }
        Self::append_event_tx(
            &tx,
            parent_id,
            &Event::Delegated {
                run_id: run_id.to_string(),
                task_ids: ids.clone(),
            },
        )?;
        tx.commit()?;
        Ok(ids)
    }

    pub(super) fn retry_task_impl(
        &self,
        original: TaskId,
        new_task: &Task,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(orig) = Self::get_locked(&tx, original)? else {
            return Err(StoreError::Invalid(format!("task not found: {original}")));
        };
        if !matches!(orig.status, Status::Failed | Status::Cancelled) {
            return Err(StoreError::InvalidTransition(InvalidTransition {
                status: orig.status,
                kind: orig.kind,
                trigger: "retry",
            }));
        }

        Self::insert_tx(&tx, new_task)?;
        Self::append_event_tx(
            &tx,
            new_task.id,
            &Event::Created {
                task: Box::new(new_task.clone()),
                origin: None,
            },
        )?;
        Self::append_event_tx(&tx, new_task.id, &Event::Retried { from: original })?;

        let mut rewired = Vec::new();
        for dep_id in Self::dependents_of_tx(&tx, original)? {
            let Some(dep) = Self::get_locked(&tx, dep_id)? else {
                continue;
            };
            let eligible = match dep.status {
                Status::Draft | Status::Ready | Status::Blocked => true,
                Status::Cancelled => {
                    Self::last_transitioned_reason_tx(&tx, dep_id)?.as_deref()
                        == Some(Trigger::DependencyFailed.name())
                }
                _ => false,
            };
            if !eligible {
                continue;
            }
            let mut updated = dep.clone();
            updated.depends_on = updated
                .depends_on
                .iter()
                .map(|d| if *d == original { new_task.id } else { *d })
                .collect();
            let was_cancelled = updated.status == Status::Cancelled;
            if was_cancelled {
                updated.status = Status::Draft;
            }
            updated.updated_at = OffsetDateTime::now_utc();
            Self::rewrite_task_tx(&tx, &updated)?;
            if was_cancelled {
                Self::append_event_tx(
                    &tx,
                    dep_id,
                    &Event::Transitioned {
                        from: Status::Cancelled,
                        to: Status::Draft,
                        reason: "retried".to_string(),
                    },
                )?;
            }
            rewired.push(dep_id);
        }

        tx.commit()?;
        Ok(rewired)
    }

    pub(super) fn children_impl(&self, parent_id: TaskId) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            // 同じトランザクションで挿入した子（created_at が同じ）は挿入順（rowid）で返す。
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE parent_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![parent_id.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    pub(super) fn renew_lease_impl(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        let expires_at = OffsetDateTime::now_utc()
            + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let ours = task.status == Status::Running
            && task.lease.as_ref().map(|l| l.worker_run_id.as_str()) == Some(worker_run_id);
        if !ours {
            // ADR-0074 D1.5（Phase F2）: v2 の並列 WU の run は Task の lease の保持者ではなく
            // WU の lease（`work_units.lease_run_id`）を持つ。その WU の lease を延ばし、Task の
            // lease の期限も（短くせずに）そこまで延ばす。
            if task.status != Status::Running {
                return Ok(false);
            }
            let expires_str = format_rfc3339(expires_at)?;
            let n = tx.execute(
                "UPDATE work_units SET lease_expires_at = ?1 WHERE task_id = ?2 \
                 AND lease_run_id = ?3 AND status = 'running'",
                params![expires_str, task_id.to_string(), worker_run_id],
            )?;
            if n == 0 {
                return Ok(false);
            }
            if let Some(lease) = task.lease.as_mut()
                && lease.expires_at < expires_at
            {
                lease.expires_at = expires_at;
                tx.execute(
                    "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                    params![
                        expires_str,
                        serde_json::to_string(&task)?,
                        task_id.to_string(),
                    ],
                )?;
            }
            tx.commit()?;
            return Ok(true);
        }
        task.lease = Some(crate::model::Lease {
            worker_run_id: worker_run_id.to_string(),
            expires_at,
        });
        let affected = tx.execute(
            "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3 AND status = ?4 AND lease_worker_run_id = ?5",
            params![
                format_rfc3339(expires_at)?,
                serde_json::to_string(&task)?,
                task_id.to_string(),
                status_str(Status::Running),
                worker_run_id,
            ],
        )?;
        tx.commit()?;
        Ok(affected == 1)
    }

    pub(super) fn list_page_impl(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let (filter_sql, filter_params) = filter_predicate(filter);

            let total: i64 = {
                let sql = format!("SELECT COUNT(*) FROM tasks WHERE {filter_sql}");
                conn.query_row(&sql, params_from_iter(filter_params.iter()), |row| {
                    row.get(0)
                })?
            };

            let mut where_sql = format!("({filter_sql})");
            let mut query_params = filter_params;
            if let Some(c) = cursor {
                let payload = decode_cursor(c)?;
                let (keyset_sql, keyset_params) = keyset_predicate(order, &payload);
                where_sql.push_str(&format!(" AND ({keyset_sql})"));
                query_params.extend(keyset_params);
            }

            let order_sql = order_by_sql(order);
            let fetch_limit = usize_to_i64(limit.saturating_add(1));
            let sql =
                format!("SELECT json FROM tasks WHERE {where_sql} ORDER BY {order_sql} LIMIT ?");
            query_params.push(SqlValue::Integer(fetch_limit));

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| {
                row.get::<_, String>(0)
            })?;
            let mut items = Vec::new();
            for row in rows {
                items.push(Self::row_to_task(row?)?);
            }

            let has_more = items.len() > limit;
            if has_more {
                items.truncate(limit);
            }
            let next_cursor = if has_more {
                match items.last() {
                    Some(last) => Some(encode_cursor(&CursorPayload::from_task(last)?)?),
                    None => None,
                }
            } else {
                None
            };

            Ok(Page {
                items,
                next_cursor,
                total: total as u64,
            })
        })
    }

    pub(super) fn count_by_status_impl(&self) -> Result<Vec<(Status, u64)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM tasks GROUP BY status")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (s, c) = row?;
                out.push((parse_status(&s)?, c as u64));
            }
            Ok(out)
        })
    }

    pub(super) fn update_task_impl(&self, task: &Task, event: Event) -> Result<Task, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = Self::get_locked(&tx, task.id)? else {
            return Err(StoreError::Invalid(format!("task not found: {}", task.id)));
        };
        // 状態機械が持つ 3 つ（`status` / `attempts` / `lease`）だけは**この tx の中で読んだ行**の値を使う。
        // 編集を組み立てている間にディスパッチャが `acquire_lease` を通していたら、渡された `task` は
        // 古い `ready` / `lease: None` を持っている。そのまま書くと `json` と `status` 列が食い違い、
        // そのタスクは二度と dispatch されず run の結果も捨てられる（Phase 53 の監査で発見）。
        let merged = Task {
            status: current.status,
            attempts: current.attempts,
            lease: current.lease.clone(),
            ..task.clone()
        };
        Self::update_task_tx(&tx, &merged)?;
        Self::append_event_tx(&tx, task.id, &event)?;
        tx.commit()?;
        Ok(merged)
    }

    pub(super) fn extend_task_lease_impl(
        &self,
        task_id: TaskId,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        if task.status != Status::Running {
            return Ok(false);
        }
        let expires_at = OffsetDateTime::now_utc()
            + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let Some(lease) = task.lease.as_mut() else {
            return Ok(false);
        };
        if lease.expires_at < expires_at {
            lease.expires_at = expires_at;
            tx.execute(
                "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                params![
                    format_rfc3339(expires_at)?,
                    serde_json::to_string(&task)?,
                    task_id.to_string(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }
}
