//! ADR 2026-10-11-task-wait-reasons: dispatcher の見送りの理由（メモリの M1〜M9 は snapshot の `task_waits`、
//! DB の理由は task-core の `db_waits` / `resolve_children` と同じ関数）。偽のアダプタと一時ディレクトリだけで、
//! 外部ネットワークに出ない。

use super::*;

fn slow_adapter() -> Arc<InstantAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(30),
    })
}

fn true_task(dir: &std::path::Path) -> Task {
    new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    )
}

fn publish(d: &mut Dispatcher) -> tokio::sync::watch::Receiver<Option<DaemonSnapshot>> {
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-10-11T00:00:00Z".into(),
        tick_ms: 50,
        providers: vec![],
        provider_checks: Default::default(),
    });
    rx
}

fn snapshot_wait(
    rx: &tokio::sync::watch::Receiver<Option<DaemonSnapshot>>,
    id: TaskId,
) -> Option<task_core::TaskWait> {
    rx.borrow()
        .as_ref()
        .expect("snapshot")
        .task_waits
        .iter()
        .find(|w| w.task_id == id)
        .map(|w| w.wait.clone())
}

/// 枠満杯の見送り（M1/M9）と経路なし（M2）が tick ごとに snapshot の `task_waits` に task ごとに載る。
#[tokio::test]
async fn task_wait_full_capacity_and_unroutable_skips_are_in_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut no_route = true_task(dir.path());
    no_route.priority = 10;
    no_route.worker_hint.adapter = Some("nonexistent".into());
    store.insert(&no_route).unwrap();
    let mut first = true_task(dir.path());
    first.priority = 5;
    store.insert(&first).unwrap();
    let second = true_task(dir.path());
    store.insert(&second).unwrap();

    let mut d = dispatcher(store.clone(), slow_adapter(), 1);
    let rx = publish(&mut d);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(
        store.get(first.id).unwrap().unwrap().status,
        Status::Running
    );

    assert!(matches!(
        snapshot_wait(&rx, no_route.id),
        Some(task_core::TaskWait::Unroutable { .. })
    ));
    assert_eq!(
        snapshot_wait(&rx, second.id),
        Some(task_core::TaskWait::Capacity {
            scope: task_core::CapacityScope::Global,
            id: None
        })
    );
    assert_eq!(snapshot_wait(&rx, first.id), None, "dispatched: no wait");

    // 次の tick（枠は埋まったまま）: 照合しない ready にも全体の枠待ちが付く。
    d.tick().unwrap();
    assert_eq!(
        snapshot_wait(&rx, second.id),
        Some(task_core::TaskWait::Capacity {
            scope: task_core::CapacityScope::Global,
            id: None
        })
    );
}

/// depends_on 待ちと祖先の一時停止: dispatcher は `ready_tasks`（`db_waits`）で見送り、一覧の API 用の
/// `db_waits_for` も同じ理由を返す。
#[tokio::test]
async fn task_wait_depends_on_and_paused_ancestor_are_not_dispatched() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let mut paused_parent = true_task(dir.path());
    paused_parent.status = Status::Running;
    paused_parent.paused_at = Some(OffsetDateTime::now_utc());
    store.insert(&paused_parent).unwrap();
    let mut under_pause = true_task(dir.path());
    under_pause.parent_id = Some(paused_parent.id);
    store.insert(&under_pause).unwrap();
    let mut pred = true_task(dir.path());
    pred.status = Status::Running;
    store.insert(&pred).unwrap();
    let mut dependent = true_task(dir.path());
    dependent.depends_on = vec![pred.id];
    store.insert(&dependent).unwrap();

    let mut d = dispatcher(store.clone(), slow_adapter(), 4);
    assert_eq!(d.tick().unwrap().dispatched, 0);
    for id in [under_pause.id, dependent.id] {
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Ready);
    }
    let waits = sqlite
        .db_waits_for(&[under_pause.clone(), dependent.clone()])
        .unwrap();
    assert_eq!(
        waits[&under_pause.id],
        vec![task_core::TaskWait::Paused {
            by: paused_parent.id
        }]
    );
    assert_eq!(
        waits[&dependent.id],
        vec![task_core::TaskWait::DependsOn {
            task_ids: vec![pred.id]
        }]
    );
}

/// 子待ちの親: dispatcher の `resolve_child_dependencies` と一覧・木の `plan_waits` が同じ
/// `task_core::resolve_children` で「未完の子を待つ unit」を決める。
#[tokio::test]
async fn task_wait_parent_waiting_on_child_uses_the_shared_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let parent = true_task(dir.path());
    store.insert(&parent).unwrap();
    let mut child = true_task(dir.path());
    child.parent_id = Some(parent.id);
    child.status = Status::Running;
    child.labels = vec!["child-report".into()];
    store.insert(&child).unwrap();

    let units = vec![task_core::WorkUnitRow::new(
        "wu-merge".into(),
        parent.id.to_string(),
        "plan".into(),
        0,
        wu_spec("merge", &["child:report"]),
        task_core::WorkUnitStatus::Pending,
        "2026-10-11T00:00:00Z".into(),
    )];
    let d = dispatcher(store.clone(), slow_adapter(), 1);
    let (changed, waiting) = d.resolve_child_dependencies(parent.id, &units).unwrap();
    assert!(!changed);
    assert!(waiting, "the dispatcher holds the parent for its child");

    let children = vec![task_core::ChildFacts {
        task_id: child.id,
        status: child.status,
        labels: child.labels.clone(),
    }];
    assert_eq!(
        task_core::plan_waits(&units, &children, &[]).first(),
        Some(&task_core::TaskWait::Children {
            task_ids: vec![child.id],
            unit_keys: vec!["merge".into()],
        })
    );
}

