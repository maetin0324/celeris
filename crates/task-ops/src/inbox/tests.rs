use super::*;
use std::cell::Cell;
use std::time::Duration as StdDuration;
use task_core::{
    Budget, Check, Criterion, DeliveryStore, SqliteStore, Task, TaskId, Tier, WorkspaceSpec,
};

fn view_ctx() -> ViewContext {
    ViewContext {
        workspace_root: std::path::PathBuf::from("/tmp/workspaces"),
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
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: task_core::WorkerHint {
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

fn no_evidence(_task: &Task, _run_id: &str) -> Vec<EvidenceView> {
    Vec::new()
}

#[test]
fn inbox_approvals_section_links_parent_run_and_calls_evidence_for_done_run() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut parent = sample_task(TaskKind::Execute, Status::Reviewing);
    parent.acceptance = vec![Criterion {
        text: "looks good".into(),
        check: Check::Human,
    }];
    store.insert(&parent).expect("insert parent");
    store
        .append_event(
            parent.id,
            &Event::WorkerStarted {
                run_id: "run-1".into(),
                adapter: "claude-code".into(),
                model: "m".into(),
                provider: Some("claude-a".into()),
                account: None,
                role: None,
                task_role: None,
            },
        )
        .expect("started");
    store
        .append_event(
            parent.id,
            &Event::WorkerFinished {
                run_id: "run-1".into(),
                outcome: "done: implemented".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("finished");
    store
        .append_event(
            parent.id,
            &Event::ReviewVerdict {
                run_id: "run-1".into(),
                criterion_idx: 0,
                pass: false,
                reason: "needs human sign-off".into(),
            },
        )
        .expect("verdict");

    let mut approval = sample_task(TaskKind::Approval, Status::Ready);
    approval.parent_id = Some(parent.id);
    approval.title = derive::human_approval_title(&parent, 0);
    store.insert(&approval).expect("insert approval");
    store
        .append_event(approval.id, &Event::ApprovalRequested)
        .expect("requested");

    let calls: Cell<u32> = Cell::new(0);
    let evidence_fn = |task: &Task, run_id: &str| {
        calls.set(calls.get() + 1);
        assert_eq!(task.id, parent.id);
        assert_eq!(run_id, "run-1");
        vec![EvidenceView {
            criterion: 0,
            command: Some("cargo test".into()),
            exit: Some(0),
            stdout_tail: Some("ok".into()),
        }]
    };

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &evidence_fn).expect("inbox");

    assert_eq!(result.approvals.len(), 1);
    let item = &result.approvals[0];
    assert_eq!(item.approval.id, approval.id);
    assert_eq!(item.parent.as_ref().map(|p| p.id), Some(parent.id));
    assert_eq!(item.criterion_idx, Some(0));
    assert_eq!(item.attempt, Some(1));
    assert_eq!(item.criterion_text, "looks good");
    assert_eq!(
        item.last_run.as_ref().map(|r| r.run_id.clone()),
        Some("run-1".to_string())
    );
    assert_eq!(item.other_verdicts.len(), 1);
    assert_eq!(item.artifacts.len(), 0);
    assert_eq!(item.evidence.len(), 1);
    assert_eq!(calls.get(), 1);
    assert_eq!(result.counts.approvals, 1);
}

#[test]
fn inbox_approvals_ordered_by_requested_at_and_includes_previous_decisions() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut parent = sample_task(TaskKind::Execute, Status::Reviewing);
    parent.acceptance = vec![Criterion {
        text: "looks good".into(),
        check: Check::Human,
    }];
    store.insert(&parent).expect("insert parent");

    // attempt 1: already decided (rejected).
    let mut attempt1 = sample_task(TaskKind::Approval, Status::Failed);
    attempt1.parent_id = Some(parent.id);
    attempt1.title = derive::human_approval_title(&parent, 0);
    store.insert(&attempt1).expect("insert attempt1");
    store
        .append_event(
            attempt1.id,
            &Event::ApprovalDecided {
                by: "human".into(),
                approved: false,
                note: Some("not yet".into()),
            },
        )
        .expect("decide attempt1");

    // attempt 2: pending, requested after attempt 1's decision.
    let mut attempt2_parent_snapshot = parent.clone();
    attempt2_parent_snapshot.attempts = 1;
    let mut attempt2 = sample_task(TaskKind::Approval, Status::Ready);
    attempt2.parent_id = Some(parent.id);
    attempt2.title = derive::human_approval_title(&attempt2_parent_snapshot, 0);
    store.insert(&attempt2).expect("insert attempt2");
    store
        .append_event(attempt2.id, &Event::ApprovalRequested)
        .expect("requested attempt2");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");

    assert_eq!(result.approvals.len(), 1, "only the ready approval appears");
    let pending = &result.approvals[0];
    assert_eq!(pending.approval.id, attempt2.id);
    assert_eq!(pending.previous_decisions.len(), 1);
    assert!(!pending.previous_decisions[0].approved);
    assert_eq!(
        pending.previous_decisions[0].note.as_deref(),
        Some("not yet")
    );
}

#[test]
fn inbox_questions_section_reports_question_asked_at_and_run_id() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&task).expect("insert task");
    store
        .append_event(
            task.id,
            &Event::WorkerFinished {
                run_id: "run-7".into(),
                outcome: "question: which version?".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("finished");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    assert_eq!(result.questions.len(), 1);
    let q = &result.questions[0];
    assert_eq!(q.task.id, task.id);
    assert_eq!(q.question, "which version?");
    assert_eq!(q.run_id.as_deref(), Some("run-7"));
    assert!(q.asked_at.is_some());
    assert_eq!(result.counts.questions, 1);
    assert_eq!(q.approval_id, None, "approvals の行がまだ無ければ null");
}

/// GUI 監査対応 Phase 29: 質問に対応する未決の `approvals` の id が付き、GUI が認可画面へ
/// 直接リンクできる。決定済みの approval は付かない（`pending = true` でしか引かないため）。
#[test]
fn inbox_questions_carry_the_id_of_their_pending_approval() {
    use task_core::approval::{Approval, ApprovalId, ApprovalStore};

    let store = SqliteStore::open_in_memory().expect("open store");
    let with_pending = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&with_pending).expect("insert");
    let pending = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: Some(with_pending.id),
        question: "どのクラスタを使いますか".into(),
        decision: None,
        answer: None,
        created_at: OffsetDateTime::now_utc(),
        decided_at: None,
    };
    store.approval_append(&pending).expect("append");

    // すでに決定済みの approval を持つ別のタスク（新しい質問はまだ来ていない想定）には付かない。
    let with_decided_only = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&with_decided_only).expect("insert");
    let decided = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "secretary".into(),
        task_id: Some(with_decided_only.id),
        question: "別の質問".into(),
        decision: Some(task_core::approval::Decision::Once),
        answer: Some("x".into()),
        created_at: OffsetDateTime::now_utc(),
        decided_at: Some(OffsetDateTime::now_utc()),
    };
    store.approval_append(&decided).expect("append");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    let find = |id: TaskId| {
        result
            .questions
            .iter()
            .find(|q| q.task.id == id)
            .expect("question")
    };
    assert_eq!(find(with_pending.id).approval_id, Some(pending.id));
    assert_eq!(find(with_decided_only.id).approval_id, None);
}

