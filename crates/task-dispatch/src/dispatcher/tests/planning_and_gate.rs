use super::*;

#[tokio::test]
async fn plan_task_inserts_draft_children_and_they_run_after_accept() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let plan = plan_task(dir.path(), 0);
    store.insert(&plan).unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: VALID_PLAN.into(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"fine"}]}"#.into(),
        delay: Duration::from_millis(5),
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let p = store.get(plan.id).unwrap().unwrap();
    assert_eq!(p.status, Status::Done);
    let children: Vec<Task> = store.list(Some(Status::Draft)).unwrap();
    assert_eq!(
        children.len(),
        3,
        "auto_accept=false leaves children in draft"
    );
    for c in &children {
        assert_eq!(c.parent_id, Some(plan.id));
        assert_eq!(c.workspace, plan.workspace);
    }
    let verdicts: Vec<(usize, bool, String)> = store
        .events_for(plan.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ReviewVerdict {
                criterion_idx,
                pass,
                reason,
                ..
            } => Some((criterion_idx, pass, reason)),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].0, 0);
    assert!(verdicts[0].1);
    assert!(verdicts[0].2.contains("3 tasks"));

    // 人間が approve（Accept）すると子が順に実行され、c は Reviewer 条件を LLM run（FileAdapter）で判定して done。
    for c in &children {
        store.apply_transition(c.id, Trigger::Accept, None).unwrap();
    }
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);
    for c in &children {
        let t = store.get(c.id).unwrap().unwrap();
        assert_eq!(
            t.status,
            Status::Done,
            "{}: {:?}",
            c.title,
            store.events_for(c.id).unwrap()
        );
    }
    let c = children.iter().find(|c| c.title == "c").unwrap();
    assert_eq!(c.worker_hint.tier, Tier::Cheap);
    let events = store.events_for(c.id).unwrap();
    let run_id = last_run_id(&events).unwrap();
    let reviewer_progress = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::WorkerProgress { run_id: r, msg, .. } if r == &run_id && msg.starts_with("reviewer run ")))
        .count();
    assert!(reviewer_progress >= 2, "{events:?}");
    assert!(events.iter().any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: true, reason, .. } if reason.contains("reviewer(") && reason.contains("fine"))));
    // WorkerStarted はワーカー run の 1 回と、ADR-0014 D1 で記録する Reviewer run の 1 回。
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, Event::WorkerStarted { role: None, .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(
                e,
                Event::WorkerStarted {
                    role: Some(RunRole::Reviewer),
                    ..
                }
            ))
            .count(),
        1
    );
}

/// ADR-0079 D13（Phase R5a）: 途中目標の Go（ADR-0074 D3.2）・dispatch での `in_progress` / 自動 `reached`（ADR-0077）は
/// 廃止。案件計画の途中目標（`plan_key` あり、`auto_advance = false`）に結ばれた既存の root task 同士でも、依存先が
/// `done` になれば依存する側はそのまま dispatch され、凍結した途中目標の行の状態は 1 つも変わらない。新しい
/// root task の完了で途中目標の行はできない。
#[tokio::test]
async fn frozen_milestones_do_not_gate_or_advance_root_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let now = OffsetDateTime::now_utc();
    let project = task_core::Project {
        auto_advance: false,
        slug: None,
        id: task_core::ProjectId::new(),
        title: "案件".into(),
        request: "やって".into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let mut milestones = Vec::new();
    for (key, status) in [
        ("survey", task_core::MilestoneStatus::Approved),
        ("poc", task_core::MilestoneStatus::Approved),
    ] {
        let m = store.milestone_create(project.id, key, "", status).unwrap();
        assert!(store.milestone_set_plan_key(m.id, key).unwrap());
        milestones.push(m);
    }
    let ok = || Check::Command {
        cmd: "true".into(),
        expect_exit: 0,
    };
    let mut survey = new_task(&dir.path().join("survey"), ok(), 0);
    survey.title = "調査".into();
    survey.project_id = Some(project.id);
    survey.milestone_id = Some(milestones[0].id);
    std::fs::create_dir_all(dir.path().join("survey")).unwrap();
    let mut poc = new_task(&dir.path().join("poc"), ok(), 0);
    poc.title = "PoC".into();
    poc.project_id = Some(project.id);
    poc.milestone_id = Some(milestones[1].id);
    poc.depends_on = vec![survey.id];
    std::fs::create_dir_all(dir.path().join("poc")).unwrap();
    // 新しい root task（途中目標なし）。
    let mut fresh = new_task(&dir.path().join("fresh"), ok(), 0);
    fresh.title = "新しい root".into();
    fresh.project_id = Some(project.id);
    std::fs::create_dir_all(dir.path().join("fresh")).unwrap();
    for t in [&survey, &poc, &fresh] {
        assert!(task_core::is_root_task(t));
        store.insert(t).unwrap();
    }
    let before: Vec<_> = store
        .milestone_list(project.id)
        .unwrap()
        .into_iter()
        .map(|m| (m.id, m.status))
        .collect();

    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    assert!(run_until_idle(&mut d, 300).await.idle);
    for t in [&survey, &poc, &fresh] {
        assert_eq!(
            store.get(t.id).unwrap().unwrap().status,
            Status::Done,
            "{}: {:?}",
            t.title,
            store.events_for(t.id).unwrap()
        );
    }
    let after: Vec<_> = store
        .milestone_list(project.id)
        .unwrap()
        .into_iter()
        .map(|m| (m.id, m.status))
        .collect();
    assert_eq!(
        before, after,
        "凍結: dispatch で in_progress にも、done で reached にもならず、新しい行もできない"
    );
    assert_eq!(store.get(fresh.id).unwrap().unwrap().milestone_id, None);
}

