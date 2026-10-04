//! ADR-0130 D4: behind-target snapshots. Git measurement belongs to callers (`task-ops`).

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::behind_target::{BehindTargetObservation, BehindTargetSnapshot, next_behind_since};
use crate::model::TaskId;

use super::{SqliteStore, StoreError, parse_rfc3339};

const COLUMNS: &str = "task_id, repo_id, target_ref, target_sha, head_sha, behind_commits, \
                       behind_since, observed_at";

fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, BehindRow)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        BehindRow {
            target_ref: row.get(2)?,
            target_sha: row.get(3)?,
            head_sha: row.get(4)?,
            behind_commits: row.get(5)?,
            behind_since: row.get(6)?,
            observed_at: row.get(7)?,
        },
    ))
}

struct BehindRow {
    target_ref: String,
    target_sha: Option<String>,
    head_sha: Option<String>,
    behind_commits: Option<i64>,
    behind_since: Option<String>,
    observed_at: String,
}

fn into_snapshot(
    (task_id, repo_id, row): (String, String, BehindRow),
) -> Result<BehindTargetSnapshot, StoreError> {
    Ok(BehindTargetSnapshot {
        task_id: task_id
            .parse()
            .map_err(|e| StoreError::Invalid(format!("task_behind_targets.task_id: {e}")))?,
        repo_id: repo_id
            .parse()
            .map_err(|e| StoreError::Invalid(format!("task_behind_targets.repo_id: {e:?}")))?,
        target_ref: row.target_ref,
        target_sha: row.target_sha,
        head_sha: row.head_sha,
        behind_target_commits: row.behind_commits.and_then(|n| u64::try_from(n).ok()),
        behind_target_since: row.behind_since,
        behind_target_observed_at: row.observed_at,
    })
}

fn load_one(
    conn: &Connection,
    task_id: &str,
    repo_id: &str,
) -> Result<Option<BehindTargetSnapshot>, StoreError> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM task_behind_targets WHERE task_id = ?1 AND repo_id = ?2"),
        params![task_id, repo_id],
        row_to_snapshot,
    )
    .optional()?
    .map(into_snapshot)
    .transpose()
}

impl SqliteStore {
    /// ADR-0130 D4: store one measurement. An observation older than the stored one is
    /// ignored (returns the stored snapshot) so a stale reading never overwrites a newer one.
    pub fn record_behind_target(
        &self,
        obs: &BehindTargetObservation,
    ) -> Result<BehindTargetSnapshot, StoreError> {
        let observed = parse_rfc3339(&obs.observed_at)?;
        let task_id = obs.task_id.to_string();
        let repo_id = obs.repo_id.to_string();
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prev = load_one(&tx, &task_id, &repo_id)?;
        if let Some(prev) = &prev
            && parse_rfc3339(&prev.behind_target_observed_at)? > observed
        {
            return Ok(prev.clone());
        }
        let since = next_behind_since(prev.as_ref(), obs);
        let commits = obs
            .commits
            .map(|n| {
                i64::try_from(n)
                    .map_err(|_| StoreError::Invalid(format!("behind commits too large: {n}")))
            })
            .transpose()?;
        tx.execute(
            &format!(
                "INSERT INTO task_behind_targets ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(task_id, repo_id) DO UPDATE SET target_ref = excluded.target_ref, \
                 target_sha = excluded.target_sha, head_sha = excluded.head_sha, \
                 behind_commits = excluded.behind_commits, behind_since = excluded.behind_since, \
                 observed_at = excluded.observed_at"
            ),
            params![
                task_id,
                repo_id,
                obs.target_ref,
                obs.target_sha,
                obs.head_sha,
                commits,
                since,
                obs.observed_at
            ],
        )?;
        tx.commit()?;
        Ok(BehindTargetSnapshot {
            task_id: obs.task_id,
            repo_id: obs.repo_id,
            target_ref: obs.target_ref.clone(),
            target_sha: obs.target_sha.clone(),
            head_sha: obs.head_sha.clone(),
            behind_target_commits: obs.commits,
            behind_target_since: since,
            behind_target_observed_at: obs.observed_at.clone(),
        })
    }

    /// Last snapshot per repo (repo id order).
    pub fn behind_targets(&self, task_id: TaskId) -> Result<Vec<BehindTargetSnapshot>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM task_behind_targets WHERE task_id = ?1 ORDER BY repo_id"
            ))?;
            let rows = stmt
                .query_map(params![task_id.to_string()], row_to_snapshot)?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().map(into_snapshot).collect()
        })
    }
}
