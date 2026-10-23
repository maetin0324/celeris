//! Deterministic detection of completed task branches that have not reached the default branch.
use std::{path::Path, process::Command};

use serde::{Deserialize, Serialize};
use task_core::{
    RepoId, StoreError, TaskId, TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore,
};
use time::OffsetDateTime;

/// Caller supplied repository/release context; this module never reads daemon configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct IntegrationContext {
    pub repositories: std::collections::HashMap<RepoId, IntegrationRepoContext>,
    /// 最新の promoted release。`None` なら promotion は付けない。要求ごとに 1 回だけ読む（ADR 2026-10-21）。
    pub release: Option<NewestRelease>,
}

/// `releases_dir` を走査して得た最新の promoted release の sha（無ければ `None`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewestRelease(pub Option<String>);

impl NewestRelease {
    pub fn read(releases_dir: &Path) -> Self {
        Self(newest_release_sha(releases_dir))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntegrationRepoContext {
    pub name: String,
    pub path: std::path::PathBuf,
    pub default_branch: String,
}

/// Deployment status is independent of whether a task branch has reached the repository's main.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStatus {
    NotApplicable,
    PromotionPending,
    Promoted,
}

/// Read-only promotion view for a task branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PromotionView {
    pub status: PromotionStatus,
    pub release_sha: Option<String>,
}

/// Determine deployment state from the newest successfully promoted release and git ancestry.
/// Only the agent-platform repository participates; `releases_dir` is supplied by the caller.
pub fn promotion_view(
    repository: &str,
    repository_path: &Path,
    branch_head: &str,
    releases_dir: &Path,
) -> PromotionView {
    promotion_view_for(
        repository,
        repository_path,
        branch_head,
        &NewestRelease::read(releases_dir),
    )
}

fn newest_release_sha(releases_dir: &Path) -> Option<String> {
    std::fs::read_dir(releases_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("promoted.json").is_file())
        .filter_map(|entry| {
            let manifest = std::fs::read(entry.path().join("manifest.json")).ok()?;
            let value: serde_json::Value = serde_json::from_slice(&manifest).ok()?;
            let sha = value.get("sha")?.as_str()?.to_owned();
            let promoted = std::fs::read(entry.path().join("promoted.json")).ok()?;
            let value: serde_json::Value = serde_json::from_slice(&promoted).ok()?;
            let at = value.get("promoted_at")?.as_str()?.to_owned();
            Some((at, sha))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, sha)| sha)
}

/// `promotion_view` の本体。最新 release は呼び出し側が 1 回だけ読んで渡す。
pub fn promotion_view_for(
    repository: &str,
    repository_path: &Path,
    branch_head: &str,
    newest: &NewestRelease,
) -> PromotionView {
    if repository != "agent-platform" {
        return PromotionView {
            status: PromotionStatus::NotApplicable,
            release_sha: None,
        };
    }
    let Some(release_sha) = newest.0.clone() else {
        return PromotionView {
            status: PromotionStatus::PromotionPending,
            release_sha: None,
        };
    };
    let included = release_contains(repository_path, branch_head, &release_sha);
    PromotionView {
        status: if included {
            PromotionStatus::Promoted
        } else {
            PromotionStatus::PromotionPending
        },
        release_sha: Some(release_sha),
    }
}

/// ADR 2026-10-12-stranded-sweep-single-flight D4: ancestry between two fixed SHAs never changes,
/// so a decided answer (git exit 0 or 1) is remembered in-process. Failures are not remembered.
const PROMOTION_CACHE_LIMIT: usize = 4096;

type PromotionKey = (std::path::PathBuf, String, String);

fn promotion_cache() -> &'static std::sync::Mutex<std::collections::HashMap<PromotionKey, bool>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PromotionKey, bool>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn release_contains(repository_path: &Path, branch_head: &str, release_sha: &str) -> bool {
    let key = (
        repository_path.to_path_buf(),
        branch_head.to_owned(),
        release_sha.to_owned(),
    );
    if let Some(known) = promotion_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).copied())
    {
        return known;
    }
    let decided = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args(["merge-base", "--is-ancestor", branch_head, release_sha])
        .status()
        .ok()
        .and_then(|status| match status.code() {
            Some(0) => Some(true),
            Some(1) => Some(false),
            _ => None,
        });
    let Some(included) = decided else {
        return false;
    };
    if let Ok(mut cache) = promotion_cache().lock() {
        if cache.len() >= PROMOTION_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, included);
    }
    included
}