/// 受け入れ 1〜3・5: 役割の指示文が run に載り、`delegate` の検証を通った 2 件だけが子になり、親は子が終わるまで
/// reviewing のまま、`aggregate = true` なら最後に 1 回だけ集約 run が走って summary.md が暗黙の条件で判定される。
#[tokio::test]
async fn delegate_inserts_validated_children_and_aggregate_parent_runs_once_more() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut parent = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    parent.role = Some("lead".into());
    parent.aggregate = true;
    store.insert(&parent).unwrap();
    let mut bad_title = proposal("", vec![]);
    bad_title.title = "  ".into();
    let adapter = Arc::new(DelegatingAdapter {
        proposals: vec![
            proposal("a", vec![]),
            proposal("b", vec![task_core::DelegateDep::Index(0)]),
            bad_title,
            proposal(
                "self",
                vec![task_core::DelegateDep::Id(parent.id.to_string())],
            ),
        ],
        child_delay: Duration::from_millis(30),
        seen_role: std::sync::Mutex::new(None),
        aggregate_children: AtomicUsize::new(0),
        write_summary: true,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 4);
    d.config.roles = roles();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);

    let p = store.get(parent.id).unwrap().unwrap();
    assert_eq!(
        p.status,
        Status::Done,
        "{:?}",
        store.events_for(parent.id).unwrap()
    );
    assert_eq!(p.attempts, 0, "aggregate does not consume attempts");
    let children = store.children(parent.id).unwrap();
    assert_eq!(
        children.len(),
        2,
        "only the two valid proposals were inserted"
    );
    assert_eq!(children[0].title, "a");
    assert_eq!(children[1].title, "b");
    assert_eq!(children[1].depends_on, vec![children[0].id]);
    assert_eq!(children[0].role.as_deref(), Some("implementer"));
    assert_eq!(
        children[0].worker_hint.tier,
        Tier::Cheap,
        "role default applied to the child"
    );
    for c in &children {
        assert_eq!(store.get(c.id).unwrap().unwrap().status, Status::Done);
    }

    let events = store.events_for(parent.id).unwrap();
    let delegated: Vec<Vec<TaskId>> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::Delegated { task_ids, .. } => Some(task_ids.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(delegated, vec![vec![children[0].id, children[1].id]]);
    let msgs = progress_msgs(&store, parent.id);
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("delegate rejected: tasks[2]") && m.contains("title")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("delegate rejected: tasks[3]")
                && m.contains("delegating task itself")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("waiting for ") && m.contains("delegated child task")),
        "{msgs:?}"
    );
    assert_eq!(
        transition_reasons(&store, parent.id),
        vec![
            "dispatch",
            "worker_done",
            "aggregate",
            "dispatch",
            "worker_done",
            "review_pass"
        ]
    );
    // 親の run は 2 回（最初 + 集約）。役割名が WorkerStarted に残り、指示文が RunContext に載る。
    let started: Vec<Option<String>> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerStarted {
                role: None,
                task_role,
                ..
            } => Some(task_role.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![Some("lead".to_string()), Some("lead".to_string())]
    );
    let role = adapter
        .seen_role
        .lock()
        .unwrap()
        .clone()
        .expect("role context");
    assert_eq!(role.id, "lead");
    assert_eq!(role.instructions, "You lead; delegate implementation.");
    assert_eq!(
        adapter.aggregate_children.load(Ordering::SeqCst),
        2,
        "aggregate run saw both children"
    );
    // 集約 run のレビューには暗黙の summary.md 条件（idx = acceptance.len()）が入る。
    assert!(events.iter().any(|(_, e)| matches!(e, Event::ReviewVerdict { criterion_idx: 1, pass: true, reason, .. } if reason.contains("summary.md"))), "{events:?}");
}

