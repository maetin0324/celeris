use super::*;
use std::time::Duration as StdDuration;
use task_core::{ArtifactRef, Budget, Criterion, Lease, SqliteStore, WorkerHint};

fn view_ctx() -> ViewContext {
    ViewContext {
        workspace_root: PathBuf::from("/tmp/workspaces"),
        retry_backoff_base: StdDuration::from_secs(10),
        retry_backoff_max: StdDuration::from_secs(300),
        max_requeues: 5,
        clusters: Default::default(),
    }
}

fn sample_task(kind: TaskKind, status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
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
        inputs: vec![ArtifactRef {
            name: "spec".to_string(),
            path: "spec.md".to_string(),
            sha256: "abc".to_string(),
            kind: "doc".to_string(),
            declared: true,
        }],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
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

fn row(seq: u64, ts: &str, task_id: TaskId, event: Event) -> EventRow {
    EventRow {
        id: seq,
        task_id,
        seq,
        ts: ts.to_string(),
        event,
    }
}

fn artifact(name: &str) -> ArtifactRef {
    ArtifactRef {
        name: name.to_string(),
        path: format!("artifacts/{name}"),
        sha256: "abc".to_string(),
        kind: "doc".to_string(),
        declared: true,
    }
}

/// プールの run（ADR-0024 D4）: `WorkerStarted.account` が要約に出る。
#[test]
fn runs_expose_the_pool_account_of_the_run() {
    let tid = TaskId::new();
    let pooled = vec![row(
        0,
        "t0",
        tid,
        started_with_account("r1", Some("claude-pool"), Some("acct-a")),
    )];
    let summaries = runs(&pooled);
    assert_eq!(summaries[0].account.as_deref(), Some("acct-a"));
    assert_eq!(summaries[0].provider.as_deref(), Some("claude-pool"));

    // プールでない run は None のまま（既存の形）。
    let plain = vec![row(0, "t0", tid, started("r2", Some("claude-a")))];
    assert!(runs(&plain)[0].account.is_none());
}

fn started_with_account(run_id: &str, provider: Option<&str>, account: Option<&str>) -> Event {
    let Event::WorkerStarted {
        run_id,
        adapter,
        model,
        provider,
        role,
        task_role,
        ..
    } = started(run_id, provider)
    else {
        unreachable!("started builds a WorkerStarted")
    };
    Event::WorkerStarted {
        run_id,
        adapter,
        model,
        provider,
        account: account.map(str::to_string),
        role,
        task_role,
    }
}

fn started(run_id: &str, provider: Option<&str>) -> Event {
    Event::WorkerStarted {
        run_id: run_id.to_string(),
        adapter: "claude-code".to_string(),
        model: "claude-sonnet-5".to_string(),
        provider: provider.map(str::to_string),
        account: None,
        role: None,
        task_role: None,
    }
}

fn finished(run_id: &str, outcome: &str) -> Event {
    Event::WorkerFinished {
        run_id: run_id.to_string(),
        outcome: outcome.to_string(),
        usage: None,
        role: None,
        metrics: None,
        end: None,
    }
}

// ---- runs ----

/// ADR-0014 D1: Reviewer run も一覧に現れ、`role` で区別される（イベントに `role` が無ければ worker）。
#[test]
fn runs_include_reviewer_runs_with_role() {
    let task_id = TaskId::new();
    let events = vec![
        started("run-1", Some("acct-a")),
        finished("run-1", "done: implemented"),
        Event::WorkerStarted {
            run_id: "rev-1".into(),
            adapter: "claude-code".into(),
            model: "m".into(),
            provider: Some("acct-b".into()),
            account: None,
            role: Some(RunRole::Reviewer),
            task_role: None,
        },
        Event::WorkerFinished {
            run_id: "rev-1".into(),
            outcome: "done: reviewed".into(),
            usage: Some(Usage {
                input_tokens: Some(5),
                output_tokens: Some(7),
                cache_read_tokens: None,
                cache_creation_tokens: None,
                cost_usd: None,
                duplicate_reads: None,
                session_resumed: None,
            }),
            role: Some(RunRole::Reviewer),
            metrics: None,
            end: None,
        },
    ];
    let rows: Vec<EventRow> = events
        .into_iter()
        .enumerate()
        .map(|(i, event)| EventRow {
            id: i as u64 + 1,
            task_id,
            seq: i as u64,
            ts: format!("2026-09-14T00:00:0{i}Z"),
            event,
        })
        .collect();
    let runs = runs(&rows);
    assert_eq!(runs.len(), 2);
    assert_eq!(
        (runs[0].run_id.as_str(), runs[0].role),
        ("run-1", RunRole::Worker)
    );
    assert_eq!(
        (runs[1].run_id.as_str(), runs[1].role),
        ("rev-1", RunRole::Reviewer)
    );
    assert_eq!(runs[1].provider.as_deref(), Some("acct-b"));
    assert_eq!(
        (runs[1].outcome, runs[1].outcome_text.as_deref()),
        (Some(RunOutcomeKind::Done), Some("reviewed"))
    );
    assert_eq!(runs[1].usage.and_then(|u| u.input_tokens), Some(5));
}

#[test]
fn runs_classifies_done_outcome_with_provider_and_outcome_text() {
    let tid = TaskId::new();
    let rows = vec![
        row(
            0,
            "2024-01-01T00:00:00Z",
            tid,
            started("r1", Some("claude-a")),
        ),
        row(
            1,
            "2024-01-01T00:00:05Z",
            tid,
            finished("r1", "done: all good"),
        ),
    ];
    let summaries = runs(&rows);
    assert_eq!(summaries.len(), 1);
    let s = &summaries[0];
    assert_eq!(s.run_id, "r1");
    assert_eq!(s.provider.as_deref(), Some("claude-a"));
    assert_eq!(s.outcome, Some(RunOutcomeKind::Done));
    assert_eq!(s.outcome_text.as_deref(), Some("all good"));
    assert_eq!(s.finished_at.as_deref(), Some("2024-01-01T00:00:05Z"));
    assert_eq!(s.started_at, "2024-01-01T00:00:00Z");
    assert!(s.files.is_none());
}

#[test]
fn runs_classifies_question_outcome() {
    let tid = TaskId::new();
    let rows = vec![
        row(0, "t0", tid, started("r1", None)),
        row(1, "t1", tid, finished("r1", "question: which?")),
    ];
    let s = &runs(&rows)[0];
    assert_eq!(s.outcome, Some(RunOutcomeKind::Question));
    assert_eq!(s.outcome_text.as_deref(), Some("which?"));
    assert!(s.provider.is_none());
}

#[test]
fn runs_classifies_requeue_outcome() {
    let tid = TaskId::new();
    let rows = vec![
        row(0, "t0", tid, started("r1", None)),
        row(1, "t1", tid, finished("r1", "requeue: adapter: throttled")),
    ];
    let s = &runs(&rows)[0];
    assert_eq!(s.outcome, Some(RunOutcomeKind::Requeue));
    assert_eq!(s.outcome_text.as_deref(), Some("adapter: throttled"));
}

#[test]
fn runs_classifies_lease_expired_as_exact_match() {
    let tid = TaskId::new();
    let rows = vec![
        row(0, "t0", tid, started("r1", None)),
        row(1, "t1", tid, finished("r1", "lease_expired")),
    ];
    let s = &runs(&rows)[0];
    assert_eq!(s.outcome, Some(RunOutcomeKind::LeaseExpired));
    assert!(s.outcome_text.is_none());
}

#[test]
fn runs_classifies_anything_else_as_error() {
    let tid = TaskId::new();
    let rows = vec![
        row(0, "t0", tid, started("r1", None)),
        row(1, "t1", tid, finished("r1", "error(retryable=true): boom")),
    ];
    let s = &runs(&rows)[0];
    assert_eq!(s.outcome, Some(RunOutcomeKind::Error));
}

#[test]
fn runs_in_progress_run_has_no_finished_at_or_outcome() {
    let tid = TaskId::new();
    let rows = vec![row(0, "t0", tid, started("r1", None))];
    let s = &runs(&rows)[0];
    assert!(s.finished_at.is_none());
    assert!(s.outcome.is_none());
}

#[test]
fn runs_counts_progress_artifacts_verdicts_and_reviewer_deferrals() {
    let tid = TaskId::new();
    let rows = vec![
        row(0, "t0", tid, started("r1", None)),
        row(1, "t1", tid, Event::worker_progress("r1", "chugging along")),
        row(
            2,
            "t2",
            tid,
            Event::worker_progress(
                "r1",
                format!("{}throttled", derive::REVIEWER_REQUEUED_PREFIX),
            ),
        ),
        row(
            3,
            "t3",
            tid,
            Event::ArtifactProduced {
                run_id: "r1".into(),
                artifact: artifact("a"),
            },
        ),
        row(
            4,
            "t4",
            tid,
            Event::ReviewVerdict {
                run_id: "r1".into(),
                criterion_idx: 0,
                pass: true,
                reason: "ok".into(),
            },
        ),
        row(5, "t5", tid, finished("r1", "done: x")),
    ];
    let s = &runs(&rows)[0];
    assert_eq!(s.progress, 2);
    assert_eq!(s.artifacts, 1);
    assert_eq!(s.verdicts, 1);
    assert_eq!(s.reviewer_deferrals, 1);
}

#[test]
fn runs_are_sorted_by_started_at_ascending_regardless_of_input_order() {
    let tid = TaskId::new();
    let rows = vec![
        row(0, "2024-01-02T00:00:00Z", tid, started("later", None)),
        row(1, "2024-01-01T00:00:00Z", tid, started("earlier", None)),
    ];
    let s = runs(&rows);
    assert_eq!(s[0].run_id, "earlier");
    assert_eq!(s[1].run_id, "later");
}

// ---- timers ----

#[test]
fn timers_reports_lease_expires_at_when_running() {
    let mut task = sample_task(TaskKind::Execute, Status::Running);
    let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(60);
    task.lease = Some(Lease {
        worker_run_id: "run-1".to_string(),
        expires_at,
    });
    let ctx = view_ctx();
    let t = timers(&task, &[], &ctx, OffsetDateTime::now_utc());
    assert_eq!(t.lease_expires_at, Some(to_rfc3339(expires_at)));
    // `timers.now` はスケルトンの `to_string()` ではなく RFC 3339 でなければならない。
    assert!(OffsetDateTime::parse(&t.now, &Rfc3339).is_ok());
}

#[test]
fn timers_lease_expires_at_is_none_when_not_running() {
    let task = sample_task(TaskKind::Execute, Status::Ready);
    let ctx = view_ctx();
    let t = timers(&task, &[], &ctx, OffsetDateTime::now_utc());
    assert!(t.lease_expires_at.is_none());
}

#[test]
fn timers_backoff_until_set_for_ready_task_with_attempts() {
    let now = OffsetDateTime::now_utc();
    let mut task = sample_task(TaskKind::Execute, Status::Ready);
    task.attempts = 2;
    task.updated_at = now;
    let ctx = view_ctx();
    let t = timers(&task, &[], &ctx, now);
    assert!(t.backoff_until.is_some());
}

#[test]
fn timers_backoff_until_none_when_base_is_zero() {
    let now = OffsetDateTime::now_utc();
    let mut task = sample_task(TaskKind::Execute, Status::Ready);
    task.attempts = 2;
    task.updated_at = now;
    let mut ctx = view_ctx();
    ctx.retry_backoff_base = StdDuration::ZERO;
    let t = timers(&task, &[], &ctx, now);
    assert!(t.backoff_until.is_none());
}

#[test]
fn timers_backoff_until_none_when_already_past() {
    let now = OffsetDateTime::now_utc();
    let mut task = sample_task(TaskKind::Execute, Status::Ready);
    task.attempts = 1;
    task.updated_at = now - time::Duration::hours(1);
    let ctx = view_ctx();
    let t = timers(&task, &[], &ctx, now);
    assert!(t.backoff_until.is_none());
}

#[test]
fn timers_counts_consecutive_requeues_from_rows() {
    let now = OffsetDateTime::now_utc();
    let task = sample_task(TaskKind::Execute, Status::Ready);
    let rows = vec![
        row(
            0,
            "t0",
            task.id,
            Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "requeue".into(),
            },
        ),
        row(
            1,
            "t1",
            task.id,
            Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "dispatch".into(),
            },
        ),
        row(
            2,
            "t2",
            task.id,
            Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "requeue".into(),
            },
        ),
    ];
    let ctx = view_ctx();
    let t = timers(&task, &rows, &ctx, now);
    assert_eq!(t.consecutive_requeues, 2);
    assert_eq!(t.max_requeues, ctx.max_requeues);
}

