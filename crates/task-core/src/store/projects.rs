use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Task, WorkspaceSpec};
use crate::org::{Milestone, MilestoneId, MilestoneStatus, Project, ProjectId, ProjectStatus};

use super::{SqliteStore, StoreError, parse_rfc3339};

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
}