/// 受け入れ 2: 上限（1 run の件数・木の深さ・木の run 数）を超える提案は拒否され、理由が WorkerProgress に残り、親は失敗しない。
#[tokio::test]
async fn delegation_limits_reject_with_reasons_and_do_not_fail_the_run() {
    async fn run_with(
        limits: DelegationLimits,
        proposals: Vec<DelegateTask>,
        depth: u32,
    ) -> (Vec<String>, usize, Status) {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        // depth 個の祖先の下に親を置く（根 = 深さ 1）。
        let mut ancestor: Option<TaskId> = None;
        for _ in 1..depth {
            let mut a = new_task(
                dir.path(),
                Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
                0,
            );
            a.parent_id = ancestor;
            a.status = Status::Done;
            store.insert(&a).unwrap();
            ancestor = Some(a.id);
        }
        let mut parent = new_task(
            dir.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            0,
        );
        parent.parent_id = ancestor;
        store.insert(&parent).unwrap();
        let adapter = Arc::new(DelegatingAdapter {
            proposals,
            child_delay: Duration::from_millis(1),
            seen_role: std::sync::Mutex::new(None),
            aggregate_children: AtomicUsize::new(0),
            write_summary: false,
        });
        let mut d = dispatcher(store.clone(), adapter, 4);
        d.config.delegation = limits;
        let report = run_until_idle(&mut d, 400).await;
        assert!(report.idle);
        let p = store.get(parent.id).unwrap().unwrap();
        (
            progress_msgs(&store, parent.id),
            store.children(parent.id).unwrap().len(),
            p.status,
        )
    }

    // 1 run の件数: 2 件のうち 1 件だけ。
    let (msgs, n, status) = run_with(
        DelegationLimits {
            max_delegate_per_run: 1,
            ..DelegationLimits::default()
        },
        vec![proposal("a", vec![]), proposal("b", vec![])],
        1,
    )
    .await;
    assert_eq!(n, 1, "{msgs:?}");
    assert_eq!(status, Status::Done);
    assert!(
        msgs.iter()
            .any(|m| m.contains("delegate rejected: tasks[1]")
                && m.contains("per-run delegation limit (1)")),
        "{msgs:?}"
    );

    // 木の深さ: 深さ 2 の親は max_tree_depth = 2 で子を作れない。
    let (msgs, n, status) = run_with(
        DelegationLimits {
            max_tree_depth: 2,
            ..DelegationLimits::default()
        },
        vec![proposal("a", vec![])],
        2,
    )
    .await;
    assert_eq!(n, 0, "{msgs:?}");
    assert_eq!(status, Status::Done);
    assert!(
        msgs.iter()
            .any(|m| m.contains("delegate rejected")
                && m.contains("tree depth would become 3 (max 2)")),
        "{msgs:?}"
    );

    // 木の run 数: 親自身の run が 1 回目なので max_tree_runs = 1 で拒否。
    let (msgs, n, status) = run_with(
        DelegationLimits {
            max_tree_runs: 1,
            ..DelegationLimits::default()
        },
        vec![proposal("a", vec![])],
        1,
    )
    .await;
    assert_eq!(n, 0, "{msgs:?}");
    assert_eq!(status, Status::Done);
    assert!(
        msgs.iter()
            .any(|m| m.contains("delegate rejected") && m.contains("worker runs (max 1)")),
        "{msgs:?}"
    );
}

/// ADR-0072 §6 E3 (d): gate = on の compound task で最初の run が planner run になり、
/// `execution-plan.json` を検証して採用し（`Continue{planned}`）、WU を順に実行する。
#[tokio::test]
async fn gate_on_compound_task_runs_a_planner_then_the_planned_work_units_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    let valid_plan = plan_json(vec![wu_spec("a", &[]), wu_spec("b", &["a"])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(valid_plan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(decision.mode, task_core::ExecutionMode::Compound);
    assert_eq!(decision.source, task_core::GateSource::Human);

    let plan = store
        .execution_plan_active(task_id)
        .unwrap()
        .expect("plan adopted");
    assert_eq!(plan.origin, task_core::PlanOrigin::Planner);
    let units = store.work_units_for(task_id).unwrap();
    assert_eq!(units.len(), 2);
    assert!(
        units
            .iter()
            .all(|u| u.status == task_core::WorkUnitStatus::Done)
    );

    let runs = store.runs_for_task(task_id).unwrap();
    let planner_runs: Vec<&task_core::RunRow> = runs
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Planner)
        .collect();
    assert_eq!(planner_runs.len(), 1, "{runs:?}");
    assert_eq!(planner_runs[0].status, task_core::RunIndexStatus::Completed);
    assert!(planner_runs[0].work_unit_id.is_none());

    let events = store.events_for(task_id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::ExecutionGated { .. })),
        "{events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned {
                origin: task_core::PlanOrigin::Planner,
                ..
            }
        )),
        "{events:?}"
    );
    let planned_reasons = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "planned"))
        .count();
    assert_eq!(planned_reasons, 1, "{events:?}");

    // (g): planner run は node_sessions を resume しない（対話ではないので session は常に無い）。
    let planner_contexts: Vec<task_worker::RunContext> = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_some())
        .cloned()
        .collect();
    assert_eq!(planner_contexts.len(), 1);
    assert!(planner_contexts[0].session.is_none());

    // (h): WU の run は `RoutingDecided.work_unit_id` を持つ。
    let routing_records: Vec<task_core::RoutingRecord> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::RoutingDecided { record, .. } => Some((**record).clone()),
            _ => None,
        })
        .collect();
    let wu_routing = routing_records
        .iter()
        .filter(|r| r.work_unit_id.is_some())
        .count();
    assert_eq!(wu_routing, 2, "{routing_records:?}");
}

