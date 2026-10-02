//! ADR-0130 D4: behind commits / age of a task branch relative to its current target.
//!
//! Git collection lives in `task-ops`; this module only holds the stored snapshot and the
//! pure rules (when `behind_target_since` is kept or cleared, how the task value is chosen).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::model::TaskId;
use crate::repos::RepoId;

/// One measurement of `git rev-list --count <head_sha>..<target_sha>` for one repo.
/// `commits == None` means Git/ref could not be read (never stored as 0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehindTargetObservation {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    /// The target series (e.g. `refs/heads/main` or the parent task branch).
    pub target_ref: String,
    pub target_sha: Option<String>,
    pub head_sha: Option<String>,
    pub commits: Option<u64>,
    pub observed_at: String,
}

/// Stored per-repo snapshot (`task_behind_targets`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehindTargetSnapshot {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    pub target_ref: String,
    pub target_sha: Option<String>,
    pub head_sha: Option<String>,
    pub behind_target_commits: Option<u64>,
    /// First observation (UTC) of a positive behind in the same target series.
    pub behind_target_since: Option<String>,
    pub behind_target_observed_at: String,
}

/// ADR-0130 D4: `behind_target_since` after `obs`, given the previous snapshot.
///
/// - positive behind: keep the previous `since` while the target series is the same and it was
///   already positive, otherwise start at `obs.observed_at`
/// - 0: cleared
/// - unreadable (`None`): keep the previous value of the same series (no evidence either way)
pub fn next_behind_since(
    prev: Option<&BehindTargetSnapshot>,
    obs: &BehindTargetObservation,
) -> Option<String> {
    let same_series = prev.filter(|p| p.target_ref == obs.target_ref);
    match obs.commits {
        Some(0) => None,
        Some(_) => same_series
            .and_then(|p| p.behind_target_since.clone())
            .or_else(|| Some(obs.observed_at.clone())),
        None => same_series.and_then(|p| p.behind_target_since.clone()),
    }
}

/// Per-repo value with the age computed at `now`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BehindTargetRepo {
    pub repo_id: String,
    pub target_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// `null` = 計測不可（0 にしない）。
    pub behind_target_commits: Option<u64>,
    /// `null` = 未観測・計測不可。behind 0 なら 0。
    pub behind_target_age_seconds: Option<u64>,
    pub behind_target_observed_at: String,
}

/// Task value: the repo with the largest behind commits (ADR-0130 D4), plus all repos.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BehindTarget {
    pub behind_target_commits: Option<u64>,
    pub behind_target_age_seconds: Option<u64>,
    pub behind_target_observed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<BehindTargetRepo>,
}

fn age_seconds(snapshot: &BehindTargetSnapshot, now: OffsetDateTime) -> Option<u64> {
    match snapshot.behind_target_commits? {
        0 => Some(0),
        _ => {
            let since =
                OffsetDateTime::parse(snapshot.behind_target_since.as_deref()?, &Rfc3339).ok()?;
            let secs = (now - since).whole_seconds();
            Some(u64::try_from(secs).unwrap_or(0))
        }
    }
}

/// Fold stored snapshots into the API/metrics value. Never touches Git.
pub fn summarize_behind_target(
    snapshots: &[BehindTargetSnapshot],
    now: OffsetDateTime,
) -> BehindTarget {
    let mut repos: Vec<BehindTargetRepo> = snapshots
        .iter()
        .map(|s| BehindTargetRepo {
            repo_id: s.repo_id.to_string(),
            target_ref: s.target_ref.clone(),
            target_sha: s.target_sha.clone(),
            head_sha: s.head_sha.clone(),
            behind_target_commits: s.behind_target_commits,
            behind_target_age_seconds: age_seconds(s, now),
            behind_target_observed_at: s.behind_target_observed_at.clone(),
        })
        .collect();
    repos.sort_by(|a, b| a.repo_id.cmp(&b.repo_id));
    let representative = repos
        .iter()
        .filter(|r| r.behind_target_commits.is_some())
        .max_by(|a, b| {
            a.behind_target_commits
                .cmp(&b.behind_target_commits)
                .then(
                    a.behind_target_age_seconds
                        .cmp(&b.behind_target_age_seconds),
                )
                // ties: the smallest repo id wins (max_by keeps the last max)
                .then(b.repo_id.cmp(&a.repo_id))
        });
    BehindTarget {
        behind_target_commits: representative.and_then(|r| r.behind_target_commits),
        behind_target_age_seconds: representative.and_then(|r| r.behind_target_age_seconds),
        behind_target_observed_at: representative.map(|r| r.behind_target_observed_at.clone()),
        repos,
    }
}

#[cfg(test)]
mod tests;