#[test]
fn inbox_drafts_grouped_by_parent_with_root_group_last() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let plan = sample_task(TaskKind::Plan, Status::Done);
    store.insert(&plan).expect("insert plan");
    store
        .append_event(
            plan.id,
            &Event::WorkerFinished {
                run_id: "run-1".into(),
                outcome: "done: built the plan".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("finished");

    let mut child = sample_task(TaskKind::Execute, Status::Draft);
    child.parent_id = Some(plan.id);
    store.insert(&child).expect("insert child");

    let root_draft = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&root_draft).expect("insert root draft");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");

    assert_eq!(result.drafts.len(), 2);
    assert_eq!(
        result.drafts[0].parent.as_ref().map(|p| p.id),
        Some(plan.id)
    );
    assert_eq!(
        result.drafts[0].plan_summary.as_deref(),
        Some("built the plan")
    );
    assert_eq!(result.drafts[0].drafts.len(), 1);
    assert_eq!(result.drafts[0].drafts[0].id, child.id);

    assert!(result.drafts[1].parent.is_none(), "root group must be last");
    assert_eq!(result.drafts[1].drafts.len(), 1);
    assert_eq!(result.drafts[1].drafts[0].id, root_draft.id);
    assert_eq!(result.counts.drafts, 2);
}

