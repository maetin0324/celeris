//! ADR 2026-10-27-project-brief: `project_briefs`（案件の状態の要約、1 案件 1 行・全置換）と、
//! 案件ごとの task の活動（状態ごとの件数・最終更新）。

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::org::ProjectId;
use crate::project_brief::{ProjectActivity, ProjectBrief};

use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

const BRIEF_COLUMNS: &str = "project_id, objective, state, open_issues_json, next_actions_json, \
     waiting_on_json, evidence_refs_json, updated_at, updated_by";

fn brief_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<ProjectBrief, StoreError>> {
    let id: String = row.get(0)?;
    let objective: String = row.get(1)?;
    let state: String = row.get(2)?;
    let lists: [String; 4] = [row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?];
    let updated_at: String = row.get(7)?;
    let updated_by: String = row.get(8)?;
    Ok((|| {
        let project_id = id
            .parse::<ProjectId>()
            .map_err(|_| StoreError::Invalid(format!("invalid project_briefs row: id={id}")))?;
        let [open_issues, next_actions, waiting_on, evidence_refs] = lists;
        Ok(ProjectBrief {
            project_id,
            objective,
            state,
            open_issues: serde_json::from_str(&open_issues)?,
            next_actions: serde_json::from_str(&next_actions)?,
            waiting_on: serde_json::from_str(&waiting_on)?,
            evidence_refs: serde_json::from_str(&evidence_refs)?,
            updated_at: parse_rfc3339(&updated_at)?,
            updated_by,
        })
    })())
}

