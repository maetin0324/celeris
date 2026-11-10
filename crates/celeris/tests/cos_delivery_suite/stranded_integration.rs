//! ADR 2026-10-11 D6: deterministic end-to-end exercise of stranded branch state.
use std::path::{Path, PathBuf};

use task_core::{
    Budget, Check, Criterion, RepoId, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskRepoIntegrationStatus, TaskStore, Tier, WorkspaceSpec,
};
use task_ops::stranded::{self, BranchRecord, MergeRequest};
use time::OffsetDateTime;

struct GitRepo(tempfile::TempDir);

impl GitRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary git repo");
        git(dir.path(), &["init", "-b", "main"]);
        git(
            dir.path(),
            &["config", "user.email", "test@example.invalid"],
        );
        git(dir.path(), &["config", "user.name", "stranded test"]);
        std::fs::write(dir.path().join("base"), "base\n").expect("write base");
        git(dir.path(), &["add", "base"]);
        git(dir.path(), &["commit", "-m", "base"]);
        Self(dir)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn branch(&self, name: &str, file: &str) -> String {
        git(self.path(), &["switch", "-c", name]);
        std::fs::write(self.path().join(file), file).expect("write branch file");
        git(self.path(), &["add", file]);
        git(self.path(), &["commit", "-m", name]);
        git(self.path(), &["rev-parse", "HEAD"])
    }
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

fn done_task(id: TaskId) -> Task {
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "stranded fixture".into(),
        objective: "exercise branch disposition".into(),
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
            path: "workspace".into(),
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
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn no_evidence(_task: &Task, _run_id: &str) -> Vec<task_ops::inbox::EvidenceView> {
    Vec::new()
}

#[test]
fn stranded_detects_inbox_merge_discard_hold_and_promotion_independently() {
    let repo = GitRepo::new();
    let first_head = repo.branch("task/merge", "merge.txt");
    git(repo.path(), &["switch", "main"]);
    let second_head = repo.branch("task/discard", "discard.txt");
    git(repo.path(), &["switch", "main"]);

    let store = SqliteStore::open_in_memory().expect("sqlite store");
    let repo_id = RepoId::new();
    let merge_task = TaskId::new();
    let discard_task = TaskId::new();
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("fixed clock");
    store
        .insert(&done_task(merge_task))
        .expect("insert merge task");
    store
        .insert(&done_task(discard_task))
        .expect("insert discard task");
    let records = [
        BranchRecord {
            task_id: merge_task,
            repo_id,
            repo_path: Some(PathBuf::from(repo.path())),
            default_branch: Ok("main".into()),
            branch: Some("task/merge".into()),
            head: Some(first_head.clone()),
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: false,
        },
        BranchRecord {
            task_id: discard_task,
            repo_id,
            repo_path: Some(PathBuf::from(repo.path())),
            default_branch: Ok("main".into()),
            branch: Some("task/discard".into()),
            head: Some(second_head.clone()),
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: false,
        },
    ];

    assert_eq!(
        stranded::sweep(&store, &records, now).expect("initial sweep"),
        2
    );
    assert_eq!(
        store
            .task_integration_list(Some(TaskRepoIntegrationStatus::Unintegrated))
            .expect("unintegrated list")
            .len(),
        2
    );
    let inbox = task_ops::inbox::inbox(
        &store,
        None,
        &task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                std::env::temp_dir(),
                None,
            ),
            retry_backoff_base: std::time::Duration::from_secs(1),
            retry_backoff_max: std::time::Duration::from_secs(30),
            max_requeues: 2,
            clusters: Default::default(),
        },
        now,
        &no_evidence,
    )
    .expect("inbox");
    assert_eq!(
        inbox
            .attention
            .iter()
            .filter(|item| matches!(item, task_ops::inbox::AttentionItem::Unintegrated { .. }))
            .count(),
        2
    );
    assert_eq!(
        stranded::sweep(&store, &records, now).expect("repeat sweep"),
        0
    );
    assert_eq!(
        store.events_for(merge_task).expect("event history").len(),
        1
    );

    let merge = MergeRequest {
        task_id: merge_task,
        repo_id,
        repo: repo.path(),
        branch: "task/merge",
        head: &first_head,
        default_branch: "main",
        actor: "test",
        now,
    };
    assert_eq!(
        stranded::merge(&store, &merge).expect("merge task branch"),
        stranded::MergeOutcome::Integrated
    );
    assert_eq!(
        store
            .open_integration_requests()
            .expect("integration requests")
            .len(),
        0
    );
    assert_eq!(
        stranded::sweep(&store, &records, now).expect("confirm delivery merge"),
        0
    );
    assert_eq!(
        store
            .task_integration_get(merge_task, repo_id)
            .expect("state")
            .expect("row")
            .state,
        TaskRepoIntegrationStatus::Integrated
    );

    stranded::discard(
        &store,
        discard_task,
        repo_id,
        "obsolete change",
        "test",
        now,
    )
    .expect("discard");
    assert_eq!(
        store
            .task_integration_get(discard_task, repo_id)
            .expect("state")
            .expect("row")
            .state,
        TaskRepoIntegrationStatus::Discarded
    );
    let held_task = TaskId::new();
    let mut held = records[1].clone();
    held.task_id = held_task;
    stranded::sweep(&store, &[held], now).expect("seed held candidate");
    stranded::hold(&store, held_task, repo_id, "awaiting decision", "test", now).expect("hold");
    assert_eq!(
        store
            .task_integration_get(held_task, repo_id)
            .expect("held state")
            .expect("row")
            .state,
        TaskRepoIntegrationStatus::Held
    );
    stranded::reopen(&store, held_task, repo_id, "test", now).expect("reopen held item");
    assert_eq!(
        store
            .task_integration_get(held_task, repo_id)
            .expect("reopened state")
            .expect("row")
            .state,
        TaskRepoIntegrationStatus::Unintegrated
    );
    assert_eq!(
        store
            .task_integration_get(merge_task, repo_id)
            .expect("merged state")
            .expect("row")
            .state,
        TaskRepoIntegrationStatus::Integrated
    );

    let releases = tempfile::tempdir().expect("release fixture");
    let promotion =
        stranded::promotion_view("agent-platform", repo.path(), &first_head, releases.path());
    assert_eq!(
        promotion.status,
        stranded::PromotionStatus::PromotionPending
    );
}