/// ADR-0072「Phase F6 実装時の決定」(a): shadow の下で atomic に走る判定を持つ起票済みの Task を、人が
/// `set_execution_mode(compound)`（`POST /tasks/{id}/execution/decompose` と同じ関数）で分解の経路に
/// 入れると、次の dispatch で gate が `human/explicit` として判定し直し（新しい `ExecutionGated`）、
/// その run が planner run になって計画を採用し、WU を実行して done になる（gate = shadow のまま）。
#[tokio::test]
async fn regate_of_an_atomic_task_starts_a_planner_run_and_yields_a_plan() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = shadow_gated_task(dir.path(), task_core::ExecutionMode::Atomic);
    let task_id = task.id;
    store.insert(&task).unwrap();
    let result = task_ops::regate::set_execution_mode(
        store.as_ref(),
        task_id,
        task_core::ExecutionMode::Compound,
        "human",
        Some("計画を作って分けて進めて".to_string()),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(!result.replan);
    assert!(result.task.routing.as_ref().unwrap().execution.is_none());

    let valid_plan = plan_json(vec![wu_spec("a", &[]), wu_spec("b", &["a"])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(valid_plan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::Shadow;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("re-gated");
    assert_eq!(decision.mode, task_core::ExecutionMode::Compound);
    assert_eq!(decision.source, task_core::GateSource::Human);
    assert_eq!(decision.rule_id, "human/explicit");
    let plan = store
        .execution_plan_active(task_id)
        .unwrap()
        .expect("plan adopted");
    assert_eq!(plan.origin, task_core::PlanOrigin::Planner);
    assert_eq!(store.work_units_for(task_id).unwrap().len(), 2);
    let runs = store.runs_for_task(task_id).unwrap();
    assert_eq!(
        runs.iter()
            .filter(|r| r.role == task_core::RunIndexRole::Planner)
            .count(),
        1,
        "{runs:?}"
    );
    // 監査: ExecutionHintSet（source human）の後に、新しい ExecutionGated（human/explicit）。
    let events = store.events_for(task_id).unwrap();
    let hint_at = events
        .iter()
        .position(|(_, e)| matches!(e, Event::ExecutionHintSet { source, .. } if source == "human"))
        .expect("hint set recorded");
    let gated_at = events
        .iter()
        .position(|(_, e)| matches!(e, Event::ExecutionGated { decision } if decision.rule_id == "human/explicit"))
        .expect("fresh gate decision");
    assert!(hint_at < gated_at, "{events:?}");
}

/// ADR-0072「Phase F6 実装時の決定」(b): 計画を持つ Task への compound の依頼は replan の依頼
/// （`ExecutionHintSet{replan: true}`）で、次の dispatch が replan の planner run になり、版が 2 になる。
/// 人の note は planner の「起こした理由」に渡る。
#[tokio::test]
async fn regate_of_a_planned_task_is_a_replan_request() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();
    task_ops::execution::adopt_plan(
        store.as_ref(),
        task_id,
        serde_json::from_str(&plan_json(vec![wu_spec("a", &[])])).unwrap(),
        task_core::PlanOrigin::Human,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let result = task_ops::regate::set_execution_mode(
        store.as_ref(),
        task_id,
        task_core::ExecutionMode::Compound,
        "mcp:chatgpt",
        Some("migration を先に".to_string()),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(result.replan);
    // 計画を持つ Task を atomic に戻すことはしない。
    let err = task_ops::regate::set_execution_mode(
        store.as_ref(),
        task_id,
        task_core::ExecutionMode::Atomic,
        "human",
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        matches!(err, task_ops::OpsError::InvalidState { .. }),
        "{err:?}"
    );

    let replan = plan_json(vec![wu_spec("m", &[]), wu_spec("a", &["m"])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(replan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let plan = store.execution_plan_active(task_id).unwrap().expect("plan");
    assert_eq!(plan.version, 2, "{plan:?}");
    let planner_contexts: Vec<task_worker::RunContext> = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_some())
        .cloned()
        .collect();
    assert_eq!(planner_contexts.len(), 1, "exactly one replan run");
    let ctx = planner_contexts[0].execution_planner.as_ref().unwrap();
    assert!(ctx.replan);
    assert!(
        ctx.replan_reason.contains("migration を先に") && ctx.replan_reason.contains("mcp:chatgpt"),
        "{:?}",
        ctx.replan_reason
    );
}

/// ADR-0072「Phase F6 実装時の決定」(c)（本番 2026-09-28 の 01M3MBV3… → 01M3MFS5…）: gate = shadow の下で
/// 判定された Task を中止し、gate = on に切り替えてから retry すると、複製先は元の判定（`shadow: true`）を
/// 写さず、最初の dispatch で今の設定で判定し直して、自分自身の `ExecutionGated`（`shadow: false`）を残す。
#[tokio::test]
async fn retry_after_switching_gate_to_on_regates_the_copy() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = shadow_gated_task(dir.path(), task_core::ExecutionMode::Compound);
    let task_id = task.id;
    store.insert(&task).unwrap();
    store
        .apply_transition(task_id, task_core::Trigger::Cancel, None)
        .unwrap();
    let retried = task_ops::retry::retry_task(
        store.as_ref(),
        task_id,
        true,
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let copy_id = retried.task_id;
    let copy = store.get(copy_id).unwrap().unwrap();
    assert!(copy.routing.as_ref().unwrap().execution.is_none());

    let valid_plan = plan_json(vec![wu_spec("a", &[])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(valid_plan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let copy = store.get(copy_id).unwrap().unwrap();
    let decision = copy
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("the copy is gated under the current config");
    assert!(!decision.shadow, "{decision:?}");
    let events = store.events_for(copy_id).unwrap();
    let gated: Vec<&task_core::ExecutionGateDecision> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ExecutionGated { decision } => Some(decision.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(gated.len(), 1, "a fresh gate event on the copy: {events:?}");
    assert!(!gated[0].shadow);
    // CoS のヒント（explicit=false）は引き継ぎ、規則表の判定に +2 として効く。
    assert_eq!(
        copy.routing.as_ref().unwrap().execution_hint,
        Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: false
        })
    );
    assert_eq!(gated[0].source, task_core::GateSource::Hint);
    assert_eq!(copy.status, Status::Done, "{copy:?}");
}

/// ADR-0074 D3.7（Phase F4b (f)）: execution-plan/2 の `children` は採用と同じトランザクションで既存の
/// 委譲の検証を通って子 Task（`parent_id` = この Task、`child-<key>` の印、ready）になり、
/// `Event::Delegated{run_id: <planner run>}` が残る。`depends_on: ["child:<key>"]` の WU は子が
/// `done` になるまで待ち（`child_done` で ready）、Task は最後に done になる。
#[tokio::test]
async fn planner_children_become_delegated_child_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
        rationale: "a child deliverable first".to_string(),
        phases: vec![task_core::PhaseSpec {
            key: "build".into(),
            kind: task_core::WorkUnitKind::Implement,
            title: "build".into(),
        }],
        work_units: vec![{
            let mut a = wu_spec("a", &["child:lit"]);
            a.phase = Some("build".into());
            a
        }],
        children: vec![task_core::ExecutionChildSpec {
            key: "lit".into(),
            title: "関連研究の調査".into(),
            objective: "別の deliverable として調べる".into(),
            acceptance: vec![task_core::Criterion {
                text: "ok".into(),
                check: Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            }],
            genre: None,
            skills: vec!["survey".into()],
            features: None,
            depends_on: vec![],
        }],
    };
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(serde_json::to_string(&spec).unwrap())],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 600).await;
    assert!(report.idle, "{report:?}");

    let children = store.children(task_id).unwrap();
    assert_eq!(children.len(), 1, "{children:?}");
    let child = &children[0];
    assert_eq!(child.parent_id, Some(task_id));
    assert!(child.labels.contains(&"child-lit".to_string()));
    assert_eq!(child.skills, vec!["survey".to_string()]);
    assert_eq!(
        child.assignee, None,
        "ADR-0069 D1: owner is decided by matching"
    );
    assert!(!task_core::is_root_task(child));
    assert_eq!(
        child.status,
        Status::Done,
        "{:?}",
        store.events_for(child.id).unwrap()
    );

    let events = store.events_for(task_id).unwrap();
    let planner_run = store
        .runs_for_task(task_id)
        .unwrap()
        .into_iter()
        .find(|r| r.role == task_core::RunIndexRole::Planner)
        .expect("planner run")
        .run_id;
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::Delegated { run_id, task_ids } if run_id == &planner_run && task_ids == &vec![child.id]
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkUnitTransitioned { key, reason, .. } if key == "a" && reason == "child_done"
        )),
        "{events:?}"
    );
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{events:?}");
}

/// ADR-0074「Phase F3（途中確認）実装時の逸脱・明確化」（0 区切り）: `gate = "shadow"` でも、
/// 人が `execution: compound` を明示した Task（`source = human`, `rule_id = human/explicit`）は
/// 採用され、planner run に進んで計画どおり WU を実行する（F5-1 dogfood で見つかった不具合の
/// 修正の確認）。
#[tokio::test]
async fn shadow_gate_adopts_a_human_explicit_compound_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    let valid_plan = plan_json(vec![wu_spec("a", &[]), wu_spec("b", &["a"])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(valid_plan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::Shadow;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(decision.mode, task_core::ExecutionMode::Compound);
    assert_eq!(decision.source, task_core::GateSource::Human);
    assert_eq!(decision.rule_id, "human/explicit");
    assert!(decision.shadow, "shadow flag should still be recorded");

    let plan = store
        .execution_plan_active(task_id)
        .unwrap()
        .expect("plan adopted even though gate = shadow");
    assert_eq!(plan.origin, task_core::PlanOrigin::Planner);
    let units = store.work_units_for(task_id).unwrap();
    assert_eq!(units.len(), 2);
    assert!(
        units
            .iter()
            .all(|u| u.status == task_core::WorkUnitStatus::Done)
    );
    let runs = store.runs_for_task(task_id).unwrap();
    let planner_runs = runs
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Planner)
        .count();
    assert_eq!(planner_runs, 1, "{runs:?}");
}

/// ADR-0074「Phase F3（途中確認）実装時の逸脱・明確化」（0 区切り）: `gate = "shadow"` では、
/// 規則表（score）が compound と判定しても（`source = policy`）、記録だけで採用されない
/// （atomic のまま実行され、計画は作られない）。
#[tokio::test]
async fn shadow_gate_does_not_adopt_a_rule_based_compound_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    // 強制規則 `atomic/small`（max_turns<=10 かつ目的が 400 文字未満）に当たらないようにする
    // （score による判定に純粋に依らせるため）。
    task.budget.max_turns = 40;
    // 人の明示は無いが、規則表のスコアが閾値（5）に達するように features を直接指定する
    // （F1 context_size=high(+2) + F2 expected_length=high(+2) + F3 tool_intensity=high(+1) = 5）。
    task.routing = Some(task_core::TaskRouting {
        features: Some(task_core::model_policy::TaskFeatureHints {
            context_size: Some(task_core::model_policy::Level::High),
            expected_length: Some(task_core::model_policy::Level::High),
            tool_intensity: Some(task_core::model_policy::Level::High),
            ..Default::default()
        }),
        ..Default::default()
    });
    let task_id = task.id;
    store.insert(&task).unwrap();

    // planner の adapter は呼ばれないはず（呼ばれたらテストが失敗する形にする）。
    let adapter = Arc::new(PlannerScriptAdapter::new(vec![None], HashMap::new()));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::Shadow;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(decision.mode, task_core::ExecutionMode::Compound);
    assert_eq!(decision.source, task_core::GateSource::Policy);
    assert!(decision.shadow, "shadow flag should be recorded");

    // shadow では採用しないので、計画は作られず atomic のまま 1 本の run で終わる。
    assert!(store.execution_plan_active(task_id).unwrap().is_none());
    let runs = store.runs_for_task(task_id).unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(runs[0].work_unit_id.is_none());
    assert_eq!(runs[0].role, task_core::RunIndexRole::Worker);
    assert!(
        adapter
            .seen
            .lock()
            .unwrap()
            .iter()
            .all(|c| c.execution_planner.is_none()),
        "no run should be dispatched as a planner run in shadow mode for a policy-sourced decision"
    );
}

/// ADR-0072 §6 E3 (e)(f): planner の出力が不正なら 1 回だけ再試行し、それでも不正なら
/// Task を失敗させずに atomic に倒す。`assignee`/`tier` を書いた出力は schema 違反
/// （`deny_unknown_fields`）として同じ経路で拒否される。
#[tokio::test]
async fn invalid_planner_output_retries_once_then_falls_back_to_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    // 1 回目: `assignee` を書いた不正な出力（schema 違反、(f)）。2 回目: ファイル自体を書かない。
    let bad_plan = r#"{"schema":"celeris.execution-plan/1","rationale":"r","work_units":[
        {"key":"a","kind":"implement","title":"t","objective":"o","depends_on":[],
         "done_when":[],"checks":[],
         "context":{"paths":[],"from_work_units":[],"knowledge":[]},"outputs":[],
         "assignee":"someone"}
    ]}"#
    .to_string();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(bad_plan), None],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Done,
        "an invalid plan must not fail the task: {stored:?}"
    );
    assert_eq!(
        stored.attempts, 0,
        "planner retries must not consume Task.attempts"
    );
    assert!(
        store.execution_plan_active(task_id).unwrap().is_none(),
        "no plan should have been adopted"
    );
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(
        decision.mode,
        task_core::ExecutionMode::Atomic,
        "falls back to atomic after exhausting retries: {decision:?}"
    );
    assert_eq!(decision.rule_id, "atomic/planner-invalid");

    let planner_attempts = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_some())
        .count();
    assert_eq!(planner_attempts, 2, "exactly one retry (2 attempts total)");

    let events = store.events_for(task_id).unwrap();
    let gated = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::ExecutionGated { .. }))
        .count();
    assert_eq!(
        gated, 2,
        "the original decision plus the atomic fallback: {events:?}"
    );
}