impl SqliteStore {
    /// 案件の状態の要約（無ければ `None`）。
    pub fn project_brief_get(&self, id: ProjectId) -> Result<Option<ProjectBrief>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                &format!("SELECT {BRIEF_COLUMNS} FROM project_briefs WHERE project_id = ?1"),
                params![id.to_string()],
                brief_row,
            )
            .optional()?
            .transpose()
        })
    }

    /// 全案件の状態の要約（案件 id → 要約）。
    pub fn project_brief_list(&self) -> Result<HashMap<ProjectId, ProjectBrief>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!("SELECT {BRIEF_COLUMNS} FROM project_briefs"))?;
            let rows = stmt.query_map([], brief_row)?;
            let mut out = HashMap::new();
            for row in rows {
                let brief = row??;
                out.insert(brief.project_id, brief);
            }
            Ok(out)
        })
    }

    /// 要約を全置換で書く。案件が無ければ `false`（何も書かない）。
    pub fn project_brief_put(&self, brief: &ProjectBrief) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        Self::project_brief_put_tx(&conn, brief)
    }

    /// [`Self::project_brief_put`] on a caller-owned transaction (CoS op `project.status_put`).
    pub fn project_brief_put_tx(
        conn: &Connection,
        brief: &ProjectBrief,
    ) -> Result<bool, StoreError> {
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                params![brief.project_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(false);
        }
        conn.execute(
            &format!(
                "INSERT INTO project_briefs ({BRIEF_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(project_id) DO UPDATE SET objective = excluded.objective, \
                 state = excluded.state, open_issues_json = excluded.open_issues_json, \
                 next_actions_json = excluded.next_actions_json, \
                 waiting_on_json = excluded.waiting_on_json, \
                 evidence_refs_json = excluded.evidence_refs_json, \
                 updated_at = excluded.updated_at, updated_by = excluded.updated_by"
            ),
            params![
                brief.project_id.to_string(),
                brief.objective,
                brief.state,
                serde_json::to_string(&brief.open_issues)?,
                serde_json::to_string(&brief.next_actions)?,
                serde_json::to_string(&brief.waiting_on)?,
                serde_json::to_string(&brief.evidence_refs)?,
                format_rfc3339(brief.updated_at)?,
                brief.updated_by,
            ],
        )?;
        Ok(true)
    }

    /// 案件ごとの task の活動（状態ごとの件数と `updated_at` の最大）。task の無い案件は入らない。
    pub fn project_activity(&self) -> Result<HashMap<ProjectId, ProjectActivity>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT project_id, status, COUNT(*), MAX(json_extract(json, '$.updated_at')) \
                 FROM tasks WHERE project_id IS NOT NULL GROUP BY project_id, status",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            let mut out: HashMap<ProjectId, ProjectActivity> = HashMap::new();
            for row in rows {
                let (project, status, count, updated_at) = row?;
                // 案件 id の形でない値（手で書かれた行など）は数えない。
                let Ok(project) = project.parse::<ProjectId>() else {
                    continue;
                };
                let activity = out.entry(project).or_default();
                *activity.task_counts.entry(status).or_default() +=
                    u32::try_from(count).unwrap_or(u32::MAX);
                if let Some(at) = updated_at.as_deref().map(parse_rfc3339).transpose()? {
                    activity.last_task_update = activity.last_task_update.max(Some(at));
                }
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use crate::org::{Project, ProjectStatus};
    use crate::project_brief::{ProjectBriefInput, WaitingOn, WaitingOnKind};
    use crate::{SqliteStore, TaskStore};

    use super::*;

    fn project(store: &SqliteStore) -> Project {
        let now = OffsetDateTime::now_utc();
        let p = Project {
            id: ProjectId::new(),
            title: "案件".into(),
            request: "依頼".into(),
            status: ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            auto_integrate: false,
            created_at: now,
            updated_at: now,
        };
        store.project_create(&p).unwrap();
        p
    }

    fn brief(id: ProjectId, state: &str) -> ProjectBrief {
        ProjectBrief::from_input(
            id,
            ProjectBriefInput {
                objective: "目的".into(),
                state: state.into(),
                open_issues: vec!["未解決".into()],
                next_actions: vec!["次".into()],
                waiting_on: vec![WaitingOn {
                    kind: WaitingOnKind::External,
                    target: Some("sirius".into()),
                    note: "保守".into(),
                }],
                evidence_refs: vec!["run:1".into()],
            },
            "cos:t",
            OffsetDateTime::now_utc().replace_nanosecond(0).unwrap(),
        )
    }

    #[test]
    fn put_replaces_and_reads_back() {
        let store = SqliteStore::open_in_memory().unwrap();
        let p = project(&store);
        assert!(store.project_brief_get(p.id).unwrap().is_none());
        let first = brief(p.id, "一");
        assert!(store.project_brief_put(&first).unwrap());
        assert_eq!(store.project_brief_get(p.id).unwrap(), Some(first));
        let mut second = brief(p.id, "二");
        second.open_issues.clear();
        assert!(store.project_brief_put(&second).unwrap());
        assert_eq!(store.project_brief_get(p.id).unwrap(), Some(second.clone()));
        assert_eq!(
            store.project_brief_list().unwrap().get(&p.id),
            Some(&second)
        );
    }

    #[test]
    fn put_for_a_missing_project_writes_nothing() {
        let store = SqliteStore::open_in_memory().unwrap();
        let id = ProjectId::new();
        assert!(!store.project_brief_put(&brief(id, "x")).unwrap());
        assert!(store.project_brief_list().unwrap().is_empty());
    }

    #[test]
    fn activity_counts_tasks_by_status_and_latest_update() {
        let store = SqliteStore::open_in_memory().unwrap();
        let p = project(&store);
        let other = project(&store);
        let mut tasks = Vec::new();
        for (i, status) in [
            crate::Status::Ready,
            crate::Status::Ready,
            crate::Status::Done,
        ]
        .into_iter()
        .enumerate()
        {
            let mut t = crate::store::tests::sample_task(status);
            t.project_id = Some(p.id);
            t.updated_at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(i as i64 + 1);
            store.insert(&t).unwrap();
            tasks.push(t);
        }
        let activity = store.project_activity().unwrap();
        let a = activity.get(&p.id).unwrap();
        assert_eq!(a.task_counts.get("ready"), Some(&2));
        assert_eq!(a.task_counts.get("done"), Some(&1));
        assert_eq!(a.open_tasks(), 2);
        assert_eq!(a.last_task_update, Some(tasks[2].updated_at));
        assert!(!activity.contains_key(&other.id));
    }

    /// D1: migration 0095 は `secretary_summary` に値のあった案件を `state` に写してから列を消す。
    #[test]
    fn migration_0095_moves_a_secretary_summary_into_a_brief() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("celeris.db");
        let (kept, empty) = {
            let store = SqliteStore::open(&path).unwrap();
            (project(&store), project(&store))
        };
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE project_briefs; ALTER TABLE projects ADD COLUMN secretary_summary TEXT; \
                 DELETE FROM schema_migrations WHERE version = 95;",
            )
            .unwrap();
            conn.execute(
                "UPDATE projects SET secretary_summary = ?1 WHERE id = ?2",
                params!["方針の要約", kept.id.to_string()],
            )
            .unwrap();
            conn.execute(
                "UPDATE projects SET secretary_summary = '  ' WHERE id = ?1",
                params![empty.id.to_string()],
            )
            .unwrap();
        }
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
        let brief = store.project_brief_get(kept.id).unwrap().unwrap();
        assert_eq!(brief.state, "方針の要約");
        assert_eq!(brief.objective, "依頼");
        assert_eq!(brief.updated_by, "migration:0095");
        assert!(store.project_brief_get(empty.id).unwrap().is_none());
        let conn = Connection::open(&path).unwrap();
        let columns: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('projects')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!columns.iter().any(|c| c == "secretary_summary"));
    }
}
