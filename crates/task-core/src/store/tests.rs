use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Barrier};

use crate::comment::{CommentAuthorKind, TaskComment};
use crate::execution_plan::{
    ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus, RunRow, WorkUnitKind, WorkUnitRow,
    WorkUnitSpec, WorkUnitStatus,
};
use crate::integrations::{IntegrationMethod, IntegrationState, TaskIntegration};
use crate::message::{Message, MessageId, MessageRole};
use crate::model::{ArtifactRef, Budget, Check, Criterion, Tier, WorkerHint, WorkspaceSpec};
use crate::org::{OrgError, OrgKind, OrgNode, Project, ProjectId, ProjectStatus};
use crate::repos::{ProjectRepo, RepoId, RepoRun};

use super::*;

pub(super) fn sample_task(status: Status) -> Task {
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
        kind: TaskKind::Execute,
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
            path: "/tmp/workspace".into(),
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

#[test]
fn insert_then_get_roundtrips() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Draft);
    store.insert(&task).expect("insert");

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched, task);
}

#[test]
fn list_filters_by_status() {
    let store = SqliteStore::open_in_memory().expect("open");
    let ready = sample_task(Status::Ready);
    let draft = sample_task(Status::Draft);
    store.insert(&ready).expect("insert ready");
    store.insert(&draft).expect("insert draft");

    let ready_list = store.list(Some(Status::Ready)).expect("list ready");
    assert_eq!(ready_list.len(), 1);
    assert_eq!(ready_list[0].id, ready.id);

    let all = store.list(None).expect("list all");
    assert_eq!(all.len(), 2);
}

#[test]
fn append_event_reads_back_in_seq_order() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Draft);
    store.insert(&task).expect("insert");

    let e0 = Event::ApprovalRequested;
    let e1 = Event::Transitioned {
        from: Status::Draft,
        to: Status::Ready,
        reason: "ready to go".to_string(),
    };

    let seq0 = store.append_event(task.id, &e0).expect("append e0");
    let seq1 = store.append_event(task.id, &e1).expect("append e1");
    assert_eq!(seq0, 0);
    assert_eq!(seq1, 1);

    let events = store.events_for(task.id).expect("events_for");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], (0, e0));
    assert_eq!(events[1], (1, e1));
}

#[test]
fn acquire_lease_is_exclusive_under_concurrency() {
    let store = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let task = sample_task(Status::Ready);
    store.insert(&task).expect("insert");

    let barrier = Arc::new(Barrier::new(2));

    let store_a = Arc::clone(&store);
    let barrier_a = Arc::clone(&barrier);
    let task_id = task.id;
    let handle_a = std::thread::spawn(move || {
        barrier_a.wait();
        store_a.acquire_lease(task_id, "worker-a", StdDuration::from_secs(60))
    });

    let store_b = Arc::clone(&store);
    let barrier_b = Arc::clone(&barrier);
    let handle_b = std::thread::spawn(move || {
        barrier_b.wait();
        store_b.acquire_lease(task_id, "worker-b", StdDuration::from_secs(60))
    });

    let result_a = handle_a.join().expect("join a").expect("acquire a");
    let result_b = handle_b.join().expect("join b").expect("acquire b");

    assert_ne!(result_a, result_b, "exactly one acquisition must succeed");
    assert!(result_a || result_b);
}

#[test]
fn release_lease_then_reacquire_succeeds() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Ready);
    store.insert(&task).expect("insert");

    let acquired = store
        .acquire_lease(task.id, "worker-a", StdDuration::from_secs(60))
        .expect("acquire");
    assert!(acquired);

    // status is now Running, so a second acquire must fail.
    let second = store
        .acquire_lease(task.id, "worker-b", StdDuration::from_secs(60))
        .expect("acquire second");
    assert!(!second);

    store.release_lease(task.id, "worker-a").expect("release");

    let after_release = store.get(task.id).expect("get").expect("some");
    assert!(after_release.lease.is_none());

    // release_lease intentionally does not change status (still Running,
    // per its contract): that is the dispatcher's job via transition().
    // To verify "release then reacquire succeeds" on the very same task,
    // we simulate that dispatcher step here with a direct status update
    // (test-only; this module owns `conn` so it may reach in directly).
    {
        let conn = store.conn.lock().expect("lock");
        let mut readied = after_release.clone();
        readied.status = Status::Ready;
        let json = serde_json::to_string(&readied).expect("serialize");
        conn.execute(
            "UPDATE tasks SET status = 'ready', json = ?1 WHERE id = ?2",
            params![json, task.id.to_string()],
        )
        .expect("reset to ready");
    }

    let reacquired = store
        .acquire_lease(task.id, "worker-c", StdDuration::from_secs(60))
        .expect("acquire third");
    assert!(reacquired);
}

#[test]
fn ready_tasks_excludes_incomplete_dependencies_and_pending_approval_parent() {
    let store = SqliteStore::open_in_memory().expect("open");

    // Case 1: dependency not done -> excluded.
    let dep = sample_task(Status::Running);
    store.insert(&dep).expect("insert dep");
    let mut blocked_by_dep = sample_task(Status::Ready);
    blocked_by_dep.depends_on = vec![dep.id];
    store.insert(&blocked_by_dep).expect("insert blocked");

    // Case 2: dependency done -> included.
    let done_dep = sample_task(Status::Done);
    store.insert(&done_dep).expect("insert done dep");
    let mut unblocked = sample_task(Status::Ready);
    unblocked.depends_on = vec![done_dep.id];
    store.insert(&unblocked).expect("insert unblocked");

    // Case 3: parent is Approval and not Done -> excluded.
    let mut approval_parent = sample_task(Status::Reviewing);
    approval_parent.kind = TaskKind::Approval;
    store.insert(&approval_parent).expect("insert approval");
    let mut child_of_pending_approval = sample_task(Status::Ready);
    child_of_pending_approval.parent_id = Some(approval_parent.id);
    store
        .insert(&child_of_pending_approval)
        .expect("insert child pending");

    let ready = store.ready_tasks(10).expect("ready_tasks");
    let ready_ids: Vec<TaskId> = ready.iter().map(|t| t.id).collect();

    assert!(!ready_ids.contains(&blocked_by_dep.id));
    assert!(ready_ids.contains(&unblocked.id));
    assert!(!ready_ids.contains(&child_of_pending_approval.id));
}

/// ADR-0002 D8: `Dispatch`（ready -> running）は `kind == Approval` では
/// 無効。`Approval` タスクは `acquire_lease` によって running に入っては
/// ならない（DESIGN §4.2）。
#[test]
fn acquire_lease_rejects_approval_kind_even_when_ready() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut approval = sample_task(Status::Ready);
    approval.kind = TaskKind::Approval;
    store.insert(&approval).expect("insert approval");

    let acquired = store
        .acquire_lease(approval.id, "worker-a", StdDuration::from_secs(60))
        .expect("acquire_lease call");
    assert!(!acquired);

    let fetched = store.get(approval.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Ready);
    assert!(fetched.lease.is_none());
}

/// ADR-0002 D2: 成功した遷移は `Event::Transitioned` を同一トランザクションで
/// 追記する。`acquire_lease` が実質的に駆動する ready->running 遷移でも
/// この不変条件が成り立つことを確認する。
#[test]
fn acquire_lease_appends_transitioned_event_atomically() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Ready);
    store.insert(&task).expect("insert");

    let acquired = store
        .acquire_lease(task.id, "worker-a", StdDuration::from_secs(60))
        .expect("acquire");
    assert!(acquired);

    let events = store.events_for(task.id).expect("events_for");
    assert_eq!(events.len(), 1);
    match &events[0].1 {
        Event::Transitioned { from, to, reason } => {
            assert_eq!(*from, Status::Ready);
            assert_eq!(*to, Status::Running);
            assert_eq!(reason, "dispatch");
        }
        other => panic!("expected Transitioned event, got {other:?}"),
    }

    // 失敗した acquire_lease（既にリース済み）はイベントを追加しない。
    let second = store
        .acquire_lease(task.id, "worker-b", StdDuration::from_secs(60))
        .expect("second acquire call");
    assert!(!second);
    let events_after = store.events_for(task.id).expect("events_for after");
    assert_eq!(events_after.len(), 1);
}

/// ADR-0004 D1: `apply_transition` は draft -> ready (Accept) を適用し、
/// tasks の status と Event::Transitioned を同一トランザクションで反映する。
#[test]
fn apply_transition_accept_moves_draft_to_ready_and_appends_event() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Draft);
    store.insert(&task).expect("insert");

    let outcome = store
        .apply_transition(task.id, Trigger::Accept, None)
        .expect("apply_transition");
    assert_eq!(outcome.next, Status::Ready);

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Ready);

    let events = store.events_for(task.id).expect("events_for");
    assert_eq!(events.len(), 1);
    match &events[0].1 {
        Event::Transitioned { from, to, reason } => {
            assert_eq!(*from, Status::Draft);
            assert_eq!(*to, Status::Ready);
            assert_eq!(reason, "accept");
        }
        other => panic!("expected Transitioned event, got {other:?}"),
    }
}

/// ADR-0004 D1: 無効な遷移は `StoreError::InvalidTransition` を返し、
/// タスクの状態もイベントログも変更しない。
#[test]
fn apply_transition_rejects_invalid_trigger_without_side_effects() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Done);
    store.insert(&task).expect("insert");

    let result = store.apply_transition(task.id, Trigger::Accept, None);
    assert!(matches!(result, Err(StoreError::InvalidTransition(_))));

    let fetched = store.get(task.id).expect("get").expect("some");
    assert_eq!(fetched.status, Status::Done);
    assert!(store.events_for(task.id).expect("events_for").is_empty());
}

/// ADR-0004 D2: `Approve`/`Reject` は `extra_event` として
/// `Event::ApprovalDecided` を同一トランザクションで追記できる。
#[test]
fn apply_transition_appends_extra_event_atomically() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut approval = sample_task(Status::Ready);
    approval.kind = TaskKind::Approval;
    store.insert(&approval).expect("insert");

    let extra = Event::ApprovalDecided {
        by: "human".to_string(),
        approved: true,
        note: None,
    };
    let outcome = store
        .apply_transition(approval.id, Trigger::Approve, Some(extra.clone()))
        .expect("apply_transition");
    assert_eq!(outcome.next, Status::Done);

    let events = store.events_for(approval.id).expect("events_for");
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0].1, Event::Transitioned { .. }));
    assert_eq!(events[1].1, extra);
}

/// ADR-0008 D1: `Approval` を reject すると、まだ終端でない直接の子だけが cancelled になる。
/// 既に done/failed/cancelled の子や、他タスクの子は触らない。
#[test]
fn reject_cascades_cancel_to_non_terminal_direct_children_only() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut approval = sample_task(Status::Ready);
    approval.kind = TaskKind::Approval;
    store.insert(&approval).expect("insert approval");

    let mut pending = sample_task(Status::Draft);
    pending.parent_id = Some(approval.id);
    store.insert(&pending).expect("insert pending child");

    let mut running = sample_task(Status::Running);
    running.parent_id = Some(approval.id);
    running.lease = Some(crate::model::Lease {
        worker_run_id: "run-x".into(),
        expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(60),
    });
    store.insert(&running).expect("insert running child");

    let mut already_done = sample_task(Status::Done);
    already_done.parent_id = Some(approval.id);
    store.insert(&already_done).expect("insert done child");

    let unrelated = sample_task(Status::Draft);
    store.insert(&unrelated).expect("insert unrelated");

    let outcome = store
        .apply_transition(
            approval.id,
            Trigger::Reject,
            Some(Event::ApprovalDecided {
                by: "human".to_string(),
                approved: false,
                note: None,
            }),
        )
        .expect("apply_transition reject");
    assert_eq!(outcome.next, Status::Failed);

    let pending_after = store.get(pending.id).expect("get").expect("some");
    assert_eq!(pending_after.status, Status::Cancelled);
    assert!(
        store
            .events_for(pending.id)
            .expect("events_for")
            .iter()
            .any(|(_, e)| matches!(e, Event::Transitioned { to: Status::Cancelled, reason, .. } if reason == "cancel"))
    );

    let running_after = store.get(running.id).expect("get").expect("some");
    assert_eq!(running_after.status, Status::Cancelled);
    assert!(
        running_after.lease.is_none(),
        "leaving running must release the lease"
    );

    // 既に done の子は触らない。
    assert_eq!(
        store
            .get(already_done.id)
            .expect("get")
            .expect("some")
            .status,
        Status::Done
    );
    // 他タスクの子（parent_id が違う）も触らない。
    assert_eq!(
        store.get(unrelated.id).expect("get").expect("some").status,
        Status::Draft
    );
}

/// apply_transition が存在しない task_id に対して呼ばれた場合の扱い。
#[test]
fn apply_transition_missing_task_returns_invalid_error() {
    let store = SqliteStore::open_in_memory().expect("open");
    let result = store.apply_transition(TaskId::new(), Trigger::Accept, None);
    assert!(matches!(result, Err(StoreError::Invalid(_))));
}

/// ADR-0002 D1: running から出る全遷移でリースを解放する。`apply_transition` が
/// 駆動する遷移（ここでは WorkerDone）でも `lease` が None になり、
/// `lease_worker_run_id`/`lease_expires_at` 列も NULL になることを確認する。
#[test]
fn apply_transition_releases_lease_when_leaving_running() {
    let store = SqliteStore::open_in_memory().expect("open");
    let task = sample_task(Status::Ready);
    store.insert(&task).expect("insert");
    store
        .acquire_lease(task.id, "worker-a", StdDuration::from_secs(60))
        .expect("acquire_lease")
        .then_some(())
        .expect("lease should be acquired");

    let running = store.get(task.id).expect("get").expect("some");
    assert!(running.lease.is_some());

    store
        .apply_transition(task.id, Trigger::WorkerDone, None)
        .expect("apply_transition");

    let after = store.get(task.id).expect("get").expect("some");
    assert_eq!(after.status, Status::Reviewing);
    assert!(after.lease.is_none());

    let (lease_worker_run_id, lease_expires_at): (Option<String>, Option<String>) = {
        let conn = store.conn.lock().expect("lock");
        conn.query_row(
            "SELECT lease_worker_run_id, lease_expires_at FROM tasks WHERE id = ?1",
            params![task.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query lease columns")
    };
    assert!(lease_worker_run_id.is_none());
    assert!(lease_expires_at.is_none());
}

#[test]
fn apply_transition_with_events_appends_transitioned_then_extras_in_order() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Ready);
    store.insert(&task).unwrap();
    assert!(
        store
            .acquire_lease(task.id, "run-1", StdDuration::from_secs(60))
            .unwrap()
    );
    let extras = vec![
        Event::WorkerFinished {
            run_id: "run-1".into(),
            outcome: "done: x".into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        },
        Event::worker_progress("run-1", "extra"),
    ];
    let outcome = store
        .apply_transition_with_events(task.id, Trigger::WorkerDone, extras)
        .unwrap();
    assert_eq!(outcome.next, Status::Reviewing);
    let events = store.events_for(task.id).unwrap();
    let tail: Vec<(u64, String)> = events[events.len() - 3..]
        .iter()
        .map(|(seq, e)| {
            (
                *seq,
                match e {
                    Event::Transitioned { to, reason, .. } => {
                        format!("transitioned:{to:?}:{reason}")
                    }
                    Event::WorkerFinished { outcome, .. } => format!("finished:{outcome}"),
                    Event::WorkerProgress { msg, .. } => format!("progress:{msg}"),
                    _ => "other".into(),
                },
            )
        })
        .collect();
    assert_eq!(
        tail,
        vec![
            (1, "transitioned:Reviewing:worker_done".to_string()),
            (2, "finished:done: x".to_string()),
            (3, "progress:extra".to_string()),
        ]
    );
    assert!(store.get(task.id).unwrap().unwrap().lease.is_none());
    // 無効な遷移では何も追記されない。
    let before = store.events_for(task.id).unwrap().len();
    assert!(
        store
            .apply_transition_with_events(
                task.id,
                Trigger::WorkerDone,
                vec![Event::ApprovalRequested]
            )
            .is_err()
    );
    assert_eq!(store.events_for(task.id).unwrap().len(), before);
}

/// ADR-0007 D3: 子の挿入と親の ReviewPass が同一トランザクションで、accept_children に応じて
/// 子が draft / ready になる。親の遷移が無効なら子も挿入されない。
#[test]
fn complete_plan_inserts_children_and_passes_parent_atomically() {
    use crate::model::TaskKind;
    let store = SqliteStore::open_in_memory().expect("open");
    let mut plan = sample_task(Status::Reviewing);
    plan.kind = TaskKind::Plan;
    store.insert(&plan).expect("insert plan");
    let mut c1 = sample_task(Status::Draft);
    c1.parent_id = Some(plan.id);
    let mut c2 = sample_task(Status::Draft);
    c2.parent_id = Some(plan.id);
    c2.depends_on = vec![c1.id];
    let verdict = Event::ReviewVerdict {
        run_id: "r".into(),
        criterion_idx: 0,
        pass: true,
        reason: "plan ok".into(),
    };
    let outcome = store
        .complete_plan(plan.id, vec![verdict], vec![c1.clone(), c2.clone()], false)
        .expect("complete_plan");
    assert_eq!(outcome.next, Status::Done);
    assert_eq!(store.get(plan.id).unwrap().unwrap().status, Status::Done);
    let ev: Vec<Event> = store
        .events_for(plan.id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(
        matches!(&ev[0], Event::Transitioned { to: Status::Done, reason, .. } if reason == "review_pass")
    );
    assert!(matches!(&ev[1], Event::ReviewVerdict { pass: true, .. }));
    for c in [&c1, &c2] {
        let got = store.get(c.id).unwrap().expect("child inserted");
        assert_eq!(got.status, Status::Draft);
        let ev = store.events_for(c.id).unwrap();
        assert_eq!(ev.len(), 1);
        assert!(matches!(&ev[0].1, Event::Created { .. }));
    }
    // draft の子は ready_tasks に出ない。
    assert!(store.ready_tasks(10).unwrap().is_empty());

    // accept_children = true: 子は ready、Created + Transitioned(accept)。
    let mut plan2 = sample_task(Status::Reviewing);
    plan2.kind = TaskKind::Plan;
    store.insert(&plan2).expect("insert plan2");
    let mut c3 = sample_task(Status::Draft);
    c3.parent_id = Some(plan2.id);
    store
        .complete_plan(plan2.id, vec![], vec![c3.clone()], true)
        .expect("complete_plan 2");
    let got = store.get(c3.id).unwrap().unwrap();
    assert_eq!(got.status, Status::Ready);
    let ev = store.events_for(c3.id).unwrap();
    assert_eq!(ev.len(), 2);
    assert!(
        matches!(&ev[1].1, Event::Transitioned { from: Status::Draft, to: Status::Ready, reason } if reason == "accept")
    );
    assert_eq!(store.ready_tasks(10).unwrap().len(), 1);

    // 親が reviewing でなければ全体がロールバックされ、子は挿入されない。
    let mut not_reviewing = sample_task(Status::Ready);
    not_reviewing.kind = TaskKind::Plan;
    store.insert(&not_reviewing).unwrap();
    let mut c4 = sample_task(Status::Draft);
    c4.parent_id = Some(not_reviewing.id);
    let err = store
        .complete_plan(not_reviewing.id, vec![], vec![c4.clone()], true)
        .unwrap_err();
    assert!(matches!(err, StoreError::InvalidTransition(_)));
    assert!(store.get(c4.id).unwrap().is_none());
    assert!(store.events_for(c4.id).unwrap().is_empty());

    // 親子関係が違う子は拒否。
    let stranger = sample_task(Status::Draft);
    let mut plan3 = sample_task(Status::Reviewing);
    plan3.kind = TaskKind::Plan;
    store.insert(&plan3).unwrap();
    assert!(matches!(
        store.complete_plan(plan3.id, vec![], vec![stranger], false),
        Err(StoreError::Invalid(_))
    ));
}

fn reasons(store: &SqliteStore, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::Transitioned { reason, .. } => Some(reason),
            _ => None,
        })
        .collect()
}