/// Explicit human disposition. Reasons are retained in the current row and append-only event log.
pub fn discard(
    store: &dyn TaskStore,
    task_id: TaskId,
    repo_id: RepoId,
    reason: &str,
    actor: &str,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    set_disposition(
        store,
        task_id,
        repo_id,
        TaskRepoIntegrationStatus::Discarded,
        reason,
        actor,
        now,
    )
}

pub fn hold(
    store: &dyn TaskStore,
    task_id: TaskId,
    repo_id: RepoId,
    reason: &str,
    actor: &str,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    set_disposition(
        store,
        task_id,
        repo_id,
        TaskRepoIntegrationStatus::Held,
        reason,
        actor,
        now,
    )
}

pub fn reopen(
    store: &dyn TaskStore,
    task_id: TaskId,
    repo_id: RepoId,
    actor: &str,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    let mut state = store
        .task_integration_get(task_id, repo_id)?
        .ok_or_else(|| StoreError::Invalid("task integration not found".into()))?;
    if state.state == TaskRepoIntegrationStatus::NotApplicable {
        return Err(StoreError::Invalid(
            "not-applicable integrations cannot be changed locally".into(),
        ));
    }
    if !matches!(
        state.state,
        TaskRepoIntegrationStatus::Held | TaskRepoIntegrationStatus::Discarded
    ) {
        return Err(StoreError::Invalid(
            "only held or discarded integrations can be reopened".into(),
        ));
    }
    state.state = TaskRepoIntegrationStatus::Unintegrated;
    state.reason = None;
    state.actor = actor.to_owned();
    state.checked_at = now;
    state.updated_at = now;
    store.task_integration_upsert(&state)
}

fn set_disposition(
    store: &dyn TaskStore,
    task_id: TaskId,
    repo_id: RepoId,
    disposition: TaskRepoIntegrationStatus,
    reason: &str,
    actor: &str,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    if reason.trim().is_empty() {
        return Err(StoreError::Invalid("a non-empty reason is required".into()));
    }
    if actor.trim().is_empty() {
        return Err(StoreError::Invalid("a non-empty actor is required".into()));
    }
    let mut state = store
        .task_integration_get(task_id, repo_id)?
        .ok_or_else(|| StoreError::Invalid("task integration not found".into()))?;
    if state.state == TaskRepoIntegrationStatus::NotApplicable {
        return Err(StoreError::Invalid(
            "not-applicable integrations cannot be changed locally".into(),
        ));
    }
    state.state = disposition;
    state.reason = Some(reason.trim().to_owned());
    state.actor = actor.to_owned();
    state.checked_at = now;
    state.updated_at = now;
    store.task_integration_upsert(&state)
}

pub struct MergeRequest<'a> {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    pub repo: &'a Path,
    pub branch: &'a str,
    pub head: &'a str,
    pub default_branch: &'a str,
    pub actor: &'a str,
    pub now: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Integrated,
    AlreadyIntegrated,
    NotMerged { reason: String },
    Busy { reason: String },
}

