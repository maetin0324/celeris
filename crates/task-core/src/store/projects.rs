use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use crate::model::{Task, WorkspaceSpec};
use crate::org::{Milestone, MilestoneId, MilestoneStatus, Project, ProjectId, ProjectStatus};
use crate::repos::{ProjectRepo, RepoId, RepoKind, RepoRun};

use super::{SqliteStore, StoreError, detect_repo_kind, format_rfc3339, parse_rfc3339};

impl SqliteStore {
    pub(super) fn project_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<Project, StoreError>> {
        let id: String = row.get(0)?;
        let status_col: String = row.get(3)?;
        let created_at: String = row.get(5)?;
        let updated_at: String = row.get(6)?;
        // ADR-0039 D1 / ADR-0043 D1: 7 列目は **primary のリポジトリの `location_json`**、無ければ
        // 従来の `projects.workspace` 列（`COALESCE`。導入前の行と作業場所を決めていない案件は NULL）。
        let workspace_col: Option<String> = row.get(7)?;
        // ADR-0044 D6（Phase 55）: 8 列目 `archived_at`、9 列目 `paused_from`。
        let archived_at_col: Option<String> = row.get(8)?;
        let paused_from_col: Option<String> = row.get(9)?;
        let (Ok(id), Some(status)) = (id.parse::<ProjectId>(), ProjectStatus::parse(&status_col))
        else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid project row: id={id} status={status_col}"
            ))));
        };
        let paused_from = match paused_from_col.as_deref() {
            Some(raw) => match ProjectStatus::parse(raw) {
                Some(parsed) => Some(parsed),
                None => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid project paused_from for {id}: {raw}"
                    ))));
                }
            },
            None => None,
        };
        let workspace = match workspace_col.as_deref() {
            Some(raw) => match serde_json::from_str::<WorkspaceSpec>(raw) {
                Ok(spec) => Some(spec),
                Err(e) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid project workspace for {id}: {e}"
                    ))));
                }
            },
            None => None,
        };
        Ok((|| {
            Ok(Project {
                id,
                title: row.get(1)?,
                request: row.get(2)?,
                status,
                secretary_summary: row.get(4)?,
                workspace,
                archived_at: match archived_at_col.as_deref() {
                    Some(raw) => Some(parse_rfc3339(raw)?),
                    None => None,
                },
                paused_from,
                // ADR-0074 D3.2（Phase F4b）: 11 列目 `auto_advance`。
                auto_advance: row.get::<_, i64>(10)? != 0,
                // Phase K-1: 12 列目 `slug`。
                slug: row.get(11)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    /// ADR-0039 D1: 案件の作業場所を DB の列に入れる形（JSON か NULL）にする。
    pub(super) fn project_workspace_column(
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<Option<String>, StoreError> {
        match workspace {
            Some(spec) => serde_json::to_string(spec)
                .map(Some)
                .map_err(|e| StoreError::Invalid(format!("cannot serialize workspace: {e}"))),
            None => Ok(None),
        }
    }

    /// Phase K-1: 渡された slug の検査（綴りと、他の案件との重複）。
    pub(super) fn check_project_slug(
        conn: &Connection,
        id: &str,
        slug: &str,
    ) -> Result<(), StoreError> {
        if !crate::knowledge::is_valid_project_slug(slug) {
            return Err(StoreError::Invalid(format!(
                "project slug must be lowercase [a-z0-9-] (1..64 chars, no leading/trailing/double '-', not an id): {slug:?}"
            )));
        }
        let other: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE slug = ?1 AND id <> ?2",
                params![slug, id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(other) = other {
            return Err(StoreError::InUse {
                kind: "project slug",
                id: slug.to_string(),
                detail: format!("already used by project {other}"),
            });
        }
        Ok(())
    }

    /// Phase K-1（migration 0029）: `slug` の無い案件に、作った順に slug を付ける
    /// （[`crate::knowledge::derive_project_slug`]。題名 → primary リポジトリの名前 → id の末尾。
    /// 先に作った案件が先に取る。重複は `<slug>-<id の末尾 8 文字>`）。
    pub(super) fn backfill_project_slugs(conn: &Connection) -> Result<(), StoreError> {
        let mut stmt = conn.prepare(
            "SELECT p.id, p.title, \
                    (SELECT r.name FROM project_repos r WHERE r.project_id = p.id AND r.is_primary = 1 \
                     ORDER BY r.created_at ASC, r.id ASC LIMIT 1) \
             FROM projects p WHERE p.slug IS NULL ORDER BY p.created_at ASC, p.id ASC",
        )?;
        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for (id, title, repo) in rows {
            let slug = Self::unique_project_slug(conn, &title, &id, repo.as_deref())?;
            conn.execute(
                "UPDATE projects SET slug = ?1 WHERE id = ?2",
                params![slug, id],
            )?;
        }
        Ok(())
    }

    /// 他の案件が使っていない slug を決める（Phase K-1）。
    pub(super) fn unique_project_slug(
        conn: &Connection,
        title: &str,
        id: &str,
        primary_repo: Option<&str>,
    ) -> Result<String, StoreError> {
        let mut stmt =
            conn.prepare("SELECT slug FROM projects WHERE slug IS NOT NULL AND id <> ?1")?;
        let taken: std::collections::HashSet<String> = stmt
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(crate::knowledge::derive_project_slug(
            title,
            id,
            primary_repo,
            &|s| taken.contains(s),
        ))
    }

    /// ADR-0044 D6（Phase 55）: いま dispatch を止めている案件の id（`paused` / `cancelled` /
    /// アーカイブ済み）。`ready_tasks` が 1 tick に 1 回だけ引く。
    pub(super) fn halted_projects_locked(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut stmt = conn.prepare(
            "SELECT id FROM projects WHERE status IN ('paused', 'cancelled') OR archived_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// ADR-0044 D6（Phase 55）: いま dispatch を止めている途中目標の id（`paused` / `cancelled`）。
    pub(super) fn halted_milestones_locked(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut stmt =
            conn.prepare("SELECT id FROM milestones WHERE status IN ('paused', 'cancelled')")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// ADR-0079 D13（Phase R5a）: `task` が subtree の一時停止で止まっているか。自分か祖先が `paused_at` を
    /// 持つ、または祖先が `halted_projects`（paused / cancelled / archived の案件）に属するなら `true`。
    /// 祖先は `parent_id`、無ければ木の親（`tree.parent_unit.task_id`。採用で `parent_id` を書き換えない子）
    /// を辿る。循環と深すぎる鎖は `MAX_ANCESTRY` で打ち切る（壊れた行で dispatch 全体を止めない）。
    pub(super) fn halted_by_ancestry_locked(
        conn: &Connection,
        task: &Task,
        halted_projects: &std::collections::HashSet<String>,
    ) -> Result<bool, StoreError> {
        const MAX_ANCESTRY: usize = 32;
        if task.paused_at.is_some() {
            return Ok(true);
        }
        let parent_of = |t: &Task| {
            t.parent_id.or_else(|| {
                t.tree
                    .as_ref()
                    .and_then(|tree| tree.parent_unit.as_ref())
                    .map(|u| u.task_id)
            })
        };
        let mut seen = std::collections::HashSet::new();
        let mut next = parent_of(task);
        while let Some(id) = next {
            if !seen.insert(id) || seen.len() > MAX_ANCESTRY {
                break;
            }
            let Some(ancestor) = Self::get_locked(conn, id)? else {
                break;
            };
            if ancestor.paused_at.is_some()
                || ancestor
                    .project_id
                    .is_some_and(|p| halted_projects.contains(&p.to_string()))
            {
                return Ok(true);
            }
            next = parent_of(&ancestor);
        }
        Ok(false)
    }

    pub(super) fn milestone_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<Milestone, StoreError>> {
        let id: String = row.get(0)?;
        let project_id: String = row.get(1)?;
        let status_col: String = row.get(5)?;
        let created_at: String = row.get(6)?;
        let updated_at: String = row.get(7)?;
        // ADR-0044 D6（Phase 55）: 8 列目 `paused_from`。
        let paused_from_col: Option<String> = row.get(8)?;
        let (Ok(id), Ok(project_id), Some(status)) = (
            id.parse::<MilestoneId>(),
            project_id.parse::<ProjectId>(),
            MilestoneStatus::parse(&status_col),
        ) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid milestone row: id={id} project_id={project_id} status={status_col}"
            ))));
        };
        let paused_from = match paused_from_col.as_deref() {
            Some(raw) => match MilestoneStatus::parse(raw) {
                Some(parsed) => Some(parsed),
                None => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid milestone paused_from for {id}: {raw}"
                    ))));
                }
            },
            None => None,
        };
        Ok((|| {
            Ok(Milestone {
                id,
                project_id,
                seq: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
                status,
                paused_from,
                // ADR-0074 D3.3（Phase F4b）: 10 列目 `plan_key`。
                plan_key: row.get(9)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    pub(super) fn project_create_impl(&self, project: &Project) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Phase K-1: slug は案件を作るときに決める（渡されたものは検査だけ）。
        let slug = match project.slug.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => {
                Self::check_project_slug(&tx, &project.id.to_string(), s)?;
                s.to_string()
            }
            _ => Self::unique_project_slug(
                &tx,
                &project.title,
                &project.id.to_string(),
                project
                    .workspace
                    .as_ref()
                    .map(crate::repos::default_repo_name)
                    .as_deref(),
            )?,
        };
        tx.execute(
            "INSERT INTO projects (id, title, request, status, secretary_summary, created_at, updated_at, workspace, \
             archived_at, paused_from, auto_advance, slug) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                project.id.to_string(),
                project.title,
                project.request,
                project.status.as_str(),
                project.secretary_summary,
                format_rfc3339(project.created_at)?,
                format_rfc3339(project.updated_at)?,
                Self::project_workspace_column(project.workspace.as_ref())?,
                project.archived_at.map(format_rfc3339).transpose()?,
                project.paused_from.map(|s| s.as_str()),
                i64::from(project.auto_advance),
                slug,
            ],
        )?;
        // ADR-0043 D1: 案件の作業場所は `is_primary = 1` のリポジトリ 1 件として持つ
        // （`Project.workspace` はその写し）。`POST /projects {workspace}`（従来のフォーム）も
        // これで複数リポジトリの世界に入る。
        if let Some(location) = &project.workspace {
            let repo = ProjectRepo {
                id: RepoId::new(),
                project_id: project.id,
                name: crate::repos::default_repo_name(location),
                kind: detect_repo_kind(location),
                location: location.clone(),
                default_branch: None,
                sync: None,
                run: RepoRun::Auto,
                is_primary: true,
                created_at: project.created_at,
            };
            crate::repos::validate_upsert(&[], &repo)?;
            Self::repo_write_tx(&tx, &repo)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(super) fn project_get_impl(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT p.id, p.title, p.request, p.status, p.secretary_summary, p.created_at, p.updated_at, \
                     COALESCE((SELECT r.location_json FROM project_repos r \
                               WHERE r.project_id = p.id AND r.is_primary = 1 \
                               ORDER BY r.created_at ASC, r.id ASC LIMIT 1), p.workspace), \
                     p.archived_at, p.paused_from, p.auto_advance, p.slug \
                     FROM projects p WHERE p.id = ?1",
                    params![id.to_string()],
                    Self::project_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    pub(super) fn project_list_impl(&self) -> Result<Vec<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.title, p.request, p.status, p.secretary_summary, p.created_at, p.updated_at, \
                     COALESCE((SELECT r.location_json FROM project_repos r \
                               WHERE r.project_id = p.id AND r.is_primary = 1 \
                               ORDER BY r.created_at ASC, r.id ASC LIMIT 1), p.workspace), \
                     p.archived_at, p.paused_from, p.auto_advance, p.slug \
                 FROM projects p ORDER BY p.created_at DESC, p.id DESC",
            )?;
            let rows = stmt.query_map([], Self::project_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(super) fn project_set_status_impl(
        &self,
        id: ProjectId,
        status: ProjectStatus,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE projects SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                status.as_str(),
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    /// ADR-0044 D6（Phase 55）: `pause` / `resume` / `cancel` は状態と `paused_from` を 1 回の UPDATE で書く
    /// （`resume` が「戻り先」を読んだ後に別の書き込みが割り込まないように）。
    pub(super) fn project_set_lifecycle_impl(
        &self,
        id: ProjectId,
        status: ProjectStatus,
        paused_from: Option<Option<ProjectStatus>>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        Self::project_set_lifecycle_tx(&conn, id, status, paused_from)
    }

    /// `project_set_lifecycle` on a caller-owned transaction (ADR 2026-10-09-cos-operations-all-mutations D3).
    pub fn project_set_lifecycle_tx(
        conn: &Connection,
        id: ProjectId,
        status: ProjectStatus,
        paused_from: Option<Option<ProjectStatus>>,
    ) -> Result<bool, StoreError> {
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let affected = match paused_from {
            Some(from) => conn.execute(
                "UPDATE projects SET status = ?1, paused_from = ?2, updated_at = ?3 WHERE id = ?4",
                params![
                    status.as_str(),
                    from.map(|s| s.as_str()),
                    now,
                    id.to_string()
                ],
            )?,
            None => conn.execute(
                "UPDATE projects SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.to_string()],
            )?,
        };
        Ok(affected == 1)
    }

    pub(super) fn project_set_archived_at_impl(
        &self,
        id: ProjectId,
        at: Option<OffsetDateTime>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        Self::project_set_archived_at_tx(&conn, id, at)
    }

    /// `project_set_archived_at` on a caller-owned transaction.
    pub fn project_set_archived_at_tx(
        conn: &Connection,
        id: ProjectId,
        at: Option<OffsetDateTime>,
    ) -> Result<bool, StoreError> {
        let affected = conn.execute(
            "UPDATE projects SET archived_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                at.map(format_rfc3339).transpose()?,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn project_set_slug_impl(
        &self,
        id: ProjectId,
        slug: &str,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let slug = slug.trim();
        Self::check_project_slug(&tx, &id.to_string(), slug)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let n = tx.execute(
            "UPDATE projects SET slug = ?1, updated_at = ?2 WHERE id = ?3",
            params![slug, now, id.to_string()],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    pub(super) fn project_set_text_impl(
        &self,
        id: ProjectId,
        title: Option<&str>,
        request: Option<&str>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        Self::project_set_text_tx(&conn, id, title, request)
    }

    /// `project_set_text` の本体。呼び出し側の transaction 内で使う（CoS の監査付き操作。ADR 2026-10-05 D3）。
    pub fn project_set_text_tx(
        conn: &rusqlite::Connection,
        id: ProjectId,
        title: Option<&str>,
        request: Option<&str>,
    ) -> Result<bool, StoreError> {
        let affected = conn.execute(
            "UPDATE projects SET title = COALESCE(?1, title), request = COALESCE(?2, request), \
             updated_at = ?3 WHERE id = ?4",
            params![
                title,
                request,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn project_set_workspace_impl(
        &self,
        id: ProjectId,
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<bool, StoreError> {
        let column = Self::project_workspace_column(workspace)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let affected = tx.execute(
            "UPDATE projects SET workspace = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                column,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        if affected != 1 {
            return Ok(false);
        }
        // ADR-0043 D1: 従来の `PATCH /projects {workspace}` は **primary のリポジトリ**を書き換える。
        let existing = Self::repo_list_tx(&tx, id)?;
        let primary = existing.iter().find(|r| r.is_primary).cloned();
        match (workspace, primary) {
            // 差し替え: primary の場所（と kind）だけを直す。名前・run・default_branch は人の設定を残す。
            (Some(location), Some(mut repo)) => {
                repo.location = location.clone();
                repo.kind = detect_repo_kind(location);
                if repo.kind == RepoKind::Dir {
                    repo.default_branch = None;
                }
                if matches!(location, WorkspaceSpec::Local { .. }) {
                    repo.sync = None;
                }
                let others: Vec<ProjectRepo> = existing
                    .iter()
                    .filter(|r| r.id != repo.id)
                    .cloned()
                    .collect();
                crate::repos::validate_upsert(&others, &repo)?;
                Self::repo_write_tx(&tx, &repo)?;
            }
            // 新規: primary がまだ無い案件に作業場所を付けた。
            (Some(location), None) => {
                let mut name = crate::repos::default_repo_name(location);
                if existing.iter().any(|r| r.name == name) {
                    name = format!("{name}-2");
                }
                let repo = ProjectRepo {
                    id: RepoId::new(),
                    project_id: id,
                    name,
                    kind: detect_repo_kind(location),
                    location: location.clone(),
                    default_branch: None,
                    sync: None,
                    run: RepoRun::Auto,
                    is_primary: true,
                    created_at: OffsetDateTime::now_utc(),
                };
                crate::repos::validate_upsert(&existing, &repo)?;
                Self::repo_write_tx(&tx, &repo)?;
                Self::repo_clear_other_primaries_tx(&tx, id, repo.id)?;
            }
            // 消す: `"workspace": null` は「案件を作業場所なしに戻す」なので primary の行を消す。
            // 未終端のタスクが使っていれば 409（`DELETE /repos/{id}` と同じ規律）。
            (None, Some(repo)) => {
                let open = Self::repo_active_tasks_tx(&tx, repo.id)?;
                if !open.is_empty() {
                    return Err(StoreError::InUse {
                        kind: "project repo",
                        id: repo.id.to_string(),
                        detail: format!("{} task(s) using it have not finished", open.len()),
                    });
                }
                tx.execute(
                    "DELETE FROM project_repos WHERE id = ?1",
                    params![repo.id.to_string()],
                )?;
            }
            (None, None) => {}
        }
        Self::sync_project_workspace_tx(&tx, id)?;
        tx.commit()?;
        Ok(true)
    }

    pub(super) fn milestone_create_impl(
        &self,
        project_id: ProjectId,
        title: &str,
        description: &str,
        status: MilestoneStatus,
    ) -> Result<Milestone, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![project_id.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::Invalid(format!(
                "project not found: {project_id}"
            )));
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM milestones WHERE project_id = ?1",
            params![project_id.to_string()],
            |row| row.get(0),
        )?;
        let now = OffsetDateTime::now_utc();
        let milestone = Milestone {
            plan_key: None,
            id: MilestoneId::new(),
            project_id,
            seq,
            title: title.to_string(),
            description: description.to_string(),
            status,
            paused_from: None,
            created_at: now,
            updated_at: now,
        };
        tx.execute(
            "INSERT INTO milestones (id, project_id, seq, title, description, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                milestone.id.to_string(),
                milestone.project_id.to_string(),
                milestone.seq,
                milestone.title,
                milestone.description,
                milestone.status.as_str(),
                format_rfc3339(milestone.created_at)?,
                format_rfc3339(milestone.updated_at)?,
            ],
        )?;
        tx.commit()?;
        Ok(milestone)
    }

    pub(super) fn milestone_set_plan_key_impl(
        &self,
        id: MilestoneId,
        plan_key: &str,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE milestones SET plan_key = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                plan_key,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn milestone_list_impl(
        &self,
        project_id: ProjectId,
    ) -> Result<Vec<Milestone>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, seq, title, description, status, created_at, updated_at, paused_from, plan_key \
                 FROM milestones WHERE project_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![project_id.to_string()], Self::milestone_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    pub(super) fn milestone_get_impl(
        &self,
        id: MilestoneId,
    ) -> Result<Option<Milestone>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, project_id, seq, title, description, status, created_at, updated_at, paused_from, plan_key \
                     FROM milestones WHERE id = ?1",
                    params![id.to_string()],
                    Self::milestone_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    pub(super) fn milestone_set_status_impl(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                status.as_str(),
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    pub(super) fn milestone_transition_status_impl(
        &self,
        id: MilestoneId,
        from: MilestoneStatus,
        to: MilestoneStatus,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status = ?4",
            params![
                to.as_str(),
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string(),
                from.as_str(),
            ],
        )?;
        Ok(affected == 1)
    }

    /// ADR-0044 D6（Phase 55）: 状態と `paused_from` を 1 回の UPDATE で書く（`project_set_lifecycle` と同じ）。
    pub(super) fn milestone_set_lifecycle_impl(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
        paused_from: Option<Option<MilestoneStatus>>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        Self::milestone_set_lifecycle_tx(&conn, id, status, paused_from)
    }

    /// `milestone_set_lifecycle` on a caller-owned transaction.
    pub fn milestone_set_lifecycle_tx(
        conn: &Connection,
        id: MilestoneId,
        status: MilestoneStatus,
        paused_from: Option<Option<MilestoneStatus>>,
    ) -> Result<bool, StoreError> {
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let affected = match paused_from {
            Some(from) => conn.execute(
                "UPDATE milestones SET status = ?1, paused_from = ?2, updated_at = ?3 WHERE id = ?4",
                params![status.as_str(), from.map(|s| s.as_str()), now, id.to_string()],
            )?,
            None => conn.execute(
                "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.to_string()],
            )?,
        };
        Ok(affected == 1)
    }
}
