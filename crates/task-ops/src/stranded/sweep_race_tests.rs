//! Fixed interleavings at the delivery snapshot and ancestry result boundaries.
use super::*;
use task_core::{
    Budget, Check, Criterion, Delivery, DeliveryState, DeliveryStore, Event, ProjectId,
    SqliteStore, Status, Task, TaskKind, Tier, WorkspaceSpec,
};

struct Fixture {
    dir: tempfile::TempDir,
    store: SqliteStore,
    record: BranchRecord,
    now: OffsetDateTime,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        tests::git(p, &["init", "-q", "-b", "main"]);
        tests::git(p, &["config", "user.email", "test@example.invalid"]);
        tests::git(p, &["config", "user.name", "test"]);
        tests::git(p, &["commit", "--allow-empty", "-qm", "base"]);
        tests::git(p, &["switch", "-c", "task"]);
        tests::git(p, &["commit", "--allow-empty", "-qm", "task"]);
        let head = tests::git(p, &["rev-parse", "HEAD"]);
        tests::git(p, &["switch", "main"]);
        let store = SqliteStore::open_in_memory().unwrap();
        let task = sample_task(TaskKind::Execute, Status::Done);
        store.create_task(&task, vec![]).unwrap();
        Self {
            record: BranchRecord {
                task_id: task.id,
                repo_id: RepoId::new(),
                repo_path: Some(p.to_owned()),
                default_branch: Ok("main".into()),
                branch: Some("task".into()),
                head: Some(head),
                done: true,
                root: true,
                changed_repo: true,
                not_applicable: false,
            },
            dir,
            store,
            now: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
        }
    }

    fn delivery(&self, state: DeliveryState) {
        let old = self.store.delivery_get(self.record.task_id).unwrap();
        let mut next = old.clone().unwrap_or_else(|| Delivery {
            task_id: self.record.task_id,
            project_id: ProjectId::new(),
            repo_id: self.record.repo_id,
            repo: self.dir.path().display().to_string(),
            branch: "task".into(),
            base: tests::git(self.dir.path(), &["rev-parse", "main"]),
            head: self.record.head.clone().unwrap(),
            target_sha: None,
            reviewed_sha: None,
            merge_candidate_sha: None,
            default_branch: "main".into(),
            department: "engineering".into(),
            review_run: "review".into(),
            worker_run: "worker".into(),
            criterion_idx: 0,
            decision: Some(true),
            state,
            detail: String::new(),
            release: None,
            prepare_pid: None,
            notification: None,
            pushed_at: None,
            push_error: None,
        });
        next.state = state;
        assert!(self.store.delivery_save(old.as_ref(), &next).unwrap());
    }

    fn merge(&self) {
        tests::git(self.dir.path(), &["merge", "--ff-only", "task"]);
    }

    fn sweep(&self) -> usize {
        sweep(&self.store, std::slice::from_ref(&self.record), self.now).unwrap()
    }

    fn state(&self) -> Option<TaskRepoIntegrationStatus> {
        self.store
            .task_integration_get(self.record.task_id, self.record.repo_id)
            .unwrap()
            .map(|r| r.state)
    }

    fn inbox_count(&self) -> usize {
        let ctx = crate::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                self.dir.path().into(),
                None,
            ),
            retry_backoff_base: std::time::Duration::from_secs(10),
            retry_backoff_max: std::time::Duration::from_secs(300),
            max_requeues: 5,
            clusters: Default::default(),
        };
        crate::inbox::inbox(&self.store, None, &ctx, self.now, &|_, _| vec![])
            .unwrap()
            .attention
            .iter()
            .filter(|i| matches!(i, crate::inbox::AttentionItem::Unintegrated { .. }))
            .count()
    }

    fn assert_no_detection(&self) {
        assert_eq!(self.inbox_count(), 0);
        assert!(
            !self
                .store
                .events_for(self.record.task_id)
                .unwrap()
                .iter()
                .any(|(_, e)| matches!(e, Event::IntegrationDetected { .. }))
        );
        assert!(
            self.store
                .task_integration_list(Some(TaskRepoIntegrationStatus::Unintegrated))
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn sweep_does_not_detect_active_delivery_before_merge() {
    for state in [
        DeliveryState::Reviewing,
        DeliveryState::MergeQueued,
        DeliveryState::Merging,
        DeliveryState::Preparing,
    ] {
        let f = Fixture::new();
        f.delivery(state);
        assert_eq!(f.sweep(), 0, "{state:?}");
        f.assert_no_detection();
        f.merge();
        assert_eq!(f.sweep(), 1);
        assert_eq!(f.state(), Some(TaskRepoIntegrationStatus::Integrated));
        f.assert_no_detection();
    }
}

#[test]
fn sweep_rejects_stale_negative_when_delivery_starts_or_finishes_after_git() {
    for finish in [false, true] {
        let f = Fixture::new();
        let mut hook_called = false;
        let changed = sweep_inner(
            &f.store,
            std::slice::from_ref(&f.record),
            f.now,
            |_, after_git| {
                if after_git {
                    hook_called = true;
                    f.delivery(DeliveryState::Merging);
                    if finish {
                        f.merge();
                        f.delivery(DeliveryState::Ready);
                    }
                }
            },
        )
        .unwrap();
        assert!(hook_called);
        assert_eq!(changed, 0);
        f.assert_no_detection();
        if !finish {
            f.merge();
        }
        assert_eq!(f.sweep(), 1);
        assert_eq!(f.state(), Some(TaskRepoIntegrationStatus::Integrated));
        f.assert_no_detection();
    }
}

#[test]
fn sweep_reads_current_main_after_delivery_snapshot() {
    let f = Fixture::new();
    f.delivery(DeliveryState::Merging);
    let changed = sweep_inner(
        &f.store,
        std::slice::from_ref(&f.record),
        f.now,
        |_, after_git| {
            if !after_git {
                f.merge();
            }
        },
    )
    .unwrap();
    assert_eq!(changed, 1);
    assert_eq!(f.state(), Some(TaskRepoIntegrationStatus::Integrated));
    f.assert_no_detection();
}

#[test]
fn sweep_reconciles_old_detection_during_prepare_and_keeps_events() {
    let f = Fixture::new();
    assert_eq!(f.sweep(), 1);
    assert_eq!(f.state(), Some(TaskRepoIntegrationStatus::Unintegrated));
    assert_eq!(f.inbox_count(), 1);
    f.delivery(DeliveryState::Merging);
    f.merge();
    f.delivery(DeliveryState::Preparing);
    assert_eq!(f.sweep(), 1);
    assert_eq!(f.state(), Some(TaskRepoIntegrationStatus::Integrated));
    assert_eq!(f.sweep(), 0);
    assert_eq!(f.inbox_count(), 0);
    let events: Vec<_> = f
        .store
        .events_for(f.record.task_id)
        .unwrap()
        .into_iter()
        .filter(|(_, e)| {
            matches!(
                e,
                Event::IntegrationDetected { .. } | Event::IntegrationMerged { .. }
            )
        })
        .collect();
    assert!(matches!(events[0].1, Event::IntegrationDetected { .. }));
    assert!(matches!(events[1].1, Event::IntegrationMerged { .. }));
    assert_eq!(events.len(), 2);
}

#[test]
fn sweep_preserves_disposition_written_after_git() {
    for disposition in [
        TaskRepoIntegrationStatus::Held,
        TaskRepoIntegrationStatus::Discarded,
        TaskRepoIntegrationStatus::Integrated,
    ] {
        let f = Fixture::new();
        assert_eq!(f.sweep(), 1);
        f.merge();
        assert_eq!(
            sweep_inner(
                &f.store,
                std::slice::from_ref(&f.record),
                f.now,
                |_, after_git| {
                    if after_git {
                        let mut row = f
                            .store
                            .task_integration_get(f.record.task_id, f.record.repo_id)
                            .unwrap()
                            .unwrap();
                        row.state = disposition;
                        row.actor = "human".into();
                        f.store.task_integration_upsert(&row).unwrap();
                    }
                }
            )
            .unwrap(),
            0
        );
        assert_eq!(f.state(), Some(disposition));
    }
}

#[test]
fn sweep_still_detects_stranded_branches_after_terminal_delivery() {
    for state in [DeliveryState::Ready, DeliveryState::Blocked] {
        let f = Fixture::new();
        f.delivery(state);
        assert_eq!(f.sweep(), 1);
        assert_eq!(f.state(), Some(TaskRepoIntegrationStatus::Unintegrated));
    }
}

fn sample_task(kind: TaskKind, status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind,
        title: "do something".to_string(),
        objective: "make it work".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
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