/// ADR-0074 D2.4（Phase F3 途中確認）: 工程の後で止まった Task は `attention` の `PhaseCheckpoint`
/// に出て、`questions` には出ない。操作は `phase_gate`（`answer` は出さない）。
#[test]
fn inbox_phase_checkpoint_is_attention_not_a_question() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&task).expect("insert");
    store
        .apply_transition(task.id, task_core::Trigger::Dispatch, None)
        .expect("dispatch");
    store
        .apply_transition_with_events(
            task.id,
            task_core::Trigger::PhaseGate {
                phase: "design".into(),
            },
            vec![
                Event::ArtifactProduced {
                    run_id: "daemon:phase-gate:design".into(),
                    artifact: ArtifactRef {
                        name: "1-design.md".into(),
                        path: "artifacts/phase-reports/1-design.md".into(),
                        sha256: "abc".into(),
                        kind: "md".into(),
                        declared: true,
                    },
                },
                Event::PhaseReported {
                    phase: "design".into(),
                    report: Box::new(task_core::PhaseReport {
                        phase: "design".into(),
                        phase_title: "設計".into(),
                        next_phase: Some("build".into()),
                        ..Default::default()
                    }),
                },
            ],
        )
        .expect("phase gate");

    let result = inbox(
        &store,
        None,
        &view_ctx(),
        OffsetDateTime::now_utc(),
        &no_evidence,
    )
    .expect("inbox");
    assert!(result.questions.is_empty(), "{:?}", result.questions);
    let item = result
        .attention
        .iter()
        .find_map(|a| match a {
            AttentionItem::PhaseCheckpoint {
                task: t,
                phase,
                phase_title,
                phases_done,
                report_idx,
                next_phase,
                ..
            } => Some((t, phase, phase_title, *phases_done, *report_idx, next_phase)),
            _ => None,
        })
        .expect("phase checkpoint item");
    assert_eq!(item.0.id, task.id);
    assert_eq!(item.1, "design");
    assert_eq!(item.2, "設計");
    assert_eq!(item.3, 1);
    assert_eq!(item.4, Some(0));
    assert_eq!(item.5.as_deref(), Some("build"));
    assert!(item.0.actions.contains(&view::Action::PhaseGate));
    assert!(!item.0.actions.contains(&view::Action::Answer));
}