/// Phase F5-fix3（dogfood 4 回目の不具合 1）: planner は検証と同じ `ExecutionLimits`（config の値）を
/// プロンプトで受け取り、計画が拒否されたら次の試行はその検証エラーを受け取る。拒否された計画の
/// ファイルは `execution-plan.rejected.json` に移る（次の試行が「検証済み」と思い込んで再提出しない）。
#[tokio::test]
async fn the_retry_planner_run_receives_the_previous_validation_error_and_config_limits() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    let mut wu = wu_spec("sync-main", &[]);
    wu.checks = (0..4)
        .map(|i| task_core::WorkUnitCheck {
            cmd: format!("true {i}"),
            expect_exit: 0,
        })
        .collect();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(plan_json(vec![wu])), None],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    // 既定（6）と違う上限。検証とプロンプトの両方がこの値を使う。
    d.config.execution.limits.max_checks = 3;
    d.config.execution.limits.max_title_chars = 77;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let planner_contexts: Vec<task_worker::protocol::ExecutionPlannerContext> = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| c.execution_planner.clone())
        .collect();
    assert_eq!(planner_contexts.len(), 2, "{planner_contexts:?}");
    assert_eq!(planner_contexts[0].max_checks, 3);
    assert_eq!(planner_contexts[0].max_title_chars, 77);
    assert!(planner_contexts[0].previous_attempt_errors.is_empty());
    assert_eq!(
        planner_contexts[1].previous_attempt_errors,
        vec!["work unit sync-main: too many checks: 4 > 3".to_string()],
    );
    let events = store.events_for(task_id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerFinished { outcome, .. }
            if outcome == "error(retryable=true): invalid execution plan: work unit sync-main: too many checks: 4 > 3"
    )));
    // 拒否された計画は移され、2 回目（ファイルを書かない）は古い計画を読まずに「見つからない」になる。
    let stored = store.get(task_id).unwrap().unwrap();
    let artifacts = d
        .task_dir(&stored)
        .map(|w| d.artifacts_dir(&stored, &w))
        .expect("artifacts dir");
    assert!(artifacts.join(REJECTED_PLAN_FILE).exists());
    assert!(!artifacts.join("execution-plan.json").exists());
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerProgress { msg, .. } if msg.contains("execution-plan.json が見つからない")
    )));
}