/// ADR-0010 D2: create_task は insert + Created + extra を 1 トランザクションで行い、失敗時は何も残さない。
/// ADR-0016 M2: 委譲された子は Created → Accept で ready になり、親に Delegated が残る。全て 1 トランザクション。
#[test]
fn delegate_children_inserts_ready_children_and_records_delegated_on_the_parent() {
    let store = SqliteStore::open_in_memory().unwrap();
    let parent = sample_task(Status::Running);
    store.insert(&parent).unwrap();
    let mut a = sample_task(Status::Draft);
    a.parent_id = Some(parent.id);
    let mut b = sample_task(Status::Draft);
    b.parent_id = Some(parent.id);
    b.depends_on = vec![a.id];
    let ids = store
        .delegate_children(parent.id, "run-1", vec![a.clone(), b.clone()])
        .unwrap();
    assert_eq!(ids, vec![a.id, b.id]);
    for id in &ids {
        let t = store.get(*id).unwrap().unwrap();
        assert_eq!(t.status, Status::Ready);
        assert_eq!(reasons(&store, *id), vec!["accept"]);
    }
    let children = store.children(parent.id).unwrap();
    assert_eq!(children.iter().map(|t| t.id).collect::<Vec<_>>(), ids);
    let events = store.events_for(parent.id).unwrap();
    assert!(
        matches!(&events.last().unwrap().1, Event::Delegated { run_id, task_ids } if run_id == "run-1" && *task_ids == ids)
    );
    assert_eq!(
        store.get(parent.id).unwrap().unwrap().status,
        Status::Running
    );
    // 親が違う子は拒否され、何も挿入されない。
    let mut stray = sample_task(Status::Draft);
    stray.parent_id = Some(a.id);
    assert!(
        store
            .delegate_children(parent.id, "run-2", vec![stray.clone()])
            .is_err()
    );
    assert!(store.get(stray.id).unwrap().is_none());
    assert_eq!(store.children(parent.id).unwrap().len(), 2);
    // ready_tasks は a を返し、b は a が done になるまで返さない。
    let ready = store.ready_tasks(10).unwrap();
    assert_eq!(ready.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a.id]);
}

#[test]
fn create_task_inserts_task_and_events_atomically() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Ready);
    store
        .create_task(&task, vec![Event::ApprovalRequested])
        .unwrap();
    assert_eq!(store.get(task.id).unwrap().unwrap(), task);
    let ev = store.events_for(task.id).unwrap();
    assert_eq!(ev.len(), 2);
    assert!(matches!(&ev[0].1, Event::Created { task: t, .. } if t.id == task.id));
    assert_eq!(ev[1].1, Event::ApprovalRequested);
    // 同じ id の再作成は insert で失敗し、イベントも追記されない。
    assert!(
        store
            .create_task(&task, vec![Event::ApprovalRequested])
            .is_err()
    );
    assert_eq!(store.events_for(task.id).unwrap().len(), 2);
}

/// ADR-0010 D7（P-7）: renew_lease は running かつ run_id が一致するときだけ期限を更新する。
#[test]
fn renew_lease_extends_only_the_matching_running_lease() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Ready);
    store.insert(&task).unwrap();
    assert!(
        store
            .acquire_lease(task.id, "run-a", StdDuration::from_secs(5))
            .unwrap()
    );
    let before = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .unwrap()
        .expires_at;
    assert!(
        store
            .renew_lease(task.id, "run-a", StdDuration::from_secs(3600))
            .unwrap()
    );
    let after = store.get(task.id).unwrap().unwrap().lease.unwrap();
    assert_eq!(after.worker_run_id, "run-a");
    assert!(after.expires_at > before + time::Duration::seconds(3000));
    let col: String = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT lease_expires_at FROM tasks WHERE id = ?1",
            params![task.id.to_string()],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(col, format_rfc3339(after.expires_at).unwrap());
    // 別 run_id や running でないタスクには効かない。
    assert!(
        !store
            .renew_lease(task.id, "run-b", StdDuration::from_secs(1))
            .unwrap()
    );
    store
        .apply_transition(task.id, Trigger::WorkerDone, None)
        .unwrap();
    assert!(
        !store
            .renew_lease(task.id, "run-a", StdDuration::from_secs(1))
            .unwrap()
    );
    assert!(
        !store
            .renew_lease(TaskId::new(), "run-a", StdDuration::from_secs(1))
            .unwrap()
    );
    // 状態遷移ではないのでイベントは増えない（dispatch と worker_done の 2 件だけ）。
    assert_eq!(reasons(&store, task.id), vec!["dispatch", "worker_done"]);
}

/// ADR-0010 D2（P-36）: ready_tasks は Approval を返さない。
#[test]
fn ready_tasks_excludes_approval_kind() {
    let store = SqliteStore::open_in_memory().unwrap();
    for _ in 0..3 {
        let mut a = sample_task(Status::Ready);
        a.kind = TaskKind::Approval;
        store.insert(&a).unwrap();
    }
    let exec = sample_task(Status::Ready);
    store.insert(&exec).unwrap();
    let ready = store.ready_tasks(1).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, exec.id);
}

/// ADR-0010 D1（P-4）: 終端タスクへの Cancel は無効で、状態もイベントも変わらない。
#[test]
fn cancel_is_invalid_for_terminal_tasks() {
    let store = SqliteStore::open_in_memory().unwrap();
    for status in [Status::Done, Status::Failed, Status::Cancelled] {
        let t = sample_task(status);
        store.insert(&t).unwrap();
        assert!(matches!(
            store.apply_transition(t.id, Trigger::Cancel, None),
            Err(StoreError::InvalidTransition(_))
        ));
        assert_eq!(store.get(t.id).unwrap().unwrap().status, status);
        assert!(store.events_for(t.id).unwrap().is_empty());
    }
}

/// ADR-0010 D2: Approval が cancel された場合も（reject と同じく）終端でない直接の子が cancelled になる。
#[test]
fn cancelling_an_approval_cascades_to_its_children() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut approval = sample_task(Status::Ready);
    approval.kind = TaskKind::Approval;
    store.insert(&approval).unwrap();
    let mut child = sample_task(Status::Ready);
    child.parent_id = Some(approval.id);
    store.insert(&child).unwrap();
    store
        .apply_transition(approval.id, Trigger::Cancel, None)
        .unwrap();
    assert_eq!(
        store.get(child.id).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert_eq!(reasons(&store, child.id), vec!["cancel"]);
}

/// ADR-0010 D2（P-37）: Approval 以外のタスクが終端になると、未決の Approval 子だけが cancelled になる。
#[test]
fn terminal_task_cancels_its_pending_approval_children_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    let parent = sample_task(Status::Reviewing);
    store.insert(&parent).unwrap();
    let mut pending_approval = sample_task(Status::Ready);
    pending_approval.kind = TaskKind::Approval;
    pending_approval.parent_id = Some(parent.id);
    store.insert(&pending_approval).unwrap();
    let mut decided_approval = sample_task(Status::Done);
    decided_approval.kind = TaskKind::Approval;
    decided_approval.parent_id = Some(parent.id);
    store.insert(&decided_approval).unwrap();
    let mut exec_child = sample_task(Status::Draft);
    exec_child.parent_id = Some(parent.id);
    store.insert(&exec_child).unwrap();

    store
        .apply_transition(parent.id, Trigger::ReviewPass, None)
        .unwrap();
    assert_eq!(
        store.get(pending_approval.id).unwrap().unwrap().status,
        Status::Cancelled
    );
    assert_eq!(
        store.get(decided_approval.id).unwrap().unwrap().status,
        Status::Done
    );
    assert_eq!(
        store.get(exec_child.id).unwrap().unwrap().status,
        Status::Draft
    );
}

/// ADR-0010 D2（P-9）: 先行タスクが failed になると、終端でない後続が推移的に cancelled（dependency_failed）になる。
#[test]
fn dependency_failure_cancels_dependents_transitively() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut a = sample_task(Status::Ready);
    a.budget.max_retries = 0;
    store.insert(&a).unwrap();
    let mut b = sample_task(Status::Draft);
    b.depends_on = vec![a.id];
    store.insert(&b).unwrap();
    let mut c = sample_task(Status::Ready);
    c.depends_on = vec![b.id];
    store.insert(&c).unwrap();
    let mut d = sample_task(Status::Done);
    d.depends_on = vec![a.id];
    store.insert(&d).unwrap();
    let unrelated = sample_task(Status::Ready);
    store.insert(&unrelated).unwrap();

    assert!(
        store
            .acquire_lease(a.id, "run-a", StdDuration::from_secs(60))
            .unwrap()
    );
    let outcome = store
        .apply_transition(a.id, Trigger::WorkerError { retryable: false }, None)
        .unwrap();
    assert_eq!(outcome.next, Status::Failed);

    for id in [b.id, c.id] {
        let t = store.get(id).unwrap().unwrap();
        assert_eq!(t.status, Status::Cancelled);
        assert_eq!(t.attempts, 0);
        assert_eq!(reasons(&store, id), vec!["dependency_failed"]);
    }
    assert_eq!(store.get(d.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        store.get(unrelated.id).unwrap().unwrap().status,
        Status::Ready
    );
}

/// ADR-0012 D1: `WorkerStarted.provider` は任意。導入前に記録されたイベント（provider 無し）も読める。
#[test]
fn worker_started_without_provider_still_deserializes() {
    let old = r#"{"type":"worker_started","run_id":"r","adapter":"fake","model":"m"}"#;
    let ev: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        ev,
        Event::WorkerStarted {
            run_id: "r".into(),
            adapter: "fake".into(),
            model: "m".into(),
            provider: None,
            account: None,
            role: None,
            task_role: None,
        }
    );
    let new = Event::WorkerStarted {
        run_id: "r".into(),
        adapter: "fake".into(),
        model: "m".into(),
        provider: Some("acct-a".into()),
        account: None,
        role: None,
        task_role: None,
    };
    assert!(
        serde_json::to_string(&new)
            .unwrap()
            .contains(r#""provider":"acct-a""#)
    );
    assert_eq!(serde_json::to_string(&ev).unwrap(), old);
}

/// ADR-0024 D4: `WorkerStarted.account` は任意。導入前のイベント（account 無し）も読め、
/// 新しいイベントは `account` を含めて往復する。
#[test]
fn worker_started_account_round_trips_and_old_events_still_deserialize() {
    let old = r#"{"type":"worker_started","run_id":"r","adapter":"claude-code","model":"m","provider":"claude-pool"}"#;
    let ev: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        ev,
        Event::WorkerStarted {
            run_id: "r".into(),
            adapter: "claude-code".into(),
            model: "m".into(),
            provider: Some("claude-pool".into()),
            account: None,
            role: None,
            task_role: None,
        }
    );
    assert_eq!(serde_json::to_string(&ev).unwrap(), old);

    let with_account = Event::WorkerStarted {
        run_id: "r2".into(),
        adapter: "claude-code".into(),
        model: "m".into(),
        provider: Some("claude-pool".into()),
        account: Some("acct-a".into()),
        role: None,
        task_role: None,
    };
    let json = serde_json::to_string(&with_account).unwrap();
    assert!(json.contains(r#""account":"acct-a""#), "{json}");
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), with_account);
}

/// ADR-0014 D1: `role` は任意。無ければワーカー run で、既存のイベントの直列化は変わらない。Reviewer run は `"role":"reviewer"`。
#[test]
fn run_events_role_is_optional_and_reviewer_serializes_explicitly() {
    let old = r#"{"type":"worker_finished","run_id":"r","outcome":"done: x","usage":null}"#;
    let ev: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        ev,
        Event::WorkerFinished {
            run_id: "r".into(),
            outcome: "done: x".into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        }
    );
    assert_eq!(serde_json::to_string(&ev).unwrap(), old);
    let reviewer = Event::WorkerStarted {
        run_id: "rv".into(),
        adapter: "fake".into(),
        model: "m".into(),
        provider: Some("acct-a".into()),
        account: None,
        role: Some(crate::RunRole::Reviewer),
        task_role: None,
    };
    let json = serde_json::to_string(&reviewer).unwrap();
    assert!(json.contains(r#""role":"reviewer""#), "{json}");
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), reviewer);
}

// ---- ADR-0013 D5/D6/D9/D10: schema migrations, PRAGMA, events の global id, list_page ----

/// 版数 1 の DB（`schema_migrations` が無く `tasks`/`events` だけがある）を素の `Connection` で
/// 作る。`0001_init.sql` だけを適用し、`insert_tx`/`append_event_tx` を経由しない生の SQL で
/// タスクと events を書く。
fn insert_legacy_task(conn: &Connection, task: &Task) {
    let json = serde_json::to_string(task).unwrap();
    let created_at = format_rfc3339(task.created_at).unwrap();
    let (lease_worker_run_id, lease_expires_at): (Option<String>, Option<String>) =
        match &task.lease {
            Some(l) => (
                Some(l.worker_run_id.clone()),
                Some(format_rfc3339(l.expires_at).unwrap()),
            ),
            None => (None, None),
        };
    conn.execute(
        "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, \
         lease_worker_run_id, lease_expires_at, json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            task.id.to_string(),
            status_str(task.status),
            kind_str(task.kind),
            task.parent_id.map(|p| p.to_string()),
            task.priority,
            created_at,
            lease_worker_run_id,
            lease_expires_at,
            json,
        ],
    )
    .unwrap();
}

fn insert_legacy_event(conn: &Connection, task_id: TaskId, seq: u64, event: &Event) {
    let ts = format_rfc3339(OffsetDateTime::now_utc()).unwrap();
    let json = serde_json::to_string(event).unwrap();
    conn.execute(
        "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
        params![task_id.to_string(), seq, ts, json],
    )
    .unwrap();
}

/// ADR-0013 D5/D6: 版数 1 の DB を `open` すると版数 3 に上がり、`events` の rowid 順が
/// `events_since` の id 順に保たれ、`events_for` は移行前と同一の結果を返し、`title`/
/// `updated_at` 列が埋まる。2 回目の `open` は何も再適用しない（`schema_migrations` の
/// `applied_at` が変わらない）。
#[test]
fn open_migrates_legacy_v1_db_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite3");

    let a = sample_task(Status::Draft);
    let b = sample_task(Status::Ready);
    let mut expected_a: Vec<(u64, Event)> = Vec::new();
    let mut expected_b: Vec<(u64, Event)> = Vec::new();
    let mut expected_global: Vec<(TaskId, u64)> = Vec::new();

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATION_0001).unwrap();
        insert_legacy_task(&conn, &a);
        insert_legacy_task(&conn, &b);

        // events は task をまたいで rowid 順をばらして挿入する（各 task 内の seq 順は保つ）。
        let ev = Event::Created {
            task: Box::new(a.clone()),
            origin: None,
        };
        insert_legacy_event(&conn, a.id, 0, &ev);
        expected_a.push((0, ev));
        expected_global.push((a.id, 0));

        let ev = Event::Created {
            task: Box::new(b.clone()),
            origin: None,
        };
        insert_legacy_event(&conn, b.id, 0, &ev);
        expected_b.push((0, ev));
        expected_global.push((b.id, 0));

        let ev = Event::Transitioned {
            from: Status::Draft,
            to: Status::Ready,
            reason: "accept".into(),
        };
        insert_legacy_event(&conn, a.id, 1, &ev);
        expected_a.push((1, ev));
        expected_global.push((a.id, 1));

        let ev = Event::ApprovalRequested;
        insert_legacy_event(&conn, b.id, 1, &ev);
        expected_b.push((1, ev));
        expected_global.push((b.id, 1));

        let ev = Event::worker_progress("r", "go");
        insert_legacy_event(&conn, a.id, 2, &ev);
        expected_a.push((2, ev));
        expected_global.push((a.id, 2));
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);

    let since = store.events_since(0, 100).unwrap();
    assert_eq!(since.len(), 5);
    let got_order: Vec<(TaskId, u64)> = since.iter().map(|r| (r.task_id, r.seq)).collect();
    assert_eq!(
        got_order, expected_global,
        "events_since id order must match original rowid order"
    );
    assert!(since.windows(2).all(|w| w[0].id < w[1].id));

    assert_eq!(store.events_for(a.id).unwrap(), expected_a);
    assert_eq!(store.events_for(b.id).unwrap(), expected_b);

    let (title_a, updated_at_a, objective_a): (String, String, String) = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT title, updated_at, objective FROM tasks WHERE id = ?1",
            params![a.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    };
    assert_eq!(title_a, a.title);
    // ADR-0014 D2: マイグレーション 0004 が既存行の objective を json から埋める。
    assert_eq!(objective_a, a.objective);
    assert_eq!(updated_at_a, format_rfc3339(a.updated_at).unwrap());

    let applied_before: Vec<(i64, String)> = {
        let conn = store.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT version, applied_at FROM schema_migrations ORDER BY version")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    drop(store);

    let store2 = SqliteStore::open(&path).unwrap();
    assert_eq!(store2.schema_version().unwrap(), SCHEMA_VERSION);
    let applied_after: Vec<(i64, String)> = {
        let conn = store2.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT version, applied_at FROM schema_migrations ORDER BY version")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(
        applied_before, applied_after,
        "second open must not re-apply migrations"
    );
}

/// ADR-0027 D1: 版数 4 の DB（`genre` 列が無い）を `open` すると版数 5 に上がり、
/// 既存行は `genre` 列が `NULL`（= `Task::genre == None`）のまま読める。0003/0004 の
/// マイグレーションテストと同じ形（生の SQL で旧スキーマを作ってから `open` する）。
#[test]
fn open_migrates_schema_4_db_and_old_rows_read_back_with_genre_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema4.sqlite3");
    let task = sample_task(Status::Draft);
    assert!(task.genre.is_none());

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATION_0001).unwrap();
        conn.execute_batch(MIGRATION_0002).unwrap();
        conn.execute_batch(MIGRATION_0003).unwrap();
        conn.execute_batch(MIGRATION_0004).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES \
             (1, '2020-01-01T00:00:00Z'), (2, '2020-01-01T00:00:00Z'), \
             (3, '2020-01-01T00:00:00Z'), (4, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
        // 版数 4 の列（genre はまだ無い）で直接挿入する。
        let json = serde_json::to_string(&task).unwrap();
        let created_at = format_rfc3339(task.created_at).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, \
             lease_worker_run_id, lease_expires_at, json, title, updated_at, objective) \
             VALUES (?1,?2,?3,?4,?5,?6,NULL,NULL,?7,?8,?9,?10)",
            params![
                task.id.to_string(),
                status_str(task.status),
                kind_str(task.kind),
                task.parent_id.map(|p| p.to_string()),
                task.priority,
                created_at,
                json,
                task.title,
                created_at,
                task.objective,
            ],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);

    let got = store
        .get(task.id)
        .unwrap()
        .expect("task still readable after migration");
    assert_eq!(got.genre, None);
    assert_eq!(got.objective, task.objective);

    let genre_col: Option<String> = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT genre FROM tasks WHERE id = ?1",
            params![task.id.to_string()],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        genre_col, None,
        "migration must not invent a genre for pre-existing rows"
    );
}

/// ADR-0013 D5: `schema_migrations` の最大版数がこのバイナリの `SCHEMA_VERSION` より大きい DB は
/// `StoreError::SchemaTooNew` で開けない。
#[test]
fn open_rejects_db_with_schema_version_newer_than_supported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("toonew.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES (99, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
    }
    let result = SqliteStore::open(&path);
    assert!(matches!(
        result,
        Err(StoreError::SchemaTooNew { found: 99, supported }) if supported == SCHEMA_VERSION
    ));
}

/// ADR-0059 D6（Phase 99）: `cluster_settings` の get/list/set。`work_dir = None` で行を削除する
/// （上書きを消す）。
/// ADR-0078 D5: 接続の記録は追記され、`since` より前のものは返らない（古い順）。
#[test]
fn cluster_connection_log_records_and_lists_since() {
    let store = SqliteStore::open_in_memory().unwrap();
    let t0 = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
    let rec = |kind: &str, at: OffsetDateTime| ClusterConnectionRecord {
        cluster_id: "pegasus".into(),
        kind: kind.into(),
        method: (kind == "connected").then(|| "totp".to_string()),
        cause: (kind == "lost").then(|| "check_failed".to_string()),
        uptime_secs: (kind == "lost").then_some(42),
        at,
    };
    store
        .cluster_connection_record(&rec("connected", t0))
        .unwrap();
    store
        .cluster_connection_record(&rec("lost", t0 + time::Duration::milliseconds(1500)))
        .unwrap();
    store
        .cluster_connection_record(&rec("connected", t0 + time::Duration::hours(30)))
        .unwrap();
    let all = store.cluster_connection_list_since(t0).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[1].kind, "lost");
    assert_eq!(all[1].uptime_secs, Some(42));
    assert_eq!(all[1].cause.as_deref(), Some("check_failed"));
    let recent = store
        .cluster_connection_list_since(t0 + time::Duration::hours(1))
        .unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].method.as_deref(), Some("totp"));
    let stats = ClusterConnectionStats::from_records(&all, "pegasus");
    assert_eq!(stats.connects_totp, 2);
    assert_eq!(stats.losses, 1);
    assert_eq!(stats.losses_by_cause.get("check_failed"), Some(&1));
    assert_eq!(stats.last_lost_cause.as_deref(), Some("check_failed"));
    assert!(stats.last_lost_at.is_some());
    assert_eq!(
        ClusterConnectionStats::from_records(&all, "sirius"),
        ClusterConnectionStats::default()
    );
}