#[test]
fn inbox_attention_includes_recent_failure_and_requeue_near_limit() {
    let store = SqliteStore::open_in_memory().expect("open store");

    let failed = sample_task(TaskKind::Execute, Status::Failed);
    store.insert(&failed).expect("insert failed");
    store
        .append_event(
            failed.id,
            &Event::WorkerFinished {
                run_id: "run-1".into(),
                outcome: "error(retryable=false): boom".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("finished");

    let mut near_limit = sample_task(TaskKind::Execute, Status::Ready);
    near_limit.attempts = 1;
    store.insert(&near_limit).expect("insert near_limit");
    for _ in 0..4 {
        store
            .append_event(
                near_limit.id,
                &Event::Transitioned {
                    from: Status::Running,
                    to: Status::Ready,
                    reason: "requeue".into(),
                },
            )
            .expect("requeue event");
    }

    let ctx = view_ctx(); // max_requeues = 5, so >= 4 triggers RequeueLimitNear.
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");

    assert!(
        result
            .attention
            .iter()
            .any(|a| matches!(a, AttentionItem::Failed { task, .. } if task.id == failed.id))
    );
    assert!(result.attention.iter().any(
        |a| matches!(a, AttentionItem::RequeueLimitNear { task, count, max, .. } if task.id == near_limit.id && *count == 4 && *max == 5)
    ));
    assert!(
        !result
            .attention
            .iter()
            .any(|a| matches!(a, AttentionItem::Unroutable { .. }))
    );
}

/// ADR-0070 D1（Phase 116。D6(a)）: `AttentionItem::Failed` の `class` が
/// infra / work を見分け、配送済みタスクは `delivered_release` を持ち、`review_fail` だけが
/// 原因のタスクだけ `rereview` 操作が付く。
#[test]
fn inbox_attention_failed_items_are_classified_and_carry_operations() {
    let store = SqliteStore::open_in_memory().expect("open store");

    // infra: `infra failure ×N` の接頭辞。
    let infra = sample_task(TaskKind::Execute, Status::Failed);
    store.insert(&infra).expect("insert infra");
    store
        .append_event(
            infra.id,
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

    // work: reviewer 条件を持ち、review_fail だけが原因（rereview が使えるはず）。
    let mut work = sample_task(TaskKind::Execute, Status::Failed);
    work.acceptance = vec![task_core::Criterion {
        text: "reviewer checks it".into(),
        check: Check::Reviewer,
    }];
    store.insert(&work).expect("insert work");
    store
        .append_event(
            work.id,
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
            work.id,
            &Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Failed,
                reason: "review_fail".into(),
            },
        )
        .expect("transitioned");

    // delivered: 配送済みなのに failed。
    let delivered = sample_task(TaskKind::Execute, Status::Failed);
    store.insert(&delivered).expect("insert delivered");
    store
        .append_event(
            delivered.id,
            &Event::WorkerFinished {
                run_id: "run-1".into(),
                outcome: "error(retryable=false): cargo test failed".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .expect("finished");
    store
        .delivery_save(
            None,
            &task_core::Delivery {
                task_id: delivered.id,
                project_id: task_core::ProjectId::new(),
                repo_id: task_core::RepoId::new(),
                repo: "agent-platform".into(),
                branch: "celeris/x".into(),
                base: "main".into(),
                head: "abc123".into(),
                default_branch: "main".into(),
                department: "engineering".into(),
                review_run: "rev-1".into(),
                worker_run: "run-1".into(),
                criterion_idx: 0,
                decision: None,
                state: task_core::DeliveryState::Ready,
                detail: String::new(),
                release: Some("51d24a61c2ba".into()),
                prepare_pid: None,
                notification: None,
                pushed_at: None,
                push_error: None,
            },
        )
        .expect("delivery save");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");

    let find = |id: TaskId| {
        result
            .attention
            .iter()
            .find_map(|a| match a {
                AttentionItem::Failed {
                    task,
                    class,
                    delivered_release,
                    ..
                } if task.id == id => Some((task.clone(), *class, delivered_release.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no failed attention item for {id}"))
    };

    let (infra_task, infra_class, infra_release) = find(infra.id);
    assert_eq!(infra_class, derive::FailureClass::Infra);
    assert!(infra_release.is_none());
    assert!(infra_task.actions.contains(&view::Action::Retry));
    assert!(!infra_task.actions.contains(&view::Action::Rereview));

    let (work_task, work_class, _) = find(work.id);
    assert_eq!(work_class, derive::FailureClass::Work);
    assert!(work_task.actions.contains(&view::Action::Rereview));

    let (_, _, delivered_release) = find(delivered.id);
    assert_eq!(delivered_release.as_deref(), Some("51d24a61c2ba"));
}

#[test]
fn inbox_attention_unroutable_only_populated_with_snapshot() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let stuck = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&stuck).expect("insert stuck");

    let ctx = view_ctx();
    let without_snapshot =
        inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    assert!(
        !without_snapshot
            .attention
            .iter()
            .any(|a| matches!(a, AttentionItem::Unroutable { .. }))
    );

    let snapshot = DaemonSnapshot {
        instance_id: "01J000000000000000000000AA".into(),
        pid: 1,
        hostname: "host".into(),
        started_at: view::to_rfc3339(OffsetDateTime::now_utc()),
        last_tick_at: view::to_rfc3339(OffsetDateTime::now_utc()),
        ticks: 1,
        tick_ms: 2000,
        in_flight: vec![],
        cooldowns: vec![],
        awaiting_human: vec![],
        awaiting_children: vec![],
        unroutable: vec![stuck.id],
        reports: None,
        approvals_pending: 0,
        decisions_open: 0,
        clusters: vec![],
        providers: vec![],
        accounts_root: None,
        accounts_roots: std::collections::HashMap::new(),
        max_runs_per_account: None,
        accounts: vec![],
        containers: None,
        scratch: None,
    };
    let with_snapshot = inbox(
        &store,
        Some(&snapshot),
        &ctx,
        OffsetDateTime::now_utc(),
        &no_evidence,
    )
    .expect("inbox");
    assert!(
        with_snapshot
            .attention
            .iter()
            .any(|a| matches!(a, AttentionItem::Unroutable { task, .. } if task.id == stuck.id))
    );
}

#[test]
fn inbox_counts_match_section_lengths_and_status_totals() {
    let store = SqliteStore::open_in_memory().expect("open store");
    store
        .insert(&sample_task(TaskKind::Execute, Status::Draft))
        .expect("insert");
    store
        .insert(&sample_task(TaskKind::Execute, Status::Ready))
        .expect("insert");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    assert_eq!(result.counts.approvals, result.approvals.len() as u32);
    assert_eq!(result.counts.questions, result.questions.len() as u32);
    assert_eq!(
        result.counts.drafts,
        result
            .drafts
            .iter()
            .map(|g| g.drafts.len() as u32)
            .sum::<u32>()
    );
    assert_eq!(result.counts.attention, result.attention.len() as u32);
    assert_eq!(result.counts.by_status.get("draft").copied(), Some(1));
    assert_eq!(result.counts.by_status.get("ready").copied(), Some(1));
}

/// Phase 9 監査: `counts.drafts` は draft タスクの件数（グループ数ではない）。draft の子の件数は一覧と同じ規則で 1 回だけ数える。
#[test]
fn inbox_draft_count_is_tasks_not_groups_and_child_counts_are_filled() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let parent = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&parent).expect("insert parent");
    for status in [Status::Draft, Status::Done] {
        let mut child = sample_task(TaskKind::Execute, status);
        child.parent_id = Some(parent.id);
        store.insert(&child).expect("insert child");
    }
    store
        .insert(&sample_task(TaskKind::Execute, Status::Draft))
        .expect("insert other root");

    let result = inbox(
        &store,
        None,
        &view_ctx(),
        OffsetDateTime::now_utc(),
        &no_evidence,
    )
    .expect("inbox");
    assert_eq!(result.drafts.len(), 2, "root group + the parent's group");
    assert_eq!(result.counts.drafts, 3);
    let root_group = result
        .drafts
        .iter()
        .find(|g| g.parent.is_none())
        .expect("root group");
    let summary = root_group
        .drafts
        .iter()
        .find(|s| s.id == parent.id)
        .expect("parent summary");
    assert_eq!((summary.children, summary.pending_children), (2, 1));
}

/// Phase 9 監査: 一度も requeue していない ready タスクは、`max_requeues = 1` でも `requeue_limit_near` にならない。
#[test]
fn inbox_requeue_limit_near_ignores_tasks_that_never_requeued() {
    let store = SqliteStore::open_in_memory().expect("open store");
    store
        .insert(&sample_task(TaskKind::Execute, Status::Ready))
        .expect("insert fresh");
    let requeued = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&requeued).expect("insert requeued");
    store
        .append_event(
            requeued.id,
            &Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "requeue".into(),
            },
        )
        .expect("requeue event");

    let ctx = ViewContext {
        max_requeues: 1,
        ..view_ctx()
    };
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    let near: Vec<(TaskId, u32)> = result
        .attention
        .iter()
        .filter_map(|a| match a {
            AttentionItem::RequeueLimitNear { task, count, .. } => Some((task.id, *count)),
            _ => None,
        })
        .collect();
    assert_eq!(near, vec![(requeued.id, 1)]);
}

fn remote_task(status: Status, cluster: &str) -> Task {
    let mut t = sample_task(TaskKind::Execute, status);
    t.workspace = WorkspaceSpec::Remote {
        cluster: cluster.to_string(),
        path: "workspace".into(),
        mode: None,
    };
    t
}

fn cluster_unavailable_find<'a>(
    items: &'a [AttentionItem],
    cluster: &str,
) -> Option<(&'a str, &'a str, u32)> {
    items.iter().find_map(|a| match a {
        AttentionItem::ClusterUnavailable {
            cluster: c,
            host,
            at,
            tasks,
        } if c == cluster => Some((host.as_str(), at.as_str(), *tasks)),
        _ => None,
    })
}

/// ADR-0018 D2 / 受け入れ条件 9: `Remote` タスク 2 件で `ClusterUnavailable` が起きたら、クラスタ 1 件にまとまる。
/// `Local` タスクの同イベントは対象外。
#[test]
fn inbox_attention_cluster_unavailable_groups_remote_tasks_by_cluster() {
    let store = SqliteStore::open_in_memory().expect("open store");

    let remote1 = remote_task(Status::Ready, "pegasus");
    store.insert(&remote1).expect("insert remote1");
    store
        .append_event(
            remote1.id,
            &Event::ClusterUnavailable {
                cluster: "pegasus".into(),
                host: "pegasus".into(),
                reason: "no multiplexed connection".into(),
            },
        )
        .expect("cluster unavailable 1");

    let remote2 = remote_task(Status::Ready, "pegasus");
    store.insert(&remote2).expect("insert remote2");
    store
        .append_event(
            remote2.id,
            &Event::ClusterUnavailable {
                cluster: "pegasus".into(),
                host: "pegasus".into(),
                reason: "no multiplexed connection".into(),
            },
        )
        .expect("cluster unavailable 2");

    let local = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&local).expect("insert local");
    store
        .append_event(
            local.id,
            &Event::ClusterUnavailable {
                cluster: "pegasus".into(),
                host: "pegasus".into(),
                reason: "no multiplexed connection".into(),
            },
        )
        .expect("cluster unavailable local");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    let (host, at, tasks) =
        cluster_unavailable_find(&result.attention, "pegasus").expect("cluster item present");
    assert_eq!(host, "pegasus");
    assert_eq!(
        tasks, 2,
        "only the remote tasks count, the local one does not"
    );
    assert!(!at.is_empty());
    assert_eq!(result.counts.attention, result.attention.len() as u32);
}