// ---- task_list ----

#[test]
fn task_list_filters_by_status() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let t1 = sample_task(TaskKind::Execute, Status::Draft);
    let t2 = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&t1).expect("insert t1");
    store.insert(&t2).expect("insert t2");

    let ctx = view_ctx();
    let filter = ListFilter {
        statuses: vec![Status::Ready],
        ..Default::default()
    };
    let list = task_list(
        &store,
        &filter,
        ListOrder::CreatedDesc,
        None,
        100,
        &ctx,
        OffsetDateTime::now_utc(),
    )
    .expect("task_list");
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].id, t2.id);
    assert_eq!(list.total, 1);
}

#[test]
fn task_list_counts_by_status_ignores_filter() {
    let store = SqliteStore::open_in_memory().expect("open store");
    store
        .insert(&sample_task(TaskKind::Execute, Status::Draft))
        .expect("insert");
    store
        .insert(&sample_task(TaskKind::Execute, Status::Ready))
        .expect("insert");
    store
        .insert(&sample_task(TaskKind::Execute, Status::Ready))
        .expect("insert");

    let ctx = view_ctx();
    let filter = ListFilter {
        statuses: vec![Status::Ready],
        ..Default::default()
    };
    let list = task_list(
        &store,
        &filter,
        ListOrder::CreatedDesc,
        None,
        100,
        &ctx,
        OffsetDateTime::now_utc(),
    )
    .expect("task_list");
    assert_eq!(list.items.len(), 2);
    assert_eq!(list.counts_by_status.get("draft").copied(), Some(1));
    assert_eq!(list.counts_by_status.get("ready").copied(), Some(2));
}

