//! ADR 2026-11-10-taskstore-consumer-ports D2: 組織のノード 1 つ・案件・途中目標の名簿を読む能力。
//!
//! `TaskStore` の supertrait（`dyn TaskStore`・`T: TaskStore` の呼び手は import なしで使える）。
//! 名簿を読むだけの consumer（celeris-mcp の `tools/org`・`tools/projects`）はこの trait に依存する。
//! 実装は `org.rs`・`projects.rs` の `<method>_impl` への 1 行転送で、SQL はここに無い。
//! `org_list`・`org_upsert` は `TaskStore` に残す（UFCS `TaskStore::org_upsert` と具体型の呼び手が
//! R3 の write_set の外にあるため。ADR の付記 2026-11-10 v2）。

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
        self.org_get_impl(id)
    }

    fn project_list(&self) -> Result<Vec<Project>, StoreError> {
        self.project_list_impl()
    }

    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        self.project_get_impl(id)
    }

    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError> {
        self.milestone_list_impl(project_id)
    }
}
