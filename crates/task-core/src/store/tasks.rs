use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Task, TaskId};

use super::{SqliteStore, StoreError, format_rfc3339, kind_str, status_str};

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
}
