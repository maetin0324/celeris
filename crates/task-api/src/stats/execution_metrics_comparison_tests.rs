use super::*;
use task_core::{
    Budget, Check, Criterion, RunIndexRole, RunIndexStatus, RunRow, SqliteStore, TaskId, TaskKind,
    WorkerHint, WorkspaceSpec,
};

fn task(status: Status, genre: &str, age: i64) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc() - time::Duration::seconds(age);
    Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: genre.into(),
        objective: "exercise metrics".into(),
        acceptance: vec![Criterion {
            text: "done".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local(id.to_string()),
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
        genre: Some(genre.into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: Some("engineering".into()),
        conversation: None,
        labels: vec![],
        category: Default::default(),
        mode: Default::default(),
        skills: vec![],
        repos: vec![],
        routing: None,
    }
}

fn started(id: &str) -> Event {
    Event::WorkerStarted {
        run_id: id.into(),
        adapter: "codex".into(),
        model: "gpt-6-sol".into(),
        provider: None,
        account: None,
        role: None,
        task_role: None,
    }
}

fn run(store: &SqliteStore, task: &Task, id: &str, status: RunIndexStatus) {
    let at = task.updated_at.format(&Rfc3339).unwrap();
    store
        .run_index_start(RunRow {
            run_id: id.into(),
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
            started_at: at,
            finished_at: None,
        })
        .unwrap();
    store
        .run_index_finish(id, status, None, None, None, task.updated_at)
        .unwrap();
}

fn fixture() -> SqliteStore {
    let store = SqliteStore::open_in_memory().unwrap();
    let atomic_done = task(Status::Done, "atomic-done", 120);
    store.create_task(&atomic_done, vec![]).unwrap();
    let atomic_failed = task(Status::Failed, "atomic-failed", 110);
    store.create_task(&atomic_failed, vec![]).unwrap();

    let plan: task_core::ExecutionPlanSpec = serde_json::from_value(serde_json::json!({
        "schema": "celeris.execution-plan/1", "rationale": "test",
        "work_units": [{"key":"a","kind":"implement","title":"A","objective":"do A"}]
    }))
    .unwrap();
    let planned = task(Status::Running, "compound", 100);
    store.create_task(&planned, vec![]).unwrap();
    task_ops::execution::adopt_plan(
        &store,
        planned.id,
        plan.clone(),
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let mut unit = store.work_units_for(planned.id).unwrap().remove(0);
    let from = unit.status;
    unit.status = task_core::WorkUnitStatus::Done;
    store
        .work_unit_transition(
            planned.id,
            unit.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: unit.id,
                key: unit.key,
                from,
                to: task_core::WorkUnitStatus::Done,
                reason: "completed".into(),
                run_id: None,
            },
        )
        .unwrap();

    let repair = task(Status::Reviewing, "repair", 90);
    store.create_task(&repair, vec![]).unwrap();
    let repair_plan: task_core::ExecutionPlanSpec = serde_json::from_value(serde_json::json!({
            "schema": "celeris.execution-plan/1", "rationale": "repair",
            "work_units": [{"key":"repair-1","kind":"repair","title":"repair (format): fix","objective":"fix"}]
        })).unwrap();
    task_ops::execution::adopt_plan(
        &store,
        repair.id,
        repair_plan,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    // review_repair の WorkUnitTransitioned でも同じ repair key を数える。
    let repair_unit = store.work_units_for(repair.id).unwrap().remove(0);
    store
        .work_unit_transition(
            repair.id,
            repair_unit.clone(),
            Event::WorkUnitTransitioned {
                work_unit_id: repair_unit.id,
                key: repair_unit.key,
                from: repair_unit.status,
                to: repair_unit.status,
                reason: "review_repair".into(),
                run_id: None,
            },
        )
        .unwrap();

    let replanned = task(Status::Running, "replan", 80);
    store.create_task(&replanned, vec![]).unwrap();
    task_ops::execution::adopt_plan(
        &store,
        replanned.id,
        plan.clone(),
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    task_ops::execution::replan(
        &store,
        replanned.id,
        plan,
        "retry plan".into(),
        task_core::PlanOrigin::Planner,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let mut continued = task(Status::Ready, "continue", 70);
    continued.routing = Some(task_core::TaskRouting::default());
    let decision =
        task_core::decide_for_task(&continued, &task_core::LaneCeiling::default()).unwrap();
    store
        .create_task(
            &continued,
            vec![
                started("continued-run"),
                Event::RoutingDecided {
                    run_id: "continued-run".into(),
                    record: Box::new(task_core::RoutingRecord {
                        org_node: Some("engineering".into()),
                        harness: Some("coding".into()),
                        decision,
                        resolution: task_core::model_routing::LaneResolution {
                            lane: Some(Tier::Standard),
                            ..Default::default()
                        },
                        quota_reason: None,
                        work_unit_id: None,
                    }),
                },
                Event::WorkerFinished {
                    run_id: "continued-run".into(),
                    outcome: "continue: budget".into(),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: Some(task_core::RunEnd::BudgetExhausted {
                        kind: task_core::BudgetKind::Turns,
                    }),
                },
                Event::Transitioned {
                    from: Status::Running,
                    to: Status::Ready,
                    reason: "continue".into(),
                },
            ],
        )
        .unwrap();
    // 古い索引では status が Failed でも events の end が BudgetExhausted になり得る。
    run(&store, &continued, "continued-run", RunIndexStatus::Failed);

    let yielded = task(Status::Ready, "yielded", 65);
    store
        .create_task(
            &yielded,
            vec![
                started("yielded-run"),
                Event::WorkerFinished {
                    run_id: "yielded-run".into(),
                    outcome: "continue: yielded".into(),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: Some(task_core::RunEnd::Yielded),
                },
                Event::Transitioned {
                    from: Status::Running,
                    to: Status::Ready,
                    reason: "continue".into(),
                },
            ],
        )
        .unwrap();
    run(&store, &yielded, "yielded-run", RunIndexStatus::Completed);

    let retried = task(Status::Ready, "retry", 60);
    store
        .create_task(
            &retried,
            vec![Event::Transitioned {
                from: Status::Running,
                to: Status::Ready,
                reason: "work_unit_retry".into(),
            }],
        )
        .unwrap();
    store
}

#[test]
fn indexed_summary_matches_event_reference_for_all_groups_and_since() {
    let store = fixture();
    let since = OffsetDateTime::now_utc() - time::Duration::seconds(95);
    for since in [None, Some(since)] {
        for group in EXECUTION_METRICS_GROUP_BY {
            let indexed = execution_metrics_summary(&store, since, group).unwrap();
            let events = execution_metrics_summary_from_events(&store, since, group).unwrap();
            assert_eq!(indexed, events, "since={since:?}, group={group}");
        }
    }
}

#[test]
#[ignore = "manual timing evidence; no latency threshold"]
fn indexed_summary_timing_2000_tasks_20_events() {
    let store = SqliteStore::open_in_memory().unwrap();
    for i in 0..2000 {
        let task = task(Status::Done, "timing", 0);
        let run_id = format!("timing-{i}");
        let mut events = vec![started(&run_id)];
        // create_task が Created を 1 件付けるので、合計 20 events。
        for _ in 0..18 {
            events.push(Event::Transitioned {
                from: Status::Running,
                to: Status::Running,
                reason: "progress".into(),
            });
        }
        store.create_task(&task, events).unwrap();
        run(&store, &task, &run_id, RunIndexStatus::Completed);
    }
    let start = std::time::Instant::now();
    let indexed = execution_metrics_summary(&store, None, "genre").unwrap();
    let indexed_time = start.elapsed();
    let start = std::time::Instant::now();
    let events = execution_metrics_summary_from_events(&store, None, "genre").unwrap();
    let event_time = start.elapsed();
    assert_eq!(indexed, events);
    println!("indexed={indexed_time:?} events={event_time:?}");
}

#[test]
fn continuation_metrics_api_groups_task_and_work_unit_runs() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = task(Status::Done, "continuation", 0);
    store.create_task(&task, vec![]).unwrap();
    let run_id = "continuation-api-run";
    let wu_id = "wu-1";
    let at = task.updated_at.format(&Rfc3339).unwrap();
    let usage = task_core::Usage {
        input_tokens: Some(8),
        cache_read_tokens: Some(2),
        duplicate_reads: Some(3),
        session_resumed: Some(true),
        ..Default::default()
    };
    let metrics = task_core::RunMetrics {
        wall_ms: 42,
        ..Default::default()
    };
    store
        .run_index_start(RunRow {
            run_id: run_id.into(),
            task_id: task.id.to_string(),
            work_unit_id: Some(wu_id.into()),
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: Some("claude-code".into()),
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: at,
            finished_at: None,
        })
        .unwrap();
    store
        .run_index_finish(
            run_id,
            RunIndexStatus::Completed,
            None,
            Some(usage),
            Some(metrics),
            task.updated_at,
        )
        .unwrap();
    let summary = execution_metrics_summary(&store, None, "genre").unwrap();
    assert_eq!(summary.continuation.resumed.runs, 1);
    assert_eq!(summary.continuation.resumed.wall_ms, 42);
    assert_eq!(
        summary.continuation_by_work_unit[wu_id]
            .resumed
            .input_tokens,
        10
    );
    assert_eq!(summary.groups[0].continuation.resumed.duplicate_reads, 3);
}
