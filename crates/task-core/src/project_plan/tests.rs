use super::*;
use crate::model::{Check, Criterion};

fn acceptance() -> Vec<Criterion> {
    vec![Criterion {
        text: "done".into(),
        check: Check::Human,
    }]
    .into_iter()
    .chain(std::iter::once(Criterion {
        text: "artifact".into(),
        check: Check::ArtifactExists {
            name: "report.md".into(),
        },
    }))
    .collect()
}

fn milestone(key: &str, depends_on: &[&str]) -> MilestoneSpec {
    MilestoneSpec {
        pause_after: None,
        key: key.into(),
        title: format!("title-{key}"),
        objective: "objective".into(),
        reach_criteria: "criteria".into(),
        acceptance: acceptance(),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        genre: None,
        skills: Vec::new(),
        repos: Vec::new(),
        features: None,
        execution: None,
    }
}

fn plan(milestones: Vec<MilestoneSpec>) -> ProjectPlanSpec {
    ProjectPlanSpec {
        schema: PROJECT_PLAN_SCHEMA.to_string(),
        rationale: "rationale".into(),
        milestones,
    }
}

#[test]
fn a_valid_plan_is_accepted_and_topologically_ordered() {
    let spec = plan(vec![
        milestone("survey", &[]),
        milestone("poc", &["survey"]),
    ]);
    let validated =
        validate(&spec, ProjectPlanLimits::default(), &BTreeSet::new()).expect("valid plan");
    let order: Vec<&str> = validated
        .topological_order
        .iter()
        .map(|&i| validated.spec.milestones[i].key.as_str())
        .collect();
    assert_eq!(order, vec!["survey", "poc"]);
}

#[test]
fn rejects_cycles_and_unknown_keys() {
    let unknown = plan(vec![milestone("a", &["ghost"])]);
    let errors =
        validate(&unknown, ProjectPlanLimits::default(), &BTreeSet::new()).expect_err("rejected");
    assert!(
        errors.iter().any(|e| matches!(
            e,
            ProjectPlanValidationError::UnknownDependency { key, depends_on }
                if key == "a" && depends_on == "ghost"
        )),
        "{errors:?}"
    );

    let cyclic = plan(vec![milestone("a", &["b"]), milestone("b", &["a"])]);
    let errors =
        validate(&cyclic, ProjectPlanLimits::default(), &BTreeSet::new()).expect_err("rejected");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ProjectPlanValidationError::CyclicDependency { .. })),
        "{errors:?}"
    );
}

#[test]
fn rejects_wrong_schema_duplicate_keys_and_count_limits() {
    let wrong_schema = ProjectPlanSpec {
        schema: "celeris.project-plan/2".into(),
        ..plan(vec![milestone("a", &[])])
    };
    let errors = validate(
        &wrong_schema,
        ProjectPlanLimits::default(),
        &BTreeSet::new(),
    )
    .expect_err("rejected");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ProjectPlanValidationError::WrongSchema { .. })),
        "{errors:?}"
    );

    let empty = plan(vec![]);
    let errors =
        validate(&empty, ProjectPlanLimits::default(), &BTreeSet::new()).expect_err("rejected");
    assert!(errors.contains(&ProjectPlanValidationError::NoMilestones));

    let dup = plan(vec![milestone("a", &[]), milestone("a", &[])]);
    let errors =
        validate(&dup, ProjectPlanLimits::default(), &BTreeSet::new()).expect_err("rejected");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ProjectPlanValidationError::DuplicateKey { key } if key == "a")),
        "{errors:?}"
    );

    let too_many: Vec<MilestoneSpec> = (0..13).map(|i| milestone(&format!("m{i}"), &[])).collect();
    let too_many = plan(too_many);
    let limits = ProjectPlanLimits::default();
    let errors = validate(&too_many, limits, &BTreeSet::new()).expect_err("rejected");
    assert!(
            errors.iter().any(|e| matches!(
                e,
                ProjectPlanValidationError::TooManyMilestones { count: 13, max } if *max == limits.max_milestones
            )),
            "{errors:?}"
        );
}

#[test]
fn rejects_empty_acceptance_and_human_checks_without_a_deliverable() {
    let mut no_acceptance = milestone("a", &[]);
    no_acceptance.acceptance = Vec::new();
    let errors = validate(
        &plan(vec![no_acceptance]),
        ProjectPlanLimits::default(),
        &BTreeSet::new(),
    )
    .expect_err("rejected");
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ProjectPlanValidationError::NoAcceptance { key } if key == "a")),
        "{errors:?}"
    );

    let mut bare_human = milestone("a", &[]);
    bare_human.acceptance = vec![Criterion {
        text: "done".into(),
        check: Check::Human,
    }];
    let errors = validate(
        &plan(vec![bare_human]),
        ProjectPlanLimits::default(),
        &BTreeSet::new(),
    )
    .expect_err("rejected");
    assert!(
        errors.iter().any(
            |e| matches!(e, ProjectPlanValidationError::InvalidAcceptance { key, .. } if key == "a")
        ),
        "{errors:?}"
    );
}

#[test]
fn is_milestones_plan_task_needs_the_kind_and_the_label() {
    use crate::model::{
        Budget, Check, Criterion, Status, Task, TaskId, TaskKind, Tier, WorkerHint, WorkspaceSpec,
    };
    let now = time::OffsetDateTime::now_utc();
    let base = Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Plan,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
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
    assert!(!is_milestones_plan_task(&base), "no label yet");

    let mut labeled = base.clone();
    labeled.labels = vec![MILESTONES_PLAN_LABEL.to_string()];
    assert!(is_milestones_plan_task(&labeled));

    let mut wrong_kind = labeled.clone();
    wrong_kind.kind = TaskKind::Execute;
    assert!(!is_milestones_plan_task(&wrong_kind));
}

/// ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
#[test]
fn committed_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/project-plan.schema.json"
    );
    let generated = serde_json::to_string_pretty(&schema_value()).unwrap() + "\n";
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

#[test]
fn committed_delta_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/project-plan-delta.schema.json"
    );
    let generated = serde_json::to_string_pretty(&delta_schema_value()).unwrap() + "\n";
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

fn spec(key: &str, deps: &[&str]) -> MilestoneSpec {
    MilestoneSpec {
        key: key.into(),
        title: key.into(),
        objective: "o".into(),
        reach_criteria: "r".into(),
        acceptance: acceptance(),
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
        genre: None,
        skills: vec![],
        repos: vec![],
        features: None,
        execution: None,
        pause_after: None,
    }
}

fn base_plan() -> ProjectPlanSpec {
    ProjectPlanSpec {
        schema: PROJECT_PLAN_SCHEMA.into(),
        rationale: "r".into(),
        milestones: vec![spec("survey", &[]), spec("poc", &["survey"])],
    }
}

fn delta() -> ProjectPlanDelta {
    ProjectPlanDelta {
        schema: PROJECT_PLAN_DELTA_SCHEMA.into(),
        base_version: 1,
        rationale: "見直し".into(),
        add: vec![],
        modify: vec![],
        remove: vec![],
        cancel: vec![],
    }
}

/// ADR-0074 D3.4（Phase F4b (e)）: dispatch 済みのマイルストーンは `modify` / `remove` できない
/// （`cancel` を明示する）。dispatch 前のものは変えられ、当てた後の計画が返る。
#[test]
fn delta_modify_and_remove_only_touch_undispatched_milestones() {
    let mut states = BTreeMap::new();
    states.insert(
        "survey".to_string(),
        PlanNodeState {
            dispatched: true,
            terminal: false,
        },
    );
    let mut d = delta();
    d.modify.push(MilestoneModify {
        key: "survey".into(),
        title: Some("x".into()),
        ..MilestoneModify::default()
    });
    let errs =
        validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap_err();
    assert!(
        errs.contains(&ProjectPlanDeltaError::ModifiesStartedMilestone {
            key: "survey".into()
        })
    );

    let mut d = delta();
    d.remove.push("survey".into());
    assert!(
        validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default())
            .unwrap_err()
            .contains(&ProjectPlanDeltaError::ModifiesStartedMilestone {
                key: "survey".into()
            })
    );

    // cancel は明示すれば通る（poc は survey に依存するので一緒に外す必要がある）。
    let mut d = delta();
    d.cancel.push("survey".into());
    d.remove.push("poc".into());
    let ok = validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default());
    assert!(ok.is_err(), "empty result plan is rejected: {ok:?}");
    d.add.push(spec("redo", &[]));
    let ok = validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap();
    assert_eq!(ok.result.milestones.len(), 1);
    assert_eq!(ok.result.milestones[0].key, "redo");

    // dispatch 前の poc の modify は通り、書いた欄だけが変わる。
    let mut d = delta();
    d.modify.push(MilestoneModify {
        key: "poc".into(),
        title: Some("PoC v2".into()),
        ..MilestoneModify::default()
    });
    d.add.push(spec("paper", &["poc"]));
    let ok = validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap();
    let poc = ok
        .result
        .milestones
        .iter()
        .find(|m| m.key == "poc")
        .unwrap();
    assert_eq!(poc.title, "PoC v2");
    assert_eq!(poc.depends_on, vec!["survey".to_string()]);
    assert!(ok.is_added("paper"));
    assert!(!ok.is_added("poc"));
}

#[test]
fn delta_rejects_stale_base_unknown_keys_collisions_and_dangling_dependencies() {
    let states = BTreeMap::new();
    let mut d = delta();
    d.base_version = 0;
    d.remove.push("nope".into());
    d.add.push(spec("survey", &[]));
    let errs =
        validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap_err();
    assert!(errs.contains(&ProjectPlanDeltaError::StaleBaseVersion {
        base_version: 0,
        current: 1
    }));
    assert!(errs.contains(&ProjectPlanDeltaError::UnknownMilestone { key: "nope".into() }));
    assert!(
        errs.contains(&ProjectPlanDeltaError::KeyCollidesWithExisting {
            key: "survey".into()
        })
    );

    assert_eq!(
        validate_delta(
            &delta(),
            &base_plan(),
            1,
            &states,
            ProjectPlanLimits::default()
        )
        .unwrap_err(),
        vec![ProjectPlanDeltaError::EmptyDelta]
    );

    // survey を外すと、残る poc の依存が宙に浮く。
    let mut d = delta();
    d.remove.push("survey".into());
    let errs =
        validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap_err();
    assert!(matches!(
        errs.as_slice(),
        [ProjectPlanDeltaError::Plan(
            ProjectPlanValidationError::UnknownDependency { .. }
        )]
    ));

    let mut d = delta();
    d.remove.push("poc".into());
    d.cancel.push("poc".into());
    assert!(
        validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default())
            .unwrap_err()
            .contains(&ProjectPlanDeltaError::ConflictingChange { key: "poc".into() })
    );

    // 終端のものは cancel できない。
    let mut st = BTreeMap::new();
    st.insert(
        "survey".to_string(),
        PlanNodeState {
            dispatched: true,
            terminal: true,
        },
    );
    let mut d = delta();
    d.cancel.push("survey".into());
    assert!(
        validate_delta(&d, &base_plan(), 1, &st, ProjectPlanLimits::default())
            .unwrap_err()
            .contains(&ProjectPlanDeltaError::CancelsFinishedMilestone {
                key: "survey".into()
            })
    );
}