#[test]
fn cluster_settings_get_list_and_set_including_clearing_the_override() {
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    assert_eq!(store.cluster_settings_get("pegasus").unwrap(), None);
    assert_eq!(store.cluster_settings_list().unwrap(), vec![]);

    store
        .cluster_settings_set("pegasus", Some("/work/NBB/rmaeda"), now)
        .unwrap();
    let got = store.cluster_settings_get("pegasus").unwrap().unwrap();
    assert_eq!(got.cluster_id, "pegasus");
    assert_eq!(got.work_dir.as_deref(), Some("/work/NBB/rmaeda"));

    // upsert（同じ id を上書き）。
    store
        .cluster_settings_set("pegasus", Some("/work/NBB/other"), now)
        .unwrap();
    assert_eq!(
        store
            .cluster_settings_get("pegasus")
            .unwrap()
            .unwrap()
            .work_dir
            .as_deref(),
        Some("/work/NBB/other")
    );

    store
        .cluster_settings_set("sirius", Some("~/work"), now)
        .unwrap();
    let mut list = store.cluster_settings_list().unwrap();
    list.sort_by(|a, b| a.cluster_id.cmp(&b.cluster_id));
    assert_eq!(
        list.iter()
            .map(|c| (c.cluster_id.as_str(), c.work_dir.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("pegasus", Some("/work/NBB/other")),
            ("sirius", Some("~/work"))
        ]
    );

    // `None` で消す。
    store.cluster_settings_set("pegasus", None, now).unwrap();
    assert_eq!(store.cluster_settings_get("pegasus").unwrap(), None);
    assert_eq!(store.cluster_settings_list().unwrap().len(), 1);
}

/// ADR-0013 D5: ファイル DB では `PRAGMA journal_mode` が `wal` になる。
#[test]
fn open_sets_wal_journal_mode_for_file_backed_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wal.sqlite3");
    let store = SqliteStore::open(&path).unwrap();
    let mode: String = {
        let conn = store.conn.lock().unwrap();
        conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(mode.to_lowercase(), "wal");
}

/// ADR-0064 D5: `background_checkpoint` は `wal_autocheckpoint` を 0 にする（既定は非 0）。
#[test]
fn background_checkpoint_option_disables_wal_autocheckpoint() {
    let dir = tempfile::tempdir().unwrap();

    let default_path = dir.path().join("default.sqlite3");
    let default_store = SqliteStore::open(&default_path).unwrap();
    let default_autocheckpoint: i64 = {
        let conn = default_store.conn.lock().unwrap();
        conn.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
            .unwrap()
    };
    assert_ne!(
        default_autocheckpoint, 0,
        "the default keeps sqlite's own autocheckpoint"
    );

    let bg_path = dir.path().join("bg.sqlite3");
    let bg_store = SqliteStore::open_with(
        &bg_path,
        StoreOptions {
            background_checkpoint: true,
            ..StoreOptions::default()
        },
    )
    .unwrap();
    let (bg_autocheckpoint, journal_size_limit): (i64, i64) = {
        let conn = bg_store.conn.lock().unwrap();
        (
            conn.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
                .unwrap(),
            conn.query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
                .unwrap(),
        )
    };
    assert_eq!(bg_autocheckpoint, 0);
    assert_eq!(journal_size_limit, BACKGROUND_CHECKPOINT_JOURNAL_SIZE_LIMIT);
}

/// ADR-0064 D2/D3: `backup_database` は別ファイルへ完全なコピーを作り、`integrity_check` はそれを
/// `true` と判定する。元と壊れた DB（空ファイル）は区別できる。
#[test]
fn backup_database_round_trips_and_integrity_check_detects_a_good_copy() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = dir.path().join("src.sqlite3");
    let store = SqliteStore::open(&src_path).unwrap();
    let t = sample_task(Status::Ready);
    store.insert(&t).unwrap();
    drop(store);

    let dest_path = dir.path().join("backup.sqlite3");
    backup_database(&src_path, &dest_path, StdDuration::from_millis(5000)).unwrap();

    assert!(integrity_check(&dest_path).unwrap());
    let restored = SqliteStore::open(&dest_path).unwrap();
    assert_eq!(restored.get(t.id).unwrap().map(|task| task.id), Some(t.id));

    // ゴミバイト列は正しい SQLite DB ではないので `integrity_check` はエラーを返す
    // （`SQLITE_NOTADB`。「壊れている」を「開けない」として区別できることの確認）。
    let garbage_path = dir.path().join("garbage.sqlite3");
    std::fs::write(&garbage_path, b"not a sqlite database").unwrap();
    assert!(integrity_check(&garbage_path).is_err());
}

/// ADR-0064 D4: `read_pool_size = 0` なら読み取りプールを作らず、書き込み接続にフォールバック
/// する（挙動は変わらない）。
#[test]
fn read_pool_size_zero_falls_back_to_the_write_connection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("no_pool.sqlite3");
    let store = SqliteStore::open_with(
        &path,
        StoreOptions {
            read_pool_size: 0,
            ..StoreOptions::default()
        },
    )
    .unwrap();
    assert!(store.read_pool.is_none());
    let t = sample_task(Status::Ready);
    store.insert(&t).unwrap();
    assert_eq!(store.get(t.id).unwrap().map(|task| task.id), Some(t.id));
}

/// ADR-0070 D5（Phase 116）: `SQLITE_BUSY`/`SQLITE_LOCKED` だけを拾い、他のエラー（`NOTADB` 等）は
/// 拾わない。
#[test]
fn is_busy_error_matches_only_busy_and_locked() {
    let busy = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseBusy,
            extended_code: 5,
        },
        Some("database is locked".to_string()),
    ));
    assert!(is_busy_error(&busy));

    let locked = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseLocked,
            extended_code: 6,
        },
        None,
    ));
    assert!(is_busy_error(&locked));

    let not_a_db = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::NotADatabase,
            extended_code: 26,
        },
        None,
    ));
    assert!(!is_busy_error(&not_a_db));
    assert!(!is_busy_error(&StoreError::Invalid("x".into())));
}

/// Phase 9 監査（受け入れ 2）: 2 つの接続（ディスパッチャと celerisctl / API 相当）が、読んでから書くトランザクションを
/// 同じファイルに並走させても `database is locked` にならない。DEFERRED だと読み取り後の書き込みへの格上げが
/// busy_timeout を待たずに SQLITE_BUSY で失敗するため、書き込みトランザクションは IMMEDIATE で始める。
#[test]
fn concurrent_read_then_write_transactions_on_two_connections_wait_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("writers.sqlite3");
    drop(SqliteStore::open(&path).unwrap());

    let writer = |store: Arc<SqliteStore>| {
        std::thread::spawn(move || -> Result<(), StoreError> {
            for _ in 0..200 {
                let t = sample_task(Status::Draft);
                store.create_task(&t, vec![])?;
                store.apply_transition(t.id, crate::Trigger::Accept, None)?;
                store.append_event(t.id, &Event::ApprovalRequested)?;
            }
            Ok(())
        })
    };
    let a = Arc::new(SqliteStore::open(&path).unwrap());
    let b = Arc::new(SqliteStore::open(&path).unwrap());
    let (ha, hb) = (writer(Arc::clone(&a)), writer(Arc::clone(&b)));
    ha.join()
        .unwrap()
        .expect("writer a never sees database is locked");
    hb.join()
        .unwrap()
        .expect("writer b never sees database is locked");
    assert_eq!(a.list(Some(Status::Ready)).unwrap().len(), 400);
    assert_eq!(b.events_since(0, 10_000).unwrap().len(), 400 * 3);
}

/// ADR-0013 D5: 同じファイルを開いた 2 つの `SqliteStore` が、一方の書き込み中でももう一方から
/// 読める（WAL + busy_timeout）。
#[test]
fn two_connections_read_and_write_the_same_file_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.sqlite3");
    // 先に一度開いてファイルとスキーマを作っておく。
    drop(SqliteStore::open(&path).unwrap());

    let store_a = Arc::new(SqliteStore::open(&path).unwrap());
    let store_b = Arc::new(SqliteStore::open(&path).unwrap());

    let writer = {
        let store_a = Arc::clone(&store_a);
        std::thread::spawn(move || {
            for _ in 0..20 {
                let t = sample_task(Status::Draft);
                store_a.insert(&t).unwrap();
            }
        })
    };
    let reader = {
        let store_b = Arc::clone(&store_b);
        std::thread::spawn(move || {
            for _ in 0..20 {
                store_b.list(None).unwrap();
            }
        })
    };
    writer.join().unwrap();
    reader.join().unwrap();

    assert_eq!(store_a.list(None).unwrap().len(), 20);
}

/// ADR-0064 D4: 読み取り専用の接続プールがあるおかげで、読み取り（`get`）は書き込み接続の
/// `Mutex` を長く保持している間も待たされない（同じ `SqliteStore`、同じファイル）。
#[test]
fn reads_do_not_wait_for_a_held_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("read_pool.sqlite3");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let t = sample_task(Status::Ready);
    store.insert(&t).unwrap();

    const HOLD: std::time::Duration = std::time::Duration::from_millis(400);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let holder = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            let guard = store.lock().expect("lock");
            ready_tx.send(()).expect("signal held");
            std::thread::sleep(HOLD);
            drop(guard);
        })
    };
    ready_rx.recv().expect("writer took the lock");

    let started = std::time::Instant::now();
    let found = store.get(t.id).unwrap();
    let elapsed = started.elapsed();
    holder.join().unwrap();

    assert!(found.is_some());
    assert!(
        elapsed < HOLD / 2,
        "get() took {elapsed:?} while the write lock was held for {HOLD:?}; \
         the read pool should have kept it from waiting"
    );
}

/// ADR-0013 D6: `events_since` は全タスクを跨いで id 昇順、`limit`、`after_id` を尊重し、
/// `latest_event_id` は現在の最大 id（無ければ 0）を返す。
#[test]
fn event_rows_for_returns_one_tasks_rows_with_ids_after_seq() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = sample_task(Status::Draft);
    let b = sample_task(Status::Draft);
    store.insert(&a).unwrap();
    store.insert(&b).unwrap();
    store.append_event(a.id, &Event::ApprovalRequested).unwrap();
    store.append_event(b.id, &Event::ApprovalRequested).unwrap();
    store.append_event(a.id, &Event::ApprovalRequested).unwrap();
    let rows = store.event_rows_for(a.id, None, 10).unwrap();
    assert_eq!(rows.iter().map(|r| r.seq).collect::<Vec<_>>(), vec![0, 1]);
    assert!(rows[0].id < rows[1].id);
    assert!(rows.iter().all(|r| r.task_id == a.id && !r.ts.is_empty()));
    assert_eq!(store.event_rows_for(a.id, Some(0), 10).unwrap().len(), 1);
    assert_eq!(store.event_rows_for(a.id, None, 1).unwrap().len(), 1);
    assert!(
        store
            .event_rows_for(TaskId::new(), None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn events_since_orders_globally_and_respects_after_id_and_limit() {
    let store = SqliteStore::open_in_memory().unwrap();
    let a = sample_task(Status::Draft);
    let b = sample_task(Status::Draft);
    store.insert(&a).unwrap();
    store.insert(&b).unwrap();
    assert_eq!(store.latest_event_id().unwrap(), 0);

    store.append_event(a.id, &Event::ApprovalRequested).unwrap();
    store.append_event(b.id, &Event::ApprovalRequested).unwrap();
    store
        .append_event(a.id, &Event::worker_progress("r", "x"))
        .unwrap();

    let all = store.events_since(0, 100).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 3]);
    assert_eq!(all[0].task_id, a.id);
    assert_eq!(all[1].task_id, b.id);
    assert_eq!(all[2].task_id, a.id);

    assert_eq!(store.latest_event_id().unwrap(), 3);

    let limited = store.events_since(0, 2).unwrap();
    assert_eq!(limited.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);

    let after = store.events_since(1, 100).unwrap();
    assert_eq!(after.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 3]);

    let none = store.events_since(3, 100).unwrap();
    assert!(none.is_empty());
}

fn task_with(
    title: &str,
    status: Status,
    kind: TaskKind,
    priority: i32,
    parent: Option<TaskId>,
) -> Task {
    let mut t = sample_task(status);
    t.title = title.to_string();
    t.kind = kind;
    t.priority = priority;
    t.parent_id = parent;
    t
}