#[test]
fn task_list_paginates_with_cursor_without_duplicates() {
    let store = SqliteStore::open_in_memory().expect("open store");
    for _ in 0..5 {
        store
            .insert(&sample_task(TaskKind::Execute, Status::Draft))
            .expect("insert");
    }
    let ctx = view_ctx();
    let now = OffsetDateTime::now_utc();

    let mut seen: Vec<TaskId> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = task_list(
            &store,
            &ListFilter::default(),
            ListOrder::CreatedDesc,
            cursor.as_deref(),
            2,
            &ctx,
            now,
        )
        .expect("task_list");
        seen.extend(page.items.iter().map(|t| t.id));
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 5);
}

#[test]
fn task_list_reports_children_and_pending_children() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let parent = sample_task(TaskKind::Plan, Status::Done);
    store.insert(&parent).expect("insert parent");
    let mut c1 = sample_task(TaskKind::Execute, Status::Draft);
    c1.parent_id = Some(parent.id);
    let mut c2 = sample_task(TaskKind::Execute, Status::Done);
    c2.parent_id = Some(parent.id);
    store.insert(&c1).expect("insert c1");
    store.insert(&c2).expect("insert c2");

    let ctx = view_ctx();
    let list = task_list(
        &store,
        &ListFilter::default(),
        ListOrder::CreatedDesc,
        None,
        100,
        &ctx,
        OffsetDateTime::now_utc(),
    )
    .expect("task_list");
    let parent_summary = list
        .items
        .iter()
        .find(|t| t.id == parent.id)
        .expect("parent in list");
    assert_eq!(parent_summary.children, 2);
    assert_eq!(parent_summary.pending_children, 1);
}

// ---- task_detail ----

