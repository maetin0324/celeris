//! 全体棚卸しの追記だけの記録（ADR 2026-11-08-health-enforcement-loop D6）。
//! 発火ごとの数はこの行から導き、別の集計や判定を保存しない。表 `health_inventory_runs`
//! への読み書きは `store.rs` の `SqliteStore` の固有 method（追記・読み出しだけ。更新・削除は無い）。
//! store module の外に置くのは、repo health の size.responsibility-growth の単位を
//! `task-core/src/store` と分けるため（`chat/store.rs`・`browser_wait/sql.rs` と同じ形）。
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::cron::CronJobId;
use crate::required_checks::health::HealthEvidence;
use crate::{RepoId, TaskId};

mod store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthInventoryOutcome {
    Evaluated,
    Unchanged,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthInventoryRun {
    pub id: Ulid,
    pub job_id: CronJobId,
    pub scheduled_for: OffsetDateTime,
    pub repo_id: RepoId,
    pub outcome: HealthInventoryOutcome,
    pub evidence: Option<HealthEvidence>,
    pub task_ids: Vec<TaskId>,
    pub existing_skipped: u64,
    pub limit_remaining: u64,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HealthInventoryFireCounts {
    pub repos: u64,
    pub evaluated: u64,
    pub unchanged: u64,
    pub not_applicable: u64,
    pub tasks_created: u64,
    pub existing_skipped: u64,
    pub limit_remaining: u64,
}

#[cfg(test)]
mod tests;