/// ADR-0013 D10: `ListFilter` の各条件（複数 status、kind、parent、root_only、text_contains。
/// `%` を含む検索語のエスケープ込み）。
#[test]
fn list_page_filters_by_status_kind_parent_root_only_and_title() {
    let store = SqliteStore::open_in_memory().unwrap();
    let root = task_with("root", Status::Ready, TaskKind::Plan, 0, None);
    store.insert(&root).unwrap();
    let child_exec_ready = task_with(
        "child a",
        Status::Ready,
        TaskKind::Execute,
        0,
        Some(root.id),
    );
    store.insert(&child_exec_ready).unwrap();
    let child_exec_done = task_with("child b", Status::Done, TaskKind::Execute, 0, Some(root.id));
    store.insert(&child_exec_done).unwrap();
    let child_approval = task_with(
        "approve 100%",
        Status::Ready,
        TaskKind::Approval,
        0,
        Some(root.id),
    );
    store.insert(&child_approval).unwrap();
    let other_root = task_with("other_root", Status::Draft, TaskKind::Execute, 0, None);
    store.insert(&other_root).unwrap();

    // statuses: 複数指定は OR。
    let f = ListFilter {
        statuses: vec![Status::Ready, Status::Draft],
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    let ids: HashSet<_> = page.items.iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        [
            root.id,
            child_exec_ready.id,
            child_approval.id,
            other_root.id
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(page.total, 4);

    // kind
    let f = ListFilter {
        kinds: vec![TaskKind::Approval],
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![child_approval.id]
    );
    assert_eq!(page.total, 1);

    // parent
    let f = ListFilter {
        parent_id: Some(root.id),
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    let ids: HashSet<_> = page.items.iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        [child_exec_ready.id, child_exec_done.id, child_approval.id]
            .into_iter()
            .collect()
    );
    assert_eq!(page.total, 3);

    // root_only
    let f = ListFilter {
        root_only: true,
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    let ids: HashSet<_> = page.items.iter().map(|t| t.id).collect();
    assert_eq!(ids, [root.id, other_root.id].into_iter().collect());
    assert_eq!(page.total, 2);

    // text_contains: リテラルな '%' を含む検索語（エスケープが効いているか）。
    let percent_task = task_with("100% done", Status::Ready, TaskKind::Execute, 0, None);
    store.insert(&percent_task).unwrap();
    let no_percent_task = task_with("100 done", Status::Ready, TaskKind::Execute, 0, None);
    store.insert(&no_percent_task).unwrap();
    let f = ListFilter {
        text_contains: Some("100%".to_string()),
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    let ids: HashSet<_> = page.items.iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        [child_approval.id, percent_task.id].into_iter().collect()
    );
    assert!(!ids.contains(&no_percent_task.id));

    // ADR-0014 D2: text_contains は objective も対象にする（ASCII の大文字小文字は区別しない）。
    let mut by_objective = task_with("plain title", Status::Ready, TaskKind::Execute, 0, None);
    by_objective.objective = "migrate the billing service".to_string();
    store.insert(&by_objective).unwrap();
    let f = ListFilter {
        text_contains: Some("billing".to_string()),
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![by_objective.id]
    );
    let f = ListFilter {
        text_contains: Some("BILLING".to_string()),
        ..Default::default()
    };
    assert_eq!(
        store
            .list_page(&f, ListOrder::CreatedDesc, None, 10)
            .unwrap()
            .total,
        1
    );
}

/// ADR-0027 D1: `genre` は `kind` と同じ形（完全一致、複数は OR）で絞り込める。
#[test]
fn list_page_filters_by_genre() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut coding = task_with("write code", Status::Ready, TaskKind::Execute, 0, None);
    coding.genre = Some("coding".to_string());
    store.insert(&coding).unwrap();
    let mut literature = task_with("survey papers", Status::Ready, TaskKind::Execute, 0, None);
    literature.genre = Some("literature".to_string());
    store.insert(&literature).unwrap();
    let no_genre = task_with("no genre", Status::Ready, TaskKind::Execute, 0, None);
    store.insert(&no_genre).unwrap();

    let f = ListFilter {
        genres: vec!["coding".to_string()],
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![coding.id]
    );
    assert_eq!(page.total, 1);

    let f = ListFilter {
        genres: vec!["coding".to_string(), "literature".to_string()],
        ..Default::default()
    };
    let page = store
        .list_page(&f, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    let ids: HashSet<_> = page.items.iter().map(|t| t.id).collect();
    assert_eq!(ids, [coding.id, literature.id].into_iter().collect());
    assert_eq!(page.total, 2);
}

/// ADR-0013 D10: 3 つの並び順。`updated_at` は遷移後に変わるので `UpdatedDesc` の順序も変わる。
#[test]
fn list_page_orders_dispatch_updated_desc_created_desc_and_reacts_to_transitions() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut ids = Vec::new();
    for i in 0..5i32 {
        let mut t = sample_task(Status::Ready);
        t.priority = i;
        t.title = format!("t{i}");
        store.insert(&t).unwrap();
        ids.push(t.id);
        std::thread::sleep(StdDuration::from_millis(2));
    }
    let expected_desc: Vec<TaskId> = ids.iter().rev().copied().collect();

    let page = store
        .list_page(&ListFilter::default(), ListOrder::Dispatch, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_desc
    );

    let page = store
        .list_page(&ListFilter::default(), ListOrder::CreatedDesc, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_desc
    );

    let page = store
        .list_page(&ListFilter::default(), ListOrder::UpdatedDesc, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_desc
    );

    // ids[0] は priority 最小・最も古い。cancel して updated_at を更新すると UpdatedDesc の先頭になる。
    store
        .apply_transition(ids[0], Trigger::Cancel, None)
        .unwrap();
    let page = store
        .list_page(&ListFilter::default(), ListOrder::UpdatedDesc, None, 10)
        .unwrap();
    assert_eq!(page.items[0].id, ids[0]);
    // Dispatch 順は status を見ないので変わらない（cancelled でも一覧には出る）。
    let page = store
        .list_page(&ListFilter::default(), ListOrder::Dispatch, None, 10)
        .unwrap();
    assert_eq!(
        page.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_desc
    );
}

/// ADR-0013 D10: `limit` より多い件数を cursor で辿ると、重複・欠落なく全件を1回ずつ得られる。
#[test]
fn list_page_cursor_chains_without_duplicates_or_gaps() {
    let store = SqliteStore::open_in_memory().unwrap();
    for i in 0..7i32 {
        let mut t = sample_task(Status::Ready);
        t.priority = i % 3;
        store.insert(&t).unwrap();
    }

    let full = store
        .list_page(&ListFilter::default(), ListOrder::Dispatch, None, 100)
        .unwrap();
    assert_eq!(full.total, 7);

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store
            .list_page(
                &ListFilter::default(),
                ListOrder::Dispatch,
                cursor.as_deref(),
                3,
            )
            .unwrap();
        assert_eq!(page.total, 7);
        seen.extend(page.items.iter().map(|t| t.id));
        if page.next_cursor.is_none() {
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(seen, full.items.iter().map(|t| t.id).collect::<Vec<_>>());
    let unique: HashSet<_> = seen.iter().collect();
    assert_eq!(unique.len(), 7);
}

/// ADR-0013 D10: 不正な cursor は `StoreError::Invalid`。
#[test]
fn list_page_rejects_invalid_cursor() {
    let store = SqliteStore::open_in_memory().unwrap();
    assert!(matches!(
        store.list_page(
            &ListFilter::default(),
            ListOrder::Dispatch,
            Some("not-a-cursor"),
            10
        ),
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.list_page(&ListFilter::default(), ListOrder::Dispatch, Some("abc"), 10),
        Err(StoreError::Invalid(_))
    ));
    // 偶数長・16進として妥当だが JSON として不正。
    assert!(matches!(
        store.list_page(&ListFilter::default(), ListOrder::Dispatch, Some("00"), 10),
        Err(StoreError::Invalid(_))
    ));
}

/// ADR-0013 D10: status ごとの件数集計。0 件の status は含まない。
#[test]
fn count_by_status_aggregates_present_statuses_only() {
    let store = SqliteStore::open_in_memory().unwrap();
    store.insert(&sample_task(Status::Ready)).unwrap();
    store.insert(&sample_task(Status::Ready)).unwrap();
    store.insert(&sample_task(Status::Draft)).unwrap();

    let counts = store.count_by_status().unwrap();
    let map: HashMap<Status, u64> = counts.into_iter().collect();
    assert_eq!(map.get(&Status::Ready), Some(&2));
    assert_eq!(map.get(&Status::Draft), Some(&1));
    assert_eq!(map.get(&Status::Done), None);
}

/// ADR-0013 D9: `ProviderThrottled.reason` は往復し、`reason` の無い旧 JSON も読める。
#[test]
fn provider_throttled_reason_roundtrips_and_reads_legacy_json() {
    let old = r#"{"type":"provider_throttled","provider":"acct-a","until":"2024-01-01T00:00:00Z"}"#;
    let ev: Event = serde_json::from_str(old).unwrap();
    assert_eq!(
        ev,
        Event::ProviderThrottled {
            provider: "acct-a".into(),
            until: OffsetDateTime::parse("2024-01-01T00:00:00Z", &Rfc3339).unwrap(),
            reason: None,
        }
    );
    assert_eq!(serde_json::to_string(&ev).unwrap(), old);

    let with_reason = Event::ProviderThrottled {
        provider: "acct-a".into(),
        until: OffsetDateTime::parse("2024-01-01T00:00:00Z", &Rfc3339).unwrap(),
        reason: Some("throttled".into()),
    };
    let json = serde_json::to_string(&with_reason).unwrap();
    assert!(json.contains(r#""reason":"throttled""#));
    let back: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(back, with_reason);
}

/// ADR-0013 D8: 生成した `EventRow` の JSON Schema とコミット済みファイルの一致。
/// `UPDATE_SCHEMA=1` で再生成。
#[test]
fn event_row_schema_matches_committed() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/api/v1/event.schema.json"
    );
    let generated = serde_json::to_string_pretty(&event_row_schema_value()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
    assert_eq!(
        committed, generated,
        "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-core`"
    );
}

// ---- ADR-0033 D1/D2（Phase 23）: 組織・案件・途中目標 ----

fn org_node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        profile: Default::default(),
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        name: format!("{id} の人"),
        kind,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn seed_secretary(store: &SqliteStore) {
    store
        .org_upsert(&org_node("secretary", None, OrgKind::Secretary))
        .expect("secretary");
}

/// 版数 5 の DB を開くと以後の migration（6, 7）が適用され、もう一度開いても何も起きない（冪等）。
/// 既存行は壊れない。
#[test]
fn open_migrates_schema_5_db_to_the_current_version_and_reapplying_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema5.sqlite3");
    let task = sample_task(Status::Draft);
    {
        let conn = Connection::open(&path).unwrap();
        for sql in [
            MIGRATION_0001,
            MIGRATION_0002,
            MIGRATION_0003,
            MIGRATION_0004,
            MIGRATION_0005,
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES \
             (1, '2020-01-01T00:00:00Z'), (2, '2020-01-01T00:00:00Z'), (3, '2020-01-01T00:00:00Z'), \
             (4, '2020-01-01T00:00:00Z'), (5, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
        let json = serde_json::to_string(&task).unwrap();
        let created_at = format_rfc3339(task.created_at).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, \
             lease_worker_run_id, lease_expires_at, json, title, updated_at, objective, genre) \
             VALUES (?1,?2,?3,?4,?5,?6,NULL,NULL,?7,?8,?9,?10,NULL)",
            params![
                task.id.to_string(),
                status_str(task.status),
                kind_str(task.kind),
                task.parent_id.map(|p| p.to_string()),
                task.priority,
                created_at,
                json,
                task.title,
                created_at,
                task.objective,
            ],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let got = store.get(task.id).unwrap().expect("old row still readable");
    assert_eq!(got.project_id, None);
    assert_eq!(got.milestone_id, None);
    assert_eq!(got.assignee, None);
    assert!(store.org_list().unwrap().is_empty());
    seed_secretary(&store);
    drop(store);

    // 2 回目に開いても 0006 / 0007 は再適用されず（適用済み）、中身も残る。
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(store.org_list().unwrap().len(), 1);
    let applied = |version: u32| -> i64 {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
            params![version],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(applied(6), 1, "migration 6 must be recorded exactly once");
    assert_eq!(applied(7), 1, "migration 7 must be recorded exactly once");
}

/// Phase 27 / migration 0007: 版数 6 の DB（`reports.project_id` が NOT NULL、案件なしは空文字列。
/// ADR-0034 D1）を開くと、空文字列の行が NULL になって `None` として読め、`messages.task_id` が増える。
#[test]
fn migration_0007_turns_the_empty_project_sentinel_into_null_and_adds_message_task_id() {
    use crate::report::{ReportKind, ReportStore};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema6.sqlite3");
    let report_id = crate::report::ReportId::new();
    let kept_id = crate::report::ReportId::new();
    let project = sample_project();
    {
        let conn = Connection::open(&path).unwrap();
        for sql in [
            MIGRATION_0001,
            MIGRATION_0002,
            MIGRATION_0003,
            MIGRATION_0004,
            MIGRATION_0005,
            MIGRATION_0006,
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES \
             (1, '2020-01-01T00:00:00Z'), (2, '2020-01-01T00:00:00Z'), (3, '2020-01-01T00:00:00Z'), \
             (4, '2020-01-01T00:00:00Z'), (5, '2020-01-01T00:00:00Z'), (6, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
        for (id, project_id) in [
            (report_id, String::new()),
            (kept_id, project.id.to_string()),
        ] {
            conn.execute(
                "INSERT INTO reports (id, project_id, node_id, task_id, kind, level, headline, body, \
                 sources, read_at, created_at) VALUES (?1, ?2, 'infra', NULL, 'bad_news', 1, 'h', 'b', \
                 '[]', NULL, '2026-09-17T00:00:00Z')",
                params![id.to_string(), project_id],
            )
            .unwrap();
        }
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let migrated = store
        .report_get(report_id)
        .unwrap()
        .expect("old row still readable");
    assert_eq!(
        migrated.project_id, None,
        "空文字列のセンチネルは NULL になる"
    );
    assert_eq!(migrated.kind, ReportKind::BadNews);
    assert_eq!(
        store.report_get(kept_id).unwrap().map(|r| r.project_id),
        Some(Some(project.id)),
        "案件付きの行はそのまま"
    );
    // `messages.task_id` が増えているので、書いて読み戻せる。
    let task_id = TaskId::new();
    let message = Message {
        id: MessageId::new(),
        node_id: "secretary".into(),
        project_id: None,
        role: MessageRole::User,
        text: "こんにちは".into(),
        run_id: None,
        task_id: Some(task_id),
        metadata: None,
        created_at: OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap(),
    };
    store.message_append(&message).unwrap();
    assert_eq!(
        store.message_list("secretary", None, 10).unwrap()[0].task_id,
        Some(task_id)
    );
}

/// Phase 39 / migration 0008（ADR-0037）: 版数 7 の DB を開くと版数 8 になり、`notifications` が
/// 使えるようになる（既存の行はそのまま）。2 回目に開いても 0008 は再適用されない。
#[test]
fn migration_0008_adds_the_notifications_table_to_a_schema_7_db() {
    use crate::notify::{NotificationKind, NotificationStore};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema7.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        for sql in [
            MIGRATION_0001,
            MIGRATION_0002,
            MIGRATION_0003,
            MIGRATION_0004,
            MIGRATION_0005,
            MIGRATION_0006,
            MIGRATION_0007,
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES \
             (1, '2020-01-01T00:00:00Z'), (2, '2020-01-01T00:00:00Z'), (3, '2020-01-01T00:00:00Z'), \
             (4, '2020-01-01T00:00:00Z'), (5, '2020-01-01T00:00:00Z'), (6, '2020-01-01T00:00:00Z'), \
             (7, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);
    let now = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
    assert!(
        store
            .notification_upsert_pending(NotificationKind::BadNews, "r1", "悪い知らせ", None, now)
            .unwrap()
            .is_some()
    );
    assert_eq!(store.notification_pending().unwrap().len(), 1);

    // migration 9（ADR-0037 D6 / GUI 依頼 G13i-P1）: `project_id` が新しい DB でも往復する。
    let project_id = ProjectId::new();
    let with_project = store
        .notification_upsert_pending(
            NotificationKind::MilestoneReady,
            "m1:2",
            "b",
            Some(project_id),
            now,
        )
        .unwrap()
        .unwrap();
    assert_eq!(with_project.project_id, Some(project_id));
    let recent = store.notification_recent(10).unwrap();
    let found = recent.iter().find(|n| n.id == with_project.id).unwrap();
    assert_eq!(found.project_id, Some(project_id));
    drop(store);

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(
        store.notification_pending().unwrap().len(),
        2,
        "既存の行は残る"
    );
    let found = store
        .notification_recent(10)
        .unwrap()
        .into_iter()
        .find(|n| n.id == with_project.id)
        .unwrap();
    assert_eq!(
        found.project_id,
        Some(project_id),
        "project_id も再オープン後に残る"
    );
    let applied: i64 = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 8",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(applied, 1, "migration 8 must be recorded exactly once");
    let applied_9: i64 = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 9",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(applied_9, 1, "migration 9 must be recorded exactly once");
}

#[test]
fn org_nodes_round_trip_and_upsert_keeps_created_at() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_secretary(&store);
    let mut coding = org_node("coding", Some("secretary"), OrgKind::Department);
    coding.position = 2;
    coding.brief = "コードを書く".into();
    let stored = store.org_upsert(&coding).unwrap();
    assert_eq!(store.org_get("coding").unwrap().as_ref(), Some(&stored));

    let mut renamed = stored.clone();
    renamed.name = "コーディング部".into();
    renamed.genre = Some("coding".into());
    renamed.created_at = OffsetDateTime::now_utc() + time::Duration::days(1);
    let updated = store.org_upsert(&renamed).unwrap();
    assert_eq!(
        updated.created_at, stored.created_at,
        "created_at is kept on update"
    );
    assert_eq!(updated.name, "コーディング部");
    assert_eq!(updated.genre.as_deref(), Some("coding"));

    // 並び順は position（同値なら id）の昇順。
    let mut infra = org_node("infra", Some("secretary"), OrgKind::Department);
    infra.position = 1;
    store.org_upsert(&infra).unwrap();
    let ids: Vec<String> = store
        .org_list()
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(
        ids,
        vec!["secretary".to_string(), "infra".into(), "coding".into()]
    );
}

/// 監査 D-4: `org_seed` は 1 トランザクション。途中の 1 件が不正（親が居ない）なら、それより前の
/// 行も含めて何も書かれない（部分的に蒔かれた組織が残らない）。全件が正しければ渡した順に入る。
#[test]
fn org_seed_writes_nothing_when_one_node_is_invalid() {
    let store = SqliteStore::open_in_memory().unwrap();
    let nodes = vec![
        org_node("secretary", None, OrgKind::Secretary),
        org_node("coding", Some("secretary"), OrgKind::Department),
        // 親が存在しない: この 1 件が不正。
        org_node("orphan", Some("ghost"), OrgKind::Section),
    ];
    let err = store.org_seed(&nodes).unwrap_err();
    assert!(
        matches!(err, StoreError::Org(OrgError::UnknownParent { .. })),
        "{err}"
    );
    assert!(
        store.org_list().unwrap().is_empty(),
        "nothing is written on failure"
    );

    let ok_nodes = vec![
        org_node("secretary", None, OrgKind::Secretary),
        org_node("coding", Some("secretary"), OrgKind::Department),
        org_node("poc", Some("coding"), OrgKind::Section),
    ];
    store.org_seed(&ok_nodes).unwrap();
    // `org_list` は position（同値なら id）の昇順で返す。全ノードが position = 0 なので id 順になる。
    let ids: Vec<String> = store
        .org_list()
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(
        ids,
        vec!["coding".to_string(), "poc".into(), "secretary".into()]
    );
}

#[test]
fn org_upsert_rejects_a_second_secretary_and_cycles() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_secretary(&store);
    store
        .org_upsert(&org_node("coding", Some("secretary"), OrgKind::Department))
        .unwrap();
    store
        .org_upsert(&org_node("poc", Some("coding"), OrgKind::Section))
        .unwrap();

    let err = store
        .org_upsert(&org_node("boss", None, OrgKind::Secretary))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Org(OrgError::DuplicateSecretary { .. })),
        "{err}"
    );
    let err = store
        .org_upsert(&org_node("coding", Some("coding"), OrgKind::Department))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Org(OrgError::Cycle { .. })),
        "{err}"
    );
    let err = store
        .org_upsert(&org_node("x", Some("ghost"), OrgKind::Section))
        .unwrap_err();
    assert!(
        matches!(err, StoreError::Org(OrgError::UnknownParent { .. })),
        "{err}"
    );
    assert_eq!(
        store.org_list().unwrap().len(),
        3,
        "nothing was written by the failed upserts"
    );
}

#[test]
fn org_delete_refuses_while_a_task_is_open_or_children_remain() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_secretary(&store);
    store
        .org_upsert(&org_node("coding", Some("secretary"), OrgKind::Department))
        .unwrap();
    store
        .org_upsert(&org_node("poc", Some("coding"), OrgKind::Section))
        .unwrap();

    let mut task = sample_task(Status::Ready);
    task.assignee = Some("poc".into());
    store.insert(&task).unwrap();

    let err = store.org_delete("poc").unwrap_err();
    assert!(matches!(err, StoreError::InUse { .. }), "{err}");
    // 子を抱えた部も消せない。
    let err = store.org_delete("coding").unwrap_err();
    assert!(matches!(err, StoreError::InUse { .. }), "{err}");
    // タスクが終端になれば消せる。
    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .unwrap();
    assert!(store.org_delete("poc").unwrap());
    assert!(store.org_get("poc").unwrap().is_none());
    assert!(
        !store.org_delete("poc").unwrap(),
        "deleting a missing node is Ok(false)"
    );
}

fn sample_project() -> Project {
    let now = OffsetDateTime::now_utc();
    Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "Pluvio の新テーマ".into(),
        request: "Pluvio を基盤に用いた新たな研究テーマの模索、検証".into(),
        status: ProjectStatus::Proposed,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    }
}

/// ADR-0033 D4（Phase 24）: 対話の追記と一覧（案件ごと・ノードごと・件数上限・古い順）。
#[test]
fn messages_are_appended_and_listed_oldest_first_per_node_and_project() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = sample_project();
    store.project_create(&project).unwrap();
    let other_project = sample_project();
    store.project_create(&other_project).unwrap();
    let base = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();

    let task_id = TaskId::new();
    let append = |node: &str, project_id: Option<ProjectId>, role, text: &str, n: i64| {
        let m = Message {
            id: MessageId::new(),
            node_id: node.into(),
            project_id,
            role,
            text: text.into(),
            run_id: if role == MessageRole::Node {
                Some(format!("run-{n}"))
            } else {
                None
            },
            task_id: Some(task_id),
            metadata: None,
            created_at: base + std::time::Duration::from_secs(n as u64),
        };
        store.message_append(&m).unwrap();
        m
    };

    let q = append(
        "secretary",
        Some(project.id),
        MessageRole::User,
        "この案件をお願い",
        1,
    );
    let a = append(
        "secretary",
        Some(project.id),
        MessageRole::Node,
        "承知しました",
        2,
    );
    append(
        "secretary",
        Some(other_project.id),
        MessageRole::User,
        "別の案件",
        3,
    );
    append(
        "research-survey",
        Some(project.id),
        MessageRole::User,
        "別の人",
        4,
    );
    append(
        "secretary",
        None,
        MessageRole::User,
        "案件に紐づかない雑談",
        5,
    );

    // 案件ごと・ノードごとに分かれ、古い順に並ぶ。
    let thread = store
        .message_list("secretary", Some(project.id), 20)
        .unwrap();
    assert_eq!(
        thread.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![q.id, a.id]
    );
    assert_eq!(thread[0].role, MessageRole::User);
    assert_eq!(thread[1].run_id.as_deref(), Some("run-2"));
    assert_eq!(thread[1].project_id, Some(project.id));
    // R4（migration 0007）: 1 往復の両方の行に、それを起こした対話用タスクの id が入る。
    assert_eq!(thread[0].task_id, Some(task_id));
    assert_eq!(thread[1].task_id, Some(task_id));
    assert_eq!(
        store
            .message_list("secretary", Some(other_project.id), 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .message_list("research-survey", Some(project.id), 20)
            .unwrap()
            .len(),
        1
    );
    // `project_id = None` は案件に紐づかない行だけ（案件の行は混ざらない）。
    let chat = store.message_list("secretary", None, 20).unwrap();
    assert_eq!(chat.len(), 1);
    assert_eq!(chat[0].text, "案件に紐づかない雑談");
    assert!(
        store
            .message_list("ghost", Some(project.id), 20)
            .unwrap()
            .is_empty()
    );

    // 件数上限は「新しい方を残して古い順に返す」。
    for n in 10..20 {
        append(
            "secretary",
            Some(project.id),
            MessageRole::User,
            &format!("m{n}"),
            n,
        );
    }
    let last3 = store
        .message_list("secretary", Some(project.id), 3)
        .unwrap();
    assert_eq!(
        last3.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        vec!["m17", "m18", "m19"]
    );
    assert!(
        store
            .message_list("secretary", Some(project.id), 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn projects_and_milestones_round_trip() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = sample_project();
    store.project_create(&project).unwrap();
    // Phase K-1: slug は作るときに題名から決まる（`sample_project` は `None` で渡す）。
    let expected = Project {
        slug: Some(project.kb_slug()),
        ..project.clone()
    };
    assert_eq!(expected.slug.as_deref(), Some("pluvio"));
    assert_eq!(
        store.project_get(project.id).unwrap().as_ref(),
        Some(&expected)
    );
    assert_eq!(store.project_list().unwrap().len(), 1);

    assert!(
        store
            .project_set_status(project.id, ProjectStatus::Active)
            .unwrap()
    );
    let got = store.project_get(project.id).unwrap().unwrap();
    assert_eq!(got.status, ProjectStatus::Active);
    assert!(
        !store
            .project_set_status(ProjectId::new(), ProjectStatus::Done)
            .unwrap()
    );

    let first = store
        .milestone_create(
            project.id,
            "関連研究を棚卸しする",
            "候補を 3 本",
            MilestoneStatus::Proposed,
        )
        .unwrap();
    let second = store
        .milestone_create(
            project.id,
            "小さな検証を回す",
            "",
            MilestoneStatus::Proposed,
        )
        .unwrap();
    assert_eq!(
        (first.seq, second.seq),
        (1, 2),
        "seq is numbered per project"
    );
    assert_eq!(
        store
            .milestone_list(project.id)
            .unwrap()
            .iter()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
    assert!(
        store
            .milestone_set_status(second.id, MilestoneStatus::Approved)
            .unwrap()
    );
    assert_eq!(
        store.milestone_list(project.id).unwrap()[1].status,
        MilestoneStatus::Approved
    );
    assert!(
        !store
            .milestone_set_status(MilestoneId::new(), MilestoneStatus::Reached)
            .unwrap()
    );

    let err = store
        .milestone_create(ProjectId::new(), "無い案件", "", MilestoneStatus::Proposed)
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err}");
}

/// ADR-0039 D1（migration 0010）: 案件の作業場所が None / Local / Remote で往復し、後から付け外しできる。
#[test]
fn project_workspace_round_trips_and_can_be_set_and_cleared() {
    let store = SqliteStore::open_in_memory().unwrap();
    let none = sample_project();
    store.project_create(&none).unwrap();
    assert_eq!(
        store
            .project_get(none.id)
            .unwrap()
            .and_then(|p| p.workspace),
        None
    );

    let local_spec = WorkspaceSpec::Local {
        path: std::path::PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
        mode: None,
    };
    let mut local = sample_project();
    local.workspace = Some(local_spec.clone());
    store.project_create(&local).unwrap();
    assert_eq!(
        store.project_get(local.id).unwrap().unwrap().workspace,
        Some(local_spec)
    );

    let remote_spec = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: std::path::PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
        mode: None,
    };
    let mut remote = sample_project();
    remote.workspace = Some(remote_spec.clone());
    store.project_create(&remote).unwrap();
    assert_eq!(
        store.project_get(remote.id).unwrap().unwrap().workspace,
        Some(remote_spec.clone())
    );
    // 一覧にも載る。
    let listed = store.project_list().unwrap();
    assert_eq!(listed.iter().filter(|p| p.workspace.is_some()).count(), 2);

    // 後から付ける / 消す。
    assert!(
        store
            .project_set_workspace(none.id, Some(&remote_spec))
            .unwrap()
    );
    assert_eq!(
        store.project_get(none.id).unwrap().unwrap().workspace,
        Some(remote_spec)
    );
    assert!(store.project_set_workspace(none.id, None).unwrap());
    assert_eq!(store.project_get(none.id).unwrap().unwrap().workspace, None);
    assert!(!store.project_set_workspace(ProjectId::new(), None).unwrap());
}

/// ADR-0039 D1: 版数 9 の DB に migration 0010 が当たり、既存の案件は `workspace = NULL` のまま読める。
#[test]
fn migration_0010_adds_the_projects_workspace_column_to_a_schema_9_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema9.sqlite3");
    let legacy = ProjectId::new();
    {
        let conn = Connection::open(&path).unwrap();
        for sql in [
            MIGRATION_0001,
            MIGRATION_0002,
            MIGRATION_0003,
            MIGRATION_0004,
            MIGRATION_0005,
            MIGRATION_0006,
            MIGRATION_0007,
            MIGRATION_0008,
            MIGRATION_0009,
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES \
             (1, '2020-01-01T00:00:00Z'), (2, '2020-01-01T00:00:00Z'), (3, '2020-01-01T00:00:00Z'), \
             (4, '2020-01-01T00:00:00Z'), (5, '2020-01-01T00:00:00Z'), (6, '2020-01-01T00:00:00Z'), \
             (7, '2020-01-01T00:00:00Z'), (8, '2020-01-01T00:00:00Z'), (9, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, title, request, status, created_at, updated_at) \
             VALUES (?1, '古い案件', '依頼', 'active', '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z')",
            params![legacy.to_string()],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);
    // 導入前の案件は「作業場所なし」= 従来どおり。
    assert_eq!(store.project_get(legacy).unwrap().unwrap().workspace, None);
    let spec = WorkspaceSpec::Local {
        path: std::path::PathBuf::from("/home/rmaeda/workspace/rust/pluvio-poc"),
        mode: None,
    };
    assert!(store.project_set_workspace(legacy, Some(&spec)).unwrap());
    assert_eq!(
        store.project_get(legacy).unwrap().unwrap().workspace,
        Some(spec)
    );
}

/// ADR-0043 D1（Phase 52）: 版数 11 の DB に migration 0012 が当たり、既存の `projects.workspace` が
/// `is_primary = 1` のリポジトリ 1 件に写る。`kind` は「パスが git なら git、でなければ dir」で、
/// これは Rust の backfill（`backfill_project_repos`）が決める。
#[test]
fn migration_0012_copies_each_project_workspace_into_a_primary_repo() {
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = dir.path().join("benchfs");
    std::fs::create_dir_all(repo_dir.join(".git")).unwrap();
    let plain_dir = dir.path().join("data set");
    std::fs::create_dir_all(&plain_dir).unwrap();

    let path = dir.path().join("schema11.sqlite3");
    let (git_project, plain_project, none_project, remote_project) = (
        ProjectId::new(),
        ProjectId::new(),
        ProjectId::new(),
        ProjectId::new(),
    );
    {
        let conn = Connection::open(&path).unwrap();
        for sql in [
            MIGRATION_0001,
            MIGRATION_0002,
            MIGRATION_0003,
            MIGRATION_0004,
            MIGRATION_0005,
            MIGRATION_0006,
            MIGRATION_0007,
            MIGRATION_0008,
            MIGRATION_0009,
            MIGRATION_0010,
            MIGRATION_0011,
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\
             INSERT INTO schema_migrations (version, applied_at) VALUES \
             (1, '2020-01-01T00:00:00Z'), (2, '2020-01-01T00:00:00Z'), (3, '2020-01-01T00:00:00Z'), \
             (4, '2020-01-01T00:00:00Z'), (5, '2020-01-01T00:00:00Z'), (6, '2020-01-01T00:00:00Z'), \
             (7, '2020-01-01T00:00:00Z'), (8, '2020-01-01T00:00:00Z'), (9, '2020-01-01T00:00:00Z'), \
             (10, '2020-01-01T00:00:00Z'), (11, '2020-01-01T00:00:00Z');",
        )
        .unwrap();
        let rows: [(ProjectId, Option<String>); 4] = [
            (
                git_project,
                Some(format!(
                    r#"{{"kind":"local","path":"{}"}}"#,
                    repo_dir.display()
                )),
            ),
            (
                plain_project,
                Some(format!(
                    r#"{{"kind":"local","path":"{}"}}"#,
                    plain_dir.display()
                )),
            ),
            (none_project, None),
            (
                remote_project,
                Some(
                    r#"{"kind":"remote","cluster":"pegasus","path":"/work/NBB/x/benchfs"}"#
                        .to_string(),
                ),
            ),
        ];
        for (id, workspace) in rows {
            conn.execute(
                "INSERT INTO projects (id, title, request, status, created_at, updated_at, workspace) \
                 VALUES (?1, '案件', '依頼', 'active', '2020-01-01T00:00:00Z', '2020-01-01T00:00:00Z', ?2)",
                params![id.to_string(), workspace],
            )
            .unwrap();
        }
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);

    // git のリポジトリ（`.git` がある）→ `kind = git`、名前はディレクトリ名、primary。
    let repos = store.repo_list(git_project).unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name, "benchfs");
    assert_eq!(repos[0].kind, RepoKind::Git);
    assert!(repos[0].is_primary);
    assert_eq!(repos[0].location, WorkspaceSpec::local(repo_dir.clone()));
    assert_eq!(repos[0].run, RepoRun::Auto);

    // git でないディレクトリ → `kind = dir`。名前は slug（空白は `-`）。
    let plain = store.repo_list(plain_project).unwrap();
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0].kind, RepoKind::Dir);
    assert_eq!(plain[0].name, "data-set");

    // 作業場所を決めていない案件にはリポジトリを作らない。
    assert!(store.repo_list(none_project).unwrap().is_empty());

    // リモートは `git`（クラスタ側は見られないので ADR-0018 / 0019 の前提に倒す）。
    let remote = store.repo_list(remote_project).unwrap();
    assert_eq!(remote.len(), 1);
    assert_eq!(remote[0].kind, RepoKind::Git);

    // `Project.workspace` は primary の写し（GUI の後方互換）。
    assert_eq!(
        store.project_get(git_project).unwrap().unwrap().workspace,
        Some(WorkspaceSpec::local(repo_dir))
    );
    assert_eq!(
        store.project_get(none_project).unwrap().unwrap().workspace,
        None
    );
}

