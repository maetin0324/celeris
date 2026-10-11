//! Deterministic detection of completed task branches that have not reached the default branch.
use std::{path::Path, process::Command};

use serde::{Deserialize, Serialize};
use task_core::{
    RepoId, StoreError, TaskId, TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore,
};
use time::OffsetDateTime;

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
    if repository != "agent-platform" {
        return PromotionView {
            status: PromotionStatus::NotApplicable,
            release_sha: None,
        };
    }
    let newest = std::fs::read_dir(releases_dir)
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
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((_, release_sha)) = newest else {
        return PromotionView {
            status: PromotionStatus::PromotionPending,
            release_sha: None,
        };
    };
    let included = Command::new("git")
        .arg("-C")
        .arg(repository_path)
        .args([
            "merge-base",
            "--is-ancestor",
            branch_head,
            release_sha.as_str(),
        ])
        .status()
        .is_ok_and(|status| status.success());
    PromotionView {
        status: if included {
            PromotionStatus::Promoted
        } else {
            PromotionStatus::PromotionPending
        },
        release_sha: Some(release_sha),
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

/// Confirm merge completion using git ancestry. If it is already on the default branch this is
/// idempotent; otherwise create the standard integration request for the existing merge flow.
pub fn merge(store: &dyn TaskStore, input: &MergeRequest<'_>) -> Result<bool, StoreError> {
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
        return Ok(true);
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
        recommendation: "Use the existing integration review and merge flow".into(),
        actions: vec![],
        candidate_sha: None,
    };
    store.integration_request_record(*task_id, &request, "delivery")?;
    Ok(false)
}

/// Repository and recorded task branch information resolved by the daemon from its worktree DB.
#[derive(Debug, Clone)]
pub struct BranchRecord {
    pub task_id: TaskId,
    pub repo_id: RepoId,
    pub repo_path: Option<std::path::PathBuf>,
    pub default_branch: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub done: bool,
    pub root: bool,
    pub changed_repo: bool,
}

fn ancestor(repo: &Path, head: &str, default_branch: &str) -> bool {
    Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["merge-base", "--is-ancestor", head])
        .arg(format!("refs/heads/{default_branch}"))
        .status()
        .is_ok_and(|status| status.success())
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
        if matches!(store.task_integration_get(record.task_id, record.repo_id)?, Some(s) if matches!(s.state, TaskRepoIntegrationStatus::Discarded | TaskRepoIntegrationStatus::Held))
        {
            continue;
        }
        let head = record.head.as_deref().unwrap_or("");
        let reason = if record.repo_path.is_none() {
            Some("repository cannot be resolved".to_owned())
        } else if record.branch.is_none() {
            Some("task branch is missing".to_owned())
        } else if head.is_empty() {
            Some("recorded task head is missing".to_owned())
        } else if record.default_branch.is_none() {
            Some("repository default branch is missing".to_owned())
        } else if !ancestor(
            record.repo_path.as_deref().unwrap_or(Path::new("")),
            head,
            record.default_branch.as_deref().unwrap_or(""),
        ) {
            Some("recorded head is not an ancestor of the repository default branch".to_owned())
        } else {
            None
        };
        let state = TaskRepoIntegrationState {
            task_id: record.task_id,
            repo_id: record.repo_id,
            branch: record.branch.clone().unwrap_or_default(),
            head_sha: head.to_owned(),
            state: if reason.is_some() {
                TaskRepoIntegrationStatus::Unintegrated
            } else {
                TaskRepoIntegrationStatus::Integrated
            },
            reason,
            actor: "sweep".into(),
            checked_at: now,
            updated_at: now,
        };
        let previous = store.task_integration_get(record.task_id, record.repo_id)?;
        let differs = previous.as_ref().is_none_or(|p| {
            p.state != state.state
                || p.branch != state.branch
                || p.head_sha != state.head_sha
                || p.reason != state.reason
        });
        store.task_integration_upsert(&state)?;
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
            default_branch: Some("main".into()),
            branch: Some("task".into()),
            head: Some(head),
            done: true,
            root: true,
            changed_repo: true,
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
        assert!(merge(&store, &request).expect("merge"));
        let event_count = store.events_for(task_id).expect("events").len();
        assert!(merge(&store, &request).expect("repeat"));
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

    #[test]
    fn stranded_merge_routes_unmerged_head_to_existing_integration_request() {
        let dir = std::env::temp_dir().join(format!("stranded-request-{}", TaskId::new()));
        fs::create_dir_all(&dir).expect("fixture");
        git(&dir, &["init", "-b", "main"]);
        git(&dir, &["config", "user.email", "test@example.invalid"]);
        git(&dir, &["config", "user.name", "test"]);
        fs::write(dir.join("base"), "base").expect("write");
        git(&dir, &["add", "base"]);
        git(&dir, &["commit", "-m", "base"]);
        git(&dir, &["switch", "-c", "topic"]);
        fs::write(dir.join("change"), "change").expect("write");
        git(&dir, &["add", "change"]);
        git(&dir, &["commit", "-m", "change"]);
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let store = SqliteStore::open_in_memory().expect("store");
        let task_id = TaskId::new();
        let repo_id = RepoId::new();
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
        store
            .task_integration_upsert(&TaskRepoIntegrationState {
                task_id,
                repo_id,
                branch: "topic".into(),
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
            branch: "topic",
            head: &head,
            default_branch: "main",
            actor: "alice",
            now,
        };
        assert!(!merge(&store, &request).expect("request existing flow"));
        assert_eq!(
            store.open_integration_requests().expect("requests").len(),
            1
        );
        assert_eq!(
            git(&dir, &["rev-parse", "refs/heads/main"]),
            git(&dir, &["rev-parse", "HEAD~1"])
        );
        let _ = fs::remove_dir_all(dir);
    }
}