/// Merge the recorded task head into the configured default branch without rebasing.
pub fn merge(store: &dyn TaskStore, input: &MergeRequest<'_>) -> Result<MergeOutcome, StoreError> {
    use task_core::integration_request::{FileIntent, IntegrationRequest, SideIntent};
    let MergeRequest {
        task_id,
        repo_id,
        repo,
        branch,
        head,
        default_branch,
        actor,
        now,
    } = input;
    let mut state = store
        .task_integration_get(*task_id, *repo_id)?
        .ok_or_else(|| StoreError::Invalid("task integration not found".into()))?;
    if state.state == TaskRepoIntegrationStatus::NotApplicable {
        return Err(StoreError::Invalid(
            "remote repository was not checked by the sweep".into(),
        ));
    }
    let actual_head = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["rev-parse", &format!("refs/heads/{branch}")])
        .output()
        .map_err(|e| StoreError::Invalid(format!("git rev-parse failed: {e}")))?;
    let actual_head = String::from_utf8_lossy(&actual_head.stdout)
        .trim()
        .to_owned();
    if actual_head.is_empty()
        || !actual_head.eq_ignore_ascii_case(head)
        || !actual_head.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(StoreError::Invalid(
            "task branch head changed; refresh the integration record".into(),
        ));
    }
    if ancestor(repo, head, default_branch) {
        state.state = TaskRepoIntegrationStatus::Integrated;
        state.reason = None;
        state.actor = (*actor).to_owned();
        state.checked_at = *now;
        state.updated_at = *now;
        store.task_integration_upsert(&state)?;
        return Ok(MergeOutcome::AlreadyIntegrated);
    }
    let target = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["rev-parse", &format!("refs/heads/{default_branch}")])
        .output()
        .map_err(|e| StoreError::Invalid(format!("git rev-parse default branch failed: {e}")))?;
    let target_sha = String::from_utf8_lossy(&target.stdout).trim().to_owned();
    if !target.status.success() {
        return Err(StoreError::Invalid(
            "default branch cannot be resolved".into(),
        ));
    }
    let request = IntegrationRequest {
        target_branch: (*default_branch).into(),
        target_sha: target_sha.clone(),
        source_branch: (*branch).into(),
        source_sha: (*head).into(),
        merge_base: None,
        conflict_files: vec![],
        intent: vec![FileIntent {
            path: ".".into(),
            target: SideIntent {
                branch: (*default_branch).into(),
                path: ".".into(),
                commits: vec![],
                diffstat: None,
                unavailable: Some("review required by explicit merge request".into()),
            },
            source: SideIntent {
                branch: (*branch).into(),
                path: ".".into(),
                commits: vec![],
                diffstat: None,
                unavailable: Some("review required by explicit merge request".into()),
            },
        }],
        reason: format!("Explicit merge requested by {actor}"),
        recommendation: "Resolve the reported merge issue and retry the explicit merge".into(),
        actions: vec![],
        candidate_sha: None,
    };
    let record_failure = |reason: String| -> Result<MergeOutcome, StoreError> {
        let mut failed = state.clone();
        failed.state = TaskRepoIntegrationStatus::Unintegrated;
        failed.reason = Some(reason.clone());
        failed.actor = (*actor).to_owned();
        failed.checked_at = *now;
        failed.updated_at = *now;
        store.task_integration_upsert(&failed)?;
        store.integration_request_record(*task_id, &request, "delivery")?;
        Ok(MergeOutcome::NotMerged { reason })
    };

    let branch_ref = format!("refs/heads/{default_branch}");
    let worktree_root = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let worktree = worktree_root.join(format!("stranded-merge-{}", TaskId::new()));
    let checked_out = git_output(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .is_some_and(|branch| branch == *default_branch);
    if checked_out && !git_output(repo, &["status", "--porcelain"]).is_some_and(|s| s.is_empty()) {
        let reason = "default branch checkout has uncommitted changes".to_owned();
        let _ = record_failure(reason.clone())?;
        return Ok(MergeOutcome::Busy { reason });
    }
    let new_sha = if ancestor(repo, head, default_branch) {
        return Ok(MergeOutcome::AlreadyIntegrated);
    } else if git_status(repo, &["merge-base", "--is-ancestor", &target_sha, head]).is_some() {
        (*head).to_owned()
    } else {
        let add = Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(["worktree", "add", "--detach"])
            .arg(&worktree)
            .arg(&branch_ref)
            .output();
        if !add.is_ok_and(|o| o.status.success()) {
            let _ = Command::new("git")
                .args(["-C"])
                .arg(repo)
                .args(["worktree", "remove", "--force"])
                .arg(&worktree)
                .status();
            let _ = Command::new("git")
                .args(["-C"])
                .arg(repo)
                .args(["worktree", "prune"])
                .status();
            return record_failure("could not create temporary merge worktree".into());
        }
        let merge_out = Command::new("git")
            .args(["-C"])
            .arg(&worktree)
            .args(["merge", "--no-ff", "--no-edit"])
            .arg(head)
            .output();
        let result = if merge_out.as_ref().is_ok_and(|o| o.status.success()) {
            git_output(&worktree, &["rev-parse", "HEAD"])
        } else {
            if git_status(&worktree, &["merge", "--abort"]).is_none() { /* retain original merge error */
            }
            None
        };
        let _ = Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(["worktree", "remove", "--force"])
            .arg(&worktree)
            .status();
        let _ = Command::new("git")
            .args(["-C"])
            .arg(repo)
            .args(["worktree", "prune"])
            .status();
        match result {
            Some(sha) => sha,
            None => {
                return record_failure("git merge --no-ff failed (conflict or merge error)".into());
            }
        }
    };
    let update = if checked_out {
        git_status(repo, &["merge", "--ff-only", &new_sha])
    } else {
        git_status(repo, &["update-ref", &branch_ref, &new_sha, &target_sha])
    };
    if update.is_none() {
        return record_failure("default branch could not be advanced".into());
    }
    if !ancestor(repo, head, default_branch) {
        return record_failure("post-merge ancestry verification failed".into());
    }
    state.state = TaskRepoIntegrationStatus::Integrated;
    state.reason = None;
    state.actor = (*actor).to_owned();
    state.checked_at = *now;
    state.updated_at = *now;
    store.task_integration_upsert(&state)?;
    Ok(MergeOutcome::Integrated)
}