/// 24h の窓の外（`now` を +25h にする）になったら消える。
#[test]
fn inbox_attention_cluster_unavailable_drops_outside_24h_window() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let remote = remote_task(Status::Ready, "sirius");
    store.insert(&remote).expect("insert remote");
    store
        .append_event(
            remote.id,
            &Event::ClusterUnavailable {
                cluster: "sirius".into(),
                host: "sirius".into(),
                reason: "no multiplexed connection".into(),
            },
        )
        .expect("cluster unavailable");

    let ctx = view_ctx();
    let now = OffsetDateTime::now_utc();
    let fresh = inbox(&store, None, &ctx, now, &no_evidence).expect("inbox");
    assert!(cluster_unavailable_find(&fresh.attention, "sirius").is_some());

    let later = now + time::Duration::hours(25);
    let expired = inbox(&store, None, &ctx, later, &no_evidence).expect("inbox");
    assert!(cluster_unavailable_find(&expired.attention, "sirius").is_none());
}

/// スナップショットの `clusters[].connected == true` なら「ログインし直してください」の呼びかけは用済みなので消える。
/// `connected == false` なら残る。第 1 段階の行（`host: ""`）はスナップショットの `host` で補われる。
#[test]
fn inbox_attention_cluster_unavailable_hidden_once_reconnected_and_host_filled_from_snapshot() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let remote = remote_task(Status::Ready, "pegasus");
    store.insert(&remote).expect("insert remote");
    store
        .append_event(
            remote.id,
            &Event::ClusterUnavailable {
                cluster: "pegasus".into(),
                host: String::new(),
                reason: "no multiplexed connection".into(),
            },
        )
        .expect("cluster unavailable");

    let ctx = view_ctx();
    let now = OffsetDateTime::now_utc();

    let disconnected_snapshot = DaemonSnapshot {
        instance_id: "01J000000000000000000000AA".into(),
        pid: 1,
        hostname: "host".into(),
        started_at: view::to_rfc3339(now),
        last_tick_at: view::to_rfc3339(now),
        ticks: 1,
        tick_ms: 2000,
        in_flight: vec![],
        cooldowns: vec![],
        awaiting_human: vec![],
        awaiting_children: vec![],
        unroutable: vec![],
        reports: None,
        approvals_pending: 0,
        decisions_open: 0,
        clusters: vec![crate::daemon::ClusterLive {
            id: "pegasus".into(),
            host: "pegasus".into(),
            concurrency: 1,
            in_use: 0,
            connected: false,
            cooldown_until: None,
            auth: "manual".into(),
            connect_pending: false,
            tunnel_login_needed: false,
            connection_stats: Default::default(),
            tunnel_forwards: vec![],
        }],
        providers: vec![],
        accounts_root: None,
        accounts_roots: std::collections::HashMap::new(),
        max_runs_per_account: None,
        accounts: vec![],
        containers: None,
        scratch: None,
    };
    let still_present = inbox(
        &store,
        Some(&disconnected_snapshot),
        &ctx,
        now,
        &no_evidence,
    )
    .expect("inbox");
    let (host, _, _) = cluster_unavailable_find(&still_present.attention, "pegasus")
        .expect("item present while disconnected");
    assert_eq!(
        host, "pegasus",
        "host filled from the snapshot's cluster entry"
    );

    let connected_snapshot = DaemonSnapshot {
        clusters: vec![crate::daemon::ClusterLive {
            id: "pegasus".into(),
            host: "pegasus".into(),
            concurrency: 1,
            in_use: 0,
            connected: true,
            cooldown_until: None,
            auth: "manual".into(),
            connect_pending: false,
            tunnel_login_needed: false,
            connection_stats: Default::default(),
            tunnel_forwards: vec![],
        }],
        ..disconnected_snapshot
    };
    let hidden = inbox(&store, Some(&connected_snapshot), &ctx, now, &no_evidence).expect("inbox");
    assert!(cluster_unavailable_find(&hidden.attention, "pegasus").is_none());
}

