use super::*;
use task_core::{Budget, Check, Criterion, SqliteStore, TaskKind, Tier, WorkerHint, WorkspaceSpec};
use time::OffsetDateTime;

fn blocked_task(store: &SqliteStore, awaiting: bool) -> TaskId {
    let now = OffsetDateTime::now_utc();
    let task = Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".to_string(),
        objective: "o".to_string(),
        acceptance: vec![Criterion {
            text: "ok".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: Vec::new(),
        depends_on: vec![],
        status: Status::Ready,
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
    };
    store.insert(&task).unwrap();
    store
        .apply_transition(task.id, Trigger::Dispatch, None)
        .unwrap();
    if awaiting {
        store
            .apply_transition_with_events(
                task.id,
                Trigger::PhaseGate {
                    phase: "design".to_string(),
                },
                vec![Event::PhaseReported {
                    phase: "design".to_string(),
                    report: Box::new(PhaseReport {
                        phase: "design".to_string(),
                        phase_title: "Design".to_string(),
                        next_phase: Some("build".to_string()),
                        ..Default::default()
                    }),
                }],
            )
            .unwrap();
    } else {
        store
            .apply_transition(task.id, Trigger::WorkerQuestion, None)
            .unwrap();
    }
    task.id
}

#[test]
fn continue_resumes_and_keeps_the_note_as_an_answer() {
    let store = SqliteStore::open_in_memory().unwrap();
    let id = blocked_task(&store, true);
    let r = phase_gate(
        &store,
        id,
        PhaseGateAction::Continue,
        Some("  keep it small ".to_string()),
    )
    .unwrap();
    assert_eq!(r.to, Status::Ready);
    assert_eq!(r.reason, "phase_continue");
    let events = store.events_for(id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e,
        Event::Answered { question, answer }
            if question == "途中確認: 工程『design』の後" && answer == "keep it small")));
}

#[test]
fn replan_requires_a_note_and_does_not_change_the_state() {
    let store = SqliteStore::open_in_memory().unwrap();
    let id = blocked_task(&store, true);
    for note in [None, Some("   ".to_string())] {
        let err = phase_gate(&store, id, PhaseGateAction::Replan, note).unwrap_err();
        assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    }
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);
    let r = phase_gate(
        &store,
        id,
        PhaseGateAction::Replan,
        Some("split build".to_string()),
    )
    .unwrap();
    assert_eq!(r.reason, "phase_replan");
    let events = store.events_for(id).unwrap();
    assert_eq!(
        phase_replan_instruction(&events).as_deref(),
        Some("人の指示: split build")
    );
}

/// D2.2: `Trigger::Answer` は awaiting_human の Task には 409（`InvalidState`）。
#[test]
fn answer_is_rejected_while_awaiting_human() {
    let store = SqliteStore::open_in_memory().unwrap();
    let id = blocked_task(&store, true);
    let err = crate::gate::answer(&store, id, "go".to_string(), None).unwrap_err();
    assert!(matches!(err, OpsError::InvalidState { .. }), "{err:?}");
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);
}

#[test]
fn withdraw_cancels() {
    let store = SqliteStore::open_in_memory().unwrap();
    let id = blocked_task(&store, true);
    let r = phase_gate(&store, id, PhaseGateAction::Withdraw, None).unwrap();
    assert_eq!(r.to, Status::Cancelled);
}

#[test]
fn a_question_block_is_not_a_phase_gate() {
    let store = SqliteStore::open_in_memory().unwrap();
    let id = blocked_task(&store, false);
    for action in [
        PhaseGateAction::Continue,
        PhaseGateAction::Replan,
        PhaseGateAction::Withdraw,
    ] {
        let err = phase_gate(&store, id, action, Some("x".to_string())).unwrap_err();
        assert!(matches!(err, OpsError::InvalidState { .. }), "{err:?}");
    }
    assert_eq!(store.get(id).unwrap().unwrap().status, Status::Blocked);
}

#[test]
fn latest_checkpoint_reads_the_report_only_while_awaiting_human() {
    let store = SqliteStore::open_in_memory().unwrap();
    let id = blocked_task(&store, true);
    let task = store.get(id).unwrap().unwrap();
    let events = store.events_for(id).unwrap();
    let info = latest_phase_checkpoint(&task, &events).expect("checkpoint");
    assert_eq!(info.report.phase, "design");
    assert_eq!(info.report_idx, None);
    phase_gate(&store, id, PhaseGateAction::Continue, None).unwrap();
    let task = store.get(id).unwrap().unwrap();
    let events = store.events_for(id).unwrap();
    assert!(latest_phase_checkpoint(&task, &events).is_none());
}