/// Phase F5-fix3: `planner_rejections_since_last_plan` は直近の `ExecutionPlanned` より後の拒否だけを
/// 古い順・重複なしで返す（再試行の進捗と、2 回とも拒否されたときの質問の両方）。
#[test]
fn planner_rejections_are_collected_since_the_last_adopted_plan() {
    let progress = |msg: String| Event::worker_progress("run-p", msg);
    let events: Vec<(u64, Event)> = vec![
        (0, progress(planner_retry_message("old reason"))),
        (
            1,
            Event::ExecutionPlanned {
                plan_id: "p1".into(),
                version: 1,
                origin: task_core::PlanOrigin::Planner,
                supersedes: None,
                reason: None,
                plan: Box::new(task_core::ExecutionPlanSpec {
                    stages: Vec::new(),
                    units: Vec::new(),
                    decisions: Vec::new(),
                    schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
                    rationale: String::new(),
                    work_units: Vec::new(),
                    phases: Vec::new(),
                    children: Vec::new(),
                }),
            },
        ),
        (
            2,
            progress(planner_retry_message("work unit x: too many checks: 8 > 6")),
        ),
        (
            3,
            progress("計画を採用できませんでした（unrelated".to_string()),
        ),
        (
            4,
            progress(planner_blocked_question(
                "work unit x: too many checks: 8 > 6",
            )),
        ),
        (
            5,
            progress(planner_blocked_question(
                "rationale is too long: 2000 > 1500 characters",
            )),
        ),
    ];
    assert_eq!(
        planner_rejections_since_last_plan(&events),
        vec![
            "work unit x: too many checks: 8 > 6".to_string(),
            "rationale is too long: 2000 > 1500 characters".to_string(),
        ]
    );
    assert!(planner_rejections_since_last_plan(&events[..2]).is_empty());
}

