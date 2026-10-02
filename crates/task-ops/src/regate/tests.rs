use super::*;
use task_core::{SqliteStore, TaskKind, Trigger};

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("open")
}

fn spec(title: &str) -> crate::add::NewTaskSpec {
    crate::add::NewTaskSpec {
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        title: title.to_string(),
        objective: "do it".to_string(),
        acceptance: vec![crate::add::CriterionSpec::ArtifactExists {
            name: "result.md".to_string(),
        }],
        kind: TaskKind::Execute,
        tier: None,
        priority: Some(crate::add::PriorityInput::Number(0)),
        parent: None,
        depends_on: vec![],
        max_turns: None,
        max_wall_secs: None,
        max_retries: 2,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        workspace: None,
        cluster: None,
        workspace_mode: None,
        adapter: None,
        labels: Vec::new(),
        category: None,
        status: None,
        features: None,
        execution: None,
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: crate::add::SpecProvenance::default(),
    }
}

fn gated_atomic(store: &SqliteStore) -> Task {
    let task =
        crate::add::create_task(store, spec("t"), OffsetDateTime::now_utc()).expect("create");
    store
        .apply_transition(task.id, Trigger::Accept, None)
        .expect("accept");
    let mut t = store.get(task.id).expect("get").expect("some");
    let mut routing = t.routing.clone().unwrap_or_default();
    routing.execution_hint = Some(ExecutionHintSpec {
        mode: ExecutionMode::Compound,
        explicit: false,
    });
    routing.execution = Some(task_core::ExecutionGateDecision {
        mode: ExecutionMode::Compound,
        source: task_core::GateSource::Hint,
        score: 9,
        threshold: 5,
        rule_id: "compound/score".to_string(),
        signals: Vec::new(),
        policy_version: "exec-gate/1".to_string(),
        shadow: true,
        depth: None,
    });
    t.routing = Some(routing.clone());
    store
        .update_task(
            &t,
            Event::ExecutionGated {
                decision: Box::new(routing.execution.clone().expect("decision")),
            },
        )
        .expect("gate")
}

/// R5b-fix3 (D3 d): 人の計画の採用の後、前の `atomic/small` の判定は人の compound（`human/plan`）に
/// 置き換わり、`ExecutionGated` が残る。2 回目は何もしない。
#[test]
fn human_plan_replaces_an_atomic_gate_record_with_human_compound() {
    let store = store();
    let task = gated_atomic(&store);
    let mut t = store.get(task.id).expect("get").expect("some");
    let mut routing = t.routing.clone().expect("routing");
    if let Some(d) = routing.execution.as_mut() {
        d.mode = ExecutionMode::Atomic;
        d.rule_id = "atomic/small".to_string();
        d.source = task_core::GateSource::Policy;
    }
    t.routing = Some(routing);
    let t = store
        .update_task(
            &t,
            Event::ExecutionGated {
                decision: Box::new(
                    t.routing
                        .as_ref()
                        .and_then(|r| r.execution.clone())
                        .expect("decision"),
                ),
            },
        )
        .expect("regate");
    let updated = record_human_plan_gate(&store, t.id, OffsetDateTime::now_utc())
        .expect("record")
        .expect("changed");
    let routing = updated.routing.expect("routing");
    let d = routing.execution.expect("decision");
    assert_eq!(d.mode, ExecutionMode::Compound);
    assert_eq!(d.source, task_core::GateSource::Human);
    assert_eq!(d.rule_id, HUMAN_PLAN_RULE_ID);
    assert!(!d.shadow);
    assert_eq!(
        routing.execution_hint,
        Some(ExecutionHintSpec {
            mode: ExecutionMode::Compound,
            explicit: true
        })
    );
    let events = store.events_for(t.id).expect("events");
    assert!(matches!(
        events.last().map(|(_, e)| e),
        Some(Event::ExecutionGated { decision }) if decision.rule_id == HUMAN_PLAN_RULE_ID
    ));
    assert!(
        record_human_plan_gate(&store, t.id, OffsetDateTime::now_utc())
            .expect("again")
            .is_none()
    );
}

