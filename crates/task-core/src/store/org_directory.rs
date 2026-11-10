//! ADR 2026-11-10-taskstore-consumer-ports D2: 組織のノード 1 つ・案件・途中目標の名簿を読む能力。
//!
//! `TaskStore` の supertrait（`dyn TaskStore`・`T: TaskStore` の呼び手は import なしで使える）。
//! 名簿を読むだけの consumer（celeris-mcp の `tools/org`・`tools/projects`）はこの trait に依存する。
//! 実装（SQL）は下の `impl OrgDirectory for SqliteStore` にだけある（前例は `model_catalog.rs`）。行の変換は
//! `org.rs` の `org_row`・`projects.rs` の `project_row`・`milestone_row` を使う。
//! `org_list`・`org_upsert` は `TaskStore` に残す（UFCS `TaskStore::org_upsert` と具体型の呼び手が
//! R3 の write_set の外にあるため。ADR の付記 2026-11-10 v2）。

use rusqlite::{OptionalExtension, params};

use crate::org::{Milestone, OrgNode, Project, ProjectId};

use super::{SqliteStore, StoreError};

pub trait OrgDirectory: Send + Sync {
    /// ADR-0033 D1: 組織のノード 1 つ。無ければ `None`。
    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError>;
    /// ADR-0033 D2: 案件を `created_at` の降順（新しい案件が先）で返す。
    fn project_list(&self) -> Result<Vec<Project>, StoreError>;
    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError>;
    /// その案件の途中目標を `seq` 昇順で返す。
    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError>;
}

impl OrgDirectory for SqliteStore {
    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, parent_id, name, kind, genre, brief, position, created_at, updated_at, profile_json \
                     FROM org_nodes WHERE id = ?1",
                    params![id],
                    Self::org_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    fn project_list(&self) -> Result<Vec<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.title, p.request, p.status, p.created_at, p.updated_at, \
                     COALESCE((SELECT r.location_json FROM project_repos r \
                               WHERE r.project_id = p.id AND r.is_primary = 1 \
                               ORDER BY r.created_at ASC, r.id ASC LIMIT 1), p.workspace), \
                     p.archived_at, p.paused_from, p.auto_advance, p.slug, p.auto_integrate \
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

    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT p.id, p.title, p.request, p.status, p.created_at, p.updated_at, \
                     COALESCE((SELECT r.location_json FROM project_repos r \
                               WHERE r.project_id = p.id AND r.is_primary = 1 \
                               ORDER BY r.created_at ASC, r.id ASC LIMIT 1), p.workspace), \
                     p.archived_at, p.paused_from, p.auto_advance, p.slug, p.auto_integrate \
                     FROM projects p WHERE p.id = ?1",
                    params![id.to_string()],
                    Self::project_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError> {
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
}