/// ADR-0074 §6 F1 (b): WU の `features` が `TaskFeatureHints` として読めない計画（未知の欄
/// `"lane"` を書いた）は検証エラーになり、1 回だけ再試行してそれでも駄目なら atomic に倒れる
/// （`invalid_planner_output_retries_once_then_falls_back_to_atomic` と同じ経路）。
#[tokio::test]
async fn plan_with_unreadable_features_retries_once_then_falls_back_to_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    // `features.lane` は `TaskFeatureHints` に無い欄（`deny_unknown_fields`）。
    let bad_plan = r#"{"schema":"celeris.execution-plan/1","rationale":"r","work_units":[
        {"key":"a","kind":"implement","title":"t","objective":"o","depends_on":[],
         "done_when":[],"checks":[],
         "context":{"paths":[],"from_work_units":[],"knowledge":[]},"outputs":[],
         "features":{"lane":"frontier"}}
    ]}"#
    .to_string();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(bad_plan), None],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Done,
        "an invalid plan must not fail the task: {stored:?}"
    );
    assert!(
        store.execution_plan_active(task_id).unwrap().is_none(),
        "no plan should have been adopted"
    );
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(
        decision.mode,
        task_core::ExecutionMode::Atomic,
        "{decision:?}"
    );
    let planner_attempts = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_some())
        .count();
    assert_eq!(planner_attempts, 2, "exactly one retry (2 attempts total)");
}

/// ADR-0074 §6 F1 (e): 計画のサイズ上限（`rationale` ≤ 1,500 文字）を超えた出力は拒否され、
/// 1 回だけ再試行してそれでも駄目なら atomic に倒れる。
#[tokio::test]
async fn oversized_plan_retries_once_then_falls_back_to_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    let too_long_rationale = "x".repeat(1_501);
    let bad_plan = serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA,
        "rationale": too_long_rationale,
        "work_units": [wu_spec("a", &[])],
    })
    .to_string();
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(bad_plan), None],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Done,
        "an invalid plan must not fail the task: {stored:?}"
    );
    assert!(store.execution_plan_active(task_id).unwrap().is_none());
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(
        decision.mode,
        task_core::ExecutionMode::Atomic,
        "{decision:?}"
    );
    let planner_attempts = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.execution_planner.is_some())
        .count();
    assert_eq!(planner_attempts, 2, "exactly one retry (2 attempts total)");
}

/// ADR-0074 §6 F1 (d): 人が Task に `tier:frontier` を明示していれば、planner run も frontier で
/// 走る（`[execution.planner] tier` の既定 standard より優先）。
#[tokio::test]
async fn planner_run_uses_frontier_when_the_task_explicitly_sets_it() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(dir.path());
    task.worker_hint.tier = Tier::Frontier;
    if let Some(routing) = &mut task.routing {
        routing.tier_source = task_core::TierSource::Human;
    }
    let task_id = task.id;
    store.insert(&task).unwrap();

    let valid_plan = plan_json(vec![wu_spec("a", &[])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(valid_plan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let events = store.events_for(task_id).unwrap();
    let planner_record = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::RoutingDecided { record, .. }
                if record.decision.rule_id.starts_with("planner/") =>
            {
                Some((**record).clone())
            }
            _ => None,
        })
        .expect("a planner RoutingDecided event");
    assert_eq!(
        planner_record.decision.lane,
        Tier::Frontier,
        "{planner_record:?}"
    );
    assert_eq!(planner_record.decision.rule_id, "planner/human-frontier");
    assert_eq!(planner_record.decision.source, task_core::TierSource::Human);
}