fn gate_plan(schema: &str, review: task_core::StageReview) -> task_core::ExecutionPlanRow {
    task_core::ExecutionPlanRow {
        id: "plan-1".to_string(),
        task_id: TaskId::new().to_string(),
        version: 1,
        origin: task_core::PlanOrigin::Planner,
        planner_run_id: None,
        status: task_core::PlanStatus::Active,
        spec: task_core::ExecutionPlanSpec {
            stages: vec![task_core::StageSpec {
                key: "design".to_string(),
                kind: task_core::WorkUnitKind::Implement,
                title: "design".to_string(),
                review,
            }],
            units: Vec::new(),
            decisions: Vec::new(),
            schema: schema.to_string(),
            rationale: String::new(),
            work_units: Vec::new(),
            phases: Vec::new(),
            children: Vec::new(),
        },
        created_at: "2026-10-11T00:00:00Z".to_string(),
        superseded_at: None,
    }
}

/// (task の status, 有効な計画, events, 期待する gate, 期待する hold の理由)。
type GateCase<'a> = (
    Status,
    &'a task_core::ExecutionPlanRow,
    Vec<Event>,
    Option<task_core::HumanGateKind>,
    Option<&'static str>,
);

/// D12（葉 stage-gate）: 同じ入力（task・有効な計画・events）で、`human_gate_hold` が `Some` ⇔
/// `task_ops::wait::human_gate_waits` が非空。理由の文字列は waits の gate から決まる。
#[tokio::test]
async fn task_wait_stage_gate_hold_iff_waits() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let d = dispatcher(store, slow_adapter(), 1);
    let blocked = |reason: &str| Event::Transitioned {
        from: Status::Running,
        to: Status::Blocked,
        reason: reason.to_string(),
    };
    let reported = Event::PhaseReported {
        phase: "design".into(),
        report: Box::new(task_core::PhaseReport {
            phase: "design".into(),
            phase_title: "design".into(),
            ..Default::default()
        }),
    };
    let requested = Event::PlanApprovalRequested {
        plan_id: "plan-1".into(),
        reasons: vec!["review_human:design".into()],
    };
    let approved = blocked(task_core::PhaseResumeMode::PlanApprove.name());
    let v3h = gate_plan(
        task_core::EXECUTION_PLAN_SCHEMA_V3,
        task_core::StageReview::Human,
    );
    let v3n = gate_plan(
        task_core::EXECUTION_PLAN_SCHEMA_V3,
        task_core::StageReview::None,
    );
    let v2 = gate_plan(
        task_core::EXECUTION_PLAN_SCHEMA_V2,
        task_core::StageReview::Human,
    );
    let gate = |g| Some(g);
    let cases: Vec<GateCase> = vec![
        (Status::Ready, &v3h, vec![], None, None),
        (
            Status::Running,
            &v3h,
            vec![requested.clone()],
            gate(task_core::HumanGateKind::PlanApproval),
            Some("plan_gate_pending"),
        ),
        (
            Status::Running,
            &v3h,
            vec![requested.clone(), approved.clone()],
            None,
            None,
        ),
        (
            Status::Blocked,
            &v3h,
            vec![blocked(task_ops::plan_gate::AWAITING_PLAN_APPROVAL)],
            gate(task_core::HumanGateKind::PlanApproval),
            Some("plan_gate_pending"),
        ),
        (
            Status::Blocked,
            &v3h,
            vec![
                reported.clone(),
                blocked(task_ops::phase_gate::AWAITING_HUMAN),
            ],
            gate(task_core::HumanGateKind::StageReview),
            Some("stage_review"),
        ),
        (
            Status::Blocked,
            &v3n,
            vec![
                reported.clone(),
                blocked(task_ops::phase_gate::AWAITING_HUMAN),
            ],
            gate(task_core::HumanGateKind::PhaseCheckpoint),
            Some("human_gate"),
        ),
        (
            Status::Blocked,
            &v2,
            vec![
                reported.clone(),
                blocked(task_ops::phase_gate::AWAITING_HUMAN),
            ],
            gate(task_core::HumanGateKind::PhaseCheckpoint),
            Some("human_gate"),
        ),
        (
            Status::Blocked,
            &v3h,
            vec![reported.clone(), blocked("question")],
            None,
            None,
        ),
    ];
    for (i, (status, plan, events, want_gate, want_hold)) in cases.into_iter().enumerate() {
        let mut task = true_task(dir.path());
        task.status = status;
        let events: Vec<(u64, Event)> = events
            .into_iter()
            .enumerate()
            .map(|(n, e)| (n as u64 + 1, e))
            .collect();
        let waits = task_ops::wait::human_gate_waits(&task, Some(plan), &events);
        let hold = d.human_gate_hold(&task, plan, &events);
        assert_eq!(
            hold.is_some(),
            !waits.is_empty(),
            "case {i}: {waits:?} {hold:?}"
        );
        assert_eq!(hold, want_hold, "case {i}");
        let got_gate = waits.first().and_then(|w| match w {
            task_core::TaskWait::HumanGate { gate, .. } => Some(*gate),
            _ => None,
        });
        assert_eq!(got_gate, want_gate, "case {i}");
    }
}