/// ADR-0043 D1: リポジトリの CRUD と primary の不変条件（1 案件に 1 つ）。
#[test]
fn project_repos_are_created_updated_and_have_exactly_one_primary() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = sample_project();
    store.project_create(&project).unwrap();

    let code = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "benchfs".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local("/srv/benchfs"),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: false,
        created_at: OffsetDateTime::now_utc(),
    };
    store.repo_create(&code).unwrap();
    // 最初の 1 件は自動的に primary。
    assert!(store.repo_get(code.id).unwrap().unwrap().is_primary);
    assert_eq!(
        store.project_get(project.id).unwrap().unwrap().workspace,
        Some(WorkspaceSpec::local("/srv/benchfs")),
        "Project.workspace は primary の写し"
    );

    let paper = ProjectRepo {
        id: RepoId::new(),
        name: "benchfs-paper".into(),
        location: WorkspaceSpec::local("/srv/benchfs-paper"),
        ..code.clone()
    };
    store.repo_create(&paper).unwrap();
    let repos = store.repo_list(project.id).unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0].name, "benchfs", "primary が先頭");
    assert!(!repos[1].is_primary);

    // 名前が重複したら 422（`StoreError::Repo`）。
    let dup = ProjectRepo {
        id: RepoId::new(),
        ..paper.clone()
    };
    assert!(matches!(
        store.repo_create(&dup),
        Err(StoreError::Repo(RepoError::DuplicateName(_)))
    ));

    // primary を移すと写しも移る。
    assert!(store.repo_set_primary(paper.id).unwrap());
    assert!(!store.repo_get(code.id).unwrap().unwrap().is_primary);
    assert_eq!(
        store.project_get(project.id).unwrap().unwrap().workspace,
        Some(WorkspaceSpec::local("/srv/benchfs-paper"))
    );

    // 更新（名前と run）。`project_id` / `created_at` は動かない。
    let renamed = ProjectRepo {
        name: "paper".into(),
        run: RepoRun::Host,
        project_id: ProjectId::new(),
        ..store.repo_get(paper.id).unwrap().unwrap()
    };
    assert!(store.repo_update(&renamed).unwrap());
    let back = store.repo_get(paper.id).unwrap().unwrap();
    assert_eq!(back.name, "paper");
    assert_eq!(back.run, RepoRun::Host);
    assert_eq!(back.project_id, project.id, "案件の付け替えはしない");

    // 消すと、残りのうち一番古いものが primary になる。
    assert!(store.repo_delete(paper.id).unwrap());
    assert!(store.repo_get(code.id).unwrap().unwrap().is_primary);
    assert_eq!(
        store.project_get(project.id).unwrap().unwrap().workspace,
        Some(WorkspaceSpec::local("/srv/benchfs"))
    );
    assert!(!store.repo_delete(paper.id).unwrap(), "無い id は false");
}

/// ADR-0043 D1: 未終端のタスクが参照しているリポジトリは消せない（API は 409）。
#[test]
fn a_repo_used_by_an_unfinished_task_cannot_be_deleted() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = sample_project();
    store.project_create(&project).unwrap();
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "benchfs".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local("/srv/benchfs"),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: OffsetDateTime::now_utc(),
    };
    store.repo_create(&repo).unwrap();

    let mut task = sample_task(Status::Ready);
    task.project_id = Some(project.id);
    task.repos = vec![crate::repos::RepoRef::of(&repo)];
    store.insert(&task).unwrap();

    assert_eq!(store.repo_active_tasks(repo.id).unwrap(), vec![task.id]);
    assert!(matches!(
        store.repo_delete(repo.id),
        Err(StoreError::InUse {
            kind: "project repo",
            ..
        })
    ));

    // 終端になれば消せる。
    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .unwrap();
    assert!(store.repo_active_tasks(repo.id).unwrap().is_empty());
    assert!(store.repo_delete(repo.id).unwrap());
    // primary が消えたので案件は「作業場所なし」に戻る。
    assert_eq!(
        store.project_get(project.id).unwrap().unwrap().workspace,
        None
    );
}

/// ADR-0043 D5（Phase 54）: 取り込みの記録を書く → 最新を引く → 案件の一覧は
/// 「タスク × リポジトリごとに最新の 1 件」。
#[test]
fn integrations_are_recorded_and_the_latest_one_per_task_and_repo_is_listed() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = sample_project();
    store.project_create(&project).unwrap();
    let mut task = sample_task(Status::Done);
    task.project_id = Some(project.id);
    store.insert(&task).unwrap();
    // 案件に属さないタスクの記録は案件の一覧に出ない。
    let other = sample_task(Status::Done);
    store.insert(&other).unwrap();

    let t0 = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
    let mut first = TaskIntegration::new(
        task.id,
        None,
        "code",
        IntegrationMethod::Pr,
        IntegrationState::Open,
        t0,
    );
    first.pr_number = Some(7);
    first.pr_url = Some("https://example.invalid/pull/7".into());
    store.integration_put(&first).unwrap();
    let paper = TaskIntegration::new(
        task.id,
        None,
        "paper",
        IntegrationMethod::Merge,
        IntegrationState::Done,
        t0 + time::Duration::seconds(1),
    );
    store.integration_put(&paper).unwrap();
    let elsewhere = TaskIntegration::new(
        other.id,
        None,
        "code",
        IntegrationMethod::Discard,
        IntegrationState::Done,
        t0 + time::Duration::seconds(2),
    );
    store.integration_put(&elsewhere).unwrap();

    assert_eq!(
        store.integration_get(first.id).unwrap().as_ref(),
        Some(&first)
    );
    assert_eq!(
        store.integration_latest(task.id, "code").unwrap().as_ref(),
        Some(&first)
    );
    assert_eq!(store.integration_latest(task.id, "nope").unwrap(), None);
    // 新しい順（ADR-0044 B1 の timeline はこれを読む）。
    assert_eq!(
        store
            .integration_list_for_task(task.id)
            .unwrap()
            .iter()
            .map(|i| i.repo.as_str())
            .collect::<Vec<_>>(),
        vec!["paper", "code"]
    );

    // 同じタスク・同じリポジトリをもう一度取り込むと、一覧には新しい方だけ出る。
    let mut second = first.clone();
    second.id = crate::integrations::IntegrationId::new();
    second.state = IntegrationState::Merged;
    second.created_at = t0 + time::Duration::seconds(10);
    second.updated_at = second.created_at;
    store.integration_put(&second).unwrap();
    assert_eq!(
        store.integration_latest(task.id, "code").unwrap().as_ref(),
        Some(&second)
    );

    let listed = store.integration_list_for_project(project.id, 100).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|i| (i.repo.as_str(), i.state))
            .collect::<Vec<_>>(),
        vec![
            ("code", IntegrationState::Merged),
            ("paper", IntegrationState::Done)
        ],
        "タスク × リポジトリごとに最新の 1 件、新しい順"
    );
    assert_eq!(
        store
            .integration_list_for_project(project.id, 1)
            .unwrap()
            .len(),
        1,
        "limit が効く"
    );
}

/// ADR-0043 D1: `PATCH /projects {workspace}`（従来のフォーム）は primary のリポジトリを書き換える。
#[test]
fn setting_the_project_workspace_keeps_the_primary_repo_in_step() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut project = sample_project();
    project.workspace = None;
    store.project_create(&project).unwrap();
    assert!(store.repo_list(project.id).unwrap().is_empty());

    // 作業場所を付けると primary のリポジトリが 1 件できる。
    let first = WorkspaceSpec::local("/srv/benchfs");
    assert!(
        store
            .project_set_workspace(project.id, Some(&first))
            .unwrap()
    );
    let repos = store.repo_list(project.id).unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name, "benchfs");
    assert!(repos[0].is_primary);

    // 差し替えは場所だけを直す（名前は人の設定を残す）。
    let moved = WorkspaceSpec::local("/srv/moved");
    assert!(
        store
            .project_set_workspace(project.id, Some(&moved))
            .unwrap()
    );
    let repos = store.repo_list(project.id).unwrap();
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name, "benchfs");
    assert_eq!(repos[0].location, moved);
    assert_eq!(
        store.project_get(project.id).unwrap().unwrap().workspace,
        Some(moved)
    );

    // `null` は primary を消す（= 案件を「作業場所なし」に戻す）。
    assert!(store.project_set_workspace(project.id, None).unwrap());
    assert!(store.repo_list(project.id).unwrap().is_empty());
    assert_eq!(
        store.project_get(project.id).unwrap().unwrap().workspace,
        None
    );
}

#[test]
fn tasks_can_be_listed_by_project() {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = sample_project();
    store.project_create(&project).unwrap();
    let milestone = store
        .milestone_create(project.id, "最初の途中目標", "", MilestoneStatus::Approved)
        .unwrap();

    let mut mine = sample_task(Status::Ready);
    mine.project_id = Some(project.id);
    mine.milestone_id = Some(milestone.id);
    mine.assignee = Some("poc".into());
    store.insert(&mine).unwrap();
    store.insert(&sample_task(Status::Ready)).unwrap();

    let filter = ListFilter {
        project_id: Some(project.id),
        ..ListFilter::default()
    };
    let page = store
        .list_page(&filter, ListOrder::CreatedDesc, None, 10)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].id, mine.id);
    let back = store.get(mine.id).unwrap().unwrap();
    assert_eq!(back.project_id, Some(project.id));
    assert_eq!(back.milestone_id, Some(milestone.id));
    assert_eq!(back.assignee.as_deref(), Some("poc"));
}

/// ADR-0033 D2: 既存の JSON（3 つの列を持たない）もそのまま読める。
#[test]
fn tasks_without_the_new_fields_still_deserialize() {
    let task = sample_task(Status::Draft);
    let mut json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&task).unwrap()).unwrap();
    let obj = json.as_object_mut().unwrap();
    assert!(
        !obj.contains_key("project_id"),
        "None is skipped on serialization"
    );
    obj.remove("genre");
    let back: Task = serde_json::from_value(json).unwrap();
    assert_eq!(back.project_id, None);
    assert_eq!(back.assignee, None);
    // ADR-0044 D3（Phase 53）: 導入前のタスクはラベル無し・種類 other として読める。
    assert!(back.labels.is_empty());
    assert_eq!(back.category, crate::model::TaskCategory::Other);
}

// ---- ADR-0044 D6（Phase 55）: 中止・一時停止・アーカイブ ----

/// migration 0015 が schema 14 の DB に `projects.archived_at` / `projects.paused_from` /
/// `milestones.paused_from` を足す。既存の行はどれも NULL（＝止まっていない・アーカイブされていない）。
#[test]
fn migration_0015_adds_the_lifecycle_columns_to_a_schema_14_db() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy.sqlite3");
    let project_id = ProjectId::new();
    let milestone_id = MilestoneId::new();
    let now = format_rfc3339(OffsetDateTime::now_utc()).unwrap();
    {
        let mut conn = Connection::open(&path).unwrap();
        SqliteStore::configure_pragmas(&conn, &StoreOptions::default()).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=14 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        // 14 版の列だけで案件と途中目標を 1 件ずつ書く（`archived_at` / `paused_from` はまだ無い）。
        conn.execute(
            "INSERT INTO projects (id, title, request, status, created_at, updated_at) \
             VALUES (?1, '昔の案件', 'やって', 'active', ?2, ?2)",
            params![project_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO milestones (id, project_id, seq, title, description, status, created_at, updated_at) \
             VALUES (?1, ?2, 1, '昔の途中目標', '', 'in_progress', ?3, ?3)",
            params![milestone_id.to_string(), project_id.to_string(), now],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);

    let project = store.project_get(project_id).unwrap().expect("project");
    assert_eq!(project.status, ProjectStatus::Active);
    assert_eq!(project.archived_at, None);
    assert_eq!(project.paused_from, None);
    let milestone = store
        .milestone_get(milestone_id)
        .unwrap()
        .expect("milestone");
    assert_eq!(milestone.status, MilestoneStatus::InProgress);
    assert_eq!(milestone.paused_from, None);

    // 新しい値も往復する。
    store
        .project_set_lifecycle(
            project_id,
            ProjectStatus::Paused,
            Some(Some(ProjectStatus::Active)),
        )
        .unwrap();
    store.project_set_archived_at(project_id, None).unwrap();
    let project = store.project_get(project_id).unwrap().expect("project");
    assert_eq!(project.status, ProjectStatus::Paused);
    assert_eq!(project.paused_from, Some(ProjectStatus::Active));

    store
        .milestone_set_lifecycle(milestone_id, MilestoneStatus::Cancelled, Some(None))
        .unwrap();
    let milestone = store
        .milestone_get(milestone_id)
        .unwrap()
        .expect("milestone");
    assert_eq!(milestone.status, MilestoneStatus::Cancelled);
    assert_eq!(milestone.paused_from, None);
}

/// ADR-0048 D3（Phase 60b）: migration 0017 が schema 16 の DB に `messages.metadata_json` と
/// `console_action_runs` を足す。既存の行は `metadata = None` のまま読める。
#[test]
fn migration_0017_adds_message_metadata_and_console_action_runs_to_a_schema_16_db() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy.sqlite3");
    let message_id = MessageId::new();
    let now = format_rfc3339(OffsetDateTime::now_utc()).unwrap();
    {
        let mut conn = Connection::open(&path).unwrap();
        SqliteStore::configure_pragmas(&conn, &StoreOptions::default()).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=16 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        // 16 版の列だけで対話の行を 1 件書く（`metadata_json` はまだ無い）。
        conn.execute(
            "INSERT INTO messages (id, node_id, project_id, role, text, run_id, created_at, task_id) \
             VALUES (?1, 'secretary', NULL, 'user', '昔の発言', NULL, ?2, NULL)",
            params![message_id.to_string(), now],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);

    // 導入前の行は `metadata = None` として読める。
    let messages = store.message_list("secretary", None, 10).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, message_id);
    assert_eq!(messages[0].metadata, None);

    // 新しい行は `metadata` を往復できる。
    let with_metadata = Message {
        id: MessageId::new(),
        node_id: "secretary".into(),
        project_id: None,
        role: MessageRole::Node,
        text: "できました".into(),
        run_id: Some("run-1".into()),
        task_id: None,
        metadata: Some(crate::MessageMetadata {
            actions_executed: vec![crate::MessageActionResult {
                kind: "create_task".into(),
                summary: "タスクを作りました: 直す".into(),
                task_id: None,
                project_id: None,
                milestone_id: None,
            }],
            actions_failed: vec![],
            author: None,
        }),
        created_at: OffsetDateTime::now_utc(),
    };
    store.message_append(&with_metadata).unwrap();
    let messages = store.message_list("secretary", None, 10).unwrap();
    assert_eq!(messages[1].metadata, with_metadata.metadata);

    // `console_action_runs`: 同じ run の 2 回目の claim は何もしない。
    let task_id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    assert!(
        store
            .console_action_run_claim("run-1", task_id, now)
            .unwrap(),
        "1 回目は新規に記録する"
    );
    assert!(
        !store
            .console_action_run_claim("run-1", task_id, now)
            .unwrap(),
        "2 回目は既に実行済みなので何もしない"
    );
    assert!(
        store
            .console_action_run_claim("run-2", task_id, now)
            .unwrap(),
        "別の run は新規に記録する"
    );
}

// ---- ADR-0044 D2/D3/D4（Phase 53）: コメント・ラベル・種類・ボードのフィルタ ----

