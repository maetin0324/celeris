//! ADR-0130 D4: measure how far a task branch is behind its current target
//! (`git rev-list --count <head_sha>..<target_sha>`) and keep the snapshot in the store.
//!
//! Git failures never fail the caller: the fields become `None` (never 0).

use std::path::Path;

use task_core::behind_target::{
    BehindTarget, BehindTargetObservation, BehindTargetSnapshot, summarize_behind_target,
};
use task_core::{RepoId, TaskId, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::changes::{GIT_TIMEOUT, git};
use crate::error::OpsError;

/// One Git reading. Both SHAs are read first and the count uses those SHAs, so a target that
/// moves during the measurement cannot mix two states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehindMeasure {
    pub target_sha: Option<String>,
    pub head_sha: Option<String>,
    pub commits: Option<u64>,
}

fn rev_parse(dir: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    let out = git(
        dir,
        &["rev-parse", "--verify", "--quiet", &spec],
        GIT_TIMEOUT,
    )?;
    let sha = out.stdout.trim();
    (out.ok && !sha.is_empty()).then(|| sha.to_string())
}

/// Read `head_ref` and `target_ref` in `dir` (a worktree or the repo) and count the commits
/// that only the target has. Any unreadable piece leaves `commits` as `None`.
pub fn measure_behind_target(dir: &Path, head_ref: &str, target_ref: &str) -> BehindMeasure {
    let target_sha = rev_parse(dir, target_ref);
    let head_sha = rev_parse(dir, head_ref);
    let commits = match (&head_sha, &target_sha) {
        (Some(head), Some(target)) => {
            let range = format!("{head}..{target}");
            git(dir, &["rev-list", "--count", &range], GIT_TIMEOUT)
                .filter(|out| out.ok)
                .and_then(|out| out.stdout.trim().parse::<u64>().ok())
        }
        _ => None,
    };
    BehindMeasure {
        target_sha,
        head_sha,
        commits,
    }
}

fn rfc3339(now: OffsetDateTime) -> Result<String, OpsError> {
    now.format(&Rfc3339).map_err(|e| {
        OpsError::Store(task_core::StoreError::Invalid(format!(
            "behind_target: cannot format time: {e}"
        )))
    })
}

/// Measure `HEAD` of `worktree` against `target_ref` and store the snapshot (ADR-0130 D4
/// measurement points: before dispatch / review sync and after sync).
pub fn observe_behind_target(
    store: &dyn TaskStore,
    task_id: TaskId,
    repo_id: RepoId,
    worktree: &Path,
    target_ref: &str,
    now: OffsetDateTime,
) -> Result<BehindTargetSnapshot, OpsError> {
    let measure = measure_behind_target(worktree, "HEAD", target_ref);
    let obs = BehindTargetObservation {
        task_id,
        repo_id,
        target_ref: target_ref.to_string(),
        target_sha: measure.target_sha,
        head_sha: measure.head_sha,
        commits: measure.commits,
        observed_at: rfc3339(now)?,
    };
    Ok(store.record_behind_target(&obs)?)
}

/// Last stored snapshots folded at `now` (API/metrics read; never touches Git).
pub fn behind_target_of(
    store: &dyn TaskStore,
    task_id: TaskId,
    now: OffsetDateTime,
) -> Result<BehindTarget, OpsError> {
    Ok(summarize_behind_target(
        &store.behind_targets(task_id)?,
        now,
    ))
}

#[cfg(test)]
mod tests;
