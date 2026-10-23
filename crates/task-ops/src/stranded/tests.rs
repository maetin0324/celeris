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
            .filter(|(_, event)| matches!(event, task_core::Event::IntegrationNotApplicable { .. }))
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
    assert!(
        !promotion_cache()
            .lock()
            .expect("cache")
            .contains_key(&(repo.clone(), other, head))
    );
}