/// ADR-0121 D3: 完了した root の `DeliverySkipped` は（24 時間より古くても）attention に出る。木の子は出さず、
/// 同じ head の delivery が後で作られたら消える。
#[test]
fn inbox_attention_shows_delivery_skip_reason_for_done_root() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let skipped = |head: Option<&str>| Event::DeliverySkipped {
        reason: task_core::DeliverySkipReason::DepartmentUnresolved,
        detail: "assignee なし".into(),
        head: head.map(str::to_string),
    };
    let mut root = sample_task(TaskKind::Execute, Status::Done);
    root.project_id = Some(task_core::ProjectId::new());
    root.updated_at = OffsetDateTime::now_utc() - time::Duration::days(3);
    store.insert(&root).expect("insert root");
    store
        .append_event(root.id, &skipped(Some("abc123")))
        .expect("skip event");

    let mut child = sample_task(TaskKind::Execute, Status::Done);
    child.project_id = root.project_id;
    child.tree = Some(task_core::TreeInfo {
        root_id: root.id,
        depth: 2,
        parent_unit: Some(task_core::ParentUnit {
            task_id: root.id,
            plan_id: "plan".into(),
            unit_key: "c".into(),
            stage: "s".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    store.insert(&child).expect("insert child");
    store
        .append_event(child.id, &skipped(None))
        .expect("child skip event");

    let ctx = view_ctx();
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    let items: Vec<_> = result
        .attention
        .iter()
        .filter_map(|a| match a {
            AttentionItem::DeliverySkipped {
                task,
                reason,
                summary,
                detail,
                head,
                ..
            } => Some((
                task.id,
                *reason,
                summary.clone(),
                detail.clone(),
                head.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].0, root.id);
    assert_eq!(
        items[0].1,
        task_core::DeliverySkipReason::DepartmentUnresolved
    );
    assert!(
        items[0]
            .2
            .contains("main への取り込みを開始できませんでした")
    );
    assert_eq!(items[0].3, "assignee なし");
    assert_eq!(items[0].4.as_deref(), Some("abc123"));
    let json = serde_json::to_value(&result.attention).expect("json");
    assert!(json.to_string().contains("\"type\":\"delivery_skipped\""));
    assert!(
        json.to_string()
            .contains("\"reason\":\"department_unresolved\"")
    );

    store
        .delivery_save(
            None,
            &task_core::Delivery {
                task_id: root.id,
                project_id: task_core::ProjectId::new(),
                repo_id: task_core::RepoId::new(),
                repo: "agent-platform".into(),
                branch: "celeris/x".into(),
                base: "main".into(),
                head: "abc123".into(),
                default_branch: "main".into(),
                department: "engineering".into(),
                review_run: "rev-1".into(),
                worker_run: "run-1".into(),
                criterion_idx: 0,
                decision: None,
                state: task_core::DeliveryState::Reviewing,
                detail: String::new(),
                release: None,
                prepare_pid: None,
                notification: None,
                pushed_at: None,
                push_error: None,
            },
        )
        .expect("delivery save");
    let result = inbox(&store, None, &ctx, OffsetDateTime::now_utc(), &no_evidence).expect("inbox");
    assert!(
        !result
            .attention
            .iter()
            .any(|a| matches!(a, AttentionItem::DeliverySkipped { .. })),
        "a delivery for the same head clears the item"
    );
}
