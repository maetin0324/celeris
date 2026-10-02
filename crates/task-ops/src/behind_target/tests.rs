use std::path::Path;

use task_core::SqliteStore;
use time::format_description::well_known::Rfc3339;

use super::*;
use crate::changes::{SyncOutcome, sync_onto_target};

fn git_must(dir: &Path, args: &[&str]) {
    let out = git(dir, args, GIT_TIMEOUT).unwrap_or_else(|| panic!("git {args:?} did not start"));
    assert!(out.ok, "git {args:?}: {}", out.stderr);
}

fn commit_file(dir: &Path, name: &str, msg: &str) {
    std::fs::write(dir.join(name), msg.as_bytes()).unwrap_or_else(|e| panic!("write: {e}"));
    git_must(dir, &["add", "-A"]);
    git_must(dir, &["commit", "-q", "-m", msg]);
}

/// A repo on `main` with a task worktree on `celeris/01TASK`.
fn repo_with_worktree(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = root.join("code");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("mkdir: {e}"));
    git_must(&repo, &["init", "-q", "-b", "main"]);
    git_must(&repo, &["config", "user.email", "t@example.com"]);
    git_must(&repo, &["config", "user.name", "t"]);
    commit_file(&repo, "README.md", "first");
    let tree = root.join("ws/01TASK/repos/code");
    let path = tree.to_string_lossy().into_owned();
    git_must(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "celeris/01TASK",
            &path,
            "main",
        ],
    );
    git_must(&tree, &["config", "user.email", "t@example.com"]);
    git_must(&tree, &["config", "user.name", "t"]);
    (repo, tree)
}

fn at(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).unwrap_or_else(|e| panic!("time: {e}"))
}

const MAIN: &str = "refs/heads/main";

/// ADR-0130 D4: only the target's commits count (ahead commits do not), and a target that is an
/// ancestor of HEAD gives 0.
#[test]
fn behind_target_counts_only_commits_the_target_has() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let (repo, tree) = repo_with_worktree(root.path());

    let m = measure_behind_target(&tree, "HEAD", MAIN);
    assert_eq!(m.commits, Some(0));
    assert_eq!(m.target_sha, m.head_sha);

    commit_file(&tree, "task.txt", "task 1");
    commit_file(&tree, "task2.txt", "task 2");
    assert_eq!(measure_behind_target(&tree, "HEAD", MAIN).commits, Some(0));

    commit_file(&repo, "a.txt", "main 1");
    commit_file(&repo, "b.txt", "main 2");
    commit_file(&repo, "c.txt", "main 3");
    let m = measure_behind_target(&tree, "HEAD", MAIN);
    assert_eq!(m.commits, Some(3));
    assert_ne!(m.target_sha, m.head_sha);
}

/// ADR-0130 D4: Git/ref failures give `None`, never 0, and do not fail the caller.
#[test]
fn behind_target_is_null_when_git_cannot_read() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let (_repo, tree) = repo_with_worktree(root.path());
    let missing = measure_behind_target(&tree, "HEAD", "refs/heads/no-such-branch");
    assert_eq!(missing.commits, None);
    assert_eq!(missing.target_sha, None);
    assert!(missing.head_sha.is_some());

    let not_git = root.path().join("plain");
    std::fs::create_dir_all(&not_git).unwrap_or_else(|e| panic!("{e}"));
    let none = measure_behind_target(&not_git, "HEAD", MAIN);
    assert_eq!(none.commits, None);
    assert_eq!(none.head_sha, None);

    let store = SqliteStore::open_in_memory().unwrap_or_else(|e| panic!("{e}"));
    let task_id = TaskId::new();
    let repo_id = RepoId::new();
    let snap = observe_behind_target(
        &store,
        task_id,
        repo_id,
        &not_git,
        MAIN,
        at("2026-10-02T00:00:00Z"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(snap.behind_target_commits, None);
    let summary = behind_target_of(&store, task_id, at("2026-10-02T01:00:00Z"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(summary.behind_target_commits, None);
    assert_eq!(summary.behind_target_age_seconds, None);
    assert_eq!(
        summary.behind_target_observed_at.as_deref(),
        None,
        "no readable repo -> no representative"
    );
    assert_eq!(summary.repos.len(), 1);
}

/// ADR-0130 D4: age runs from the first positive observation, is kept while the target keeps
/// advancing, and resets to 0 after the branch has taken in the target (sync).
#[test]
fn behind_target_age_is_kept_while_behind_and_cleared_after_sync() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let (repo, tree) = repo_with_worktree(root.path());
    let store = SqliteStore::open_in_memory().unwrap_or_else(|e| panic!("{e}"));
    let task_id = TaskId::new();
    let repo_id = RepoId::new();
    let observe = |now: &str| {
        observe_behind_target(&store, task_id, repo_id, &tree, MAIN, at(now))
            .unwrap_or_else(|e| panic!("{e}"))
    };

    commit_file(&tree, "task.txt", "task");
    let s = observe("2026-10-02T00:00:00Z");
    assert_eq!(s.behind_target_commits, Some(0));
    assert_eq!(s.behind_target_since, None);

    commit_file(&repo, "a.txt", "main 1");
    let s = observe("2026-10-02T01:00:00Z");
    assert_eq!(s.behind_target_commits, Some(1));
    assert_eq!(
        s.behind_target_since.as_deref(),
        Some("2026-10-02T01:00:00Z")
    );

    commit_file(&repo, "b.txt", "main 2");
    let s = observe("2026-10-02T02:00:00Z");
    assert_eq!(s.behind_target_commits, Some(2));
    assert_eq!(
        s.behind_target_since.as_deref(),
        Some("2026-10-02T01:00:00Z")
    );

    // API read: last snapshot, age from `since` at the read time, no Git access.
    let summary = behind_target_of(&store, task_id, at("2026-10-02T03:30:00Z"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(summary.behind_target_commits, Some(2));
    assert_eq!(summary.behind_target_age_seconds, Some(2 * 3600 + 1800));
    assert_eq!(
        summary.behind_target_observed_at.as_deref(),
        Some("2026-10-02T02:00:00Z")
    );

    // an older reading must not overwrite the newer snapshot
    let stale = observe("2026-10-02T01:30:00Z");
    assert_eq!(
        stale.behind_target_observed_at, "2026-10-02T02:00:00Z",
        "older observation ignored"
    );

    assert!(matches!(
        sync_onto_target(&tree, MAIN),
        SyncOutcome::Rebased { .. }
    ));
    let s = observe("2026-10-02T04:00:00Z");
    assert_eq!(s.behind_target_commits, Some(0));
    assert_eq!(s.behind_target_since, None);
    let summary = behind_target_of(&store, task_id, at("2026-10-02T05:00:00Z"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(summary.behind_target_age_seconds, Some(0));
}