/// ADR-0018 実装メモ M5: Remote のタスクは `cluster` にクラスタ id、`workspace_dir` に手元の写し（`workspace_root/<task_id>`）が出る。
/// Local のタスクは `cluster: None`。
#[test]
fn task_detail_reports_cluster_and_mirror_for_remote_workspaces() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut remote = sample_task(TaskKind::Execute, Status::Ready);
    remote.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/NBB/x/project"),
        mode: None,
    };
    store.insert(&remote).expect("insert");
    let local = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&local).expect("insert");

    let ctx = view_ctx();
    let detail = task_detail(&store, remote.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert_eq!(detail.cluster.as_deref(), Some("pegasus"));
    assert_eq!(
        detail.workspace_dir.as_deref(),
        Some(format!("/tmp/workspaces/{}", remote.id).as_str()),
        "the mirror where runs/ live"
    );
    let detail = task_detail(&store, local.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert_eq!(detail.cluster, None);
}

/// ADR-0019 D2: `sync = "worktree"` のクラスタでは、worktree のパスとブランチを出す（人が diff / commit する場所）。
/// `rsync` / `none` のクラスタと Local のタスクでは `null`。
#[test]
fn task_detail_reports_the_worktree_for_worktree_clusters() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut on_worktree = sample_task(TaskKind::Execute, Status::Ready);
    on_worktree.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/NBB/x/benchfs"),
        mode: None,
    };
    store.insert(&on_worktree).expect("insert");
    let mut on_rsync = sample_task(TaskKind::Execute, Status::Ready);
    on_rsync.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/work/NBB/x/scratch"),
        mode: None,
    };
    store.insert(&on_rsync).expect("insert");

    let mut ctx = view_ctx();
    ctx.clusters.insert(
        "pegasus".to_string(),
        ClusterViewInfo {
            sync: "worktree".to_string(),
            worktree_root: None,
            ..Default::default()
        },
    );
    ctx.clusters.insert(
        "sirius".to_string(),
        ClusterViewInfo {
            sync: "rsync".to_string(),
            worktree_root: None,
            ..Default::default()
        },
    );

    let detail =
        task_detail(&store, on_worktree.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    let wt = detail.worktree.expect("worktree cluster");
    assert_eq!(wt.project, "/work/NBB/x/benchfs");
    assert_eq!(
        wt.dir,
        format!("/work/NBB/x/benchfs/.celeris-worktrees/{}", on_worktree.id)
    );
    assert_eq!(wt.branch, format!("celeris/{}", on_worktree.id));

    let detail = task_detail(&store, on_rsync.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert_eq!(
        detail.worktree, None,
        "rsync のクラスタには worktree が無い"
    );

    // worktree_root を設定したらそちらが親になる。
    ctx.clusters.insert(
        "pegasus".to_string(),
        ClusterViewInfo {
            sync: "worktree".to_string(),
            worktree_root: Some(PathBuf::from("/work/NBB/x/wt")),
            ..Default::default()
        },
    );
    let detail =
        task_detail(&store, on_worktree.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert_eq!(
        detail.worktree.expect("worktree").dir,
        format!("/work/NBB/x/wt/{}", on_worktree.id)
    );
}

/// GUI-R2（ADR-0016 D1）: 一覧の各行にも `role` が出る（詳細を N+1 で引かなくてよい）。
#[test]
fn task_list_items_carry_the_role() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut lead = sample_task(TaskKind::Execute, Status::Ready);
    lead.role = Some("lead".to_string());
    store.insert(&lead).expect("insert");
    let plain = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&plain).expect("insert");

    let list = task_list(
        &store,
        &ListFilter::default(),
        ListOrder::CreatedDesc,
        None,
        100,
        &view_ctx(),
        OffsetDateTime::now_utc(),
    )
    .expect("task_list");
    let role_of = |id| {
        list.items
            .iter()
            .find(|t| t.id == id)
            .expect("in list")
            .role
            .clone()
    };
    assert_eq!(role_of(lead.id).as_deref(), Some("lead"));
    assert_eq!(role_of(plain.id), None);
}

/// GUI 監査 H4（Phase 29）: 一覧の各項目に裏方の印が出る（決定的な優先順。対話が最優先）。
#[test]
fn task_list_items_carry_the_support_kind() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut plain = sample_task(TaskKind::Execute, Status::Ready);
    plain.role = None;
    store.insert(&plain).expect("insert plain");
    let mut compaction = sample_task(TaskKind::Execute, Status::Ready);
    compaction.role = Some(task_core::COMPACTION_ROLE.to_string());
    store.insert(&compaction).expect("insert compaction");
    let approval = sample_task(TaskKind::Approval, Status::Ready);
    store.insert(&approval).expect("insert approval");
    let review = sample_task(TaskKind::Review, Status::Reviewing);
    store.insert(&review).expect("insert review");

    let list = task_list(
        &store,
        &ListFilter::default(),
        ListOrder::CreatedDesc,
        None,
        100,
        &view_ctx(),
        OffsetDateTime::now_utc(),
    )
    .expect("task_list");
    let support_of = |id| {
        list.items
            .iter()
            .find(|t| t.id == id)
            .expect("in list")
            .support
            .clone()
    };
    assert_eq!(support_of(plain.id), None);
    assert_eq!(support_of(compaction.id).as_deref(), Some("compaction"));
    assert_eq!(support_of(approval.id).as_deref(), Some("approval"));
    assert_eq!(support_of(review.id).as_deref(), Some("review"));
}