/// migration 0013 が schema 11 の DB に `task_comments` と `tasks.labels_json` / `tasks.category` を足す。
/// 既存の行は「ラベル無し・種類 other」になる。
#[test]
fn migration_0013_adds_task_comments_and_the_label_columns_to_a_schema_11_db() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy.sqlite3");
    let legacy = sample_task(Status::Ready);
    {
        let mut conn = Connection::open(&path).unwrap();
        SqliteStore::configure_pragmas(&conn, &StoreOptions::default()).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=11 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        // 11 版の列だけで 1 行書く（`labels_json` / `category` はまだ無い）。
        conn.execute(
            "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, json, title, updated_at, \
             objective) VALUES (?1, 'ready', 'execute', NULL, 0, ?2, ?3, ?4, ?2, ?5)",
            params![
                legacy.id.to_string(),
                format_rfc3339(legacy.created_at).unwrap(),
                serde_json::to_string(&legacy).unwrap(),
                legacy.title,
                legacy.objective,
            ],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);
    {
        let conn = store.lock().unwrap();
        let (labels, category): (String, String) = conn
            .query_row(
                "SELECT labels_json, category FROM tasks WHERE id = ?1",
                params![legacy.id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(labels, "[]");
        assert_eq!(category, "other");
    }
    // コメントを 1 件書ける（表がある）。
    let comment = TaskComment::new(
        legacy.id,
        CommentAuthorKind::Human,
        None,
        "移行後でも書ける".into(),
        None,
        OffsetDateTime::now_utc(),
    );
    assert!(store.comment_add(&comment, None).unwrap().is_none());
    assert_eq!(store.comments_for(legacy.id).unwrap().len(), 1);
}

/// コメントは古い順に読め、遷移と同じトランザクションで書ける（割り込み）。
/// 知らないタスクへのコメントは書けない。
#[test]
fn comments_round_trip_and_can_carry_a_transition_in_one_transaction() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Running);
    store.insert(&task).unwrap();

    let base = OffsetDateTime::now_utc();
    for (i, body) in ["ひとつめ", "ふたつめ"].iter().enumerate() {
        let c = TaskComment::new(
            task.id,
            CommentAuthorKind::Node,
            Some("impl".into()),
            (*body).to_string(),
            Some(format!("run-{i}")),
            base + time::Duration::seconds(i as i64),
        );
        store.comment_add(&c, None).unwrap();
    }
    let comments = store.comments_for(task.id).unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].body, "ひとつめ");
    assert_eq!(comments[1].run_id.as_deref(), Some("run-1"));
    assert_eq!(comments[0].author.as_deref(), Some("impl"));

    // 割り込み: コメント + `Interrupt` + `WorkerFinished` が 1 トランザクション。
    let human = TaskComment::new(
        task.id,
        CommentAuthorKind::Human,
        None,
        "止めて".into(),
        None,
        base + time::Duration::seconds(5),
    );
    let finished = Event::WorkerFinished {
        run_id: "run-1".into(),
        outcome: "interrupted: comment".into(),
        usage: None,
        role: None,
        metrics: None,
        end: None,
    };
    let outcome = store
        .comment_add(&human, Some((Trigger::Interrupt, vec![finished])))
        .unwrap()
        .expect("outcome");
    assert_eq!(outcome.next, Status::Ready);
    assert_eq!(outcome.reason, "comment");
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Ready);
    assert_eq!(store.comments_for(task.id).unwrap().len(), 3);

    // 知らないタスクには書けない（表に行が残らない）。
    let orphan = TaskComment::new(
        TaskId::new(),
        CommentAuthorKind::Human,
        None,
        "x".into(),
        None,
        base,
    );
    assert!(matches!(
        store.comment_add(&orphan, None),
        Err(StoreError::Invalid(_))
    ));
}

/// ADR-0044 D1: `update_task` は `json` と絞り込みの列を書き直し、`Event::Edited` を積む。
/// 状態機械は通らない（`status` は変わらない）。
#[test]
fn update_task_rewrites_the_denormalized_columns_and_appends_edited() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task(Status::Running);
    store.insert(&task).unwrap();

    task.title = "新しい題名".into();
    task.objective = "新しい目的".into();
    task.priority = 30;
    task.labels = vec!["infra".into()];
    task.category = crate::model::TaskCategory::Ops;
    task.assignee = Some("infra-section".into());
    store
        .update_task(
            &task,
            Event::Edited {
                fields: vec!["title".into(), "labels".into()],
                by: "human".into(),
            },
        )
        .unwrap();

    let after = store.get(task.id).unwrap().unwrap();
    assert_eq!(after.title, "新しい題名");
    assert_eq!(after.status, Status::Running, "状態機械は通らない");
    {
        let conn = store.lock().unwrap();
        let (title, objective, priority, labels, category, assignee): (
            String,
            String,
            i64,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT title, objective, priority, labels_json, category, assignee FROM tasks WHERE id = ?1",
                params![task.id.to_string()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(title, "新しい題名");
        assert_eq!(objective, "新しい目的");
        assert_eq!(priority, 30);
        assert_eq!(labels, r#"["infra"]"#);
        assert_eq!(category, "ops");
        assert_eq!(assignee.as_deref(), Some("infra-section"));
    }
    assert!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::Edited { by, .. } if by == "human"))
    );

    // 無いタスクは書けない。
    let mut ghost = sample_task(Status::Ready);
    ghost.title = "いない".into();
    assert!(matches!(
        store.update_task(
            &ghost,
            Event::Edited {
                fields: vec![],
                by: "human".into()
            }
        ),
        Err(StoreError::Invalid(_))
    ));
}

/// ADR-0044 D4: label（AND）/ category / milestone / tier / priority / `q`（コメント本文も）で絞れる。
#[test]
fn list_page_filters_by_label_category_tier_priority_and_comment_text() {
    let store = SqliteStore::open_in_memory().unwrap();

    let mut infra = task_with("infra work", Status::Ready, TaskKind::Execute, 30, None);
    infra.labels = vec!["infra".into(), "urgent".into()];
    infra.category = crate::model::TaskCategory::Ops;
    infra.worker_hint.tier = crate::model::Tier::Frontier;
    store.insert(&infra).unwrap();

    let mut docs = task_with("write docs", Status::Ready, TaskKind::Execute, 0, None);
    docs.labels = vec!["infra".into()];
    docs.category = crate::model::TaskCategory::Docs;
    docs.worker_hint.tier = crate::model::Tier::Cheap;
    store.insert(&docs).unwrap();

    let page = |filter: ListFilter| {
        store
            .list_page(&filter, ListOrder::CreatedDesc, None, 50)
            .unwrap()
            .items
            .iter()
            .map(|t| t.title.clone())
            .collect::<Vec<_>>()
    };

    // ラベルは AND。
    assert_eq!(
        page(ListFilter {
            labels: vec!["infra".into()],
            ..ListFilter::default()
        })
        .len(),
        2
    );
    assert_eq!(
        page(ListFilter {
            labels: vec!["infra".into(), "urgent".into()],
            ..ListFilter::default()
        }),
        vec!["infra work".to_string()]
    );
    // 種類・tier・優先度。
    assert_eq!(
        page(ListFilter {
            categories: vec![crate::model::TaskCategory::Docs],
            ..ListFilter::default()
        }),
        vec!["write docs".to_string()]
    );
    assert_eq!(
        page(ListFilter {
            tiers: vec![crate::model::Tier::Frontier],
            ..ListFilter::default()
        }),
        vec!["infra work".to_string()]
    );
    assert_eq!(
        page(ListFilter {
            priorities: vec![30],
            ..ListFilter::default()
        }),
        vec!["infra work".to_string()]
    );
    // 複数のフィルタは AND（種類が合わないので 0 件）。
    assert!(
        page(ListFilter {
            labels: vec!["infra".into()],
            categories: vec![crate::model::TaskCategory::Feature],
            ..ListFilter::default()
        })
        .is_empty()
    );

    // `q` はコメント本文も見る（`text_includes_comments = true` のときだけ）。
    let comment = TaskComment::new(
        docs.id,
        CommentAuthorKind::Human,
        None,
        "ここに zebra と書いてある".into(),
        None,
        OffsetDateTime::now_utc(),
    );
    store.comment_add(&comment, None).unwrap();
    assert_eq!(
        page(ListFilter {
            text_contains: Some("zebra".into()),
            text_includes_comments: true,
            ..ListFilter::default()
        }),
        vec!["write docs".to_string()]
    );
    assert!(
        page(ListFilter {
            text_contains: Some("zebra".into()),
            ..ListFilter::default()
        })
        .is_empty(),
        "コメントを含めない従来の検索では当たらない"
    );
}

/// ADR-0044 D4 の検討の記録: rusqlite の bundled には **FTS5 がある**（この検査で実測している）。
/// それでも Phase 53 が `LIKE` を選んだのは、FTS5 の既定のトークナイザ（unicode61）が**日本語を
/// 語に切らない**ため、「調査」のような部分一致がこの検査のとおり 0 件になるから。
/// ADR-0044 D4 は「FTS5。無ければ `LIKE`」と書いているが、日本語の検索としては `LIKE` が正しい。
#[test]
fn fts5_availability_of_the_bundled_sqlite_is_recorded() {
    let store = SqliteStore::open_in_memory().unwrap();
    let conn = store.lock().unwrap();
    let available = conn
        .execute_batch("CREATE VIRTUAL TABLE temp.fts5_probe USING fts5(body)")
        .is_ok();
    assert!(
        available,
        "rusqlite の bundled には FTS5 がある（ADR-0044 D4 の前提）"
    );
    // FTS5 はある。だが `unicode61` は日本語を 1 つの token にしてしまうので、
    // 「途中の語」では引けない（`LIKE` を選んだ理由）。
    conn.execute_batch("INSERT INTO temp.fts5_probe(body) VALUES ('関連研究の調査をする')")
        .unwrap();
    let hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM temp.fts5_probe WHERE temp.fts5_probe MATCH '調査'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert_eq!(
        hits, 0,
        "FTS5 の既定のトークナイザでは日本語の部分一致にならない"
    );
}
#[test]
fn migration_0019_resumes_notification_scan_from_the_existing_ledger() {
    use crate::notify::NotificationStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite3");
    let last = OffsetDateTime::now_utc() - time::Duration::minutes(10);
    {
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)").unwrap();
        for version in 1..=18 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        conn.execute("INSERT INTO notifications (id, kind, key, created_at) VALUES (?1, 'approval_pending', 'approval', ?2)", params![crate::notify::NotificationId::new().to_string(), format_rfc3339(last).unwrap()]).unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.notification_scan_at().unwrap(), Some(last));
    let next = last + time::Duration::minutes(1);
    store.notification_scan_mark(next).unwrap();
    drop(store);
    let reopened = SqliteStore::open(&path).unwrap();
    assert_eq!(reopened.notification_scan_at().unwrap(), Some(next));
    reopened.notification_scan_mark(last).unwrap();
    assert_eq!(reopened.notification_scan_at().unwrap(), Some(next));
}

/// ADR-0052 D2 / D3（Phase 64）: schema 20 の DB を開くと `knowledge_runs` に `retried_at` と
/// `via` が足され、既存の行はそのまま（両方 `NULL`）読める。
#[test]
fn migration_0021_adds_retried_at_and_via_to_an_existing_knowledge_run() {
    use crate::knowledge_run::{KnowledgeRunState, KnowledgeRunStore};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema20.sqlite3");
    let task_id = TaskId::new();
    let run_task_id = TaskId::new();
    {
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)").unwrap();
        for version in 1..=20 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        conn.execute(
            "INSERT INTO knowledge_runs (task_id, run_task_id, state, created_at) \
             VALUES (?1, ?2, 'failed', '2026-09-20T00:00:00Z')",
            params![task_id.to_string(), run_task_id.to_string()],
        )
        .unwrap();
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let run = store.knowledge_run_get(task_id).unwrap().unwrap();
    assert_eq!(run.state, KnowledgeRunState::Failed);
    assert!(run.retried_at.is_none(), "既存の行はまだやり直していない");
    assert!(run.via.is_none());
    // 本番で失敗していた行は、配備後の最初の tick で 1 回だけ作り直せる。
    assert!(
        store
            .knowledge_run_retry(task_id, TaskId::new(), OffsetDateTime::now_utc())
            .unwrap()
    );
}

// ---- ADR-0072 D5/D23（Phase E2）: migration 0026 と execution_plans/work_units/runs ----

fn sample_plan_spec() -> ExecutionPlanSpec {
    ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: crate::execution_plan::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "3 段階の直列計画".to_string(),
        work_units: vec![
            WorkUnitSpec {
                key: "a".into(),
                kind: WorkUnitKind::Implement,
                title: "A".into(),
                objective: "do A".into(),
                depends_on: vec![],
                done_when: vec![],
                checks: vec![],
                context: Default::default(),
                harness: None,
                features: None,
                budget: None,
                outputs: vec![],
                phase: None,
            },
            WorkUnitSpec {
                key: "b".into(),
                kind: WorkUnitKind::Implement,
                title: "B".into(),
                objective: "do B".into(),
                depends_on: vec!["a".into()],
                done_when: vec![],
                checks: vec![],
                context: Default::default(),
                harness: None,
                features: None,
                budget: None,
                outputs: vec![],
                phase: None,
            },
        ],
        phases: Vec::new(),
        children: Vec::new(),
    }
}

fn sample_work_units(task_id: TaskId, plan_id: &str) -> Vec<WorkUnitRow> {
    let now = "2026-09-24T00:00:00Z".to_string();
    let spec = sample_plan_spec();
    vec![
        WorkUnitRow::new(
            "wu-a".into(),
            task_id.to_string(),
            plan_id.to_string(),
            0,
            spec.work_units[0].clone(),
            WorkUnitStatus::Ready,
            now.clone(),
        ),
        WorkUnitRow::new(
            "wu-b".into(),
            task_id.to_string(),
            plan_id.to_string(),
            1,
            spec.work_units[1].clone(),
            WorkUnitStatus::Pending,
            now,
        ),
    ]
}

/// ADR-0072 D23（Phase E2）: 版数 25 の DB（migration 0026 の前）を開くと、`execution_plans` /
/// `work_units` / `runs` が作られる（`CREATE TABLE IF NOT EXISTS` のみ。既存の表には触れない）。
#[test]
fn migration_0026_adds_the_execution_tables_to_a_schema_25_db() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema25.sqlite3");
    {
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=25 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);

    // 新しい表が使える（round trip）。
    let task = sample_task(Status::Draft);
    store.insert(&task).unwrap();
    store
        .append_event(
            task.id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();
    let validated =
        crate::execution_plan::validate(&sample_plan_spec(), Default::default(), &[]).unwrap();
    let plan_id = "01PLAN".to_string();
    let plan = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: validated.spec,
        created_at: "2026-09-24T00:00:00Z".to_string(),
        superseded_at: None,
    };
    let event = Event::ExecutionPlanned {
        plan_id: plan_id.clone(),
        version: 1,
        origin: PlanOrigin::Human,
        supersedes: None,
        reason: None,
        plan: Box::new(plan.spec.clone()),
    };
    store
        .execution_plan_adopt(
            task.id,
            plan.clone(),
            sample_work_units(task.id, &plan_id),
            Vec::new(),
            event,
        )
        .unwrap();

    let active = store.execution_plan_active(task.id).unwrap().unwrap();
    assert_eq!(active.id, plan_id);
    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].key, "a");
    assert_eq!(units[1].key, "b");
}

/// ADR-0074 D1/§5.2（Phase F2 (b)）: 版数 26 の DB（migration 0027 の前）を開くと、`work_units`
/// に v2（並列実行）の phase/lease_run_id/lease_expires_at/branch/base_commit/head_commit/
/// integrated_commit の列が足される（`ALTER TABLE ADD COLUMN` のみ）。旧い行は新しい列が
/// NULL のまま読め、新しい列は書き込める（`idx_work_units_lease` の部分索引も使える）。
/// これらの列は Rust の `WorkUnitRow` にはまだ無い（Phase F2 (c)〜(h) で使う。ADR-0074 §5.2）。
/// Phase K-1: migration 0029 は `projects.slug` を足し、既存の案件に作った順で slug を付ける
/// （題名 → primary リポジトリの名前 → id の末尾。重複は `<slug>-<id の末尾 8 文字>`）。
/// 本番の 4 案件と同じ形（Pluvio が 2 件、2 件目の primary は benchfs）で確かめる。
#[test]
fn migration_29_backfills_unique_project_slugs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schema28.sqlite3");
    let rows = [
        (
            "01M2RBJBPQS3ZKG04YHGD9D5WS",
            "Pluvio を基盤に用いた新たな研究テーマの模索、検証",
            "2026-09-17T18:55:21Z",
            None,
        ),
        (
            "01M2RCYVZH6RGX8RX0JP572BAT",
            "Pluvio を基盤に用いた新たな研究テーマの模索、検証",
            "2026-09-17T19:19:39Z",
            Some("benchfs"),
        ),
        (
            "01M2WTS3DKNZBSZ2JMVB4CZMBW",
            "agent-platform の自己改善",
            "2026-09-19T12:38:08Z",
            Some("agent-platform"),
        ),
        (
            "01M35WRV77A2JPYGERQGXF6V7K",
            "BenchFS 国際会議フルペーパー化",
            "2026-09-23T01:06:07Z",
            Some("benchfs"),
        ),
        (
            "01M36AAAAAAAAAAAAAAAAAAAAA",
            "調査",
            "2026-09-24T00:00:00Z",
            Some("研究"),
        ),
    ];
    {
        let mut conn = Connection::open(&path).unwrap();
        SqliteStore::configure_pragmas(&conn, &StoreOptions::default()).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=28 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        for (id, title, at, repo) in rows {
            conn.execute(
                "INSERT INTO projects (id, title, request, status, created_at, updated_at) \
                 VALUES (?1, ?2, '', 'active', ?3, ?3)",
                params![id, title, at],
            )
            .unwrap();
            if let Some(repo) = repo {
                conn.execute(
                    "INSERT INTO project_repos (id, project_id, name, kind, location_json, is_primary, created_at) \
                     VALUES (?1, ?2, ?3, 'git', '{\"kind\":\"local\",\"path\":\"/tmp/x\"}', 1, ?4)",
                    params![format!("r-{id}"), id, repo, at],
                )
                .unwrap();
            }
        }
    }
    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    let slug_of = |id: &str| {
        store
            .project_get(id.parse().unwrap())
            .unwrap()
            .unwrap()
            .slug
            .unwrap()
    };
    assert_eq!(slug_of("01M2RBJBPQS3ZKG04YHGD9D5WS"), "pluvio");
    assert_eq!(slug_of("01M2RCYVZH6RGX8RX0JP572BAT"), "pluvio-jp572bat");
    assert_eq!(slug_of("01M2WTS3DKNZBSZ2JMVB4CZMBW"), "agent-platform");
    assert_eq!(slug_of("01M35WRV77A2JPYGERQGXF6V7K"), "benchfs");
    // 題名もリポジトリの名前も ASCII にならなければ `project-<id の末尾>`（ULID の形にしない）。
    assert_eq!(slug_of("01M36AAAAAAAAAAAAAAAAAAAAA"), "project-aaaaaaaa");
    // 変えられる。重複・綴り違いは拒否。
    let ap: ProjectId = "01M2WTS3DKNZBSZ2JMVB4CZMBW".parse().unwrap();
    assert!(matches!(
        store.project_set_slug(ap, "benchfs"),
        Err(StoreError::InUse {
            kind: "project slug",
            ..
        })
    ));
    assert!(matches!(
        store.project_set_slug(ap, "01M2WTS3DKNZBSZ2JMVB4CZMBW"),
        Err(StoreError::Invalid(_))
    ));
    assert!(store.project_set_slug(ap, "celeris").unwrap());
    assert_eq!(slug_of("01M2WTS3DKNZBSZ2JMVB4CZMBW"), "celeris");
    assert!(!store.project_set_slug(ProjectId::new(), "x").unwrap());
    // 新しい案件は作るときに付く（既に使われている slug は避ける）。
    let mut fresh = sample_project();
    fresh.title = "agent-platform の続き".into();
    store.project_create(&fresh).unwrap();
    assert_eq!(slug_of(&fresh.id.to_string()), "agent-platform");
    let mut again = sample_project();
    again.title = "celeris".into();
    store.project_create(&again).unwrap();
    assert!(slug_of(&again.id.to_string()).starts_with("celeris-"));
}