/// ADR-0124 D2: 人が明示の経路（`decompose`）を指示したら、前に記録した直行の判定
/// （`routing.route`）も gate の判定と同時に消える（次の dispatch で `human/explicit` として
/// evaluate し直す）。
#[test]
fn direct_route_decision_is_cleared_alongside_the_gate_record_on_decompose() {
    let store = store();
    let task = gated_atomic(&store);
    let mut t = store.get(task.id).expect("get").expect("some");
    let mut routing = t.routing.clone().expect("routing");
    let decision = task_core::RouteDecision {
        route: task_core::Route::Direct,
        reasons: vec![],
        gate_rule_id: "compound/score".to_string(),
        overrode_gate: true,
        shadow: true,
        policy_version: task_core::DIRECT_ROUTE_POLICY_VERSION.to_string(),
    };
    routing.route = Some(decision.clone());
    t.routing = Some(routing);
    store
        .update_task(
            &t,
            Event::ExecutionRouted {
                decision: Box::new(decision),
            },
        )
        .expect("persist route");

    let r = set_execution_mode(
        &store,
        task.id,
        ExecutionMode::Compound,
        "human",
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("set");
    let routing = r.task.routing.expect("routing");
    assert!(routing.route.is_none(), "the old route decision is cleared");
}

#[test]
fn compound_sets_an_explicit_hint_clears_the_decision_and_records_the_source() {
    let store = store();
    let task = gated_atomic(&store);
    let r = set_execution_mode(
        &store,
        task.id,
        ExecutionMode::Compound,
        "mcp:chatgpt",
        Some("  分けて進めて  ".to_string()),
        OffsetDateTime::now_utc(),
    )
    .expect("set");
    assert!(!r.replan);
    let routing = r.task.routing.expect("routing");
    assert_eq!(
        routing.execution_hint,
        Some(ExecutionHintSpec {
            mode: ExecutionMode::Compound,
            explicit: true
        })
    );
    assert!(routing.execution.is_none(), "the old decision is cleared");
    assert_eq!(
        r.previous_decision.map(|d| d.rule_id),
        Some("compound/score".to_string())
    );
    let events = store.events_for(task.id).expect("events");
    let last = events.last().map(|(_, e)| e.clone());
    match last {
        Some(Event::ExecutionHintSet {
            mode,
            previous,
            source,
            note,
            replan,
            previous_decision,
        }) => {
            assert_eq!(mode, ExecutionMode::Compound);
            assert_eq!(previous.map(|p| p.explicit), Some(false));
            assert_eq!(source, "mcp:chatgpt");
            assert_eq!(note.as_deref(), Some("分けて進めて"));
            assert!(!replan);
            assert!(previous_decision.is_some_and(|d| d.shadow));
        }
        other => panic!("unexpected last event {other:?}"),
    }
    assert_eq!(pending_replan_request(&events), None);
}

#[test]
fn running_and_terminal_tasks_are_refused() {
    let store = store();
    let task = gated_atomic(&store);
    store
        .apply_transition(task.id, Trigger::Dispatch, None)
        .expect("dispatch");
    let err = set_execution_mode(
        &store,
        task.id,
        ExecutionMode::Compound,
        "human",
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::InvalidState { .. }), "{err:?}");
    store
        .apply_transition(task.id, Trigger::WorkerError { retryable: false }, None)
        .expect("fail");
    let err = set_execution_mode(
        &store,
        task.id,
        ExecutionMode::Compound,
        "human",
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("retry"),
        "a terminal task points at retry: {err}"
    );
}

#[test]
fn out_of_scope_tasks_and_long_notes_are_validation_errors() {
    let store = store();
    // `routing` の無い旧タスク（Phase 114 より前）は gate の対象外。
    let legacy =
        crate::add::create_task(&store, spec("legacy"), OffsetDateTime::now_utc()).expect("create");
    let mut legacy_task = store.get(legacy.id).expect("get").expect("some");
    legacy_task.routing = None;
    store
        .update_task(
            &legacy_task,
            Event::Edited {
                fields: vec!["routing".into()],
                by: "test".into(),
            },
        )
        .expect("update");
    let err = set_execution_mode(
        &store,
        legacy.id,
        ExecutionMode::Compound,
        "human",
        None,
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    let task = gated_atomic(&store);
    let err = set_execution_mode(
        &store,
        task.id,
        ExecutionMode::Compound,
        "human",
        Some("x".repeat(NOTE_MAX_CHARS + 1)),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
}

#[test]
fn a_replan_request_is_pending_until_a_plan_or_a_run_follows() {
    let hint = |replan| Event::ExecutionHintSet {
        mode: ExecutionMode::Compound,
        previous: None,
        previous_decision: None,
        source: "human".to_string(),
        note: Some("migration を先に".to_string()),
        replan,
    };
    let running = Event::Transitioned {
        from: Status::Ready,
        to: Status::Running,
        reason: "dispatch".to_string(),
    };
    assert_eq!(
        pending_replan_request(&[(1, hint(true))]).as_deref(),
        Some("migration を先に")
    );
    assert_eq!(pending_replan_request(&[(1, hint(false))]), None);
    assert_eq!(
        pending_replan_request(&[(1, hint(true)), (2, running)]),
        None
    );
}