/// ADR-0027 D1: `Task.genre` は `role` と同じ理由で一覧の各項目に出る。
#[test]
fn task_list_items_carry_the_genre() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut with_genre = sample_task(TaskKind::Execute, Status::Ready);
    with_genre.genre = Some("coding".to_string());
    store.insert(&with_genre).expect("insert");
    let plain = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&plain).expect("insert");

    let list = task_list(
        &store,
        &ListFilter::default(),
        ListOrder::CreatedDesc,
        None,
        100,
        &view_ctx(),
        OffsetDateTime::now_utc(),
    )
    .expect("task_list");
    let genre_of = |id| {
        list.items
            .iter()
            .find(|t| t.id == id)
            .expect("in list")
            .genre
            .clone()
    };
    assert_eq!(genre_of(with_genre.id).as_deref(), Some("coding"));
    assert_eq!(genre_of(plain.id), None);
}

/// ADR-0016 D1/D2: `role` はトップレベルにも出て、`delegated` は `Event::Delegated` から組み立てる。
#[test]
fn task_detail_reports_role_and_delegated_children() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut parent = sample_task(TaskKind::Execute, Status::Running);
    parent.role = Some("lead".to_string());
    store.insert(&parent).expect("insert parent");

    let mut child = sample_task(TaskKind::Execute, Status::Ready);
    child.parent_id = Some(parent.id);
    store.insert(&child).expect("insert child");

    let missing_child = TaskId::new();
    store
        .append_event(
            parent.id,
            &Event::Delegated {
                run_id: "run-1".into(),
                task_ids: vec![child.id, missing_child],
            },
        )
        .expect("append delegated");

    let ctx = view_ctx();
    let detail = task_detail(&store, parent.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert_eq!(detail.role.as_deref(), Some("lead"));
    assert_eq!(detail.delegated.len(), 1);
    assert_eq!(detail.delegated[0].run_id, "run-1");
    assert_eq!(
        detail.delegated[0].tasks.len(),
        1,
        "missing child id is dropped"
    );
    assert_eq!(detail.delegated[0].tasks[0].id, child.id);
}

/// ADR-0027 D1: `genre` も `role` と同じく最上位に出る。
#[test]
fn task_detail_reports_genre() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut task = sample_task(TaskKind::Execute, Status::Running);
    task.genre = Some("literature".to_string());
    store.insert(&task).expect("insert");

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert_eq!(detail.genre.as_deref(), Some("literature"));
}

#[test]
fn task_detail_missing_task_returns_not_found() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let missing = TaskId::new();
    let ctx = view_ctx();
    let result = task_detail(&store, missing, &ctx, OffsetDateTime::now_utc());
    assert!(matches!(result, Err(OpsError::NotFound(id)) if id == missing));
}

#[test]
fn task_detail_reports_dependencies_dependents_children_and_actions() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let dep = sample_task(TaskKind::Execute, Status::Done);
    store.insert(&dep).expect("insert dep");
    let mut task = sample_task(TaskKind::Execute, Status::Ready);
    task.depends_on = vec![dep.id];
    store.insert(&task).expect("insert task");
    let mut dependent = sample_task(TaskKind::Execute, Status::Draft);
    dependent.depends_on = vec![task.id];
    store.insert(&dependent).expect("insert dependent");
    let mut child = sample_task(TaskKind::Execute, Status::Draft);
    child.parent_id = Some(task.id);
    store.insert(&child).expect("insert child");

    let ctx = view_ctx();
    let detail =
        task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("task_detail");
    assert_eq!(
        detail.dependencies.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![dep.id]
    );
    assert_eq!(
        detail.dependents.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![dependent.id]
    );
    assert_eq!(
        detail.children.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![child.id]
    );
    assert!(detail.actions.contains(&Action::Cancel));
    assert!(detail.worker_run_hint.is_some());
}

#[test]
fn task_detail_blocked_task_reports_latest_question() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&task).expect("insert task");
    store
        .append_event(
            task.id,
            &Event::WorkerFinished {
                run_id: "r1".into(),
                outcome: "question: which version?".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("append worker finished");

    let ctx = view_ctx();
    let detail =
        task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("task_detail");
    assert_eq!(detail.latest_question.as_deref(), Some("which version?"));
    assert!(detail.actions.contains(&Action::Answer));
}

#[test]
fn task_detail_terminal_task_has_no_worker_run_hint() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Done);
    store.insert(&task).expect("insert task");
    let ctx = view_ctx();
    let detail =
        task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("task_detail");
    assert!(detail.worker_run_hint.is_none());
    // ADR-0044 D2（Phase 53）: `done` は編集できないが「再開」はできる。
    assert_eq!(detail.actions, vec![Action::Reopen]);
    assert!(detail.failure.is_none(), "done is not a failure");
}

/// ADR-0070 D1/D2（Phase 116。D6(a)）: `failed` のタスク詳細は `failure`（分類・理由・配送済みの
/// release）を持ち、`review_fail` だけが原因のときだけ `Action::Rereview` が付く。
#[test]
fn task_detail_failed_task_reports_failure_and_rereview_when_review_fail_is_the_only_reason() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut task = sample_task(TaskKind::Execute, Status::Failed);
    task.acceptance = vec![Criterion {
        text: "reviewer checks it".into(),
        check: Check::Reviewer,
    }];
    store.insert(&task).expect("insert task");
    store
        .append_event(
            task.id,
            &Event::ReviewVerdict {
                run_id: "rev-1".into(),
                criterion_idx: 0,
                pass: false,
                reason: "テストが落ちている".into(),
            },
        )
        .expect("verdict");
    store
        .append_event(
            task.id,
            &Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Failed,
                reason: "review_fail".into(),
            },
        )
        .expect("transitioned");

    let ctx = view_ctx();
    let detail =
        task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("task_detail");
    let failure = detail.failure.expect("failure summary");
    assert_eq!(failure.class, derive::FailureClass::Work);
    assert_eq!(failure.reason, "テストが落ちている");
    assert!(failure.delivered_release.is_none());
    assert!(detail.actions.contains(&Action::Retry));
    assert!(detail.actions.contains(&Action::Rereview));
}

