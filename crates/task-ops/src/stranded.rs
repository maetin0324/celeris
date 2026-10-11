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
}
