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
mod tests {
    use super::*;
    use std::{fs, process::Command};
    use task_core::{SqliteStore, TaskRepoIntegrationStatus};

    #[test]
    fn stranded_promotion_tracks_latest_promoted_release_independently() {
        let tmp = tempfile::tempdir().expect("temporary directory");
        let repo = tmp.path().join("repo");
        let releases = tmp.path().join("releases");
        fs::create_dir_all(&repo).expect("repo");
        fs::create_dir_all(&releases).expect("releases");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .expect("git");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "test@example.invalid"]);
        git(&["config", "user.name", "test"]);
        fs::write(repo.join("base"), "base").expect("base");
        git(&["add", "base"]);
        git(&["commit", "-m", "base"]);
        git(&["switch", "-c", "task"]);
        fs::write(repo.join("task"), "task").expect("task");
        git(&["add", "task"]);
        git(&["commit", "-m", "task"]);
        let task_head = git(&["rev-parse", "HEAD"]);
        git(&["switch", "main"]);
        let base_head = git(&["rev-parse", "HEAD"]);
        let release = releases.join("release-1");
        fs::create_dir_all(&release).expect("release");
        fs::write(
            release.join("manifest.json"),
            format!(r#"{{"sha":"{base_head}"}}"#),
        )
        .expect("manifest");
        fs::write(
            release.join("promoted.json"),
            r#"{"promoted_at":"2026-10-10T00:00:00Z"}"#,
        )
        .expect("marker");
        assert_eq!(
            promotion_view("agent-platform", &repo, &task_head, &releases).status,
            PromotionStatus::PromotionPending
        );
        git(&["merge", "--ff-only", "task"]);
        let full_release = git(&["rev-parse", "HEAD"]);
        fs::write(
            release.join("manifest.json"),
            format!(r#"{{"sha":"{full_release}"}}"#),
        )
        .expect("manifest update");
        assert_eq!(
            promotion_view("agent-platform", &repo, &task_head, &releases).status,
            PromotionStatus::Promoted
        );
        assert_eq!(
            promotion_view("other-repo", &repo, &task_head, &releases).status,
            PromotionStatus::NotApplicable
        );
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn stranded_sweep_detects_then_integrates_without_duplicate_events() {
        let dir = std::env::temp_dir().join(format!("stranded-{}", TaskId::new()));
        fs::create_dir_all(&dir).expect("fixture");
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@example.invalid"]);
        git(&dir, &["config", "user.name", "test"]);
        fs::write(dir.join("base"), "base").expect("write");
        git(&dir, &["add", "base"]);
        git(&dir, &["commit", "-m", "base"]);
        git(&dir, &["switch", "-c", "task"]);
        fs::write(dir.join("change"), "change").expect("write");
        git(&dir, &["add", "change"]);
        git(&dir, &["commit", "-m", "task"]);
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let store = SqliteStore::open_in_memory().expect("store");
        let task = TaskId::new();
        let repo = RepoId::new();
        let candidate = BranchRecord {
            task_id: task,
            repo_id: repo,
            repo_path: Some(dir.clone()),
            default_branch: Ok("main".into()),
            branch: Some("task".into()),
            head: Some(head),
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: false,
        };
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        assert_eq!(
            sweep(&store, std::slice::from_ref(&candidate), now).expect("sweep"),
            1
        );
        assert_eq!(
            store
                .task_integration_list(Some(TaskRepoIntegrationStatus::Unintegrated))
                .expect("list")
                .len(),
            1
        );
        assert_eq!(
            sweep(&store, std::slice::from_ref(&candidate), now).expect("repeat"),
            0
        );
        assert_eq!(store.events_for(task).expect("events").len(), 1);
        git(&dir, &["switch", "main"]);
        git(&dir, &["merge", "--ff-only", "task"]);
        assert_eq!(
            sweep(&store, &[candidate], now).expect("integrated sweep"),
            1
        );
        assert_eq!(
            store
                .task_integration_get(task, repo)
                .expect("state")
                .expect("row")
                .state,
            TaskRepoIntegrationStatus::Integrated
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stranded_remote_migrates_unintegrated_row_to_not_applicable() {
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "celeris/task".into(),
                head_sha: "abc".into(),
                state: TaskRepoIntegrationStatus::Unintegrated,
                reason: Some("repository cannot be resolved".into()),
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed prior row");
        let record = BranchRecord {
            task_id,
            repo_id,
            repo_path: None,
            default_branch: Err(crate::changes::DefaultBranchError(
                "remote repository: not checked by the sweep".into(),
            )),
            branch: Some("celeris/task".into()),
            head: None,
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: true,
        };
        assert_eq!(sweep(&store, &[record], now).expect("remote sweep"), 1);
        let state = store
            .task_integration_get(task_id, repo_id)
            .expect("get")
            .expect("row");
        assert_eq!(state.state, TaskRepoIntegrationStatus::NotApplicable);
        assert_eq!(
            state.reason.as_deref(),
            Some("remote repository: not checked by the sweep")
        );
        assert!(
            store
                .task_integration_list(Some(TaskRepoIntegrationStatus::Unintegrated))
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn stranded_audit_remote_not_applicable_appends_one_event_and_no_repeat() {
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "celeris/task".into(),
                head_sha: "abc".into(),
                state: TaskRepoIntegrationStatus::Unintegrated,
                reason: Some("repository cannot be resolved".into()),
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed unintegrated row");
        let record = BranchRecord {
            task_id,
            repo_id,
            repo_path: None,
            default_branch: Err(crate::changes::DefaultBranchError(
                "remote repository: not checked by the sweep".into(),
            )),
            branch: Some("celeris/task".into()),
            head: None,
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: true,
        };
        let not_applicable_events = |store: &SqliteStore| {
            store
                .events_for(task_id)
                .expect("events")
                .iter()
                .filter(|(_, event)| {
                    matches!(event, task_core::Event::IntegrationNotApplicable { .. })
                })
                .count()
        };
        assert_eq!(
            sweep(&store, std::slice::from_ref(&record), now).expect("remote sweep"),
            1
        );
        assert_eq!(not_applicable_events(&store), 1);
        let later = now + time::Duration::minutes(5);
        assert_eq!(
            sweep(&store, std::slice::from_ref(&record), later).expect("repeat sweep"),
            0
        );
        assert_eq!(not_applicable_events(&store), 1);
    }

    #[test]
    fn stranded_remote_actions_reject_not_applicable_rows() {
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "celeris/task".into(),
                head_sha: String::new(),
                state: TaskRepoIntegrationStatus::NotApplicable,
                reason: Some("remote repository: not checked by the sweep".into()),
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed");
        assert!(discard(&store, task_id, repo_id, "reason", "human", now).is_err());
        assert!(hold(&store, task_id, repo_id, "reason", "human", now).is_err());
        assert!(reopen(&store, task_id, repo_id, "human", now).is_err());
        let request = MergeRequest {
            task_id,
            repo_id,
            repo: Path::new("/no/local/remote/repository"),
            branch: "celeris/task",
            head: "head",
            default_branch: "main",
            actor: "human",
            now,
        };
        assert!(merge(&store, &request).is_err());
    }

    #[test]
    fn stranded_discard_hold_reopen_require_reason_and_append_history() {
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "topic".into(),
                head_sha: "abc".into(),
                state: TaskRepoIntegrationStatus::Unintegrated,
                reason: None,
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed");
        assert!(discard(&store, task_id, repo_id, "  ", "alice", now).is_err());
        discard(&store, task_id, repo_id, "obsolete", "alice", now).expect("discard");
        hold(&store, task_id, repo_id, "  ", "bob", now).expect_err("hold reason required");
        hold(&store, task_id, repo_id, "waiting", "bob", now).expect("hold");
        reopen(&store, task_id, repo_id, "carol", now).expect("reopen");
        let current = store
            .task_integration_get(task_id, repo_id)
            .expect("get")
            .expect("state");
        assert_eq!(current.state, TaskRepoIntegrationStatus::Unintegrated);
        assert_eq!(current.actor, "carol");
        assert_eq!(store.events_for(task_id).expect("events").len(), 4);
    }

    #[test]
    fn stranded_merge_is_idempotent_when_head_is_already_on_default_branch() {
        let dir = std::env::temp_dir().join(format!("stranded-merge-{}", TaskId::new()));
        fs::create_dir_all(&dir).expect("fixture");
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@example.invalid"]);
        git(&dir, &["config", "user.name", "test"]);
        fs::write(dir.join("base"), "base").expect("write");
        git(&dir, &["add", "base"]);
        git(&dir, &["commit", "-m", "base"]);
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "main".into(),
                head_sha: head.clone(),
                state: TaskRepoIntegrationStatus::Unintegrated,
                reason: None,
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed");
        let request = MergeRequest {
            task_id,
            repo_id,
            repo: &dir,
            branch: "main",
            head: &head,
            default_branch: "main",
            actor: "alice",
            now,
        };
        assert_eq!(
            merge(&store, &request).expect("merge"),
            MergeOutcome::AlreadyIntegrated
        );
        let event_count = store.events_for(task_id).expect("events").len();
        assert_eq!(
            merge(&store, &request).expect("repeat"),
            MergeOutcome::AlreadyIntegrated
        );
        assert_eq!(
            store.events_for(task_id).expect("events").len(),
            event_count
        );
        assert_eq!(
            store
                .task_integration_get(task_id, repo_id)
                .expect("get")
                .expect("state")
                .state,
            TaskRepoIntegrationStatus::Integrated
        );
        let _ = fs::remove_dir_all(dir);
    }

    fn merge_fixture() -> (
        tempfile::TempDir,
        SqliteStore,
        TaskId,
        RepoId,
        OffsetDateTime,
        String,
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        git(repo, &["init", "-b", "main"]);
        git(repo, &["config", "user.email", "test@example.invalid"]);
        git(repo, &["config", "user.name", "test"]);
        fs::write(repo.join("base"), "base\n").expect("base");
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let store = SqliteStore::open_in_memory().expect("store");
        let task = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        (tmp, store, task, repo_id, now, base)
    }

    fn seed_merge(
        store: &SqliteStore,
        task: TaskId,
        repo: RepoId,
        branch: &str,
        head: &str,
        now: OffsetDateTime,
    ) {
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id: task,
                repo_id: repo,
                branch: branch.into(),
                head_sha: head.into(),
                state: TaskRepoIntegrationStatus::Unintegrated,
                reason: None,
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed");
    }
    fn worktrees(repo: &Path) {
        assert_eq!(
            git(repo, &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|l| l.starts_with("worktree "))
                .count(),
            1
        );
    }

    #[test]
    fn stranded_merge_fast_forward() {
        let (tmp, store, task, repo_id, now, _) = merge_fixture();
        let repo = tmp.path();
        git(repo, &["switch", "-c", "topic"]);
        fs::write(repo.join("topic"), "topic").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "topic"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["switch", "main"]);
        seed_merge(&store, task, repo_id, "topic", &head, now);
        assert_eq!(
            merge(
                &store,
                &MergeRequest {
                    task_id: task,
                    repo_id,
                    repo,
                    branch: "topic",
                    head: &head,
                    default_branch: "main",
                    actor: "alice",
                    now
                }
            )
            .unwrap(),
            MergeOutcome::Integrated
        );
        assert_eq!(git(repo, &["rev-parse", "main"]), head);
        assert_eq!(
            store
                .events_for(task)
                .unwrap()
                .iter()
                .filter(|(_, event)| matches!(event, task_core::Event::IntegrationMerged { .. }))
                .count(),
            1
        );
        worktrees(repo);
    }

    #[test]
    fn stranded_merge_no_ff() {
        let (tmp, store, task, repo_id, now, _) = merge_fixture();
        let repo = tmp.path();
        git(repo, &["switch", "-c", "topic"]);
        fs::write(repo.join("topic"), "topic").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "topic"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["switch", "main"]);
        fs::write(repo.join("main-change"), "main").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "main"]);
        seed_merge(&store, task, repo_id, "topic", &head, now);
        assert_eq!(
            merge(
                &store,
                &MergeRequest {
                    task_id: task,
                    repo_id,
                    repo,
                    branch: "topic",
                    head: &head,
                    default_branch: "main",
                    actor: "alice",
                    now
                }
            )
            .unwrap(),
            MergeOutcome::Integrated
        );
        assert_eq!(
            git(repo, &["rev-list", "--parents", "-n", "1", "main"])
                .split_whitespace()
                .count(),
            3
        );
        worktrees(repo);
    }

    #[test]
    fn stranded_merge_conflict_keeps_default_branch() {
        let (tmp, store, task, repo_id, now, base) = merge_fixture();
        let repo = tmp.path();
        git(repo, &["switch", "-c", "topic"]);
        fs::write(repo.join("base"), "topic\n").unwrap();
        git(repo, &["commit", "-am", "topic"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["switch", "main"]);
        fs::write(repo.join("base"), "main\n").unwrap();
        git(repo, &["commit", "-am", "main"]);
        let main = git(repo, &["rev-parse", "HEAD"]);
        seed_merge(&store, task, repo_id, "topic", &head, now);
        assert!(matches!(
            merge(
                &store,
                &MergeRequest {
                    task_id: task,
                    repo_id,
                    repo,
                    branch: "topic",
                    head: &head,
                    default_branch: "main",
                    actor: "alice",
                    now
                }
            )
            .unwrap(),
            MergeOutcome::NotMerged { .. }
        ));
        assert_eq!(git(repo, &["rev-parse", "main"]), main);
        assert_eq!(
            store
                .task_integration_get(task, repo_id)
                .unwrap()
                .unwrap()
                .state,
            TaskRepoIntegrationStatus::Unintegrated
        );
        assert_eq!(git(repo, &["rev-parse", "HEAD~1"]), base);
        assert_eq!(store.open_integration_requests().unwrap().len(), 1);
        worktrees(repo);
    }

    #[test]
    fn stranded_merge_dirty_checkout_is_busy() {
        let (tmp, store, task, repo_id, now, _) = merge_fixture();
        let repo = tmp.path();
        git(repo, &["switch", "-c", "topic"]);
        fs::write(repo.join("topic"), "topic").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "topic"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["switch", "main"]);
        fs::write(repo.join("dirty"), "dirty").unwrap();
        seed_merge(&store, task, repo_id, "topic", &head, now);
        assert!(matches!(
            merge(
                &store,
                &MergeRequest {
                    task_id: task,
                    repo_id,
                    repo,
                    branch: "topic",
                    head: &head,
                    default_branch: "main",
                    actor: "alice",
                    now
                }
            )
            .unwrap(),
            MergeOutcome::Busy { .. }
        ));
        assert_eq!(
            git(repo, &["rev-parse", "main"]),
            git(repo, &["rev-parse", "HEAD"])
        );
        worktrees(repo);
    }

    #[test]
    fn stranded_merge_other_branch_checked_out_uses_update_ref() {
        let (tmp, store, task, repo_id, now, _) = merge_fixture();
        let repo = tmp.path();
        git(repo, &["switch", "-c", "topic"]);
        fs::write(repo.join("topic"), "topic").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", "topic"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["switch", "-c", "other"]);
        seed_merge(&store, task, repo_id, "topic", &head, now);
        assert_eq!(
            merge(
                &store,
                &MergeRequest {
                    task_id: task,
                    repo_id,
                    repo,
                    branch: "topic",
                    head: &head,
                    default_branch: "main",
                    actor: "alice",
                    now
                }
            )
            .unwrap(),
            MergeOutcome::Integrated
        );
        assert_eq!(git(repo, &["symbolic-ref", "--short", "HEAD"]), "other");
        assert_eq!(git(repo, &["rev-parse", "main"]), head);
        worktrees(repo);
    }

    #[test]
    fn stranded_sweep_skips_git_for_unchanged_integrated_record() {
        // The repository path does not exist: any `git merge-base` would fail and turn the row
        // unintegrated. An unchanged integrated record must not reach git at all.
        let missing = std::env::temp_dir().join(format!("stranded-missing-{}", TaskId::new()));
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "celeris/task".into(),
                head_sha: "abc123".into(),
                state: TaskRepoIntegrationStatus::Integrated,
                reason: None,
                actor: "sweep".into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("seed integrated row");
        let events = store.events_for(task_id).expect("events").len();
        let record = |head: &str| BranchRecord {
            task_id,
            repo_id,
            repo_path: Some(missing.clone()),
            default_branch: Ok("main".into()),
            branch: Some("celeris/task".into()),
            head: Some(head.into()),
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: false,
        };
        let later = now + time::Duration::minutes(5);
        assert_eq!(sweep(&store, &[record("abc123")], later).expect("sweep"), 0);
        let state = store
            .task_integration_get(task_id, repo_id)
            .expect("get")
            .expect("row");
        assert_eq!(state.state, TaskRepoIntegrationStatus::Integrated);
        assert_eq!(state.checked_at, now);
        assert_eq!(store.events_for(task_id).expect("events").len(), events);
        // A moved head is checked again; git fails here, so the row becomes unintegrated.
        assert_eq!(sweep(&store, &[record("def456")], later).expect("moved"), 1);
        assert_eq!(
            store
                .task_integration_get(task_id, repo_id)
                .expect("get")
                .expect("row")
                .state,
            TaskRepoIntegrationStatus::Unintegrated
        );
    }

    #[test]
    fn stranded_promotion_remembers_decided_ancestry() {
        let tmp = tempfile::tempdir().expect("temporary directory");
        let repo = tmp.path().join("repo");
        let releases = tmp.path().join("releases");
        fs::create_dir_all(&repo).expect("repo");
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(&repo, &["config", "user.name", "test"]);
        fs::write(repo.join("base"), "base").expect("base");
        git(&repo, &["add", "base"]);
        git(&repo, &["commit", "-m", "base"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let release = releases.join("release-1");
        fs::create_dir_all(&release).expect("release");
        fs::write(
            release.join("manifest.json"),
            format!(r#"{{"sha":"{head}"}}"#),
        )
        .expect("manifest");
        fs::write(
            release.join("promoted.json"),
            r#"{"promoted_at":"2026-10-10T00:00:00Z"}"#,
        )
        .expect("marker");
        assert_eq!(
            promotion_view("agent-platform", &repo, &head, &releases).status,
            PromotionStatus::Promoted
        );
        // With the repository gone git cannot answer; the remembered answer is served.
        fs::remove_dir_all(&repo).expect("remove repo");
        assert_eq!(
            promotion_view("agent-platform", &repo, &head, &releases).status,
            PromotionStatus::Promoted
        );
        // An undecided answer (git failed) is not remembered.
        let other = "0".repeat(40);
        assert!(!release_contains(&repo, &other, &head));
        assert!(!promotion_cache().lock().expect("cache").contains_key(&(
            repo.clone(),
            other,
            head
        )));
    }
}