/// インフラ分類（`infra failure ×N`）は `Action::Rereview` を出さない
/// （`review_fail` が原因ではないので `can_rereview` が偽になる）。
#[test]
fn task_detail_infra_failed_task_reports_infra_class_without_rereview() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Failed);
    store.insert(&task).expect("insert task");
    store
        .append_event(
            task.id,
            &Event::WorkerFinished {
                run_id: "run-1".into(),
                outcome: "infra failure ×5: adapter: session resume rejected".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("finished");

    let ctx = view_ctx();
    let detail =
        task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("task_detail");
    let failure = detail.failure.expect("failure summary");
    assert_eq!(failure.class, derive::FailureClass::Infra);
    assert!(detail.actions.contains(&Action::Retry));
    assert!(!detail.actions.contains(&Action::Rereview));
}

#[test]
fn task_detail_links_human_approval_children_by_attempt_and_marks_decided() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut parent = sample_task(TaskKind::Execute, Status::Reviewing);
    parent.acceptance = vec![Criterion {
        text: "human ok".into(),
        check: Check::Human,
    }];
    store.insert(&parent).expect("insert parent");

    let title1 = {
        let mut t = parent.clone();
        t.attempts = 0;
        derive::human_approval_title(&t, 0)
    };
    let mut approval1 = sample_task(TaskKind::Approval, Status::Done);
    approval1.parent_id = Some(parent.id);
    approval1.title = title1;
    store.insert(&approval1).expect("insert approval1");
    store
        .append_event(
            approval1.id,
            &Event::ApprovalDecided {
                by: "human".into(),
                approved: true,
                note: Some("lgtm".into()),
            },
        )
        .expect("append decided");

    let title2 = {
        let mut t = parent.clone();
        t.attempts = 1;
        derive::human_approval_title(&t, 0)
    };
    let mut approval2 = sample_task(TaskKind::Approval, Status::Ready);
    approval2.parent_id = Some(parent.id);
    approval2.title = title2;
    store.insert(&approval2).expect("insert approval2");

    let ctx = view_ctx();
    let detail =
        task_detail(&store, parent.id, &ctx, OffsetDateTime::now_utc()).expect("task_detail");

    assert_eq!(detail.approvals.len(), 2);
    let approved = detail
        .approvals
        .iter()
        .find(|a| a.approval.id == approval1.id)
        .expect("approval1 link present");
    assert_eq!(approved.criterion_idx, Some(0));
    assert_eq!(approved.attempt, Some(1));
    assert!(
        approved
            .decided
            .as_ref()
            .is_some_and(|d| d.approved && d.note.as_deref() == Some("lgtm"))
    );

    let pending = detail
        .approvals
        .iter()
        .find(|a| a.approval.id == approval2.id)
        .expect("approval2 link present");
    assert_eq!(pending.attempt, Some(2));
    assert!(pending.decided.is_none());

    assert_eq!(detail.criteria.len(), 1);
    assert_eq!(
        detail.criteria[0].approval.as_ref().map(|a| a.approval.id),
        Some(approval2.id)
    );
}

// ---- parse_human_approval_title ----

#[test]
fn parse_human_approval_title_roundtrips_with_human_approval_title() {
    let mut task = sample_task(TaskKind::Execute, Status::Reviewing);
    task.title = "fix the bug".into();
    task.attempts = 2;
    let title = derive::human_approval_title(&task, 4);
    assert_eq!(parse_human_approval_title(&title), Some((4, 3)));
}

#[test]
fn parse_human_approval_title_handles_title_containing_the_marker() {
    let mut task = sample_task(TaskKind::Execute, Status::Reviewing);
    task.title = "weird task — criterion 9 (attempt 9)".into();
    task.attempts = 0;
    let title = derive::human_approval_title(&task, 2);
    assert_eq!(parse_human_approval_title(&title), Some((2, 1)));
}

#[test]
fn parse_human_approval_title_none_for_unrelated_or_malformed_strings() {
    assert_eq!(parse_human_approval_title("just a regular title"), None);
    assert_eq!(
        parse_human_approval_title("Approval needed: x — criterion abc (attempt 1)"),
        None
    );
    assert_eq!(
        parse_human_approval_title("Approval needed: x — criterion 1 (attempt)"),
        None
    );
}

// ---- ADR-0072 D19/D20（Phase E5）: Execution 節 ----

fn wu_spec(
    key: &str,
    kind: task_core::WorkUnitKind,
    depends_on: &[&str],
) -> task_core::WorkUnitSpec {
    task_core::WorkUnitSpec {
        key: key.to_string(),
        kind,
        title: format!("title {key}"),
        objective: format!("objective for {key}, spelled out plainly and distinctly"),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        done_when: vec![],
        checks: vec![],
        context: task_core::WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    }
}

fn plan_spec(work_units: Vec<task_core::WorkUnitSpec>) -> task_core::ExecutionPlanSpec {
    task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "investigate then implement".to_string(),
        work_units,
        phases: Vec::new(),
        children: Vec::new(),
    }
}