/// ADR-0079 D15（Phase R1a）: migration 0031 は版数 30 の DB に `tasks.root_id`・
/// `work_units.child_task_id` / `needs_decisions_json`・`decisions` を足すだけで、**既存の行は書き換えない**
/// （D13「凍結」、D15「root_id は NULL のまま。埋め戻さない」）。旧い task は `tree` を持たず深さ 1 として読め、
/// 旧い WU 行は `child_task_id = None`・`needs_decisions = []` で読める。
#[test]
fn migration_31_adds_tree_columns_without_rewriting_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schema30.sqlite3");
    let root = sample_task(Status::Running);
    let mut child = sample_task(Status::Ready);
    child.parent_id = Some(root.id);
    let now = "2026-09-28T00:00:00Z".to_string();
    let json_before: Vec<(String, String)>;
    {
        let mut conn = Connection::open(&path).unwrap();
        SqliteStore::configure_pragmas(&conn, &StoreOptions::default()).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=30 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        // 版数 30 の列だけで書く（`root_id` 列はまだ無い）。
        for t in [&root, &child] {
            conn.execute(
                "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, json, title, updated_at) \
                 VALUES (?1, ?2, 'execute', ?3, 0, ?4, ?5, ?6, ?4)",
                params![
                    t.id.to_string(),
                    status_str(t.status),
                    t.parent_id.map(|p| p.to_string()),
                    now,
                    serde_json::to_string(t).unwrap(),
                    t.title,
                ],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO work_units (id, task_id, plan_id, key, seq, kind, status, \
             depends_on_json, runs, continuations, retries, json, created_at, updated_at, phase) \
             VALUES ('wu-legacy', ?1, 'plan-legacy', 'a', 0, 'implement', 'ready', '[]', \
             0, 0, 0, ?2, ?3, ?3, 'build')",
            params![
                root.id.to_string(),
                serde_json::to_string(&crate::execution_plan::WorkUnitSpec {
                    key: "a".into(),
                    kind: WorkUnitKind::Implement,
                    title: "a".into(),
                    objective: "a".into(),
                    depends_on: vec![],
                    done_when: vec![],
                    checks: vec![],
                    context: Default::default(),
                    harness: None,
                    features: None,
                    budget: None,
                    outputs: vec![],
                    phase: Some("build".into()),
                })
                .unwrap(),
                now
            ],
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT id, json FROM tasks ORDER BY id")
            .unwrap();
        json_before = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);

    let conn = Connection::open(&path).unwrap();
    // 既存の task の行は 1 バイトも変わらず、`root_id` は NULL のまま（埋め戻さない）。
    let mut stmt = conn
        .prepare("SELECT id, json, root_id FROM tasks ORDER BY id")
        .unwrap();
    let after: Vec<(String, String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(after.len(), 2);
    for ((id_b, json_b), (id_a, json_a, root_id)) in json_before.iter().zip(&after) {
        assert_eq!(id_b, id_a);
        assert_eq!(json_b, json_a);
        assert_eq!(root_id, &None, "migration 0031 must not backfill root_id");
    }
    let reread = store.get(child.id).unwrap().unwrap();
    assert_eq!(reread.tree, None);
    assert_eq!(crate::tree::depth_of(&reread), 1);
    assert_eq!(crate::tree::root_id_of(&reread), child.id);

    // 旧い WU 行は新しい列の既定で読める。
    let units = store.work_units_for(root.id).unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].child_task_id, None);
    assert!(units[0].needs_decisions.is_empty());
    let raw: String = conn
        .query_row(
            "SELECT needs_decisions_json FROM work_units WHERE id = 'wu-legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(raw, "[]");

    // `decisions` は空の表として在り、索引も付く。
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM decisions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    let indexes: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name IN ('idx_tasks_root_id', 'idx_decisions_root', 'idx_decisions_task', 'idx_work_units_child_task') ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        indexes,
        vec![
            "idx_decisions_root",
            "idx_decisions_task",
            "idx_tasks_root_id",
            "idx_work_units_child_task"
        ]
    );
    // 旧いバイナリ（版数 30）は版数 31 以降の DB を開けない（昇格は stop → start）。
    drop(store);
    let max: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(max, i64::from(SCHEMA_VERSION));
}

/// ADR-0079 D4 (4) / D7（Phase R1a）: `Task.tree` は `tasks.root_id` に写り（挿入・更新）、木の Event は
/// 同じトランザクションで `work_units.child_task_id` と `decisions` を書く。
#[test]
fn tree_events_write_root_id_child_links_and_decisions() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut root = sample_task(Status::Running);
    root.tree = Some(crate::tree::TreeInfo::root(root.id));
    store.insert(&root).unwrap();
    let mut child = sample_task(Status::Ready);
    child.parent_id = Some(root.id);
    child.tree = Some(crate::tree::TreeInfo::child_of(
        &root,
        crate::tree::ParentUnit {
            task_id: root.id,
            plan_id: "plan-1".into(),
            unit_key: "p1".into(),
            stage: "phase-1".into(),
            attempt: 1,
        },
        Some("abc".into()),
    ));
    store.insert(&child).unwrap();
    let root_id_col = |id: TaskId| -> Option<String> {
        store
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT root_id FROM tasks WHERE id = ?1",
                    params![id.to_string()],
                    |r| r.get::<_, Option<String>>(0),
                )?)
            })
            .unwrap()
    };
    assert_eq!(root_id_col(root.id), Some(root.id.to_string()));
    assert_eq!(root_id_col(child.id), Some(root.id.to_string()));
    let reread = store.get(child.id).unwrap().unwrap();
    assert_eq!(crate::tree::depth_of(&reread), 2);
    assert_eq!(reread.tree, child.tree);

    // 木に属さない task は NULL のまま。
    let plain = sample_task(Status::Ready);
    store.insert(&plain).unwrap();
    assert_eq!(root_id_col(plain.id), None);

    // work_units の行（kind task の unit の代理）に ChildTaskCreated が結び付く。
    let mut wu = WorkUnitRow::new(
        "wu-p1".into(),
        root.id.to_string(),
        "plan-1".into(),
        0,
        crate::execution_plan::WorkUnitSpec {
            key: "p1".into(),
            kind: WorkUnitKind::Task,
            title: "p1".into(),
            objective: "p1".into(),
            depends_on: vec![],
            done_when: vec![],
            checks: vec![],
            context: Default::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: Some("phase-1".into()),
        },
        WorkUnitStatus::Ready,
        "T".into(),
    );
    wu.needs_decisions = vec!["h1".into()];
    store.work_units_replace(root.id, vec![wu]).unwrap();
    store
        .append_event(
            root.id,
            &Event::ChildTaskCreated {
                plan_id: "plan-1".into(),
                unit_key: "p1".into(),
                child_task_id: child.id,
                depth: 2,
            },
        )
        .unwrap();
    let units = store.work_units_for(root.id).unwrap();
    assert_eq!(units[0].child_task_id, Some(child.id.to_string()));
    assert_eq!(units[0].needs_decisions, vec!["h1".to_string()]);

    // 決定の要求 → 回答（`decisions` の行）。
    let request = crate::decision::DecisionRequest {
        id: "01DEC".into(),
        key: "h1".into(),
        kind: crate::decision::DecisionKind::Choice,
        question: "q".into(),
        options: vec![
            crate::decision::DecisionOption {
                key: "a".into(),
                label: "A".into(),
                consequence: None,
            },
            crate::decision::DecisionOption {
                key: "b".into(),
                label: "B".into(),
                consequence: None,
            },
        ],
        recommended: "a".into(),
        cost_of_reversal: crate::decision::CostOfReversal::Medium,
        cost_note: None,
        needed_before: vec!["p1".into()],
        path: vec![crate::decision::DecisionPathEntry {
            task_id: root.id,
            title: "root".into(),
            stage: Some("phase-1".into()),
            unit: None,
        }],
        raised_by: crate::decision::DecisionRaisedBy {
            task_id: root.id,
            run_id: None,
            origin: crate::decision::DecisionOrigin::Planner,
        },
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    store
        .append_event(
            root.id,
            &Event::DecisionRequested {
                decision: Box::new(request.clone()),
            },
        )
        .unwrap();
    let rows = store.decisions_list(Some(root.id)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, crate::decision::DecisionStatus::Open);
    assert_eq!(rows[0].request, request);
    store
        .append_event(
            root.id,
            &Event::DecisionAnswered {
                id: "01DEC".into(),
                option: "b".into(),
                note: Some("n".into()),
                by: "human".into(),
            },
        )
        .unwrap();
    let row = store.decision_get("01DEC").unwrap().unwrap();
    assert_eq!(row.status, crate::decision::DecisionStatus::Answered);
    assert!(row.answered_at.is_some());
    assert_eq!(row.request.answer.as_ref().unwrap().option, "b");
    assert!(store.decisions_list(Some(child.id)).unwrap().is_empty());
    assert_eq!(store.decisions_list(None).unwrap().len(), 1);
}

#[test]
fn migration_27_adds_work_unit_lease_columns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schema26.sqlite3");
    let task_id = TaskId::new();
    let now = "2026-09-26T00:00:00Z".to_string();
    {
        let mut conn = Connection::open(&path).unwrap();
        SqliteStore::configure_pragmas(&conn, &StoreOptions::default()).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=26 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        // 版数 26 の列だけで work_units に 1 行書く（旧いデータが保たれることの確認。
        // `foreign_keys` は既定で off なので `tasks` に対応する行が無くても挿入できる）。
        conn.execute(
            "INSERT INTO work_units (id, task_id, plan_id, key, seq, kind, status, \
             depends_on_json, runs, continuations, retries, json, created_at, updated_at) \
             VALUES ('wu-legacy', ?1, 'plan-legacy', 'a', 0, 'implement', 'ready', '[]', \
             0, 0, 0, '{}', ?2, ?2)",
            params![task_id.to_string(), now],
        )
        .unwrap();
    }

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 37);

    let conn = Connection::open(&path).unwrap();
    let mut columns: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(work_units)").unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
        for r in rows {
            columns.push(r.unwrap());
        }
    }
    for expected in [
        "phase",
        "lease_run_id",
        "lease_expires_at",
        "branch",
        "base_commit",
        "head_commit",
        "integrated_commit",
    ] {
        assert!(
            columns.contains(&expected.to_string()),
            "missing column {expected}: {columns:?}"
        );
    }

    // 旧い行は新しい列が NULL のまま読める。
    let (phase, lease_run_id, branch): (Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT phase, lease_run_id, branch FROM work_units WHERE id = 'wu-legacy'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(phase, None);
    assert_eq!(lease_run_id, None);
    assert_eq!(branch, None);

    // 新しい列は書き込める（round trip）。
    conn.execute(
        "UPDATE work_units SET phase = 'build', lease_run_id = 'run-1', \
         lease_expires_at = '2026-09-26T01:00:00Z', branch = 'celeris-wu/t/a', \
         base_commit = 'abc123', head_commit = 'def456' WHERE id = 'wu-legacy'",
        [],
    )
    .unwrap();
    let (phase, lease_run_id, base_commit): (Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT phase, lease_run_id, base_commit FROM work_units WHERE id = 'wu-legacy'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(phase.as_deref(), Some("build"));
    assert_eq!(lease_run_id.as_deref(), Some("run-1"));
    assert_eq!(base_commit.as_deref(), Some("abc123"));

    // `idx_work_units_lease`（`lease_run_id IS NOT NULL` の部分索引）が使える。`INDEXED BY` は
    // クエリの WHERE がその部分索引の条件を含んでいないと `no query solution` になるため、
    // 索引の条件（`lease_run_id IS NOT NULL`）も明示する。
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM work_units INDEXED BY idx_work_units_lease \
             WHERE lease_run_id IS NOT NULL AND lease_expires_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // 版数 27 を再度開いても冪等（idempotent）。
    drop(store);
    let store2 = SqliteStore::open(&path).unwrap();
    assert_eq!(store2.schema_version().unwrap(), SCHEMA_VERSION);
}

/// D14: 既に active な計画がある Task へ 2 個目を採用しようとすると拒否される（E2 は新規のみ、
/// replan は E4）。
#[test]
fn execution_plan_adopt_rejects_a_second_active_plan() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Draft);
    store.insert(&task).unwrap();
    let validated =
        crate::execution_plan::validate(&sample_plan_spec(), Default::default(), &[]).unwrap();
    let plan = |id: &str, version: u32| ExecutionPlanRow {
        id: id.to_string(),
        task_id: task.id.to_string(),
        version,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: "2026-09-24T00:00:00Z".to_string(),
        superseded_at: None,
    };
    let event = |plan_id: &str, version: u32| Event::ExecutionPlanned {
        plan_id: plan_id.to_string(),
        version,
        origin: PlanOrigin::Human,
        supersedes: None,
        reason: None,
        plan: Box::new(validated.spec.clone()),
    };
    store
        .execution_plan_adopt(
            task.id,
            plan("p1", 1),
            sample_work_units(task.id, "p1"),
            Vec::new(),
            event("p1", 1),
        )
        .unwrap();
    let err = store
        .execution_plan_adopt(
            task.id,
            plan("p2", 2),
            sample_work_units(task.id, "p2"),
            Vec::new(),
            event("p2", 2),
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::InUse { .. }), "{err:?}");
}

/// D6/D5: `work_unit_transition` は行の更新と `WorkUnitTransitioned` を同じトランザクションで書く。
/// ADR-0074 D1.5（Phase F2 (c-1)）: 新しい列の読み書き・WU の lease・Task の lease の延長。
fn adopt_sample_plan_with_running_task(store: &SqliteStore) -> Task {
    let task = sample_task(Status::Ready);
    store.insert(&task).unwrap();
    let validated =
        crate::execution_plan::validate(&sample_plan_spec(), Default::default(), &[]).unwrap();
    let plan = ExecutionPlanRow {
        id: "p1".into(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: "2026-09-24T00:00:00Z".into(),
        superseded_at: None,
    };
    let mut units = sample_work_units(task.id, "p1");
    units[0].phase = Some("build".into());
    units[1].phase = Some("build".into());
    units[1].status = WorkUnitStatus::Ready;
    store
        .execution_plan_adopt(
            task.id,
            plan,
            units,
            Vec::new(),
            Event::ExecutionPlanned {
                plan_id: "p1".into(),
                version: 1,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(validated.spec),
            },
        )
        .unwrap();
    assert!(
        store
            .acquire_lease(task.id, "phase:p1:build:01X", StdDuration::from_secs(10))
            .unwrap()
    );
    task
}

#[test]
fn work_unit_parallel_columns_round_trip() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = adopt_sample_plan_with_running_task(&store);
    let mut wu = store.work_unit_get("wu-a").unwrap().unwrap();
    assert_eq!(wu.phase.as_deref(), Some("build"));
    assert!(wu.lease_run_id.is_none() && wu.branch.is_none());
    wu.branch = Some(format!("celeris-wu/{}/a", task.id));
    wu.base_commit = Some("b".repeat(40));
    wu.head_commit = Some("c".repeat(40));
    wu.integrated_commit = Some("d".repeat(40));
    store
        .work_unit_transition(
            task.id,
            wu.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: "wu-a".into(),
                key: "a".into(),
                from: WorkUnitStatus::Ready,
                to: WorkUnitStatus::Ready,
                reason: "test".into(),
                run_id: None,
            },
        )
        .unwrap();
    let back = store.work_unit_get("wu-a").unwrap().unwrap();
    assert_eq!(back, wu);
    let listed = store.work_units_for(task.id).unwrap();
    assert_eq!(listed[0], wu);
}

#[test]
fn acquire_work_unit_lease_marks_running_and_extends_the_task_lease() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = adopt_sample_plan_with_running_task(&store);
    let before = store.get(task.id).unwrap().unwrap().lease.unwrap();
    assert!(
        store
            .acquire_work_unit_lease(
                task.id,
                "wu-a",
                "run-a",
                StdDuration::from_secs(3600),
                Some("celeris-wu/x/a".into()),
                Some("abc".into()),
            )
            .unwrap()
    );
    let wu = store.work_unit_get("wu-a").unwrap().unwrap();
    assert_eq!(wu.status, WorkUnitStatus::Running);
    assert_eq!(wu.runs, 1);
    assert_eq!(wu.last_run_id.as_deref(), Some("run-a"));
    assert_eq!(wu.lease_run_id.as_deref(), Some("run-a"));
    assert!(wu.lease_expires_at.is_some());
    assert_eq!(wu.branch.as_deref(), Some("celeris-wu/x/a"));
    assert_eq!(wu.base_commit.as_deref(), Some("abc"));
    let after = store.get(task.id).unwrap().unwrap().lease.unwrap();
    assert_eq!(
        after.worker_run_id, before.worker_run_id,
        "保持者は工程のまま"
    );
    assert!(
        after.expires_at > before.expires_at,
        "期限は WU の lease まで延びる"
    );
    // 2 回目（既に running）は取れない。
    assert!(
        !store
            .acquire_work_unit_lease(
                task.id,
                "wu-a",
                "run-a2",
                StdDuration::from_secs(10),
                None,
                None
            )
            .unwrap()
    );
    // 2 本目の WU は取れる（Task は Running のまま）。
    assert!(
        store
            .acquire_work_unit_lease(
                task.id,
                "wu-b",
                "run-b",
                StdDuration::from_secs(10),
                None,
                None
            )
            .unwrap()
    );
    // renew_lease は WU の lease を持つ run でも効き、Task の lease を延ばす。
    assert!(
        store
            .renew_lease(task.id, "run-b", StdDuration::from_secs(7200))
            .unwrap()
    );
    let renewed = store.get(task.id).unwrap().unwrap().lease.unwrap();
    assert!(renewed.expires_at > after.expires_at);
    assert!(
        !store
            .renew_lease(task.id, "run-unknown", StdDuration::from_secs(7200))
            .unwrap()
    );
    let events = store.events_for(task.id).unwrap();
    let dispatched = events
        .iter()
        .filter(|(_, e)| {
            matches!(e, Event::WorkUnitTransitioned { to: WorkUnitStatus::Running, reason, .. } if reason == "dispatch")
        })
        .count();
    assert_eq!(dispatched, 2);
}

#[test]
fn acquire_work_unit_lease_requires_a_running_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = adopt_sample_plan_with_running_task(&store);
    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .unwrap();
    assert!(
        !store
            .acquire_work_unit_lease(
                task.id,
                "wu-a",
                "run-a",
                StdDuration::from_secs(10),
                None,
                None
            )
            .unwrap()
    );
}

#[test]
fn running_tasks_with_runnable_work_units_lists_only_running_v2_tasks() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = adopt_sample_plan_with_running_task(&store);
    let listed = store.running_tasks_with_runnable_work_units(10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, task.id);
    for (id, run) in [("wu-a", "r1"), ("wu-b", "r2")] {
        assert!(
            store
                .acquire_work_unit_lease(task.id, id, run, StdDuration::from_secs(10), None, None)
                .unwrap()
        );
    }
    assert!(
        store
            .running_tasks_with_runnable_work_units(10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn work_unit_transition_updates_the_row_and_appends_the_event() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Draft);
    store.insert(&task).unwrap();
    let validated =
        crate::execution_plan::validate(&sample_plan_spec(), Default::default(), &[]).unwrap();
    let plan = ExecutionPlanRow {
        id: "p1".into(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: "2026-09-24T00:00:00Z".into(),
        superseded_at: None,
    };
    store
        .execution_plan_adopt(
            task.id,
            plan,
            sample_work_units(task.id, "p1"),
            Vec::new(),
            Event::ExecutionPlanned {
                plan_id: "p1".into(),
                version: 1,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(validated.spec),
            },
        )
        .unwrap();

    let mut wu = store.work_unit_get("wu-a").unwrap().unwrap();
    assert_eq!(wu.status, WorkUnitStatus::Ready);
    wu.status = WorkUnitStatus::Running;
    wu.runs = 1;
    wu.last_run_id = Some("run-1".into());
    store
        .work_unit_transition(
            task.id,
            wu.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: "wu-a".into(),
                key: "a".into(),
                from: WorkUnitStatus::Ready,
                to: WorkUnitStatus::Running,
                reason: "dispatch".into(),
                run_id: Some("run-1".into()),
            },
        )
        .unwrap();

    let reloaded = store.work_unit_get("wu-a").unwrap().unwrap();
    assert_eq!(reloaded.status, WorkUnitStatus::Running);
    assert_eq!(reloaded.runs, 1);
    assert_eq!(reloaded.last_run_id.as_deref(), Some("run-1"));

    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkUnitTransitioned {
            to: WorkUnitStatus::Running,
            ..
        }
    )));
}

/// (g): `runs` の索引が読み書きできる（start → finish → get/list）。
#[test]
fn run_index_round_trips_start_and_finish() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Draft);
    store.insert(&task).unwrap();
    let started_at = "2026-09-24T00:00:00Z".to_string();
    store
        .run_index_start(RunRow {
            run_id: "run-1".into(),
            task_id: task.id.to_string(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: Some("claude-code".into()),
            model: Some("m".into()),
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: started_at.clone(),
            finished_at: None,
        })
        .unwrap();
    let got = store.run_index_get("run-1").unwrap().unwrap();
    assert_eq!(got.status, RunIndexStatus::Running);
    assert_eq!(got.finished_at, None);

    let finished = OffsetDateTime::parse("2026-09-24T00:05:00Z", &Rfc3339).unwrap();
    let ok = store
        .run_index_finish(
            "run-1",
            RunIndexStatus::Completed,
            None,
            None,
            None,
            finished,
        )
        .unwrap();
    assert!(ok);
    let got = store.run_index_get("run-1").unwrap().unwrap();
    assert_eq!(got.status, RunIndexStatus::Completed);
    assert!(got.finished_at.is_some());

    let for_task = store.runs_for_task(task.id).unwrap();
    assert_eq!(for_task.len(), 1);
    assert!(
        !store
            .run_index_finish(
                "no-such-run",
                RunIndexStatus::Completed,
                None,
                None,
                None,
                finished
            )
            .unwrap()
    );
}