// ========== ADR-0072（Phase E4）: reviewer repair ==========

#[tokio::test]
async fn reopened_delivery_task_dispatches_only_its_ready_repair_unit() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let main_spec = wu_spec("main", &[]);
    let plan = task_ops::execution::adopt_plan(
        store.as_ref(),
        task_id,
        task_core::ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: task_core::EXECUTION_PLAN_SCHEMA.into(),
            rationale: "initial implementation".into(),
            work_units: vec![main_spec],
            phases: Vec::new(),
            children: Vec::new(),
        },
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let main = store.work_units_for(task_id).unwrap().remove(0);
    let mut done_main = main.clone();
    done_main.status = task_core::WorkUnitStatus::Done;
    store
        .work_unit_transition(
            task_id,
            done_main,
            Event::WorkUnitTransitioned {
                work_unit_id: main.id.clone(),
                key: main.key.clone(),
                from: task_core::WorkUnitStatus::Ready,
                to: task_core::WorkUnitStatus::Done,
                reason: "initial_implementation_done".into(),
                run_id: None,
            },
        )
        .unwrap();
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        store.apply_transition(task_id, trigger, None).unwrap();
    }

    let repair_spec = task_core::WorkUnitSpec {
        key: "repair-1".into(),
        kind: task_core::WorkUnitKind::Repair,
        title: "repair (merge_base): delivery".into(),
        objective: "Rebase feature on main".into(),
        ..wu_spec("repair-1", &[])
    };
    let repair = task_core::WorkUnitRow::new(
        task_core::new_id(),
        task_id.to_string(),
        plan.id,
        1,
        repair_spec,
        task_core::WorkUnitStatus::Ready,
        OffsetDateTime::now_utc().to_string(),
    );
    store
        .delivery_repair_apply(
            task_id,
            vec![Event::WorkUnitTransitioned {
                work_unit_id: repair.id.clone(),
                key: repair.key.clone(),
                from: task_core::WorkUnitStatus::Pending,
                to: task_core::WorkUnitStatus::Ready,
                reason: "delivery_repair".into(),
                run_id: None,
            }],
            None,
            vec![repair.clone()],
        )
        .unwrap();
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Ready);
    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut dispatcher = dispatcher(store.clone(), adapter.clone(), 1);
    let report = run_until_idle(&mut dispatcher, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        store.work_units_for(task_id).unwrap()[0].status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(
        store.work_units_for(task_id).unwrap()[1].status,
        task_core::WorkUnitStatus::Done
    );
    let seen: Vec<_> = adapter
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|(key, _)| key.clone())
        .collect();
    assert_eq!(seen, vec!["repair-1"], "main WU must not run again");
}

/// ADR-0131 付記（2026-10-04、Complexity Gate 例外）: 本番 task 01M448KR2GJ8RJ4NKHGKPWZKMR のように
/// objective が長い cron 発火の knowledge-curation task は、強制規則（long-and-broad）も満たすほど
/// 複雑に見えても Complexity Gate で分割されず、常に atomic の 1 run（偽アダプタ）で dispatch される。
/// 理由（rule_id）は `Event::ExecutionGated` に残る。
#[tokio::test]
async fn knowledge_curation_cron_task_skips_the_gate_and_runs_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    task.genre = Some(task_core::cron::KNOWLEDGE_CURATION_HARNESS.to_string());
    task.labels = vec![task_core::cron::CRON_TASK_LABEL.to_string()];
    task.budget.max_turns = 60;
    // 本番の全体整理 task を模した長い objective（強制規則 atomic/small には当たらない）。
    task.objective = "知識ベースの全体整理\n".repeat(400);
    task.routing = Some(task_core::TaskRouting {
        // 強制規則 compound/long-and-broad も満たすほど複雑に見える特徴を乗せる。
        features: Some(task_core::model_policy::TaskFeatureHints {
            context_size: Some(task_core::model_policy::Level::High),
            expected_length: Some(task_core::model_policy::Level::High),
            cross_cutting: Some(task_core::model_policy::Level::High),
            tool_intensity: Some(task_core::model_policy::Level::High),
            ..Default::default()
        }),
        ..Default::default()
    });
    let task_id = task.id;
    store.insert(&task).unwrap();

    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.execution.gate = task_core::GateMode::On;
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let decision = stored
        .routing
        .as_ref()
        .and_then(|r| r.execution.as_ref())
        .expect("gate decision recorded");
    assert_eq!(decision.mode, task_core::ExecutionMode::Atomic);
    assert_eq!(decision.rule_id, "atomic/knowledge-curation");
    assert!(!decision.shadow);

    // planner run も計画も無い: atomic の 1 run だけで done になった。
    assert!(store.execution_plan_active(task_id).unwrap().is_none());
    assert!(store.work_units_for(task_id).unwrap().is_empty());
    let runs = store.runs_for_task(task_id).unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(
        runs.iter()
            .all(|r| r.role != task_core::RunIndexRole::Planner),
        "{runs:?}"
    );

    // 理由（rule_id）が event に残る。
    let events = store.events_for(task_id).unwrap();
    let gated: Vec<&task_core::ExecutionGateDecision> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ExecutionGated { decision } => Some(decision.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(gated.len(), 1, "{events:?}");
    assert_eq!(gated[0].rule_id, "atomic/knowledge-curation");
}