fn set_status(store: &SqliteStore, task_id: TaskId, key: &str, status: task_core::WorkUnitStatus) {
    let units = store.work_units_for(task_id).expect("work_units_for");
    let row = units.iter().find(|u| u.key == key).expect("wu");
    let mut updated = row.clone();
    let from = updated.status;
    updated.status = status;
    store
        .work_unit_transition(
            task_id,
            updated,
            Event::WorkUnitTransitioned {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                from,
                to: status,
                reason: "completed".to_string(),
                run_id: None,
            },
        )
        .expect("work_unit_transition");
}

/// (c): 計画も gate の判定も無い（events に E-phase の活動が無い）古いタスクは
/// `execution: None` のまま（`task_detail` の他のフィールドは変わらない）。
#[test]
fn task_detail_execution_is_none_for_a_task_with_no_execution_activity() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(TaskKind::Execute, Status::Done);
    store.insert(&task).expect("insert");
    store
        .append_event(
            task.id,
            &Event::WorkerFinished {
                run_id: "r1".to_string(),
                outcome: "done: ok".to_string(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("append");

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    assert!(detail.execution.is_none());
}

#[test]
fn task_detail_write_set_and_behind_show_durable_snapshots() {
    use task_core::execution_plan::{RunIndexRole, RunIndexStatus, RunRow};
    use task_core::repos::RepoId;
    use task_core::write_set::{WriteSetRecord, WriteSetStatus};
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).unwrap();
    store
        .set_task_expected_write_paths(task.id, Some(&["src/".into()]), "2026-10-02T00:00:00Z")
        .unwrap();
    let repo_id = RepoId::new();
    store
        .run_index_start(RunRow {
            run_id: "run-1".into(),
            task_id: task.id.to_string(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Completed,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-10-02T00:00:00Z".into(),
            finished_at: Some("2026-10-02T00:01:00Z".into()),
        })
        .unwrap();
    store
        .record_run_write_set(&WriteSetRecord {
            owner_id: "run-1".into(),
            task_id: task.id,
            work_unit_id: None,
            repo_id,
            base_sha: Some("base".into()),
            head_sha: Some("head".into()),
            paths: vec!["src/main.rs".into()],
            status: WriteSetStatus::Complete,
            reason: None,
            recorded_at: "2026-10-02T00:01:00Z".into(),
        })
        .unwrap();
    store
        .record_behind_target(&task_core::behind_target::BehindTargetObservation {
            task_id: task.id,
            repo_id,
            target_ref: "refs/heads/main".into(),
            target_sha: Some("target".into()),
            head_sha: Some("head".into()),
            commits: Some(2),
            observed_at: "2026-10-02T00:00:00Z".into(),
        })
        .unwrap();
    let now = OffsetDateTime::parse(
        "2026-10-02T01:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    let detail = task_detail(&store, task.id, &view_ctx(), now).unwrap();
    assert_eq!(detail.expected_write_paths, Some(vec!["src".into()]));
    assert_eq!(detail.actual_run_write_sets[0].paths, ["src/main.rs"]);
    assert_eq!(detail.actual_run_write_sets[0].status, "complete");
    assert_eq!(detail.behind_target.behind_target_commits, Some(2));
    assert_eq!(detail.behind_target.behind_target_age_seconds, Some(3600));
    assert_eq!(
        detail.behind_target.repos[0].target_sha.as_deref(),
        Some("target")
    );
}

/// gate が atomic と判定しただけ（計画なし）の Task は Execution 節が出るが `plan` は無い
/// （D20 の「直接実行」の 1 行に対応する材料）。
#[test]
fn task_detail_execution_shows_gate_only_when_there_is_no_plan() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut task = sample_task(TaskKind::Execute, Status::Running);
    task.routing = Some(task_core::TaskRouting {
        execution: Some(task_core::ExecutionGateDecision {
            mode: task_core::ExecutionMode::Atomic,
            source: task_core::GateSource::Policy,
            score: 1,
            threshold: 5,
            rule_id: "atomic/score".to_string(),
            signals: vec![],
            policy_version: task_core::EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow: false,
            depth: None,
        }),
        ..task_core::TaskRouting::default()
    });
    store.insert(&task).expect("insert");
    store
        .append_event(
            task.id,
            &Event::ExecutionGated {
                decision: Box::new(task.routing.as_ref().unwrap().execution.clone().unwrap()),
            },
        )
        .expect("append");

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    let execution = detail.execution.expect("execution present");
    assert_eq!(
        execution.gate.map(|g| g.mode),
        Some(task_core::ExecutionMode::Atomic)
    );
    assert!(execution.plan.is_none());
    // 走っている WU が無い（そもそも計画が無い）ので phase は導出しない。
    assert!(execution.phase.is_none());
    // ADR-0124: この Task はまだ経路を評価していない（`routing.route` も `ExecutionRouted` も無い）。
    assert!(execution.route.is_none());
}

/// ADR-0124: `routing.route` と `Event::ExecutionRouted` が揃っている Task は `TaskDetail.execution.route`
/// にそのまま出る。
#[test]
fn task_detail_execution_direct_route_is_reported_when_the_event_is_present() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut task = sample_task(TaskKind::Execute, Status::Running);
    let decision = task_core::RouteDecision {
        route: task_core::Route::Direct,
        reasons: vec![task_core::RouteReason {
            rule_id: "direct/single-repo".to_string(),
            ok: true,
            detail: "repos=1, multi_environment=false".to_string(),
        }],
        gate_rule_id: "atomic/score".to_string(),
        overrode_gate: false,
        shadow: false,
        policy_version: task_core::DIRECT_ROUTE_POLICY_VERSION.to_string(),
    };
    task.routing = Some(task_core::TaskRouting {
        execution: Some(task_core::ExecutionGateDecision {
            mode: task_core::ExecutionMode::Atomic,
            source: task_core::GateSource::Policy,
            score: 1,
            threshold: 5,
            rule_id: "atomic/score".to_string(),
            signals: vec![],
            policy_version: task_core::EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow: false,
            depth: None,
        }),
        route: Some(decision.clone()),
        ..task_core::TaskRouting::default()
    });
    store.insert(&task).expect("insert");
    store
        .append_event(
            task.id,
            &Event::ExecutionGated {
                decision: Box::new(task.routing.as_ref().unwrap().execution.clone().unwrap()),
            },
        )
        .expect("append gate");
    store
        .append_event(
            task.id,
            &Event::ExecutionRouted {
                decision: Box::new(decision.clone()),
            },
        )
        .expect("append route");

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    let execution = detail.execution.expect("execution present");
    assert_eq!(execution.route, Some(decision));
}

/// ADR-0124: `Event::ExecutionRouted` が無い（評価していない）Task でも、gate の活動があれば
/// Execution 節は出るが `route` は `None`（直行の判定を記録していないことが見える）。
#[test]
fn task_detail_execution_direct_route_is_absent_without_the_event() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut task = sample_task(TaskKind::Execute, Status::Running);
    task.routing = Some(task_core::TaskRouting {
        execution: Some(task_core::ExecutionGateDecision {
            mode: task_core::ExecutionMode::Compound,
            source: task_core::GateSource::Policy,
            score: 9,
            threshold: 5,
            rule_id: "compound/score".to_string(),
            signals: vec![],
            policy_version: task_core::EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow: false,
            depth: None,
        }),
        ..task_core::TaskRouting::default()
    });
    store.insert(&task).expect("insert");
    store
        .append_event(
            task.id,
            &Event::ExecutionGated {
                decision: Box::new(task.routing.as_ref().unwrap().execution.clone().unwrap()),
            },
        )
        .expect("append gate");

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    let execution = detail.execution.expect("execution present");
    assert!(execution.route.is_none());
}

