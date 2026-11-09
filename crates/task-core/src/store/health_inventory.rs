//! Append-only inventory evidence (ADR 2026-11-08-health-enforcement-loop D6).
//! Counts are derived from these rows; no second persisted summary or verdict exists.
use rusqlite::params;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ulid::Ulid;

use super::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};
use crate::cron::CronJobId;
use crate::required_checks::health::{HealthEvidence, HealthVerdict};
use crate::{RepoId, TaskId};

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

impl SqliteStore {
    /// Append one repo's record. Existing rows are never replaced.
    pub fn health_inventory_append(&self, run: &HealthInventoryRun) -> Result<(), StoreError> {
        if (run.outcome == HealthInventoryOutcome::Evaluated) != run.evidence.is_some()
            || run
                .evidence
                .as_ref()
                .is_some_and(|e| e.repo_id != run.repo_id)
        {
            return Err(StoreError::Invalid(
                "inventory outcome/evidence mismatch".into(),
            ));
        }
        let outcome = serde_json::to_value(run.outcome)?;
        self.lock()?.execute(
            "INSERT INTO health_inventory_runs (id, job_id, scheduled_for, repo_id, outcome, \
             evidence_json, task_ids_json, existing_skipped, limit_remaining, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run.id.to_string(),
                run.job_id.to_string(),
                format_rfc3339(run.scheduled_for)?,
                run.repo_id.to_string(),
                outcome.as_str(),
                run.evidence
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                serde_json::to_string(&run.task_ids)?,
                run.existing_skipped,
                run.limit_remaining,
                format_rfc3339(run.created_at)?
            ],
        )?;
        Ok(())
    }

    /// Last successful evaluation by scheduled instant, then recording instant and append order.
    /// Error and skipped rows cannot advance the next diff's base.
    pub fn health_inventory_latest_evaluated(
        &self,
        job_id: CronJobId,
        repo_id: RepoId,
    ) -> Result<Option<HealthInventoryRun>, StoreError> {
        Ok(self
            .health_inventory_rows(job_id)?
            .into_iter()
            .filter(|r| {
                r.repo_id == repo_id
                    && r.outcome == HealthInventoryOutcome::Evaluated
                    && r.evidence
                        .as_ref()
                        .is_some_and(|e| e.verdict != HealthVerdict::Error)
            })
            .max_by_key(|r| (r.scheduled_for, r.created_at)))
    }

    /// Compare parsed instants, not RFC3339 text (offsets and fractional seconds may differ).
    pub fn health_inventory_fire_counts(
        &self,
        job_id: CronJobId,
        scheduled_for: OffsetDateTime,
    ) -> Result<HealthInventoryFireCounts, StoreError> {
        let mut counts = HealthInventoryFireCounts::default();
        for row in self
            .health_inventory_rows(job_id)?
            .into_iter()
            .filter(|r| r.scheduled_for == scheduled_for)
        {
            counts.repos += 1;
            match row.outcome {
                HealthInventoryOutcome::Evaluated => counts.evaluated += 1,
                HealthInventoryOutcome::Unchanged => counts.unchanged += 1,
                HealthInventoryOutcome::NotApplicable => counts.not_applicable += 1,
            }
            counts.tasks_created += row.task_ids.len() as u64;
            counts.existing_skipped += row.existing_skipped;
            counts.limit_remaining += row.limit_remaining;
        }
        Ok(counts)
    }

    fn health_inventory_rows(
        &self,
        job_id: CronJobId,
    ) -> Result<Vec<HealthInventoryRun>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, scheduled_for, repo_id, outcome, evidence_json, \
                task_ids_json, existing_skipped, limit_remaining, created_at \
                FROM health_inventory_runs WHERE job_id = ?1 ORDER BY rowid",
            )?;
            let rows = stmt.query_map([job_id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, u64>(6)?,
                    r.get::<_, u64>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })?;
            rows.map(|raw| {
                let (id, scheduled, repo, outcome, evidence, tasks, existing, remaining, created) =
                    raw?;
                Ok(HealthInventoryRun {
                    id: id
                        .parse()
                        .map_err(|e| StoreError::Invalid(format!("inventory id: {e}")))?,
                    job_id,
                    scheduled_for: parse_rfc3339(&scheduled)?,
                    repo_id: repo
                        .parse()
                        .map_err(|e| StoreError::Invalid(format!("inventory repo: {e}")))?,
                    outcome: serde_json::from_value(serde_json::Value::String(outcome))?,
                    evidence: evidence.as_deref().map(serde_json::from_str).transpose()?,
                    task_ids: serde_json::from_str(&tasks)?,
                    existing_skipped: existing,
                    limit_remaining: remaining,
                    created_at: parse_rfc3339(&created)?,
                })
            })
            .collect()
        })
    }
}

#[cfg(test)]
#[path = "health_inventory_tests.rs"]
mod tests;
