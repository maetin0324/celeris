//! ADR 2026-10-11-stranded-done-branches F1: the stranded sweep resolves the repository default
//! branch with `task_ops::changes::resolve_default_branch` (configured → origin/HEAD → main →
//! master), the same resolver delivery uses. A NULL `project_repos.default_branch` no longer makes
//! an integrated branch `unintegrated`; only an unresolvable repository is, with the reason.
//!
//! Fixed clock injected into the sweep; tempdir git repos and an in-memory store only.
use std::path::Path;

use task_core::{
    Budget, Check, Criterion, Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind,
    RepoRef, RepoRun, SqliteStore, Status, Task, TaskId, TaskKind, TaskRepoIntegrationStatus,
    TaskStore, Tier, WorkspaceSpec,
};
use time::OffsetDateTime;

fn now() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock")
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repo whose first branch is `trunk`, with `celeris/<task>` one commit ahead, merged into
/// `trunk` (fast-forward). The default branch name is then set by `rename` / origin/HEAD.
fn merged_repo(path: &Path, task: TaskId) -> String {
    std::fs::create_dir_all(path).expect("repo dir");
    git(path, &["init", "-b", "trunk"]);
    git(path, &["config", "user.email", "test@example.invalid"]);
    git(path, &["config", "user.name", "stranded default branch"]);
    std::fs::write(path.join("base"), "base\n").expect("base file");
    git(path, &["add", "base"]);
    git(path, &["commit", "-m", "base"]);
    let branch = format!("celeris/{task}");
    git(path, &["switch", "-c", &branch]);
    std::fs::write(path.join("task.txt"), "task\n").expect("task file");
    git(path, &["add", "task.txt"]);
    git(path, &["commit", "-m", "task change"]);
    let head = git(path, &["rev-parse", "HEAD"]);
    git(path, &["switch", "trunk"]);
    git(path, &["merge", "--ff-only", &branch]);
    head
}

/// A project and its Local git repository with `default_branch` NULL.
fn register_repo(store: &SqliteStore, path: &Path) -> ProjectRepo {
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        id: ProjectId::new(),
        title: "stranded default branch".into(),
        request: "default branch".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now(),
        updated_at: now(),
    };
    store.project_create(&project).expect("project");
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "code".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(path.to_path_buf()),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now(),
    };
    store.repo_create(&repo).expect("repo");
    repo
}

/// A done root task. `repo: None` is the repoless shape (resolved through `celeris/<id>`).
fn done_task(id: TaskId, repo: Option<&ProjectRepo>) -> Task {
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: repo
            .map(|r| RepoRef {
                repo_id: r.id,
                name: r.name.clone(),
            })
            .into_iter()
            .collect(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "stranded default branch fixture".into(),
        objective: "exercise default branch resolution".into(),
        acceptance: vec![Criterion {
            text: "fixture exists".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: Vec::new(),
        depends_on: Vec::new(),
        status: Status::Done,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: id.to_string().into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 60,
            max_retries: 0,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now(),
        updated_at: now(),
        role: None,
        genre: None,
        aggregate: false,
        project_id: repo.map(|r| r.project_id),
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn swept_state(
    store: &SqliteStore,
    task: TaskId,
    repo: RepoId,
) -> task_core::TaskRepoIntegrationState {
    assert_eq!(
        celeris::sweep_stranded_branches(store, now()).expect("sweep"),
        1
    );
    store
        .task_integration_get(task, repo)
        .expect("state")
        .expect("row")
}

#[test]
fn stranded_default_branch_null_with_main_is_integrated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let task = TaskId::new();
    let head = merged_repo(&path, task);
    git(&path, &["branch", "-m", "trunk", "main"]);
    let store = SqliteStore::open_in_memory().expect("store");
    let repo = register_repo(&store, &path);
    store.insert(&done_task(task, Some(&repo))).expect("task");

    let state = swept_state(&store, task, repo.id);
    assert_eq!(
        state.state,
        TaskRepoIntegrationStatus::Integrated,
        "{state:?}"
    );
    assert_eq!(state.reason, None);
    assert_eq!(state.head_sha, head);
}

#[test]
fn stranded_default_branch_null_with_origin_head_is_integrated_for_repoless_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let task = TaskId::new();
    let head = merged_repo(&path, task);
    // No main/master: only origin/HEAD names `trunk`.
    git(&path, &["update-ref", "refs/remotes/origin/trunk", "trunk"]);
    git(
        &path,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    let store = SqliteStore::open_in_memory().expect("store");
    let repo = register_repo(&store, &path);
    store.insert(&done_task(task, None)).expect("task");

    let state = swept_state(&store, task, repo.id);
    assert_eq!(
        state.state,
        TaskRepoIntegrationStatus::Integrated,
        "{state:?}"
    );
    assert_eq!(state.head_sha, head);
}

#[test]
fn stranded_default_branch_unresolvable_is_unintegrated_with_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("repo");
    let task = TaskId::new();
    merged_repo(&path, task);
    // No configured branch, no origin/HEAD, no main or master.
    let store = SqliteStore::open_in_memory().expect("store");
    let repo = register_repo(&store, &path);
    store.insert(&done_task(task, Some(&repo))).expect("task");

    let state = swept_state(&store, task, repo.id);
    assert_eq!(state.state, TaskRepoIntegrationStatus::Unintegrated);
    let reason = state.reason.expect("reason");
    assert!(
        reason.contains("default branch cannot be resolved"),
        "{reason}"
    );
    assert!(!reason.contains("repository default branch is missing"));
}