/// 計画のある Task: WU の表・現在の段階（`running` かつ WU が `running` なら `executing`）・
/// done の件数を確かめる。
#[test]
fn task_detail_execution_reports_the_plan_and_work_unit_table() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(TaskKind::Execute, Status::Running);
    store.insert(&task).expect("insert");
    crate::execution::adopt_plan(
        &store,
        task.id,
        plan_spec(vec![
            wu_spec("survey", task_core::WorkUnitKind::Investigate, &[]),
            wu_spec("build", task_core::WorkUnitKind::Implement, &["survey"]),
        ]),
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("adopt_plan");
    set_status(&store, task.id, "survey", task_core::WorkUnitStatus::Done);
    set_status(&store, task.id, "build", task_core::WorkUnitStatus::Running);

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    let execution = detail.execution.expect("execution present");
    assert_eq!(execution.phase, Some(ExecutionPhase::Executing));
    let plan = execution.plan.expect("plan present");
    assert_eq!(plan.work_units.len(), 2);
    assert_eq!(plan.work_units[0].key, "survey");
    assert_eq!(plan.work_units[0].status, task_core::WorkUnitStatus::Done);
    assert_eq!(plan.work_units[1].key, "build");
    assert_eq!(
        plan.work_units[1].status,
        task_core::WorkUnitStatus::Running
    );
    assert_eq!(execution.metrics.work_units_total, 2);
    assert_eq!(execution.metrics.work_units_done, 1);
    assert_eq!(plan.versions.len(), 1);
}

/// replan の後、版の履歴（`versions`）に旧版・新版が理由付きで並ぶ（D17(f)）。
#[test]
fn task_detail_execution_reports_replan_version_history() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");
    crate::execution::adopt_plan(
        &store,
        task.id,
        plan_spec(vec![wu_spec("a", task_core::WorkUnitKind::Implement, &[])]),
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("adopt_plan");
    crate::execution::replan(
        &store,
        task.id,
        plan_spec(vec![
            wu_spec("a", task_core::WorkUnitKind::Implement, &[]),
            wu_spec("b", task_core::WorkUnitKind::Implement, &["a"]),
        ]),
        "add a follow-up step".to_string(),
        task_core::PlanOrigin::Human,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("replan");

    let ctx = view_ctx();
    let detail = task_detail(&store, task.id, &ctx, OffsetDateTime::now_utc()).expect("detail");
    let plan = detail
        .execution
        .expect("execution present")
        .plan
        .expect("plan");
    assert_eq!(plan.version, 2);
    assert_eq!(plan.versions.len(), 2);
    assert_eq!(plan.versions[0].version, 1);
    assert_eq!(plan.versions[0].status, task_core::PlanStatus::Superseded);
    assert_eq!(plan.versions[1].version, 2);
    assert_eq!(plan.versions[1].status, task_core::PlanStatus::Active);
    // ADR-0074 D5.3（Phase F1）: reason の後ろに差分の件数（added/changed/removed）が付く。
    assert_eq!(
        plan.versions[1].reason.as_deref(),
        Some("add a follow-up step (added=1, changed=0, removed=0)")
    );
}

/// `reviewing` は計画の有無に関わらず常に `verifying`。
#[test]
fn execution_phase_reviewing_is_always_verifying() {
    let task = sample_task(TaskKind::Execute, Status::Reviewing);
    assert_eq!(execution_phase(&task, &[]), Some(ExecutionPhase::Verifying));
}
