use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use crate::model::{TaskId, WorkspaceSpec};
use crate::org::ProjectId;
use crate::repos::{ProjectRepo, RepoId, RepoKind, RepoRun, RepoSync};

use super::{SqliteStore, StoreError, detect_repo_kind, format_rfc3339, parse_rfc3339};

impl SqliteStore {
    // ---- ADR-0043 D1（Phase 52）: 案件のリポジトリ（`project_repos`）----

    /// `project_repos` の 1 行。
    pub(super) fn repo_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<ProjectRepo, StoreError>> {
        let id: String = row.get(0)?;
        let project_id: String = row.get(1)?;
        let kind_col: String = row.get(3)?;
        let location_col: String = row.get(4)?;
        let run_col: String = row.get(7)?;
        let created_at: String = row.get(9)?;
        let (Ok(id), Ok(project_id), Some(kind), Some(run)) = (
            id.parse::<RepoId>(),
            project_id.parse::<ProjectId>(),
            RepoKind::parse(&kind_col),
            RepoRun::parse(&run_col),
        ) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid project_repos row: id={id} project_id={project_id} kind={kind_col} run={run_col}"
            ))));
        };
        let location = match serde_json::from_str::<WorkspaceSpec>(&location_col) {
            Ok(spec) => spec,
            Err(e) => {
                return Ok(Err(StoreError::Invalid(format!(
                    "invalid project_repos location for {id}: {e}"
                ))));
            }
        };
        let sync_col: Option<String> = row.get(6)?;
        let sync = match sync_col.as_deref() {
            None => None,
            Some(raw) => match RepoSync::parse(raw) {
                Some(v) => Some(v),
                None => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid project_repos sync for {id}: {raw:?}"
                    ))));
                }
            },
        };
        Ok((|| {
            let is_primary: i64 = row.get(8)?;
            Ok(ProjectRepo {
                id,
                project_id,
                name: row.get(2)?,
                kind,
                location,
                default_branch: row.get(5)?,
                sync,
                run,
                is_primary: is_primary != 0,
                created_at: parse_rfc3339(&created_at)?,
            })
        })())
    }

    pub(super) const REPO_COLUMNS: &'static str = "id, project_id, name, kind, location_json, default_branch, sync, run, is_primary, created_at";

    pub(super) fn repo_list_tx(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<Vec<ProjectRepo>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM project_repos WHERE project_id = ?1 \
             ORDER BY is_primary DESC, created_at ASC, id ASC",
            Self::REPO_COLUMNS
        ))?;
        let rows = stmt.query_map(params![project_id.to_string()], Self::repo_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub(super) fn repo_get_tx(
        conn: &Connection,
        id: RepoId,
    ) -> Result<Option<ProjectRepo>, StoreError> {
        conn.query_row(
            &format!(
                "SELECT {} FROM project_repos WHERE id = ?1",
                Self::REPO_COLUMNS
            ),
            params![id.to_string()],
            Self::repo_row,
        )
        .optional()?
        .transpose()
    }

    /// 行を 1 件書く（INSERT OR REPLACE）。検証は呼び出し側で済ませておくこと。
    pub(super) fn repo_write_tx(conn: &Connection, repo: &ProjectRepo) -> Result<(), StoreError> {
        let location = serde_json::to_string(&repo.location)
            .map_err(|e| StoreError::Invalid(format!("cannot serialize repo location: {e}")))?;
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO project_repos ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                Self::REPO_COLUMNS
            ),
            params![
                repo.id.to_string(),
                repo.project_id.to_string(),
                repo.name,
                repo.kind.as_str(),
                location,
                repo.default_branch,
                repo.sync.map(|s| s.as_str()),
                repo.run.as_str(),
                i64::from(repo.is_primary),
                format_rfc3339(repo.created_at)?,
            ],
        )?;
        Ok(())
    }

    /// 1 案件に primary は 1 つ（ADR-0043 D1）。`keep` 以外の `is_primary` を落とす。
    pub(super) fn repo_clear_other_primaries_tx(
        conn: &Connection,
        project_id: ProjectId,
        keep: RepoId,
    ) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE project_repos SET is_primary = 0 WHERE project_id = ?1 AND id <> ?2",
            params![project_id.to_string(), keep.to_string()],
        )?;
        Ok(())
    }

    /// `projects.workspace` 列を primary のリポジトリの写しに保つ（migration 0012 のコメント参照。
    /// ADR-0043 D1 は「書かない」だが、N-1 互換〈旧バイナリが新スキーマを読む〉のために写しを残す）。
    pub(super) fn sync_project_workspace_tx(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<(), StoreError> {
        let primary: Option<String> = conn
            .query_row(
                "SELECT location_json FROM project_repos WHERE project_id = ?1 AND is_primary = 1 \
                 ORDER BY created_at ASC, id ASC LIMIT 1",
                params![project_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        // primary が消えたら NULL に戻す（案件を「作業場所なし」に戻したとき）。
        conn.execute(
            "UPDATE projects SET workspace = ?1 WHERE id = ?2",
            params![primary, project_id.to_string()],
        )?;
        Ok(())
    }

    /// そのリポジトリを参照している**未終端**のタスク（ADR-0043 D1: `DELETE /repos/{id}` の 409）。
    /// 索引は migration 0012 で足した `tasks.repos_json`（ULID は一意なので部分一致で足りる）。
    pub(super) fn repo_active_tasks_tx(
        conn: &Connection,
        id: RepoId,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM tasks WHERE {} AND repos_json IS NOT NULL AND repos_json LIKE ?1 \
             ORDER BY id ASC",
            Self::NON_TERMINAL_SQL
        ))?;
        let pattern = format!("%{id}%");
        let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(Self::parse_id(&row?)?);
        }
        Ok(out)
    }

    /// migration 0012 の写し（ADR-0043 D1）。`projects.workspace` がある案件ごとに `is_primary = 1` の
    /// リポジトリを 1 件作る。`kind` は「パスが git なら git、でなければ dir」だが、SQL からは
    /// ファイルシステムを見られないのでここ（Rust）で決める。
    pub(super) fn backfill_project_repos(conn: &Connection) -> Result<(), StoreError> {
        let mut stmt = conn.prepare(
            "SELECT id, workspace, created_at FROM projects WHERE workspace IS NOT NULL",
        )?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for (project_id, workspace, created_at) in rows {
            let Ok(project_id) = project_id.parse::<ProjectId>() else {
                continue;
            };
            let Ok(location) = serde_json::from_str::<WorkspaceSpec>(&workspace) else {
                continue;
            };
            let repo = ProjectRepo {
                id: RepoId::new(),
                project_id,
                name: crate::repos::default_repo_name(&location),
                kind: detect_repo_kind(&location),
                location,
                default_branch: None,
                sync: None,
                run: RepoRun::Auto,
                is_primary: true,
                created_at: parse_rfc3339(&created_at)
                    .unwrap_or_else(|_| OffsetDateTime::now_utc()),
            };
            Self::repo_write_tx(conn, &repo)?;
        }
        Ok(())
    }

    pub(super) fn repo_create_impl(&self, repo: &ProjectRepo) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![repo.project_id.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::Invalid(format!(
                "project not found: {}",
                repo.project_id
            )));
        }
        let existing = Self::repo_list_tx(&tx, repo.project_id)?;
        crate::repos::validate_upsert(&existing, repo)?;
        // 最初の 1 件は自動的に primary（案件に「主なリポジトリ」が無い状態を作らない）。
        let mut repo = repo.clone();
        if existing.is_empty() {
            repo.is_primary = true;
        }
        Self::repo_write_tx(&tx, &repo)?;
        if repo.is_primary {
            Self::repo_clear_other_primaries_tx(&tx, repo.project_id, repo.id)?;
        }
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(())
    }

    pub(super) fn repo_get_impl(&self, id: RepoId) -> Result<Option<ProjectRepo>, StoreError> {
        self.with_read_conn(|conn| Self::repo_get_tx(conn, id))
    }

    pub(super) fn repo_list_impl(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<ProjectRepo>, StoreError> {
        self.with_read_conn(|conn| Self::repo_list_tx(conn, project_id))
    }

    pub(super) fn repo_update_impl(&self, repo: &ProjectRepo) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = Self::repo_get_tx(&tx, repo.id)? else {
            return Ok(false);
        };
        // `project_id` と `created_at` は動かさない（付け替えは作り直し）。
        let repo = ProjectRepo {
            project_id: current.project_id,
            created_at: current.created_at,
            ..repo.clone()
        };
        let others: Vec<ProjectRepo> = Self::repo_list_tx(&tx, repo.project_id)?
            .into_iter()
            .filter(|r| r.id != repo.id)
            .collect();
        crate::repos::validate_upsert(&others, &repo)?;
        Self::repo_write_tx(&tx, &repo)?;
        if repo.is_primary {
            Self::repo_clear_other_primaries_tx(&tx, repo.project_id, repo.id)?;
        }
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn repo_delete_impl(&self, id: RepoId) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(repo) = Self::repo_get_tx(&tx, id)? else {
            return Ok(false);
        };
        let open = Self::repo_active_tasks_tx(&tx, id)?;
        if !open.is_empty() {
            return Err(StoreError::InUse {
                kind: "project repo",
                id: id.to_string(),
                detail: format!("{} task(s) using it have not finished", open.len()),
            });
        }
        tx.execute(
            "DELETE FROM project_repos WHERE id = ?1",
            params![id.to_string()],
        )?;
        // primary を消したら、残りのうち一番古いものを primary にする（案件に主なリポジトリを残す）。
        if repo.is_primary
            && let Some(next) = Self::repo_list_tx(&tx, repo.project_id)?.first()
        {
            tx.execute(
                "UPDATE project_repos SET is_primary = 1 WHERE id = ?1",
                params![next.id.to_string()],
            )?;
        }
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn repo_set_primary_impl(&self, id: RepoId) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(repo) = Self::repo_get_tx(&tx, id)? else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE project_repos SET is_primary = 1 WHERE id = ?1",
            params![id.to_string()],
        )?;
        Self::repo_clear_other_primaries_tx(&tx, repo.project_id, id)?;
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn repo_active_tasks_impl(&self, id: RepoId) -> Result<Vec<TaskId>, StoreError> {
        self.with_read_conn(|conn| Self::repo_active_tasks_tx(conn, id))
    }
}
