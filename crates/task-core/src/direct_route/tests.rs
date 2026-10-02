use super::*;
use crate::execution_gate::{EXECUTION_GATE_POLICY_VERSION, GateSignal};
use crate::model::{
    Budget, Criterion, Status, TaskCategory, TaskId, TaskKind, TaskMode, TaskRouting, Tier,
    WorkerHint, WorkspaceSpec,
};
use crate::repos::{RepoId, RepoRef};

fn task() -> Task {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    Task {
        expected_write_paths: None,
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "小さな修正".into(),
        objective: "関数を修正する".into(),
        acceptance: vec![Criterion {
            text: "test".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 10,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp/direct-route"),
        repos: vec![RepoRef {
            repo_id: RepoId::new(),
            name: "agent-platform".into(),
        }],
        budget: Budget {
            max_turns: 20,
            max_wall_secs: 600,
            max_retries: 2,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: Some("coding".into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: Some("software-engineering".into()),
        labels: vec![],
        category: TaskCategory::Other,
        skills: vec![],
        mode: TaskMode::Production,
        conversation: None,
        routing: Some(TaskRouting::default()),
        tree: None,
        paused_at: None,
    }
}

fn gate(mode: ExecutionMode, source: GateSource, rule_id: &str) -> ExecutionGateDecision {
    ExecutionGateDecision {
        mode,
        source,
        score: 5,
        threshold: 5,
        rule_id: rule_id.into(),
        signals: vec![],
        policy_version: EXECUTION_GATE_POLICY_VERSION.into(),
        shadow: false,
        depth: None,
    }
}

fn inputs() -> DirectRouteInputs {
    DirectRouteInputs {
        coding_harness: true,
        cross_department: false,
        pending_approval: false,
    }
}

fn failed(decision: &RouteDecision, rule: &str) -> bool {
    decision
        .reasons
        .iter()
        .any(|reason| reason.rule_id == rule && !reason.ok)
}

#[test]
fn direct_route_atomic_single_repo_is_direct() {
    let decision = evaluate(
        &task(),
        &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
        inputs(),
    );
    assert_eq!(decision.route, Route::Direct);
    assert_eq!(decision.reasons.len(), 8);
    assert!(decision.reasons.iter().all(|reason| reason.ok));
    assert!(!decision.overrode_gate);
}

#[test]
fn direct_route_root_policy_score_is_overridden() {
    let decision = evaluate(
        &task(),
        &gate(
            ExecutionMode::Compound,
            GateSource::Policy,
            "compound/score",
        ),
        inputs(),
    );
    assert_eq!(decision.route, Route::Direct);
    assert!(decision.overrode_gate);
    assert_eq!(decision.gate_rule_id, "compound/score");
}

#[test]
fn direct_route_human_explicit_compound_stays_planned() {
    let decision = evaluate(
        &task(),
        &gate(ExecutionMode::Compound, GateSource::Human, "human/explicit"),
        inputs(),
    );
    assert_eq!(decision.route, Route::Planned);
    assert!(failed(&decision, "direct/gate"));
}

#[test]
fn direct_route_hint_and_forced_compound_stay_planned() {
    for (source, rule) in [
        (GateSource::Hint, "compound/score"),
        (GateSource::Policy, "compound/long-and-broad"),
    ] {
        let decision = evaluate(
            &task(),
            &gate(ExecutionMode::Compound, source, rule),
            inputs(),
        );
        assert_eq!(decision.route, Route::Planned);
        assert!(failed(&decision, "direct/gate"));
    }
}

#[test]
fn direct_route_tree_child_compound_not_overridden() {
    let mut child_gate = gate(
        ExecutionMode::Compound,
        GateSource::Policy,
        "compound/score",
    );
    child_gate.depth = Some(2);
    let decision = evaluate(&task(), &child_gate, inputs());
    assert_eq!(decision.route, Route::Planned);
    assert!(!decision.overrode_gate);
}

#[test]
fn direct_route_multiple_repos_and_environment_stay_planned() {
    let mut task = task();
    task.repos.push(RepoRef {
        repo_id: RepoId::new(),
        name: "benchfs".into(),
    });
    let decision = evaluate(
        &task,
        &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
        inputs(),
    );
    assert!(failed(&decision, "direct/single-repo"));
    task.repos.pop();
    let mut mixed_gate = gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score");
    mixed_gate.signals.push(GateSignal {
        name: "S4".into(),
        weight: 1,
        detail: "multiple execution environments".into(),
    });
    assert!(failed(
        &evaluate(&task, &mixed_gate, inputs()),
        "direct/single-repo"
    ));
}

#[test]
fn direct_route_cross_department_and_unassigned_stay_planned() {
    let mut cross = inputs();
    cross.cross_department = true;
    assert!(failed(
        &evaluate(
            &task(),
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            cross
        ),
        "direct/single-department"
    ));
    let mut task = task();
    task.assignee = None;
    assert!(failed(
        &evaluate(
            &task,
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            inputs()
        ),
        "direct/single-department"
    ));
}

#[test]
fn direct_route_human_check_and_pending_approval_stay_planned() {
    let mut human_task = task();
    human_task.acceptance.push(Criterion {
        text: "approve".into(),
        check: Check::Human,
    });
    assert!(failed(
        &evaluate(
            &human_task,
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            inputs()
        ),
        "direct/no-human-approval"
    ));
    let mut pending = inputs();
    pending.pending_approval = true;
    assert!(failed(
        &evaluate(
            &task(),
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            pending
        ),
        "direct/no-human-approval"
    ));
}

#[test]
fn direct_route_without_command_check_stays_planned() {
    let mut task = task();
    task.acceptance.clear();
    assert!(failed(
        &evaluate(
            &task,
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            inputs()
        ),
        "direct/command-check"
    ));
}

#[test]
fn direct_route_aggregate_and_out_of_scope_stay_planned() {
    let mut task = task();
    task.aggregate = true;
    assert!(failed(
        &evaluate(
            &task,
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            inputs()
        ),
        "direct/no-plan"
    ));
    task.aggregate = false;
    task.routing = None;
    assert!(failed(
        &evaluate(
            &task,
            &gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score"),
            inputs()
        ),
        "direct/in-scope"
    ));
}

#[test]
fn direct_route_shadow_is_recorded() {
    let mut shadow_gate = gate(ExecutionMode::Atomic, GateSource::Policy, "atomic/score");
    shadow_gate.shadow = true;
    let decision = evaluate(&task(), &shadow_gate, inputs());
    assert_eq!(decision.route, Route::Direct);
    assert!(decision.shadow);
    assert_eq!(decision.policy_version, DIRECT_ROUTE_POLICY_VERSION);
}
