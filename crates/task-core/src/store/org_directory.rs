//! ADR 2026-11-10-taskstore-consumer-ports D2: 組織のノード 1 つ・案件・途中目標の名簿を読む能力。
//!
//! `TaskStore` の supertrait（`dyn TaskStore`・`T: TaskStore` の呼び手は import なしで使える）。
//! 名簿を読むだけの consumer（celeris-mcp の `tools/org`・`tools/projects`）はこの trait に依存する。
//! 実装（SQL）は下の `impl OrgDirectory for SqliteStore` にだけある（前例は `model_catalog.rs`）。行の変換は
//! `org.rs` の `org_row`・`projects.rs` の `project_row`・`milestone_row` を使う。
//! org の一覧・取得・upsert と案件/途中目標の名簿操作を集約する。SQL 本体はこの trait の実装に置く。

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::org::{Milestone, OrgNode, Project, ProjectId};

use super::{SqliteStore, StoreError};

pub trait OrgDirectory: Send + Sync {
    /// 全ノードを `position`、同値なら `id` の昇順で返す（木は呼び出し側が `parent_id` で組む）。
    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError>;
    /// ADR-0033 D1: 組織のノード 1 つ。無ければ `None`。
    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError>;
    /// 挿入または更新。`crate::org::validate_upsert` を通してから書く（secretary は 1 つ、親は既存、
    /// 自分を祖先にできない、secretary > department > section）。`created_at` は既存行のものを保つ。
    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError>;
    /// ADR-0033 D2: 案件を `created_at` の降順（新しい案件が先）で返す。
    fn project_list(&self) -> Result<Vec<Project>, StoreError>;
    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError>;
    /// その案件の途中目標を `seq` 昇順で返す。
    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError>;
}

impl OrgDirectory for SqliteStore {
    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError> {
        self.with_read_conn(Self::org_list_tx)
    }

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

    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = Self::org_upsert_tx(&tx, node, None)?;
        tx.commit()?;
        Ok(stored)
    }

    fn project_list(&self) -> Result<Vec<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.title, p.request, p.status, p.created_at, p.updated_at, \
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

    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT p.id, p.title, p.request, p.status, p.created_at, p.updated_at, \
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