fn git_output(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn git_status(repo: &Path, args: &[&str]) -> Option<()> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(())
}

/// Repository and recorded task branch information resolved by the daemon from its worktree DB.
#[derive(Debug, Clone)]
pub struct BranchRecord {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    pub repo_path: Option<std::path::PathBuf>,
    /// `crate::changes::resolve_default_branch` の結果。`Err` は理由つきで unintegrated になる。
    pub default_branch: Result<String, crate::changes::DefaultBranchError>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub done: bool,
    pub root: bool,
    pub changed_repo: bool,
    /// Remote workspaces are intentionally outside this local git sweep.
    pub not_applicable: bool,
}

pub(crate) fn ancestor(repo: &Path, head: &str, default_branch: &str) -> bool {
    Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["merge-base", "--is-ancestor", head])
        .arg(format!("refs/heads/{default_branch}"))
        .status()
        .is_ok_and(|status| status.success())
}

/// ADR 2026-10-12-stranded-sweep-single-flight D2: the default branch only moves forward, so a
/// stored integrated row whose branch and head match the record stays integrated without git.
/// Records that would fail resolution keep the full check so their reason is still recorded.
fn unchanged_integrated(
    record: &BranchRecord,
    head: &str,
    previous: Option<&TaskRepoIntegrationState>,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    !record.not_applicable
        && record.repo_path.is_some()
        && record.default_branch.is_ok()
        && !head.is_empty()
        && previous.state == TaskRepoIntegrationStatus::Integrated
        && previous.reason.is_none()
        && record.branch.as_deref() == Some(previous.branch.as_str())
        && previous.head_sha == head
}

/// Sweep candidates at the caller's injected timestamp. Held and discarded states are terminal
/// until an explicit reopen operation; repeat scans only append events for actual state changes.
pub fn sweep(
    store: &dyn TaskStore,
    records: &[BranchRecord],
    now: OffsetDateTime,
) -> Result<usize, StoreError> {
    let mut changed = 0;
    for record in records
        .iter()
        .filter(|r| r.done && r.root && r.changed_repo)
    {
        let previous = store.task_integration_get(record.task_id, record.repo_id)?;
        if matches!(&previous, Some(s) if matches!(s.state, TaskRepoIntegrationStatus::Discarded | TaskRepoIntegrationStatus::Held))
        {
            continue;
        }
        let head = record.head.as_deref().unwrap_or("");
        if unchanged_integrated(record, head, previous.as_ref()) {
            continue;
        }
        let reason = if record.not_applicable {
            Some("remote repository: not checked by the sweep".to_owned())
        } else if record.repo_path.is_none() {
            Some("repository cannot be resolved".to_owned())
        } else if record.branch.is_none() {
            Some("task branch is missing".to_owned())
        } else if head.is_empty() {
            Some("recorded task head is missing".to_owned())
        } else {
            match &record.default_branch {
                Err(error) => Some(error.to_string()),
                Ok(default_branch)
                    if !ancestor(
                        record.repo_path.as_deref().unwrap_or(Path::new("")),
                        head,
                        default_branch,
                    ) =>
                {
                    Some(
                        "recorded head is not an ancestor of the repository default branch"
                            .to_owned(),
                    )
                }
                Ok(_) => None,
            }
        };
        let state = TaskRepoIntegrationState {
            task_id: record.task_id,
            repo_id: record.repo_id,
            branch: record.branch.clone().unwrap_or_default(),
            head_sha: head.to_owned(),
            state: if record.not_applicable {
                TaskRepoIntegrationStatus::NotApplicable
            } else if reason.is_some() {
                TaskRepoIntegrationStatus::Unintegrated
            } else {
                TaskRepoIntegrationStatus::Integrated
            },
            reason,
            actor: "sweep".into(),
            checked_at: now,
            updated_at: now,
        };
        let differs = previous.as_ref().is_none_or(|p| {
            p.state != state.state
                || p.branch != state.branch
                || p.head_sha != state.head_sha
                || p.reason != state.reason
        });
        if differs {
            store.task_integration_upsert(&state)?;
        }
        changed += usize::from(differs);
    }
    Ok(changed)
}

#[cfg(test)]
#[path = "stranded/tests.rs"]
mod tests;