/// Phase F5-fix3: `WorkerFinished` を書いたら、まだ `running` の `runs` 行は同じトランザクションで
/// 終端になる（`end` から。無ければ `harness_error`）。既に閉じた行（`run_index_finish`）は変えない。
#[test]
fn worker_finished_closes_a_still_running_runs_row() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Draft);
    store.insert(&task).unwrap();
    let row = |run_id: &str| RunRow {
        run_id: run_id.into(),
        task_id: task.id.to_string(),
        work_unit_id: None,
        role: RunIndexRole::Worker,
        seq: 1,
        status: RunIndexStatus::Running,
        adapter: None,
        model: None,
        account: None,
        session_id: None,
        checkpoint: None,
        usage: None,
        metrics: None,
        started_at: "2026-09-28T00:00:00Z".into(),
        finished_at: None,
    };
    for id in ["lease-lost", "no-end", "already-closed"] {
        store.run_index_start(row(id)).unwrap();
    }
    store
        .run_index_finish(
            "already-closed",
            RunIndexStatus::Completed,
            None,
            None,
            None,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    let finished = |run_id: &str, end: Option<crate::execution::RunEnd>| Event::WorkerFinished {
        run_id: run_id.into(),
        outcome: "infra_requeue: lease expired".into(),
        usage: None,
        role: None,
        metrics: None,
        end,
    };
    store
        .append_event(
            task.id,
            &finished(
                "lease-lost",
                Some(crate::execution::RunEnd::HarnessError {
                    class: crate::execution::HarnessErrorClass::LeaseExpired,
                }),
            ),
        )
        .unwrap();
    store
        .append_event(task.id, &finished("no-end", None))
        .unwrap();
    store
        .append_event(
            task.id,
            &finished("already-closed", Some(crate::execution::RunEnd::Cancelled)),
        )
        .unwrap();
    let got = |id: &str| store.run_index_get(id).unwrap().unwrap();
    assert_eq!(got("lease-lost").status, RunIndexStatus::HarnessError);
    assert!(got("lease-lost").finished_at.is_some());
    assert_eq!(got("no-end").status, RunIndexStatus::HarnessError);
    assert_eq!(got("already-closed").status, RunIndexStatus::Completed);
}

#[test]
fn execution_metrics_task_rows_cover_planned_repair_replan_and_atomic() {
    let store = SqliteStore::open_in_memory().unwrap();
    let at = |s| OffsetDateTime::parse(s, &Rfc3339).unwrap();
    let mut planned = sample_task(Status::Done);
    planned.genre = Some("coding".into());
    planned.assignee = Some("engineering".into());
    planned.routing = Some(crate::model::TaskRouting::default());
    planned.updated_at = at("2026-09-25T00:00:00Z");
    let mut repair = sample_task(Status::Failed);
    repair.updated_at = at("2026-09-25T00:01:00Z");
    let mut replan = sample_task(Status::Reviewing);
    replan.updated_at = at("2026-09-25T00:02:00Z");
    let mut atomic = sample_task(Status::Ready);
    atomic.updated_at = at("2026-09-25T00:03:00Z");
    for task in [&planned, &repair, &replan, &atomic] {
        store.insert(task).unwrap();
    }
    let plan_for = |task: &Task, id: &str, version, status| ExecutionPlanRow {
        id: id.into(),
        task_id: task.id.to_string(),
        version,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status,
        spec: sample_plan_spec(),
        created_at: "2026-09-25T00:00:00Z".into(),
        superseded_at: None,
    };
    let units_for = |task: &Task, plan_id: &str| {
        let mut units = sample_work_units(task.id, plan_id);
        for unit in &mut units {
            unit.id = format!("{}-{}", task.id, unit.key);
        }
        units
    };
    let planned_plan = plan_for(&planned, "planned-v1", 1, PlanStatus::Active);
    let mut planned_units = units_for(&planned, &planned_plan.id);
    planned_units[0].continuations = 2;
    planned_units[0].retries = 1;
    planned_units[1].continuations = 1;
    store
        .execution_plan_adopt(
            planned.id,
            planned_plan.clone(),
            planned_units,
            Vec::new(),
            Event::ExecutionPlanned {
                plan_id: planned_plan.id,
                version: 1,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(sample_plan_spec()),
            },
        )
        .unwrap();

    let repair_plan = plan_for(&repair, "repair-v1", 1, PlanStatus::Active);
    let mut repair_units = units_for(&repair, &repair_plan.id);
    repair_units[1].kind = WorkUnitKind::Repair;
    repair_units[1].spec.kind = WorkUnitKind::Repair;
    repair_units[1].continuations = 1;
    store
        .execution_plan_adopt(
            repair.id,
            repair_plan.clone(),
            repair_units,
            Vec::new(),
            Event::ExecutionPlanned {
                plan_id: repair_plan.id,
                version: 1,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(sample_plan_spec()),
            },
        )
        .unwrap();

    let replan_v1 = plan_for(&replan, "replan-v1", 1, PlanStatus::Superseded);
    let replan_v2 = plan_for(&replan, "replan-v2", 2, PlanStatus::Active);
    store
        .execution_plans_replace(replan.id, vec![replan_v1, replan_v2])
        .unwrap();
    store
        .work_units_replace(replan.id, units_for(&replan, "replan-v2"))
        .unwrap();

    let start_run = |task: &Task, run_id: &str, started_at: &str| {
        store
            .run_index_start(RunRow {
                run_id: run_id.into(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: RunIndexRole::Worker,
                seq: 1,
                status: RunIndexStatus::Running,
                adapter: Some("codex".into()),
                model: Some("gpt-6-sol".into()),
                account: None,
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: started_at.into(),
                finished_at: None,
            })
            .unwrap();
    };
    start_run(&planned, "planned-run-1", "2026-09-25T00:00:00Z");
    start_run(&planned, "planned-run-2", "2026-09-25T00:01:00Z");
    store
        .run_index_finish(
            "planned-run-1",
            RunIndexStatus::BudgetExhausted,
            None,
            None,
            None,
            at("2026-09-25T00:01:00Z"),
        )
        .unwrap();
    let usage = crate::model::Usage {
        input_tokens: Some(42),
        ..Default::default()
    };
    let metrics = crate::model::RunMetrics {
        turns: Some(5),
        ..Default::default()
    };
    store
        .run_index_finish(
            "planned-run-2",
            RunIndexStatus::Completed,
            None,
            Some(usage),
            Some(metrics),
            at("2026-09-25T00:02:00Z"),
        )
        .unwrap();
    start_run(&atomic, "atomic-run", "2026-09-25T00:03:00Z");

    let rows = store.execution_metrics_task_rows(None).unwrap();
    assert_eq!(rows.len(), 4);
    let find = |id| rows.iter().find(|row| row.task_id == id).unwrap();
    let p = find(planned.id);
    assert_eq!(
        (
            p.status,
            p.repairs,
            p.replans,
            p.continuations,
            p.retries,
            p.runs_count,
            p.budget_exhausted_runs
        ),
        (Status::Done, 0, 0, 3, 1, 2, 1)
    );
    assert_eq!(p.genre.as_deref(), Some("coding"));
    assert_eq!(p.assignee.as_deref(), Some("engineering"));
    assert_eq!(
        serde_json::from_str::<crate::model::TaskRouting>(p.routing_json.as_ref().unwrap())
            .unwrap(),
        planned.routing.unwrap()
    );
    let latest = p.latest_run.as_ref().unwrap();
    assert_eq!(latest.run_id, "planned-run-2");
    assert_eq!(latest.role, RunIndexRole::Worker);
    assert_eq!(latest.status, RunIndexStatus::Completed);
    assert_eq!(latest.adapter.as_deref(), Some("codex"));
    assert_eq!(latest.model.as_deref(), Some("gpt-6-sol"));
    assert_eq!(
        serde_json::from_str::<crate::model::Usage>(latest.usage_json.as_ref().unwrap()).unwrap(),
        usage
    );
    assert_eq!(
        serde_json::from_str::<crate::model::RunMetrics>(latest.metrics_json.as_ref().unwrap())
            .unwrap(),
        metrics
    );
    let r = find(repair.id);
    assert_eq!(
        (
            r.status,
            r.repairs,
            r.replans,
            r.continuations,
            r.retries,
            r.runs_count
        ),
        (Status::Failed, 1, 0, 1, 0, 0)
    );
    let rp = find(replan.id);
    assert_eq!(
        (
            rp.status,
            rp.repairs,
            rp.replans,
            rp.continuations,
            rp.retries,
            rp.runs_count
        ),
        (Status::Reviewing, 0, 1, 0, 0, 0)
    );
    let a = find(atomic.id);
    assert_eq!(
        (
            a.status,
            a.repairs,
            a.replans,
            a.continuations,
            a.retries,
            a.runs_count
        ),
        (Status::Ready, 0, 0, 0, 0, 1)
    );
    assert!(a.latest_run.is_some());
    let since_rows = store
        .execution_metrics_task_rows(Some(at("2026-09-25T00:02:00Z")))
        .unwrap();
    assert_eq!(since_rows.len(), 2);
    assert!(since_rows.iter().any(|row| row.task_id == replan.id));
    assert!(since_rows.iter().any(|row| row.task_id == atomic.id));
    let subsecond_rows = store
        .execution_metrics_task_rows(Some(at("2026-09-25T00:02:00.000000001Z")))
        .unwrap();
    assert_eq!(subsecond_rows.len(), 1);
    assert_eq!(subsecond_rows[0].task_id, atomic.id);
}

// ---- ADR-0072 D16/D17（Phase E4）: review_repair_apply / execution_plan_replan ----

fn repair_spec(task: &Task) -> WorkUnitSpec {
    WorkUnitSpec {
        key: "repair-1".into(),
        kind: WorkUnitKind::Repair,
        title: "repair (format): 修復".into(),
        objective: crate::execution::build_repair_objective(
            crate::execution::RepairClass::Format,
            &["cmd=\"cargo fmt --check\" exit=Some(1)".to_string()],
            &task.title,
            &task.objective,
            None,
            None,
        ),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    }
}

/// (b)/(c) の下地: atomic な Task（計画無し）で repair WU を実体化すると、`main`（done）+
/// `repair-1`（ready）の 2 行を持つ `execution_plans`（`origin = repair`）が新設され、Task は
/// `Reviewing -> Ready`（attempts 据え置き）になる。
#[test]
fn review_repair_apply_materializes_main_and_the_repair_work_unit_for_an_atomic_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task(Status::Reviewing);
    task.attempts = 1;
    store.insert(&task).unwrap();

    let now = "2026-09-25T00:00:00Z".to_string();
    let plan_id = "plan-repair-1".to_string();
    let main_spec = WorkUnitSpec {
        key: "main".into(),
        kind: WorkUnitKind::Implement,
        title: task.title.clone(),
        objective: task.objective.clone(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    let main_row = WorkUnitRow::new(
        "wu-main".into(),
        task.id.to_string(),
        plan_id.clone(),
        0,
        main_spec.clone(),
        WorkUnitStatus::Done,
        now.clone(),
    );
    let repair_row = WorkUnitRow::new(
        "wu-repair-1".into(),
        task.id.to_string(),
        plan_id.clone(),
        1,
        repair_spec(&task),
        WorkUnitStatus::Ready,
        now.clone(),
    );
    let plan = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Repair,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: crate::execution_plan::EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "repair materialization".to_string(),
            work_units: vec![main_spec, repair_spec(&task)],
            phases: Vec::new(),
            children: Vec::new(),
        },
        created_at: now.clone(),
        superseded_at: None,
    };
    let plan_event = Event::ExecutionPlanned {
        plan_id: plan_id.clone(),
        version: 1,
        origin: PlanOrigin::Repair,
        supersedes: None,
        reason: Some("review_repair".to_string()),
        plan: Box::new(plan.spec.clone()),
    };

    let outcome = store
        .review_repair_apply(
            task.id,
            vec![plan_event],
            Some(plan),
            vec![main_row, repair_row],
        )
        .unwrap();
    assert_eq!(outcome.next, Status::Ready);
    assert_eq!(outcome.attempts, 1, "repair は attempts を消費しない");

    let active = store.execution_plan_active(task.id).unwrap().unwrap();
    assert_eq!(active.origin, PlanOrigin::Repair);
    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 2);
    assert!(
        units
            .iter()
            .any(|u| u.key == "main" && u.status == WorkUnitStatus::Done)
    );
    assert!(
        units
            .iter()
            .any(|u| u.key == "repair-1" && u.status == WorkUnitStatus::Ready)
    );
    let events: Vec<Event> = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(matches!(
        events.last(),
        Some(Event::ExecutionPlanned {
            origin: PlanOrigin::Repair,
            ..
        })
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_repair"))
    );
}

#[test]
fn delivery_repair_apply_reopens_and_materializes_atomically() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Done);
    store.insert(&task).unwrap();
    let stamp = "2026-09-25T00:00:00Z".to_string();
    let plan_id = "delivery-plan".to_string();
    let main = WorkUnitSpec {
        key: "main".into(),
        kind: WorkUnitKind::Implement,
        title: task.title.clone(),
        objective: task.objective.clone(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    let repair = repair_spec(&task);
    let spec = ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: crate::execution_plan::EXECUTION_PLAN_SCHEMA.into(),
        rationale: "delivery repair".into(),
        work_units: vec![main.clone(), repair.clone()],
        phases: Vec::new(),
        children: Vec::new(),
    };
    let plan = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Repair,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: spec.clone(),
        created_at: stamp.clone(),
        superseded_at: None,
    };
    let rows = vec![
        WorkUnitRow::new(
            "delivery-main".into(),
            task.id.to_string(),
            plan_id.clone(),
            0,
            main,
            WorkUnitStatus::Done,
            stamp.clone(),
        ),
        WorkUnitRow::new(
            "delivery-repair".into(),
            task.id.to_string(),
            plan_id.clone(),
            1,
            repair,
            WorkUnitStatus::Ready,
            stamp,
        ),
    ];
    let event = Event::ExecutionPlanned {
        plan_id,
        version: 1,
        origin: PlanOrigin::Repair,
        supersedes: None,
        reason: Some("delivery_repair".into()),
        plan: Box::new(spec),
    };
    let outcome = store
        .delivery_repair_apply(task.id, vec![event], Some(plan), rows)
        .unwrap();
    assert_eq!(outcome.next, Status::Ready);
    assert_eq!(
        store
            .execution_plan_active(task.id)
            .unwrap()
            .unwrap()
            .status,
        PlanStatus::Active
    );
    let units = store.work_units_for(task.id).unwrap();
    assert!(
        matches!(crate::execution_plan::next_work_unit(&units), crate::execution_plan::NextStep::RunWorkUnit(id) if id == "delivery-repair")
    );
}

/// 計画済みの Task（既に `active` な計画がある）に repair WU だけを追加する経路
/// （`new_plan = None`）。
#[test]
fn review_repair_apply_adds_a_repair_work_unit_to_an_already_planned_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task(Status::Reviewing);
    task.attempts = 0;
    store.insert(&task).unwrap();
    let spec = sample_plan_spec();
    let plan_id = "plan-1".to_string();
    let plan = ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: spec.clone(),
        created_at: "2026-09-25T00:00:00Z".to_string(),
        superseded_at: None,
    };
    let mut units = sample_work_units(task.id, &plan_id);
    for u in &mut units {
        u.status = WorkUnitStatus::Done;
    }
    store
        .execution_plan_adopt(
            task.id,
            plan,
            units,
            Vec::new(),
            Event::ExecutionPlanned {
                plan_id: plan_id.clone(),
                version: 1,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(spec),
            },
        )
        .unwrap();

    let repair_row = WorkUnitRow::new(
        "wu-repair-1".into(),
        task.id.to_string(),
        plan_id.clone(),
        2,
        repair_spec(&task),
        WorkUnitStatus::Ready,
        "2026-09-25T00:01:00Z".to_string(),
    );
    let wu_event = Event::WorkUnitTransitioned {
        work_unit_id: "wu-repair-1".into(),
        key: "repair-1".into(),
        from: WorkUnitStatus::Pending,
        to: WorkUnitStatus::Ready,
        reason: "review_repair".to_string(),
        run_id: None,
    };
    let outcome = store
        .review_repair_apply(task.id, vec![wu_event], None, vec![repair_row])
        .unwrap();
    assert_eq!(outcome.next, Status::Ready);

    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 3);
    assert!(
        units
            .iter()
            .any(|u| u.key == "repair-1" && u.status == WorkUnitStatus::Ready)
    );
}

/// D17: replan は旧版を `superseded` にし、新しい版（`version = old + 1`）を `active` にする。
#[test]
fn execution_plan_replan_supersedes_the_old_version_and_activates_the_new_one() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task(Status::Ready);
    store.insert(&task).unwrap();
    let spec = sample_plan_spec();
    let old_plan_id = "plan-1".to_string();
    let old_plan = ExecutionPlanRow {
        id: old_plan_id.clone(),
        task_id: task.id.to_string(),
        version: 1,
        origin: PlanOrigin::Human,
        planner_run_id: None,
        status: PlanStatus::Active,
        spec: spec.clone(),
        created_at: "2026-09-25T00:00:00Z".to_string(),
        superseded_at: None,
    };
    let units = sample_work_units(task.id, &old_plan_id);
    store
        .execution_plan_adopt(
            task.id,
            old_plan,
            units.clone(),
            Vec::new(),
            Event::ExecutionPlanned {
                plan_id: old_plan_id.clone(),
                version: 1,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(spec.clone()),
            },
        )
        .unwrap();

    // v2: `c` を追加し、`b` が `c` にも依存するよう spec を変える。
    let mut new_spec = spec.clone();
    new_spec.work_units.push(WorkUnitSpec {
        key: "c".into(),
        kind: WorkUnitKind::Implement,
        title: "C".into(),
        objective: "do C, a brand new step added by replan".into(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: Default::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    });
    new_spec.work_units[1].depends_on = vec!["a".into(), "c".into()];

    let new_plan_id = "plan-2".to_string();
    let new_plan = ExecutionPlanRow {
        id: new_plan_id.clone(),
        task_id: task.id.to_string(),
        version: 2,
        origin: PlanOrigin::Planner,
        planner_run_id: Some("run-planner-1".to_string()),
        status: PlanStatus::Active,
        spec: new_spec.clone(),
        created_at: "2026-09-25T00:02:00Z".to_string(),
        superseded_at: None,
    };
    let mut updated_b = units[1].clone();
    updated_b.plan_id = new_plan_id.clone();
    updated_b.spec = new_spec.work_units[1].clone();
    updated_b.depends_on = new_spec.work_units[1].depends_on.clone();
    let new_c = WorkUnitRow::new(
        "wu-c".into(),
        task.id.to_string(),
        new_plan_id.clone(),
        2,
        new_spec.work_units[2].clone(),
        WorkUnitStatus::Ready,
        "2026-09-25T00:02:00Z".to_string(),
    );
    let plan_event = Event::ExecutionPlanned {
        plan_id: new_plan_id.clone(),
        version: 2,
        origin: PlanOrigin::Planner,
        supersedes: Some(old_plan_id.clone()),
        reason: Some("replan".to_string()),
        plan: Box::new(new_spec),
    };
    store
        .execution_plan_replan(
            task.id,
            old_plan_id.clone(),
            new_plan,
            vec![updated_b],
            vec![new_c],
            vec![],
            plan_event,
        )
        .unwrap();

    let plans = store.execution_plan_list(task.id).unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].version, 1);
    assert_eq!(plans[0].status, PlanStatus::Superseded);
    assert!(plans[0].superseded_at.is_some());
    assert_eq!(plans[1].version, 2);
    assert_eq!(plans[1].status, PlanStatus::Active);
    assert_eq!(plans[1].origin, PlanOrigin::Planner);

    let active = store.execution_plan_active(task.id).unwrap().unwrap();
    assert_eq!(active.id, new_plan_id);

    let all_units = store.work_units_for(task.id).unwrap();
    assert_eq!(all_units.len(), 3, "{all_units:?}");
    let b = all_units.iter().find(|u| u.key == "b").unwrap();
    assert_eq!(b.depends_on, vec!["a".to_string(), "c".to_string()]);
    assert!(all_units.iter().any(|u| u.key == "c"));

    // 並行 replan の検出: 旧版はもう active ではないので 2 回目は失敗する。
    let err = store
        .execution_plan_replan(
            task.id,
            old_plan_id,
            ExecutionPlanRow {
                id: "plan-3".into(),
                task_id: task.id.to_string(),
                version: 3,
                origin: PlanOrigin::Human,
                planner_run_id: None,
                status: PlanStatus::Active,
                spec: sample_plan_spec(),
                created_at: "2026-09-25T00:03:00Z".to_string(),
                superseded_at: None,
            },
            vec![],
            vec![],
            vec![],
            Event::ExecutionPlanned {
                plan_id: "plan-3".into(),
                version: 3,
                origin: PlanOrigin::Human,
                supersedes: None,
                reason: None,
                plan: Box::new(sample_plan_spec()),
            },
        )
        .unwrap_err();
    assert!(matches!(err, StoreError::InUse { .. }));
}
