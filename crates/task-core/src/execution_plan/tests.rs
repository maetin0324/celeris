use super::*;

fn spec(key: &str, depends_on: &[&str]) -> WorkUnitSpec {
    WorkUnitSpec {
        key: key.to_string(),
        kind: WorkUnitKind::Implement,
        title: format!("title {key}"),
        objective: format!("objective for {key} which is sufficiently distinct"),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    }
}

fn plan(work_units: Vec<WorkUnitSpec>) -> ExecutionPlanSpec {
    ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "test".to_string(),
        work_units,
        phases: Vec::new(),
        children: Vec::new(),
    }
}

// ---- ADR-0074 D1.1（Phase F2）: v2（`phases`）のテスト用ヘルパー ----

fn phase(key: &str) -> PhaseSpec {
    PhaseSpec {
        key: key.to_string(),
        kind: WorkUnitKind::Implement,
        title: format!("phase {key}"),
    }
}

fn spec_v2(key: &str, wu_phase: &str, depends_on: &[&str]) -> WorkUnitSpec {
    let mut s = spec(key, depends_on);
    s.phase = Some(wu_phase.to_string());
    s
}

fn plan_v2(phases: Vec<PhaseSpec>, work_units: Vec<WorkUnitSpec>) -> ExecutionPlanSpec {
    ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: EXECUTION_PLAN_SCHEMA_V2.to_string(),
        rationale: "test v2".to_string(),
        work_units,
        phases,
        children: Vec::new(),
    }
}

#[test]
fn valid_three_step_plan_passes_and_orders_topologically() {
    let p = plan(vec![spec("a", &[]), spec("b", &["a"]), spec("c", &["b"])]);
    let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
    let order: Vec<&str> = validated
        .topological_order
        .iter()
        .map(|&i| validated.spec.work_units[i].key.as_str())
        .collect();
    assert_eq!(order, vec!["a", "b", "c"]);
}

#[test]
fn rejects_cycles() {
    let p = plan(vec![spec("a", &["b"]), spec("b", &["a"])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::CyclicDependency { .. })),
        "{errs:?}"
    );
}

#[test]
fn rejects_duplicate_keys() {
    let p = plan(vec![spec("a", &[]), spec("a", &[])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(errs.iter().any(|e| matches!(
        e,
        PlanValidationError::DuplicateKey { key } if key == "a"
    )));
}

#[test]
fn rejects_unknown_dependency() {
    let p = plan(vec![spec("a", &["ghost"])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::UnknownDependency { .. }))
    );
}

#[test]
fn rejects_invalid_key_format() {
    let p = plan(vec![spec("Not Valid!", &[])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::InvalidKey { .. }))
    );
}

#[test]
fn rejects_too_many_work_units() {
    let units: Vec<WorkUnitSpec> = (0..10).map(|i| spec(&format!("wu{i}"), &[])).collect();
    let p = plan(units);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::TooManyWorkUnits { .. }))
    );
}

#[test]
fn rejects_near_duplicate_objectives() {
    let mut a = spec("a", &[]);
    a.objective = "investigate the current dispatcher and review pipeline in depth".into();
    let mut b = spec("b", &[]);
    b.objective = "investigate the current dispatcher and review pipeline in depth!".into();
    let p = plan(vec![a, b]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::DuplicateWorkUnit { .. }))
    );
}

#[test]
fn rounds_budget_to_the_limits_and_records_it() {
    let mut a = spec("a", &[]);
    a.budget = Some(WorkUnitBudget {
        max_turns: Some(999),
        max_wall_secs: Some(99999),
    });
    let p = plan(vec![a]);
    let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
    assert_eq!(
        validated.spec.work_units[0].budget.unwrap().max_turns,
        Some(80)
    );
    assert_eq!(
        validated.spec.work_units[0].budget.unwrap().max_wall_secs,
        Some(3600)
    );
    assert_eq!(
        validated.rounding_notes.len(),
        2,
        "{:?}",
        validated.rounding_notes
    );
}

#[test]
fn rejects_changed_done_work_unit_on_replan() {
    let done_spec = spec("a", &[]);
    let mut changed = done_spec.clone();
    changed.objective = "a completely different objective now".to_string();
    let p = plan(vec![changed]);
    let errs = validate(
        &p,
        ExecutionLimits::default(),
        &[("a".to_string(), done_spec)],
    )
    .unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::DoneWorkUnitChanged { .. }))
    );
}

fn ctx(origin: PlanOrigin) -> PlanContext {
    PlanContext { origin, depth: 1 }
}

/// ADR-0079 R5b-fix1: 本番の task の形（done の `baseline` の check を人が直す）。
fn baseline_override_fixture() -> (WorkUnitSpec, ExecutionPlanSpec) {
    let mut done_spec = spec("baseline", &[]);
    done_spec.checks = vec![WorkUnitCheck {
        cmd: "git diff --quiet 06e9a03cffe8 -- gui".to_string(),
        expect_exit: 0,
        scope: false,
    }];
    let mut fixed = done_spec.clone();
    fixed.checks[0].cmd =
        "git diff --quiet 06e9a03cffe8 -- gui \":!gui/docs/adr/0002-frontend-stack.md\""
            .to_string();
    (done_spec, plan(vec![fixed, spec("next", &["baseline"])]))
}

/// ADR-0079 R5b-fix1: 人の replan は done の WU の spec（check）を上書きできる。
#[test]
fn human_replan_may_override_a_done_work_unit_spec() {
    let (done_spec, p) = baseline_override_fixture();
    let done = [("baseline".to_string(), done_spec)];
    validate_with(
        &p,
        ExecutionLimits::default(),
        &done,
        ctx(PlanOrigin::Human),
    )
    .expect("a human may correct the check of a done work unit");
    let overrides = done_work_unit_overrides(&p, &done);
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].0, "baseline");
    assert_eq!(overrides[0].1, p.work_units[0]);
    assert_eq!(overrides[0].2, vec!["checks".to_string()]);
}

/// ADR-0079 R5b-fix1: planner（と repair）の replan は従来どおり done の spec を変えられない。
/// ADR-0079 付記「R7-3」D1: ただし planner は `checks` だけなら変えられる（この fixture の check だけの変更は
/// planner には通る。`objective` も変えれば拒否）。repair は `checks` だけでも拒否。
#[test]
fn planner_replan_still_rejects_a_changed_done_work_unit() {
    let (done_spec, p) = baseline_override_fixture();
    let done = [("baseline".to_string(), done_spec)];
    validate_with(
        &p,
        ExecutionLimits::default(),
        &done,
        ctx(PlanOrigin::Planner),
    )
    .expect("R7-3 D1: a planner may change only the checks of a done work unit");
    let mut beyond_checks = p.clone();
    beyond_checks.work_units[0].objective = "a rewritten objective for baseline".into();
    for (origin, p) in [
        (PlanOrigin::Planner, &beyond_checks),
        (PlanOrigin::Repair, &p),
    ] {
        let errs = validate_with(p, ExecutionLimits::default(), &done, ctx(origin)).unwrap_err();
        assert!(
            errs.contains(&PlanValidationError::DoneWorkUnitChanged {
                key: "baseline".into()
            }),
            "{origin:?}: {errs:?}"
        );
    }
    let msg = PlanValidationError::DoneWorkUnitChanged {
        key: "baseline".into(),
    }
    .to_string();
    assert!(msg.contains("PUT /tasks/{id}/execution-plan"), "{msg}");
}

/// ADR-0079 R5b-fix1: 人でも done の WU は消せない・構造（kind / phase / depends_on）は変えられない。
#[test]
fn human_replan_still_rejects_a_removed_or_restructured_done_work_unit() {
    let (done_spec, p) = baseline_override_fixture();
    let done = [("baseline".to_string(), done_spec)];
    let mut removed = p.clone();
    removed.work_units.remove(0);
    removed.work_units[0].depends_on.clear();
    let errs = validate_with(
        &removed,
        ExecutionLimits::default(),
        &done,
        ctx(PlanOrigin::Human),
    )
    .unwrap_err();
    assert!(errs.contains(&PlanValidationError::DoneWorkUnitChanged {
        key: "baseline".into()
    }));

    let mut restructured = p.clone();
    restructured.work_units.push(spec("extra", &[]));
    restructured.work_units[0].depends_on = vec!["extra".into()];
    restructured.work_units[0].kind = WorkUnitKind::Test;
    let errs = validate_with(
        &restructured,
        ExecutionLimits::default(),
        &done,
        ctx(PlanOrigin::Human),
    )
    .unwrap_err();
    for field in ["kind", "depends_on"] {
        assert!(
            errs.contains(&PlanValidationError::DoneWorkUnitStructureChanged {
                key: "baseline".into(),
                field,
            }),
            "{field}: {errs:?}"
        );
    }
}

/// ADR-0074 D5.1（Phase F1）: `features` は `TaskFeatureHints` として読めなければ検証エラー
/// （黙って `.ok()` で捨てない）。
#[test]
fn features_must_parse_as_task_feature_hints() {
    let mut a = spec("a", &[]);
    a.features = Some(serde_json::json!({"judgment": "low", "ambiguity": "low"}));
    let p = plan(vec![a]);
    validate(&p, ExecutionLimits::default(), &[]).expect("valid partial features");

    let mut b = spec("b", &[]);
    // `lane` は `TaskFeatureHints` に無い欄（`deny_unknown_fields`）。
    b.features = Some(serde_json::json!({"lane": "frontier"}));
    let p = plan(vec![b]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::InvalidFeatures { key, .. } if key == "b")),
        "{errs:?}"
    );

    let mut c = spec("c", &[]);
    // 型が違う（配列であるべき欄が文字列）。
    c.features = Some(serde_json::json!({"judgment": "not-a-level"}));
    let p = plan(vec![c]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::InvalidFeatures { key, .. } if key == "c")),
        "{errs:?}"
    );
}

/// ADR-0074 D5.3（Phase F1）: 計画のサイズ上限（§4）。
#[test]
fn plan_size_limits_reject_oversized_rationale() {
    let mut p = plan(vec![spec("a", &[])]);
    p.rationale = "x".repeat(1_501);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter().any(
            |e| matches!(e, PlanValidationError::RationaleTooLong { len, max } if *len == 1_501 && *max == 1_500)
        ),
        "{errs:?}"
    );
}

#[test]
fn plan_size_limits_reject_oversized_work_unit_fields() {
    let mut a = spec("a", &[]);
    a.title = "x".repeat(121);
    a.objective = "y".repeat(2_001);
    a.done_when = (0..9).map(|i| format!("done {i}")).collect();
    a.checks = (0..7)
        .map(|i| WorkUnitCheck {
            cmd: format!("cmd {i}"),
            expect_exit: 0,
            scope: false,
        })
        .collect();
    let p = plan(vec![a]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::TitleTooLong { key, .. } if key == "a")),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::ObjectiveTooLong { key, .. } if key == "a")),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::TooManyDoneWhen { key, .. } if key == "a")),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::TooManyChecks { key, .. } if key == "a")),
        "{errs:?}"
    );
}

#[test]
fn plan_size_limits_reject_the_whole_json_being_too_large() {
    let limits = ExecutionLimits {
        max_plan_json_bytes: 200,
        ..ExecutionLimits::default()
    };
    let p = plan(vec![spec("a", &[]), spec("b", &["a"])]);
    let errs = validate(&p, limits, &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::PlanTooLarge { .. })),
        "{errs:?}"
    );
}

fn row(key: &str, seq: u32, status: WorkUnitStatus, depends_on: &[&str]) -> WorkUnitRow {
    WorkUnitRow::new(
        format!("wu-{key}"),
        "task".to_string(),
        "plan".to_string(),
        seq,
        spec(key, depends_on),
        status,
        "2026-09-24T00:00:00Z".to_string(),
    )
}

#[test]
fn next_work_unit_prefers_needs_continuation_then_ready_by_seq() {
    let units = vec![
        row("a", 0, WorkUnitStatus::Done, &[]),
        row("c", 2, WorkUnitStatus::Ready, &[]),
        row("b", 1, WorkUnitStatus::NeedsContinuation, &[]),
    ];
    assert_eq!(next_work_unit(&units), NextStep::RunWorkUnit("wu-b".into()));

    let units2 = vec![
        row("a", 0, WorkUnitStatus::Done, &[]),
        row("c", 2, WorkUnitStatus::Ready, &[]),
        row("b", 1, WorkUnitStatus::Ready, &[]),
    ];
    assert_eq!(
        next_work_unit(&units2),
        NextStep::RunWorkUnit("wu-b".into())
    );
}

#[test]
fn next_work_unit_all_done_when_everything_active_is_done() {
    let units = vec![
        row("a", 0, WorkUnitStatus::Done, &[]),
        row("b", 1, WorkUnitStatus::Superseded, &[]),
    ];
    assert_eq!(next_work_unit(&units), NextStep::AllDone);
}

// ---- ADR-0074 D1.3（Phase F2）: `runnable_work_units` ----

fn row_v2(
    key: &str,
    wu_phase: &str,
    seq: u32,
    status: WorkUnitStatus,
    depends_on: &[&str],
) -> WorkUnitRow {
    WorkUnitRow::new(
        format!("wu-{key}"),
        "task".to_string(),
        "plan".to_string(),
        seq,
        spec_v2(key, wu_phase, depends_on),
        status,
        "2026-09-24T00:00:00Z".to_string(),
    )
}

/// v1（`phase` が常に `None`）で `limit = 1` なら `next_work_unit` と同じ 1 件を返す。
#[test]
fn runnable_work_units_matches_next_work_unit_for_v1_with_limit_one() {
    let units = vec![
        row("a", 0, WorkUnitStatus::Done, &[]),
        row("c", 2, WorkUnitStatus::Ready, &[]),
        row("b", 1, WorkUnitStatus::NeedsContinuation, &[]),
    ];
    assert_eq!(runnable_work_units(&units, 0, 1), vec!["wu-b".to_string()]);
}

#[test]
fn runnable_work_units_respects_the_parallel_limit() {
    let units = vec![
        row_v2("a", "build", 0, WorkUnitStatus::Ready, &[]),
        row_v2("b", "build", 1, WorkUnitStatus::Ready, &[]),
        row_v2("c", "build", 2, WorkUnitStatus::Ready, &[]),
    ];
    assert_eq!(
        runnable_work_units(&units, 0, 2),
        vec!["wu-a".to_string(), "wu-b".to_string()],
        "2 本まで、seq 順"
    );
    assert_eq!(
        runnable_work_units(&units, 0, 3),
        vec!["wu-a".to_string(), "wu-b".to_string(), "wu-c".to_string()]
    );
    assert!(
        runnable_work_units(&units, 3, 3).is_empty(),
        "in_flight が limit に達していれば何も起こさない"
    );
    assert_eq!(
        runnable_work_units(&units, 1, 3).len(),
        2,
        "in_flight の分だけ枠が減る"
    );
}

#[test]
fn runnable_work_units_prefers_needs_continuation_over_ready() {
    let units = vec![
        row_v2("a", "build", 0, WorkUnitStatus::Ready, &[]),
        row_v2("b", "build", 1, WorkUnitStatus::NeedsContinuation, &[]),
    ];
    assert_eq!(
        runnable_work_units(&units, 0, 1),
        vec!["wu-b".to_string()],
        "needs_continuation を先に選ぶ"
    );
}

/// D1.3: 対象は現在の工程だけ（前の工程がまだ終わっていなければ、後の工程の ready な WU は
/// 対象にしない）。
#[test]
fn runnable_work_units_only_considers_the_current_phase() {
    let units = vec![
        row_v2("a", "build", 0, WorkUnitStatus::Ready, &[]),
        // `verify` 工程は `build` に依存していないが、工程の境が障壁になる。
        row_v2("z", "verify", 1, WorkUnitStatus::Ready, &[]),
    ];
    assert_eq!(
        runnable_work_units(&units, 0, 5),
        vec!["wu-a".to_string()],
        "build 工程がまだ終わっていないので verify の WU は対象外"
    );

    // build がすべて終われば（is_terminal）、verify が「現在の工程」になる。
    let mut done = units.clone();
    done[0].status = WorkUnitStatus::Done;
    assert_eq!(runnable_work_units(&done, 0, 5), vec!["wu-z".to_string()]);
}

/// D1.6: 兄弟が failed/blocked のとき、新しい（ready の）WU は起こさないが、既に走ったことの
/// ある needs_continuation の WU は続ける。
#[test]
fn runnable_work_units_does_not_start_new_ones_when_a_sibling_failed_but_continues_in_flight() {
    let units = vec![
        row_v2("a", "build", 0, WorkUnitStatus::Failed, &[]),
        row_v2("b", "build", 1, WorkUnitStatus::Ready, &[]),
        row_v2("c", "build", 2, WorkUnitStatus::NeedsContinuation, &[]),
    ];
    assert_eq!(
        runnable_work_units(&units, 0, 5),
        vec!["wu-c".to_string()],
        "ready の b は起こさないが、needs_continuation の c は続ける"
    );
}

#[test]
fn runnable_work_units_returns_empty_when_everything_is_terminal() {
    let units = vec![
        row("a", 0, WorkUnitStatus::Done, &[]),
        row("b", 1, WorkUnitStatus::Superseded, &[]),
    ];
    assert!(runnable_work_units(&units, 0, 3).is_empty());
}

#[test]
fn newly_ready_promotes_pending_whose_dependencies_are_all_done() {
    let units = vec![
        row("a", 0, WorkUnitStatus::Done, &[]),
        row("b", 1, WorkUnitStatus::Pending, &["a"]),
        row("c", 2, WorkUnitStatus::Pending, &["b"]),
    ];
    assert_eq!(newly_ready(&units), vec!["wu-b".to_string()]);
}

#[test]
fn dependents_to_block_finds_the_transitive_closure() {
    let units = vec![
        row("a", 0, WorkUnitStatus::Failed, &[]),
        row("b", 1, WorkUnitStatus::Pending, &["a"]),
        row("c", 2, WorkUnitStatus::Pending, &["b"]),
        row("d", 3, WorkUnitStatus::Done, &[]),
    ];
    let mut blocked = dependents_to_block(&units, "a");
    blocked.sort();
    assert_eq!(blocked, vec!["wu-b".to_string(), "wu-c".to_string()]);
}

/// ADR-0072 D8 / ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
#[test]
fn committed_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/execution-plan.schema.json"
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

/// ADR-0074 D5.3（Phase F1）: `execution-plan-delta/1` の生成スキーマとコミット済みファイルの一致。
#[test]
fn delta_committed_schema_matches_generated() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/execution-plan-delta.schema.json"
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

// ---- ADR-0074 D5.3（Phase F1）: replan の差分 (add/modify/remove) ----

fn delta(
    base_version: u32,
    add: Vec<WorkUnitSpec>,
    modify: Vec<WorkUnitPatch>,
    remove: Vec<&str>,
) -> ExecutionPlanDelta {
    ExecutionPlanDelta {
        schema: EXECUTION_PLAN_DELTA_SCHEMA.to_string(),
        base_version,
        rationale: "delta test".to_string(),
        add,
        modify,
        remove: remove.into_iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn apply_delta_adds_modifies_and_removes_without_restating_untouched_units() {
    let base = plan(vec![spec("a", &[]), spec("b", &["a"]), spec("c", &["b"])]);
    let d = delta(
        1,
        vec![spec("m", &[])],
        vec![WorkUnitPatch {
            key: "b".to_string(),
            depends_on: Some(vec!["a".to_string(), "m".to_string()]),
            ..WorkUnitPatch::default()
        }],
        vec!["c"],
    );
    let applied = apply_delta(&base, &d).expect("delta applies");
    let keys: Vec<&str> = applied.work_units.iter().map(|w| w.key.as_str()).collect();
    assert_eq!(keys, vec!["a", "b", "m"], "{keys:?}");
    let b = applied.work_units.iter().find(|w| w.key == "b").unwrap();
    assert_eq!(b.depends_on, vec!["a".to_string(), "m".to_string()]);
    // `a` は modify/remove の対象ではないので、`base` の spec のまま（書き写していない）。
    let a = applied.work_units.iter().find(|w| w.key == "a").unwrap();
    assert_eq!(a, &spec("a", &[]));
}

#[test]
fn apply_delta_rejects_modifying_an_unknown_or_removed_key() {
    let base = plan(vec![spec("a", &[])]);
    let d = delta(
        1,
        vec![],
        vec![WorkUnitPatch {
            key: "ghost".to_string(),
            title: Some("x".to_string()),
            ..WorkUnitPatch::default()
        }],
        vec![],
    );
    assert!(apply_delta(&base, &d).is_err());

    let d2 = delta(
        1,
        vec![],
        vec![WorkUnitPatch {
            key: "a".to_string(),
            title: Some("x".to_string()),
            ..WorkUnitPatch::default()
        }],
        vec!["a"],
    );
    assert!(apply_delta(&base, &d2).is_err(), "removed then modified");
}

#[test]
fn apply_delta_rejects_adding_a_key_that_still_exists() {
    let base = plan(vec![spec("a", &[])]);
    let d = delta(1, vec![spec("a", &[])], vec![], vec![]);
    assert!(apply_delta(&base, &d).is_err());
}

// -------------------------------------------------------------------
// ADR-0074 D1.1（Phase F2 (a)）: `celeris.execution-plan/2` の検証
// -------------------------------------------------------------------

/// v1 の計画は 1 バイトも挙動が変わらない（`phases`/`work_units[].phase` を書かない、
/// 既定の `ExecutionLimits` で通る）。既存の `valid_three_step_plan_passes_and_orders_topologically`
/// と合わせて、v1 の後方互換を確かめる。
#[test]
fn v1_plan_without_phases_still_validates_exactly_as_before() {
    let p = plan(vec![spec("a", &[]), spec("b", &["a"])]);
    assert!(p.phases.is_empty());
    assert!(p.children.is_empty());
    let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
    assert_eq!(validated.spec.work_units[0].phase, None);
}

#[test]
fn v1_rejects_phases_being_set() {
    let mut p = plan(vec![spec("a", &[])]);
    p.phases = vec![phase("build")];
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::PhasesNotAllowedInV1)),
        "{errs:?}"
    );
}

#[test]
fn v1_rejects_a_work_unit_with_a_phase_set() {
    let p = plan(vec![spec_v2("a", "build", &[])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            PlanValidationError::WorkUnitPhaseNotAllowedInV1 { key } if key == "a"
        )),
        "{errs:?}"
    );
}

/// ADR-0074 D1.4（Phase F2b）: v2 の採用は工程ごとに統合 WU（`integrate-<phase>`、依存はその工程の
/// すべての WU）を末尾に足し、工程の障壁つきで ready を決める（最初の工程の依存の無い WU だけ）。
#[test]
fn materialize_adds_integration_units_and_only_the_first_phase_is_ready() {
    let p = plan_v2(
        vec![phase("build"), phase("verify")],
        vec![
            spec_v2("a", "build", &[]),
            spec_v2("b", "build", &[]),
            spec_v2("c", "build", &["a"]),
            spec_v2("d", "verify", &[]),
            spec_v2("e", "verify", &["a", "b"]),
        ],
    );
    let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
    let rows = materialize_work_units(
        "t",
        "p",
        &validated.spec,
        &validated.topological_order,
        "2026-09-26T00:00:00Z",
        &mut |w| format!("id-{}", w.key),
    );
    let keys: Vec<(&str, u32, WorkUnitStatus)> = rows
        .iter()
        .map(|r| (r.key.as_str(), r.seq, r.status))
        .collect();
    assert_eq!(
        keys,
        vec![
            ("a", 0, WorkUnitStatus::Ready),
            ("b", 1, WorkUnitStatus::Ready),
            ("c", 2, WorkUnitStatus::Pending),
            ("integrate-build", 3, WorkUnitStatus::Pending),
            ("d", 4, WorkUnitStatus::Pending),
            ("e", 5, WorkUnitStatus::Pending),
            ("integrate-verify", 6, WorkUnitStatus::Pending),
        ],
        "後の工程の依存の無い WU（d）も、前の工程の統合が済むまで pending"
    );
    let integ = rows.iter().find(|r| r.key == "integrate-build").unwrap();
    assert_eq!(integ.kind, WorkUnitKind::Integrate);
    assert_eq!(integ.phase.as_deref(), Some("build"));
    assert_eq!(integ.depends_on, vec!["a", "b", "c"]);
    // 統合 WU は `ready` にならない（scheduler が直接走らせる）・runnable にも出ない。
    let mut all_done: Vec<WorkUnitRow> = rows.clone();
    for r in all_done
        .iter_mut()
        .filter(|r| r.phase.as_deref() == Some("build"))
    {
        if r.kind != WorkUnitKind::Integrate {
            r.status = WorkUnitStatus::Done;
        }
    }
    assert!(
        newly_ready(&all_done).is_empty(),
        "統合が済むまで次の工程は上がらない"
    );
    assert!(runnable_work_units(&all_done, 0, 3).is_empty());
    // 統合が done になれば次の工程が上がる（依存が done の d と e）。
    for r in all_done.iter_mut() {
        if r.key == "integrate-build" {
            r.status = WorkUnitStatus::Done;
        }
    }
    let mut up = newly_ready(&all_done);
    up.sort();
    assert_eq!(up, vec!["id-d".to_string(), "id-e".to_string()]);
    // v1 は従来どおり（統合 WU なし、依存が無ければ ready）。
    let v1 = plan(vec![spec("a", &[]), spec("b", &["a"])]);
    let v1v = validate(&v1, ExecutionLimits::default(), &[]).unwrap();
    let v1_rows = materialize_work_units(
        "t",
        "p",
        &v1v.spec,
        &v1v.topological_order,
        "2026-09-26T00:00:00Z",
        &mut |w| w.key.clone(),
    );
    assert_eq!(
        v1_rows
            .iter()
            .map(|r| (r.key.as_str(), r.status))
            .collect::<Vec<_>>(),
        vec![("a", WorkUnitStatus::Ready), ("b", WorkUnitStatus::Pending)]
    );
}

#[test]
fn phase_leaves_are_the_units_nobody_in_the_phase_depends_on() {
    let mut rows = vec![
        row_v2("a", "build", 0, WorkUnitStatus::Done, &[]),
        row_v2("b", "build", 1, WorkUnitStatus::Done, &["a"]),
        row_v2("c", "build", 2, WorkUnitStatus::Done, &[]),
        row_v2("d", "verify", 3, WorkUnitStatus::Pending, &["c"]),
    ];
    for r in rows.iter_mut() {
        r.branch = Some(format!("celeris-wu/t/{}", r.key));
    }
    let leaves: Vec<&str> = phase_leaves(&rows, "build")
        .iter()
        .map(|r| r.key.as_str())
        .collect();
    assert_eq!(
        leaves,
        vec!["b", "c"],
        "a は b に積み上げられている（b に含まれる）"
    );
}

#[test]
fn v2_rejects_a_key_reserved_for_integration_units() {
    let p = plan_v2(
        vec![phase("build")],
        vec![spec_v2("integrate-build", "build", &[])],
    );
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::ReservedKey { .. })),
        "{errs:?}"
    );
}

#[test]
fn v2_valid_plan_with_parallel_and_stacked_units_passes() {
    let p = plan_v2(
        vec![phase("build"), phase("verify")],
        vec![
            spec_v2("a", "build", &[]),
            spec_v2("b", "build", &[]),
            // 積み上げ（D1.2）: 同じ工程で a に依存。
            spec_v2("c", "build", &["a"]),
            // 前の工程への依存はいくつでもよい（規則 3）。
            spec_v2("d", "verify", &["a", "b", "c"]),
        ],
    );
    let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid v2 plan");
    assert_eq!(validated.spec.work_units.len(), 4);
}

#[test]
fn v2_rejects_no_phases() {
    let p = plan_v2(vec![], vec![]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::NoPhases)),
        "{errs:?}"
    );
}

#[test]
fn v2_rejects_too_many_phases() {
    let phases: Vec<PhaseSpec> = (0..6).map(|i| phase(&format!("p{i}"))).collect();
    let p = plan_v2(phases, vec![spec_v2("a", "p0", &[])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::TooManyPhases { count, max } if *count == 6 && *max == 5)),
        "{errs:?}"
    );
}

#[test]
fn v2_rejects_duplicate_phase_keys() {
    let p = plan_v2(
        vec![phase("build"), phase("build")],
        vec![spec_v2("a", "build", &[])],
    );
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::DuplicatePhaseKey { key } if key == "build")),
        "{errs:?}"
    );
}

#[test]
fn v2_rejects_a_work_unit_missing_its_phase() {
    let p = plan_v2(vec![phase("build")], vec![spec("a", &[])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::WorkUnitMissingPhase { key } if key == "a")),
        "{errs:?}"
    );
}

#[test]
fn v2_rejects_a_work_unit_with_an_unknown_phase() {
    let p = plan_v2(vec![phase("build")], vec![spec_v2("a", "ghost-phase", &[])]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            PlanValidationError::UnknownWorkUnitPhase { key, phase } if key == "a" && phase == "ghost-phase"
        )),
        "{errs:?}"
    );
}

/// ADR-0074 D1.1 の依存の規則 1: 後の工程への依存は拒否する。
#[test]
fn v2_rejects_a_dependency_on_a_later_phase() {
    let p = plan_v2(
        vec![phase("build"), phase("verify")],
        vec![spec_v2("a", "build", &["b"]), spec_v2("b", "verify", &[])],
    );
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            PlanValidationError::DependencyInLaterPhase { key, depends_on }
                if key == "a" && depends_on == "b"
        )),
        "{errs:?}"
    );
}

/// ADR-0074 D1.1 の依存の規則 2: 同じ工程の中の依存は高々 1 つ（鎖か木のみ）。
#[test]
fn v2_rejects_two_intra_phase_dependencies() {
    let p = plan_v2(
        vec![phase("build")],
        vec![
            spec_v2("a", "build", &[]),
            spec_v2("b", "build", &[]),
            spec_v2("c", "build", &["a", "b"]),
        ],
    );
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            PlanValidationError::TooManyIntraPhaseDependencies { key, phase }
                if key == "c" && phase == "build"
        )),
        "{errs:?}"
    );
}

/// 同じ工程で 1 つの WU に複数の WU が依存する「木」は許す（規則 2 は「自分の依存の数」を
/// 数えるのであって、依存されている側の数ではない）。
#[test]
fn v2_allows_a_tree_of_intra_phase_dependencies() {
    let p = plan_v2(
        vec![phase("build")],
        vec![
            spec_v2("a", "build", &[]),
            spec_v2("b", "build", &["a"]),
            spec_v2("c", "build", &["a"]),
        ],
    );
    validate(&p, ExecutionLimits::default(), &[]).expect("tree of dependencies is valid");
}

#[test]
fn v2_uses_the_v2_work_unit_limit_not_the_v1_one() {
    let limits = ExecutionLimits::default();
    assert_eq!(limits.max_work_units, 8);
    assert_eq!(limits.max_work_units_v2, 10);
    let units: Vec<WorkUnitSpec> = (0..9)
        .map(|i| spec_v2(&format!("wu{i}"), "build", &[]))
        .collect();
    let p = plan_v2(vec![phase("build")], units);
    validate(&p, limits, &[]).expect("9 work units fit under the v2 limit of 10");
}

fn child(key: &str, deps: &[&str]) -> ExecutionChildSpec {
    ExecutionChildSpec {
        requirements: Default::default(),
        key: key.into(),
        title: format!("child {key}"),
        objective: format!("do {key}"),
        acceptance: vec![crate::model::Criterion {
            text: "ok".into(),
            check: crate::model::Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        genre: None,
        skills: vec![],
        features: None,
        depends_on: deps.iter().map(|d| d.to_string()).collect(),
    }
}

/// ADR-0074 D3.7（Phase F4b (f)）: `children` は v1 では拒否、v2 では検証を通り、WU は
/// `child:<key>` で既知の子だけを指せる。
#[test]
fn children_are_v2_only_and_child_dependencies_must_be_known() {
    let mut v1 = plan(vec![spec("a", &[])]);
    v1.children = vec![child("c", &[])];
    let errs = validate(&v1, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::NonEmptyChildren)),
        "{errs:?}"
    );

    let mut v2 = plan_v2(
        vec![phase("build")],
        vec![spec_v2("a", "build", &["child:c"])],
    );
    v2.children = vec![child("c", &[]), child("d", &["c"])];
    validate(&v2, ExecutionLimits::default(), &[]).expect("valid v2 with children");

    let mut bad = v2.clone();
    bad.work_units[0].depends_on = vec!["child:zzz".into()];
    bad.children.push(child("e", &["e"]));
    bad.children.push(child("c", &[]));
    let mut no_acc = child("f", &[]);
    no_acc.acceptance.clear();
    bad.children.push(no_acc);
    let errs = validate(&bad, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(errs.iter().any(|e| matches!(e, PlanValidationError::UnknownDependency { depends_on, .. } if depends_on == "child:zzz")), "{errs:?}");
    assert!(
        errs.iter().any(
            |e| matches!(e, PlanValidationError::UnknownChildDependency { key, .. } if key == "e")
        ),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::DuplicateChildKey { key } if key == "c")),
        "{errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::ChildNoAcceptance { key } if key == "f")),
        "{errs:?}"
    );

    let mut cyclic = v2.clone();
    cyclic.children = vec![child("c", &["d"]), child("d", &["c"])];
    let errs = validate(&cyclic, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::CyclicChildDependency { .. })),
        "{errs:?}"
    );
}

#[test]
fn rejects_a_work_unit_with_the_reserved_integrate_kind() {
    let mut a = spec("a", &[]);
    a.kind = WorkUnitKind::Integrate;
    let p = plan(vec![a]);
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::ReservedKind { key } if key == "a")),
        "{errs:?}"
    );
}

#[test]
fn rejects_a_schema_that_is_neither_v1_nor_v2() {
    let mut p = plan(vec![spec("a", &[])]);
    p.schema = "celeris.execution-plan/99".to_string();
    let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            PlanValidationError::WrongSchema { found } if found == "celeris.execution-plan/99"
        )),
        "{errs:?}"
    );
}

/// v2 の topological order は工程順を tie-break にする（依存の無い WU 同士が工程をまたいでも、
/// 後の工程が先に選ばれない）。
#[test]
fn v2_topological_order_respects_phase_order_for_independent_units() {
    // `z`（verify 工程、依存なし）と `a`（build 工程、依存なし）は互いに依存が無いが、
    // key の昇順だけで tie-break すると `a` より `z` が先に来てしまう対象にした。
    let p = plan_v2(
        vec![phase("build"), phase("verify")],
        vec![spec_v2("z", "verify", &[]), spec_v2("a", "build", &[])],
    );
    let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
    let order: Vec<&str> = validated
        .topological_order
        .iter()
        .map(|&i| validated.spec.work_units[i].key.as_str())
        .collect();
    assert_eq!(order, vec!["a", "z"], "{order:?}");
}

/// ADR-0074 F5-fix（不具合 2 の再現）: v2・2 工程で `integrate-investigate`（daemon の統合 WU）と
/// 統合の repair WU が done のとき、planner の差分（`modify: [impl-quota]` だけ）を当てて
/// `validate` が通る。daemon 由来の WU は差分の結果に現れず（base の spec にも無い）、行は
/// 不変条件の対象から外れる。外さなければ（F5-1 dogfood の挙動）拒否され、文言に
/// 「daemon が足した WU は書かなくてよい」が出る。
#[test]
fn replan_delta_does_not_treat_daemon_added_done_units_as_changed() {
    let base = plan_v2(
        vec![phase("investigate"), phase("implement")],
        vec![
            spec_v2("inv-a", "investigate", &[]),
            spec_v2("inv-b", "investigate", &[]),
            spec_v2("impl-quota", "implement", &["inv-a"]),
            spec_v2("impl-api", "implement", &["inv-b"]),
            spec_v2("impl-ui", "implement", &["inv-a", "inv-b"]),
        ],
    );
    let validated = validate(&base, ExecutionLimits::default(), &[]).expect("valid base");
    let mut rows = materialize_work_units(
        "t",
        "p",
        &validated.spec,
        &validated.topological_order,
        "2026-09-27T00:00:00Z",
        &mut |w| format!("id-{}", w.key),
    );
    for r in rows.iter_mut() {
        if r.phase.as_deref() == Some("investigate") {
            r.status = WorkUnitStatus::Done;
        }
    }
    // 統合の repair WU（daemon が足す。計画の spec に無い、工程を持つ）も done。
    let mut repair = row_v2(
        "integ-repair-investigate-1",
        "investigate",
        2,
        WorkUnitStatus::Done,
        &[],
    );
    repair.kind = WorkUnitKind::Repair;
    rows.push(repair);
    let integ = rows
        .iter()
        .find(|r| r.key == "integrate-investigate")
        .expect("integration unit");
    assert!(is_daemon_added_work_unit(&validated.spec, integ));
    assert!(!is_daemon_added_work_unit(
        &validated.spec,
        rows.iter().find(|r| r.key == "inv-a").expect("inv-a")
    ));

    let d = delta(
        1,
        vec![],
        vec![WorkUnitPatch {
            key: "impl-quota".to_string(),
            objective: Some("quota を runs_by_role から数える（やり直し）".to_string()),
            ..Default::default()
        }],
        vec![],
    );
    let applied = apply_delta(&validated.spec, &d).expect("delta applies");
    assert!(
        applied
            .work_units
            .iter()
            .all(|w| w.kind != WorkUnitKind::Integrate
                && !w.key.starts_with(INTEGRATE_KEY_PREFIX)
                && w.key != "integ-repair-investigate-1"),
        "daemon 由来の WU は差分の結果（計画の spec）に入らない"
    );
    let done = replan_done_work_units(&validated.spec, &rows);
    let done_keys: Vec<&str> = done.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(done_keys, vec!["inv-a", "inv-b"]);
    validate(&applied, ExecutionLimits::default(), &done).expect("delta replan validates");

    // 全体形式でも、planner が統合 WU を書かなければ通る（daemon が採用時に補う）。
    let full = applied.clone();
    let revalidated = validate(&full, ExecutionLimits::default(), &done).expect("full replan");
    let integ_keys: Vec<String> = integration_work_unit_specs(&revalidated.spec)
        .into_iter()
        .map(|w| w.key)
        .collect();
    assert_eq!(
        integ_keys,
        vec!["integrate-investigate", "integrate-implement"]
    );

    // F5-1 dogfood の挙動（daemon 由来の WU も不変条件に入れる）は拒否され、文言が案内する。
    let naive: Vec<(String, WorkUnitSpec)> = rows
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done)
        .map(|u| (u.key.clone(), u.spec.clone()))
        .collect();
    let errs = validate(&applied, ExecutionLimits::default(), &naive).unwrap_err();
    let msg = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        msg.contains("done work unit integrate-investigate must not change on replan"),
        "{msg}"
    );
    assert!(msg.contains("daemon が足した WU"), "{msg}");
}

// ---- ADR-0079 R1a (a): /1・/2 の fixture の検証結果と出力 JSON が変わらないこと ----

/// fixture 1 本の「検証の結果と出力」を決定的な JSON にする（parse → 再直列化、検証の結果、
/// 採用時の行）。スナップショットは R1a の変更の**前**のコードで作った（`UPDATE_PLAN_FIXTURES=1`）。
fn fixture_outcome(text: &str) -> serde_json::Value {
    let spec: ExecutionPlanSpec = match serde_json::from_str(text) {
        Ok(s) => s,
        Err(e) => return serde_json::json!({ "parse_error": e.to_string() }),
    };
    let reserialized = serde_json::to_string(&spec).unwrap();
    match validate(&spec, ExecutionLimits::default(), &[]) {
        Err(errors) => serde_json::json!({
            "reserialized": reserialized,
            "errors": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
        }),
        Ok(v) => {
            let rows = materialize_work_units(
                "task",
                "plan",
                &v.spec,
                &v.topological_order,
                "T",
                &mut |w| format!("id-{}", w.key),
            );
            let rows: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id, "key": r.key, "seq": r.seq, "kind": r.kind.as_str(),
                        "status": r.status.as_str(), "phase": r.phase,
                        "depends_on": r.depends_on,
                        "spec": serde_json::to_string(&r.spec).unwrap(),
                    })
                })
                .collect();
            serde_json::json!({
                "reserialized": reserialized,
                "validated": serde_json::to_string(&v.spec).unwrap(),
                "topological_order": v.topological_order,
                "rounding_notes": v.rounding_notes,
                "rows": rows,
            })
        }
    }
}

#[test]
fn plan_v1_and_v2_fixtures_are_byte_identical() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/execution-plan");
    for name in [
        "v1-basic",
        "v1-invalid",
        "v2-phases",
        "v2-children",
        "v2-invalid",
    ] {
        let input = std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap();
        let outcome = serde_json::to_string_pretty(&fixture_outcome(&input)).unwrap() + "\n";
        let snap_path = format!("{dir}/{name}.expected.json");
        if std::env::var_os("UPDATE_PLAN_FIXTURES").is_some() {
            std::fs::write(&snap_path, &outcome).unwrap();
        }
        let expected =
            std::fs::read_to_string(&snap_path).unwrap_or_else(|e| panic!("read {snap_path}: {e}"));
        assert_eq!(expected, outcome, "fixture {name} changed");
    }
}

// ---- ADR-0079（Phase R1a）: `celeris.execution-plan/3` ----

fn tree_on() -> ExecutionLimits {
    ExecutionLimits {
        tree: crate::tree::TreeLimits {
            enabled: true,
            ..crate::tree::TreeLimits::default()
        },
        ..ExecutionLimits::default()
    }
}

fn v3_fixture() -> ExecutionPlanSpec {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/execution-plan/v3-browser.json"
    ))
    .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn v3_errors(spec: &ExecutionPlanSpec) -> Vec<PlanValidationError> {
    validate(spec, tree_on(), &[]).expect_err("must be rejected")
}

fn leaf(key: &str, stage: &str) -> PlanUnitSpec {
    PlanUnitSpec {
        requirements: Default::default(),
        expected_write_paths: None,
        key: key.into(),
        stage: stage.into(),
        kind: WorkUnitKind::Implement,
        title: format!("leaf {key}"),
        objective: format!("objective of leaf {key} that is distinct"),
        depends_on: vec![],
        needs_decisions: vec![],
        decisions: vec![],
        acceptance: vec![],
        genre: None,
        skills: vec![],
        repos: vec![],
        adopt: None,
        gate: None,
        done_when: vec![],
        checks: vec![WorkUnitCheck {
            cmd: "true".into(),
            expect_exit: 0,
            scope: false,
        }],
        context: UnitContext::default(),
        harness: None,
        budget: None,
        outputs: vec![],
        features: None,
    }
}

fn task_unit(key: &str, stage: &str) -> PlanUnitSpec {
    PlanUnitSpec {
        kind: WorkUnitKind::Task,
        title: format!("task {key}"),
        objective: format!("objective of child task {key} which differs"),
        checks: vec![],
        acceptance: vec![crate::model::Criterion {
            text: "ok".into(),
            check: crate::model::Check::Reviewer,
        }],
        ..leaf(key, stage)
    }
}

#[test]
fn plan_v3_accepts_expected_write_paths_and_rejects_invalid_format() {
    let mut plan = v3_fixture();
    plan.units[0].expected_write_paths = Some(vec!["src/".into(), "tests/a.rs".into()]);
    let validated = validate(&plan, tree_on(), &[]).unwrap();
    assert_eq!(
        validated.spec.units[0].expected_write_paths,
        Some(vec!["src".into(), "tests/a.rs".into()])
    );
    let value = serde_json::to_value(&plan).unwrap();
    let decoded: ExecutionPlanSpec = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded.units[0].expected_write_paths,
        plan.units[0].expected_write_paths
    );

    for invalid in ["", "/src", "src//x", "src/../x", "./src"] {
        plan.units[0].expected_write_paths = Some(vec![invalid.into()]);
        assert!(
            v3_errors(&plan)
                .iter()
                .any(|error| matches!(error, PlanValidationError::InvalidWritePaths { .. })),
            "{invalid:?}"
        );
    }
}

/// R1a (a): /3 の fixture が通り、段階ごとに統合 WU が付き、kind task の unit は `task` の行になり、
/// 回答を待つ決定（`needs_decisions` と `needed_before`）が行に写る。最初の段階だけが ready。
#[test]
fn v3_fixture_validates_and_materializes_stages_units_and_decisions() {
    let spec = v3_fixture();
    let v = validate(&spec, tree_on(), &[]).expect("valid /3");
    assert!(v.rounding_notes.is_empty());
    // JSON の往復（/3 は `stages`/`units`/`decisions` を出し、/2 の欄は空のまま）。
    let back: ExecutionPlanSpec =
        serde_json::from_str(&serde_json::to_string(&v.spec).unwrap()).unwrap();
    assert_eq!(back, spec);
    assert!(spec.work_units.is_empty() && spec.phases.is_empty());

    let rows = materialize_work_units("t", "plan", &v.spec, &v.topological_order, "T", &mut |w| {
        format!("id-{}", w.key)
    });
    type RowSummary<'a> = (String, &'a str, &'a str, Option<String>, Vec<String>);
    let summary: Vec<RowSummary> = rows
        .iter()
        .map(|r| {
            (
                r.key.clone(),
                r.kind.as_str(),
                r.status.as_str(),
                r.phase.clone(),
                r.needs_decisions.clone(),
            )
        })
        .collect();
    let s = |x: &str| x.to_string();
    assert_eq!(
        summary,
        vec![
            (s("p1"), "task", "ready", Some(s("phase-1")), vec![]),
            (s("p1-note"), "design", "ready", Some(s("phase-1")), vec![]),
            (
                s("integrate-phase-1"),
                "integrate",
                "pending",
                Some(s("phase-1")),
                vec![]
            ),
            (
                s("p2-a"),
                "task",
                "pending",
                Some(s("phase-2")),
                vec![s("h2")]
            ),
            (
                s("p2-b"),
                "task",
                "pending",
                Some(s("phase-2")),
                vec![s("h1"), s("h3")]
            ),
            (
                s("integrate-phase-2"),
                "integrate",
                "pending",
                Some(s("phase-2")),
                vec![]
            ),
            (
                s("p3"),
                "implement",
                "pending",
                Some(s("phase-3")),
                vec![s("h2")]
            ),
            (
                s("integrate-phase-3"),
                "integrate",
                "pending",
                Some(s("phase-3")),
                vec![]
            ),
        ]
    );
    // 統合 WU は段階のすべての unit（子 task を含む）に依存する。
    let integ2 = rows.iter().find(|r| r.key == "integrate-phase-2").unwrap();
    assert_eq!(
        integ2.depends_on,
        vec!["p2-a".to_string(), "p2-b".to_string()]
    );
    // kind task の unit は LLM run を起こさない（scheduler の候補にならない。子の生成は R1b）。
    assert_eq!(
        runnable_work_units(&rows, 0, 3),
        vec!["id-p1-note".to_string()]
    );
    // unit の `decisions` は計画の決定に `needed_before: [<unit>]` を付けて読む。
    let all = normalized_decisions(&spec);
    assert_eq!(
        all.iter().map(|d| d.key.as_str()).collect::<Vec<_>>(),
        vec!["h1", "h2", "h3"]
    );
    assert_eq!(all[2].needed_before, vec!["p2-b".to_string()]);
}

/// R1a (c): `[execution.tree] enabled = false`（既定）なら /3 は `TreeDisabled` だけで拒否される。
#[test]
fn v3_is_rejected_when_tree_is_disabled() {
    let errs = validate(&v3_fixture(), ExecutionLimits::default(), &[]).unwrap_err();
    assert_eq!(errs, vec![PlanValidationError::TreeDisabled]);
    let msg = errs[0].to_string();
    assert!(msg.contains("[execution.tree] enabled = true"), "{msg}");
    assert!(msg.contains(EXECUTION_PLAN_SCHEMA_V3), "{msg}");
}

/// R1a (b): leaf に機械的な検査が無い。
#[test]
fn rejects_leaf_without_checks() {
    let mut p = v3_fixture();
    p.units[1].checks.clear();
    let errs = v3_errors(&p);
    assert!(
        errs.contains(&PlanValidationError::LeafWithoutChecks {
            key: "p1-note".into()
        }),
        "{errs:?}"
    );
    assert!(errs.iter().any(|e| e.to_string().contains("task")));
}

/// R1a (b): leaf の `context.repo` が 2 つ。
#[test]
fn rejects_leaf_with_two_repos() {
    let mut p = v3_fixture();
    p.units[1].context.repo = Some(RepoSelector::Many(vec!["a".into(), "b".into()]));
    assert!(
        v3_errors(&p).contains(&PlanValidationError::LeafMultipleRepos {
            key: "p1-note".into(),
            count: 2
        })
    );
    // 1 つなら文字列でも配列でも通る。
    p.units[1].context.repo = Some(RepoSelector::Many(vec!["a".into()]));
    validate(&p, tree_on(), &[]).expect("one repo is fine");
}

/// R1a (b): kind task の unit に `checks`（と `budget` / `harness` / `context.paths`）。
#[test]
fn rejects_task_unit_with_checks() {
    let mut p = v3_fixture();
    p.units[0].checks = vec![WorkUnitCheck {
        cmd: "true".into(),
        expect_exit: 0,
        scope: false,
    }];
    p.units[0].budget = Some(WorkUnitBudget {
        max_turns: Some(10),
        max_wall_secs: None,
    });
    p.units[0].harness = Some("claude-code".into());
    p.units[0].context.paths = vec!["src/".into()];
    let errs = v3_errors(&p);
    for field in ["checks", "budget", "harness", "context.paths"] {
        assert!(
            errs.contains(&PlanValidationError::TaskUnitFieldNotAllowed {
                key: "p1".into(),
                field
            }),
            "{field}: {errs:?}"
        );
    }
}

/// R1a (b): kind task の unit に `acceptance` が無い。
#[test]
fn rejects_task_unit_without_acceptance() {
    let mut p = v3_fixture();
    p.units[0].acceptance.clear();
    assert!(
        v3_errors(&p).contains(&PlanValidationError::TaskUnitNoAcceptance { key: "p1".into() })
    );
    // human の受け入れ条件には成果物が要る（ADR-0067 D2。子の生成と同じ規則）。
    let mut p = v3_fixture();
    p.units[3].acceptance.truncate(1);
    assert!(v3_errors(&p).iter().any(|e| matches!(
        e,
        PlanValidationError::TaskUnitInvalidAcceptance { key, .. } if key == "p2-b"
    )));
}

/// R1a (b): /2 の `child:<key>` の依存は /3 では書けない。
#[test]
fn rejects_child_prefix_dependency_in_v3() {
    let mut p = v3_fixture();
    p.units[4].depends_on = vec!["child:p2-b".into()];
    assert!(
        v3_errors(&p).contains(&PlanValidationError::ChildDependencyNotAllowed {
            key: "p3".into(),
            depends_on: "child:p2-b".into()
        })
    );
}

/// R1a (b): /2 の `children` は /3 では書けない（`phases` / `work_units` も）。
#[test]
fn rejects_children_in_v3() {
    let mut p = v3_fixture();
    p.children = vec![child("c", &[])];
    p.phases = vec![phase("x")];
    p.work_units = vec![spec("w", &[])];
    let errs = v3_errors(&p);
    for field in ["children", "phases", "work_units"] {
        assert!(
            errs.contains(&PlanValidationError::V2FieldInV3 { field }),
            "{field}: {errs:?}"
        );
    }
    assert!(
        PlanValidationError::V2FieldInV3 { field: "children" }
            .to_string()
            .contains("kind \"task\"")
    );
}

/// R1a (b): 依存の循環（同じ段階の中で互いに依存）。
#[test]
fn rejects_cycle_in_v3() {
    let mut p = v3_fixture();
    // phase-2 の p2-a -> p2-b -> p2-a（どちらも同じ段階の依存は 1 つだけ）。
    p.units[2].depends_on = vec!["p2-b".into()];
    let errs = v3_errors(&p);
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::CyclicDependency { .. })),
        "{errs:?}"
    );
}

/// R1a (b): `needed_before` が計画に無い unit / 段階を指す。
#[test]
fn rejects_unknown_needed_before() {
    let mut p = v3_fixture();
    p.decisions[0].needed_before = vec!["nope".into(), "stage:phase-9".into()];
    let errs = v3_errors(&p);
    for target in ["nope", "stage:phase-9"] {
        assert!(
            errs.contains(&PlanValidationError::UnknownNeededBefore {
                key: "h1".into(),
                target: target.into()
            }),
            "{target}: {errs:?}"
        );
    }
    // 空の `needed_before`（何を止めるか書いていない）も拒否。
    let mut p = v3_fixture();
    p.decisions[0].needed_before.clear();
    assert!(v3_errors(&p).iter().any(|e| matches!(
        e,
        PlanValidationError::InvalidDecision { detail } if detail.contains("needed_before")
    )));
}

/// R1a (b): `needs_decisions` が計画に無い決定を指す。
#[test]
fn rejects_unknown_needs_decision() {
    let mut p = v3_fixture();
    p.units[2].needs_decisions = vec!["h9".into()];
    assert!(
        v3_errors(&p).contains(&PlanValidationError::UnknownNeedsDecision {
            key: "p2-a".into(),
            decision: "h9".into()
        })
    );
}

/// R1a (b): 段階あたり 7 unit（既定の上限 6）。
#[test]
fn rejects_seven_units_in_a_stage() {
    let mut p = v3_fixture();
    for i in 0..5 {
        p.units.push(leaf(&format!("extra-{i}"), "phase-1"));
    }
    assert_eq!(p.units.iter().filter(|u| u.stage == "phase-1").count(), 7);
    assert!(
        v3_errors(&p).contains(&PlanValidationError::TooManyUnitsInStage {
            stage: "phase-1".into(),
            count: 7,
            max: 6
        })
    );
    p.units.pop();
    validate(&p, tree_on(), &[]).expect("6 units fit");
}

/// R1a (b): 決定 9 件（計画あたりの上限 8）。
#[test]
fn rejects_nine_decisions() {
    let mut p = v3_fixture();
    let template = p.decisions[0].clone();
    for i in 0..6 {
        let mut d = template.clone();
        d.key = format!("x{i}");
        p.decisions.push(d);
    }
    assert_eq!(normalized_decisions(&p).len(), 9);
    assert!(v3_errors(&p).contains(&PlanValidationError::TooManyDecisions { count: 9, max: 8 }));
    p.decisions.pop();
    validate(&p, tree_on(), &[]).expect("8 decisions fit");
}

/// D2: 決定の key の重複と形（選択肢 1 つ・推奨が選択肢に無い）。
#[test]
fn rejects_duplicate_and_malformed_decisions() {
    let mut p = v3_fixture();
    p.decisions[1].key = "h1".into();
    p.decisions[0].options.truncate(1);
    let errs = v3_errors(&p);
    assert!(errs.contains(&PlanValidationError::DuplicateDecisionKey { key: "h1".into() }));
    assert!(errs.iter().any(|e| matches!(
        e,
        PlanValidationError::InvalidDecision { detail } if detail.contains("options")
    )));
}

/// U-R1: kind task の unit は `depth < max_depth` の task の計画だけ（既定 3 層: root・子は可、孫は不可）。
#[test]
fn rejects_task_unit_at_max_depth() {
    let p = v3_fixture();
    for depth in [1, 2] {
        validate_with(
            &p,
            tree_on(),
            &[],
            PlanContext {
                origin: PlanOrigin::Planner,
                depth,
            },
        )
        .unwrap_or_else(|e| panic!("depth {depth}: {e:?}"));
    }
    let errs = validate_with(
        &p,
        tree_on(),
        &[],
        PlanContext {
            origin: PlanOrigin::Planner,
            depth: 3,
        },
    )
    .unwrap_err();
    assert!(errs.contains(&PlanValidationError::ChildTaskTooDeep {
        key: "p1".into(),
        depth: 3,
        max_depth: 3
    }));
    // leaf だけの計画は最下段（depth 3）でも書ける。
    let mut leaves_only = p.clone();
    leaves_only.units.retain(|u| !u.is_task());
    leaves_only
        .units
        .iter_mut()
        .for_each(|u| u.depends_on.clear());
    leaves_only.decisions.clear();
    validate_with(
        &leaves_only,
        tree_on(),
        &[],
        PlanContext {
            origin: PlanOrigin::Planner,
            depth: 3,
        },
    )
    .expect("leaves are fine at the bottom level");
}

/// ADR-0079 R5b-fix1（2 つ目の規則）: planner の replan は、前の版の done の unit をそのまま写すなら `adopt` を
/// 残してよい（done の不変条件がそれを強いる）。done の写しでない unit の新しい `adopt` は従来どおり拒む。
#[test]
fn planner_replan_may_keep_adopt_on_a_verbatim_done_carry_over() {
    let mut p = v3_fixture();
    p.units[0].adopt = Some(crate::model::TaskId::new());
    let key = p.units[0].key.clone();
    let done = [(key.clone(), p.units[0].to_work_unit_spec())];
    validate_with(&p, tree_on(), &done, ctx(PlanOrigin::Planner))
        .expect("a verbatim done carry-over may keep its adopt");

    // done の写しでない（done が無い・spec が違う）unit の `adopt` は planner には許さない。
    let errs = validate_with(&p, tree_on(), &[], ctx(PlanOrigin::Planner)).unwrap_err();
    assert!(errs.contains(&PlanValidationError::AdoptNotAllowed { key: key.clone() }));
    let mut changed = p.clone();
    changed.units[0].title = "a different title for the adopted unit".into();
    let errs = validate_with(&changed, tree_on(), &done, ctx(PlanOrigin::Planner)).unwrap_err();
    assert!(errs.contains(&PlanValidationError::AdoptNotAllowed { key: key.clone() }));
    assert!(errs.contains(&PlanValidationError::DoneWorkUnitChanged { key: key.clone() }));
    let msg = PlanValidationError::AdoptNotAllowed { key }.to_string();
    assert!(msg.contains("copied verbatim"), "{msg}");
}

/// D2 / D15: `adopt` は人の計画（origin human）だけ。leaf には書けない。
#[test]
fn adopt_is_only_allowed_in_human_plans() {
    let mut p = v3_fixture();
    p.units[0].adopt = Some(crate::model::TaskId::new());
    assert!(v3_errors(&p).contains(&PlanValidationError::AdoptNotAllowed { key: "p1".into() }));
    validate_with(
        &p,
        tree_on(),
        &[],
        PlanContext {
            origin: PlanOrigin::Human,
            depth: 1,
        },
    )
    .expect("a human may adopt an existing task");
    let mut p = v3_fixture();
    p.units[1].adopt = Some(crate::model::TaskId::new());
    p.units[1].acceptance = p.units[0].acceptance.clone();
    let errs = validate_with(
        &p,
        tree_on(),
        &[],
        PlanContext {
            origin: PlanOrigin::Human,
            depth: 1,
        },
    )
    .unwrap_err();
    assert!(errs.contains(&PlanValidationError::LeafFieldNotAllowed {
        key: "p1-note".into(),
        field: "adopt"
    }));
    assert!(errs.contains(&PlanValidationError::LeafFieldNotAllowed {
        key: "p1-note".into(),
        field: "acceptance"
    }));
}

/// ADR-0079「R6-2」: kind task の unit の `gate`（`compound` | `atomic`）は人の計画・planner の計画のどちらでも
/// 読めて検証を通る。leaf に書けば `LeafFieldNotAllowed { field: "gate" }`、知らない値は JSON で拒否。
#[test]
fn plan_unit_gate_parses_on_task_units_and_is_rejected_on_leaves() {
    let mut v = serde_json::to_value(v3_fixture()).unwrap();
    v["units"][0]["gate"] = serde_json::json!("atomic");
    let p: ExecutionPlanSpec = serde_json::from_value(v.clone()).expect("gate parses");
    assert_eq!(
        p.units[0].gate,
        Some(crate::execution_gate::ExecutionMode::Atomic)
    );
    validate(&p, tree_on(), &[]).expect("a planner may write gate on a task unit");
    v["units"][0]["gate"] = serde_json::json!("compound");
    let p: ExecutionPlanSpec = serde_json::from_value(v.clone()).expect("gate parses");
    assert_eq!(
        p.units[0].gate,
        Some(crate::execution_gate::ExecutionMode::Compound)
    );
    validate_with(
        &p,
        tree_on(),
        &[],
        PlanContext {
            origin: PlanOrigin::Human,
            depth: 1,
        },
    )
    .expect("a human may write gate on a task unit");
    // 往復で残る（省けば出さない）。
    let back = serde_json::to_value(&p).unwrap();
    assert_eq!(back["units"][0]["gate"], "compound");
    assert!(back["units"][1].get("gate").is_none());
    v["units"][0]["gate"] = serde_json::json!("sometimes");
    assert!(serde_json::from_value::<ExecutionPlanSpec>(v.clone()).is_err());
    // leaf の `gate` は検証の誤り。
    let mut p = v3_fixture();
    p.units[1].gate = Some(crate::execution_gate::ExecutionMode::Compound);
    assert!(
        v3_errors(&p).contains(&PlanValidationError::LeafFieldNotAllowed {
            key: "p1-note".into(),
            field: "gate"
        })
    );
}

/// D4 (2) (a): /3 の leaf は予算を丸めずに拒否する（/1・/2 は従来どおり丸める）。
#[test]
fn v3_leaf_budget_over_the_limit_is_rejected_not_rounded() {
    let mut p = v3_fixture();
    p.units[1].budget = Some(WorkUnitBudget {
        max_turns: Some(81),
        max_wall_secs: Some(3601),
    });
    let errs = v3_errors(&p);
    assert_eq!(
        errs.iter()
            .filter(|e| matches!(e, PlanValidationError::LeafBudgetOverLimit { .. }))
            .count(),
        2,
        "{errs:?}"
    );
}

/// D2: 段階の形（空・上限・key・kind）と unit の段階・予約語。
#[test]
fn rejects_malformed_stages_and_unknown_unit_stage() {
    let mut p = v3_fixture();
    p.stages.push(StageSpec {
        key: "phase-1".into(),
        kind: WorkUnitKind::Task,
        title: "dup".into(),
        review: StageReview::None,
    });
    p.units[4].stage = "phase-9".into();
    let mut reserved = leaf("integrate-phase-1", "phase-1");
    reserved.kind = WorkUnitKind::Integrate;
    p.units.push(reserved);
    let errs = v3_errors(&p);
    assert!(errs.contains(&PlanValidationError::DuplicateStageKey {
        key: "phase-1".into()
    }));
    assert!(errs.contains(&PlanValidationError::InvalidStageKind {
        key: "phase-1".into()
    }));
    assert!(errs.contains(&PlanValidationError::UnknownUnitStage {
        key: "p3".into(),
        stage: "phase-9".into()
    }));
    assert!(errs.contains(&PlanValidationError::ReservedKey {
        key: "integrate-phase-1".into()
    }));
    assert!(errs.contains(&PlanValidationError::ReservedKind {
        key: "integrate-phase-1".into()
    }));

    let mut empty = v3_fixture();
    empty.stages.clear();
    empty.units.clear();
    empty.decisions.clear();
    let errs = v3_errors(&empty);
    assert!(errs.contains(&PlanValidationError::NoStages));
    assert!(errs.contains(&PlanValidationError::NoUnits));

    let mut many = v3_fixture();
    for i in 0..3 {
        many.stages.push(StageSpec {
            key: format!("more-{i}"),
            kind: WorkUnitKind::Test,
            title: format!("more {i}"),
            review: StageReview::None,
        });
    }
    assert!(v3_errors(&many).contains(&PlanValidationError::TooManyStages { count: 6, max: 5 }));
}

/// D3: 計画あたりの kind task の unit の上限（既定 6）。
#[test]
fn rejects_too_many_child_task_units() {
    let mut p = v3_fixture();
    p.decisions.clear();
    p.units.iter_mut().for_each(|u| {
        u.needs_decisions.clear();
        u.decisions.clear();
    });
    for i in 0..4 {
        p.units.push(task_unit(&format!("more-{i}"), "phase-3"));
    }
    assert!(v3_errors(&p).contains(&PlanValidationError::TooManyChildTasks { count: 7, max: 6 }));
}

/// ADR-0079 R7-2: `max_child_tasks_per_plan` は子を作る unit だけを数える。replan で持ち越す done の kind task の
/// unit は数えない（done 3 + 生きた 4、done 3 + 生きた 6 は上限 6 の内）。生きた 7 は拒否。
#[test]
fn child_task_limit_counts_only_units_that_are_not_done() {
    let stage = |key: &str| StageSpec {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        title: key.into(),
        review: StageReview::None,
    };
    let v3 = |live: usize| {
        // 段階あたりの上限（6）に当たらないよう、done は s1、生きた unit は s2 / s3 に置く。
        let mut units: Vec<PlanUnitSpec> = (0..3)
            .map(|i| task_unit(&format!("done-{i}"), "s1"))
            .collect();
        units.extend(
            (0..live).map(|i| task_unit(&format!("live-{i}"), if i < 4 { "s2" } else { "s3" })),
        );
        ExecutionPlanSpec {
            schema: EXECUTION_PLAN_SCHEMA_V3.into(),
            rationale: "r".into(),
            stages: vec![stage("s1"), stage("s2"), stage("s3")],
            units,
            ..plan(vec![])
        }
    };
    let done_of = |p: &ExecutionPlanSpec| -> Vec<(String, WorkUnitSpec)> {
        p.units
            .iter()
            .filter(|u| u.key.starts_with("done-"))
            .map(|u| (u.key.clone(), u.to_work_unit_spec()))
            .collect()
    };
    for live in [4, 6] {
        let p = v3(live);
        validate(&p, tree_on(), &done_of(&p))
            .unwrap_or_else(|e| panic!("3 done + {live} live must pass: {e:?}"));
    }
    // 3 done + 7 live: 生きた 7 だけを数える。
    let p = v3(7);
    let errs = validate(&p, tree_on(), &done_of(&p)).unwrap_err();
    assert!(
        errs.contains(&PlanValidationError::TooManyChildTasks { count: 7, max: 6 }),
        "{errs:?}"
    );
    // 最初の計画（done なし）は従来どおり全部を数える: 3 + 4 = 7。
    let p = v3(4);
    let errs = validate(&p, tree_on(), &[]).unwrap_err();
    assert!(
        errs.contains(&PlanValidationError::TooManyChildTasks { count: 7, max: 6 }),
        "{errs:?}"
    );
}

/// ADR-0079 付記「R7-3」D1（リファクタ retry の子の `HEAD^2`）: planner の replan は done の WU の `checks` だけを
/// 書き換えられる（段階の統合で再実行される check）。他の欄の変更・repair の計画の `checks` の変更は従来どおり拒否。
#[test]
fn planner_replan_may_change_only_the_checks_of_a_done_unit() {
    let mut done = spec("a", &[]);
    done.checks = vec![WorkUnitCheck {
        cmd: "git rev-parse HEAD^2".into(),
        expect_exit: 0,
        scope: false,
    }];
    let done_units = vec![("a".to_string(), done.clone())];
    let mut fixed = done.clone();
    fixed.checks = vec![WorkUnitCheck {
        cmd: "! git grep -n '<<<<<<<'".into(),
        expect_exit: 0,
        scope: false,
    }];
    let planner = PlanContext::default();
    let v = validate_with(
        &plan(vec![fixed.clone(), spec("b", &["a"])]),
        ExecutionLimits::default(),
        &done_units,
        planner,
    )
    .expect("the planner may rewrite the checks of a done unit");
    assert_eq!(
        done_work_unit_overrides(&v.spec, &done_units),
        vec![("a".to_string(), fixed.clone(), vec!["checks".to_string()])]
    );
    let mut objective_changed = fixed.clone();
    objective_changed.objective = "a different objective for a".into();
    let errs = validate_with(
        &plan(vec![objective_changed, spec("b", &["a"])]),
        ExecutionLimits::default(),
        &done_units,
        planner,
    )
    .unwrap_err();
    assert!(
        errs.contains(&PlanValidationError::DoneWorkUnitChanged { key: "a".into() }),
        "{errs:?}"
    );
    let repair = PlanContext {
        origin: PlanOrigin::Repair,
        depth: 1,
    };
    let errs = validate_with(
        &plan(vec![fixed, spec("b", &["a"])]),
        ExecutionLimits::default(),
        &done_units,
        repair,
    )
    .unwrap_err();
    assert!(
        errs.contains(&PlanValidationError::DoneWorkUnitChanged { key: "a".into() }),
        "{errs:?}"
    );
    assert!(
        PlanValidationError::DoneWorkUnitChanged { key: "a".into() }
            .to_string()
            .contains("`checks` だけ"),
        "the rejection tells the planner what it may change"
    );
}

/// ADR-0079 付記「R7-3」D1: /3 の replan で planner が done の unit に空でない `checks` を書けばそれを残し（他の欄は
/// 採用した spec に戻す）、書かなければ（省く・空）採用した spec のまま。検証も通る。
#[test]
fn carry_done_units_v3_keeps_the_planners_non_empty_checks() {
    let stage = |key: &str| StageSpec {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        title: key.into(),
        review: StageReview::None,
    };
    let mut merged = leaf("merge-old-tip", "s1");
    merged.checks = vec![WorkUnitCheck {
        cmd: "git rev-parse HEAD^2".into(),
        expect_exit: 0,
        scope: false,
    }];
    let active = ExecutionPlanSpec {
        schema: EXECUTION_PLAN_SCHEMA_V3.into(),
        rationale: "v1".into(),
        stages: vec![stage("s1")],
        units: vec![merged.clone(), leaf("other", "s1")],
        ..plan(vec![])
    };
    let done_keys: BTreeSet<String> = ["merge-old-tip".to_string(), "other".to_string()]
        .into_iter()
        .collect();
    let mut rewritten = merged.clone();
    rewritten.objective = "the planner paraphrased the objective".into();
    rewritten.checks = vec![WorkUnitCheck {
        cmd: "! git grep -n '<<<<<<<'".into(),
        expect_exit: 0,
        scope: false,
    }];
    let mut terse = leaf("other", "s1");
    terse.checks = vec![];
    let mut new = ExecutionPlanSpec {
        rationale: "v2".into(),
        units: vec![rewritten.clone(), terse, leaf("resolve-conflicts", "s1")],
        ..active.clone()
    };
    carry_done_units_v3(&active, &mut new, &done_keys);
    let get = |k: &str| new.units.iter().find(|u| u.key == k).unwrap().clone();
    assert_eq!(get("merge-old-tip").objective, merged.objective, "restored");
    assert_eq!(
        get("merge-old-tip").checks,
        rewritten.checks,
        "the new checks stay"
    );
    assert_eq!(
        get("other").checks,
        leaf("other", "s1").checks,
        "an empty copy does not drop the checks"
    );
    let done_units: Vec<(String, WorkUnitSpec)> = active
        .units
        .iter()
        .map(|u| (u.key.clone(), u.to_work_unit_spec()))
        .collect();
    let v = validate_with(&new, tree_on(), &done_units, PlanContext::default())
        .expect("a checks-only change of a done unit is valid");
    let overrides = done_work_unit_overrides(&v.spec, &done_units);
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].0, "merge-old-tip");
    assert_eq!(overrides[0].2, vec!["checks".to_string()]);
}

/// ADR-0079 付記「R7-3」D3（08:15Z の `UNIQUE constraint failed: work_units.task_id, key`）: 退役した行の key の
/// 再利用を検証の理由にする。unit の key と、前の版で消した段階の統合 WU の key（`integrate-<stage>`）の両方。
/// 生きた行の key（持ち越し）は当たらない。
#[test]
fn retired_key_errors_catch_unit_keys_and_removed_stage_keys() {
    let row = |key: &str, phase: &str, kind: WorkUnitKind, status: WorkUnitStatus| {
        let mut s = spec_v2(key, phase, &[]);
        s.kind = kind;
        WorkUnitRow::new(
            crate::new_id(),
            "t".into(),
            "p1".into(),
            0,
            s,
            status,
            "2026-09-30T00:00:00Z".into(),
        )
    };
    let rows = vec![
        row("a", "build", WorkUnitKind::Implement, WorkUnitStatus::Done),
        row(
            "old",
            "build",
            WorkUnitKind::Implement,
            WorkUnitStatus::Superseded,
        ),
        row(
            "integrate-build",
            "build",
            WorkUnitKind::Integrate,
            WorkUnitStatus::Pending,
        ),
        row(
            "integrate-extra",
            "extra",
            WorkUnitKind::Integrate,
            WorkUnitStatus::Superseded,
        ),
    ];
    let ok = plan_v2(
        vec![phase("build")],
        vec![spec_v2("a", "build", &[]), spec_v2("new", "build", &[])],
    );
    assert!(retired_key_errors(&ok, &rows).is_empty());
    let reuse = plan_v2(
        vec![phase("build"), phase("extra")],
        vec![
            spec_v2("a", "build", &[]),
            spec_v2("old", "build", &[]),
            spec_v2("e", "extra", &[]),
        ],
    );
    let errs = retired_key_errors(&reuse, &rows);
    assert_eq!(
        errs,
        vec![
            PlanValidationError::RetiredKeyReused {
                key: "old".into(),
                stage: None
            },
            PlanValidationError::RetiredKeyReused {
                key: "integrate-extra".into(),
                stage: Some("extra".into())
            },
        ]
    );
    assert!(errs[0].to_string().contains("choose a new key"));
    assert!(errs[1].to_string().contains("choose a new stage key"));
}

/// ADR-0079 付記「R7-3」D5: `max_units_per_stage` は生きた unit だけを数える（持ち越す done の unit と `adopt` の
/// unit は数えない）。最初の計画（done なし）は従来どおり。
#[test]
fn units_per_stage_limit_counts_only_live_units() {
    let stage = |key: &str| StageSpec {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        title: key.into(),
        review: StageReview::None,
    };
    let v3 = |live: usize| {
        let mut units: Vec<PlanUnitSpec> =
            (0..3).map(|i| leaf(&format!("done-{i}"), "s1")).collect();
        units.extend((0..live).map(|i| leaf(&format!("live-{i}"), "s1")));
        ExecutionPlanSpec {
            schema: EXECUTION_PLAN_SCHEMA_V3.into(),
            rationale: "r".into(),
            stages: vec![stage("s1")],
            units,
            ..plan(vec![])
        }
    };
    let done_of = |p: &ExecutionPlanSpec| -> Vec<(String, WorkUnitSpec)> {
        p.units
            .iter()
            .filter(|u| u.key.starts_with("done-"))
            .map(|u| (u.key.clone(), u.to_work_unit_spec()))
            .collect()
    };
    let p = v3(6);
    validate(&p, tree_on(), &done_of(&p)).expect("3 done + 6 live in one stage passes");
    let p = v3(7);
    let errs = validate(&p, tree_on(), &done_of(&p)).unwrap_err();
    assert!(
        errs.contains(&PlanValidationError::TooManyUnitsInStage {
            stage: "s1".into(),
            count: 7,
            max: 6
        }),
        "{errs:?}"
    );
    let p = v3(4);
    let errs = validate(&p, tree_on(), &[]).unwrap_err();
    assert!(
        errs.contains(&PlanValidationError::TooManyUnitsInStage {
            stage: "s1".into(),
            count: 7,
            max: 6
        }),
        "the first plan counts every unit: {errs:?}"
    );
}

/// ADR-0079 R7-2: /3 の JSON の大きさは `max_plan_json_bytes_v3`（既定 64 KiB）で測る（/1・/2 の 24 KiB は
/// 見ない）。拒否の文は planner に何を削るかを伝える。
#[test]
fn v3_plan_json_size_uses_its_own_limit_and_says_what_to_trim() {
    assert_eq!(ExecutionLimits::default().max_plan_json_bytes, 24 * 1024);
    assert_eq!(ExecutionLimits::default().max_plan_json_bytes_v3, 64 * 1024);
    let spec = v3_fixture();
    let small_v1 = ExecutionLimits {
        max_plan_json_bytes: 10,
        ..tree_on()
    };
    validate(&spec, small_v1, &[]).expect("/3 ignores the /1・/2 size limit");
    let small_v3 = ExecutionLimits {
        max_plan_json_bytes_v3: 10,
        ..tree_on()
    };
    let errs = validate(&spec, small_v3, &[]).unwrap_err();
    let too_large = errs
        .iter()
        .find(|e| matches!(e, PlanValidationError::PlanTooLarge { max: 10, .. }))
        .expect("PlanTooLarge");
    let text = too_large.to_string();
    assert!(text.contains("> 10 bytes"), "{text}");
    assert!(text.contains("objective は要点だけにし"), "{text}");
    assert!(
        text.contains("artifacts / 知識ベースのパスで参照"),
        "{text}"
    );
}

/// /1・/2 に /3 の欄・語彙を書けば拒否（`kind = task`・`stages` / `units` / `decisions`）。
#[test]
fn v1_and_v2_reject_v3_fields_and_the_task_kind() {
    let mut a = spec("a", &[]);
    a.kind = WorkUnitKind::Task;
    let mut p = plan(vec![a]);
    p.stages = v3_fixture().stages;
    p.units = vec![leaf("u", "phase-1")];
    p.decisions = v3_fixture().decisions;
    let errs = validate(&p, tree_on(), &[]).unwrap_err();
    assert!(errs.contains(&PlanValidationError::TaskKindRequiresV3 { key: "a".into() }));
    for field in ["stages", "units", "decisions"] {
        assert!(
            errs.contains(&PlanValidationError::V3FieldNotAllowed { field }),
            "{field}"
        );
    }
    let mut b = spec_v2("b", "build", &[]);
    b.kind = WorkUnitKind::Task;
    let errs = validate(&plan_v2(vec![phase("build")], vec![b]), tree_on(), &[]).unwrap_err();
    assert!(errs.contains(&PlanValidationError::TaskKindRequiresV3 { key: "b".into() }));
    // /1・/2 の検証は `[execution.tree]` を見ない（enabled でも既定でも同じ結果）。
    let v1 = plan(vec![spec("a", &[]), spec("b", &["a"])]);
    assert_eq!(
        validate(&v1, tree_on(), &[]),
        validate(&v1, ExecutionLimits::default(), &[])
    );
}

/// `schema` の値で /3 に振り分け、未知の版は従来どおり `WrongSchema`（文言は /3 を含む）。
#[test]
fn wrong_schema_message_lists_all_three_versions() {
    let msg = PlanValidationError::WrongSchema { found: "x".into() }.to_string();
    assert!(msg.contains(EXECUTION_PLAN_SCHEMA_V3), "{msg}");
    assert!(is_phased_schema(EXECUTION_PLAN_SCHEMA_V3));
    assert!(is_phased_schema(EXECUTION_PLAN_SCHEMA_V2));
    assert!(!is_phased_schema(EXECUTION_PLAN_SCHEMA));
}

/// D2: `deny_unknown_fields`（`assignee` / `tier` / `lane` は書けない）と `review` の語彙。
#[test]
fn v3_json_rejects_unknown_fields_and_parses_review() {
    let mut v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/execution-plan/v3-browser.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let parsed: ExecutionPlanSpec = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(parsed.stages[1].review, StageReview::Human);
    assert_eq!(parsed.stages[0].review, StageReview::None);
    v["units"][0]["assignee"] = serde_json::json!("dev");
    assert!(serde_json::from_value::<ExecutionPlanSpec>(v.clone()).is_err());
    v["units"][0].as_object_mut().unwrap().remove("assignee");
    v["stages"][0]["review"] = serde_json::json!("sometimes");
    assert!(serde_json::from_value::<ExecutionPlanSpec>(v).is_err());
}

/// ADR-0079 付記「R7-12」D1/D3（本番 01M3WZ1GEPC670GED0TYAXSGDF の再現）: /3 の計画がすべて done の後に配送が足した
/// repair WU（`kind = repair`、`phase: None`、計画の spec に無い）が done。dispatcher と同じ順（`replan_done_work_units` →
/// `carry_done_units_v3` → `validate_with` → `daemon_added_key_errors`）で、planner がその WU を書かない replan が通る。
/// 書けば検証の理由（`DaemonAddedKeyReused`）で返す。planner が自分で書いた repair の unit は従来どおり planner の WU。
#[test]
fn replan_skips_a_done_delivery_repair_unit_without_a_phase() {
    let stage = |key: &str| StageSpec {
        key: key.into(),
        kind: WorkUnitKind::Implement,
        title: key.into(),
        review: StageReview::None,
    };
    let active = ExecutionPlanSpec {
        schema: EXECUTION_PLAN_SCHEMA_V3.into(),
        rationale: "v1".into(),
        stages: vec![stage("design"), stage("impl"), stage("verify")],
        units: vec![
            leaf("adr", "design"),
            leaf("daemon-gate", "impl"),
            leaf("verify-all", "verify"),
        ],
        ..plan(vec![])
    };
    let validated = validate_with(&active, tree_on(), &[], PlanContext::default()).expect("v1");
    let mut rows = materialize_work_units(
        "t",
        "p1",
        &validated.spec,
        &validated.topological_order,
        "2026-10-02T00:00:00Z",
        &mut |w| format!("id-{}", w.key),
    );
    for r in rows.iter_mut() {
        r.status = WorkUnitStatus::Done;
    }
    // 配送の repair WU（`crates/celeris/src/delivery.rs` と同じ形）。
    let mut repair_spec = spec("repair-2", &[]);
    repair_spec.kind = WorkUnitKind::Repair;
    repair_spec.title = "repair (other): 配送の局所修復".into();
    assert_eq!(repair_spec.phase, None);
    let repair = WorkUnitRow::new(
        "id-repair-2".into(),
        "t".into(),
        "p1".into(),
        rows.len() as u32,
        repair_spec,
        WorkUnitStatus::Done,
        "2026-10-02T01:00:00Z".into(),
    );
    rows.push(repair.clone());

    assert!(is_daemon_added_work_unit(&validated.spec, &repair));
    let done = replan_done_work_units(&validated.spec, &rows);
    assert!(
        done.iter().all(|(k, _)| k != "repair-2"),
        "the delivery repair is not a planner-owned done unit: {done:?}"
    );
    let done_keys: BTreeSet<String> = done.iter().map(|(k, _)| k.clone()).collect();

    // planner の replan: done の unit を省き、verify に flaky test を直す unit を足す（repair-2 は書かない）。
    let mut new = ExecutionPlanSpec {
        rationale: "v2: fix the flaky browser test".into(),
        units: vec![leaf("fix-flake", "verify")],
        ..active.clone()
    };
    carry_done_units_v3(&validated.spec, &mut new, &done_keys);
    assert!(new.units.iter().all(|u| u.key != "repair-2"));
    let v = validate_with(&new, tree_on(), &done, PlanContext::default())
        .expect("a replan that omits the delivery repair unit is valid");
    assert!(daemon_added_key_errors(&validated.spec, &v.spec, &rows).is_empty());

    // 修正前の判定（`phase` を持つ行だけ）だったら repair-2 が done の不変条件に入り、どの計画も拒まれた。
    let mut with_repair = done.clone();
    with_repair.push(("repair-2".into(), repair.spec.clone()));
    let errs = validate_with(&new, tree_on(), &with_repair, PlanContext::default())
        .expect_err("the pre-R7-12 rule rejects every plan");
    assert!(
        errs.iter().any(|e| matches!(e,
            PlanValidationError::DoneWorkUnitChanged { key } if key == "repair-2")),
        "{errs:?}"
    );

    // planner が repair-2 を書き写したら（/3 は段階が要る）検証の理由で返す。
    let mut copied = leaf("repair-2", "verify");
    copied.kind = WorkUnitKind::Repair;
    let mut writes_it = new.clone();
    writes_it.units.push(copied);
    let v2 = validate_with(&writes_it, tree_on(), &done, PlanContext::default())
        .expect("structurally valid");
    let errs = daemon_added_key_errors(&validated.spec, &v2.spec, &rows);
    assert_eq!(
        errs,
        vec![PlanValidationError::DaemonAddedKeyReused {
            key: "repair-2".into()
        }]
    );
    assert!(errs[0].to_string().contains("R7-12"));

    // planner / 人が計画に書いた repair の unit（spec に key がある）は daemon の WU ではない。
    let mut planned_repair = leaf("repair-1", "verify");
    planned_repair.kind = WorkUnitKind::Repair;
    let mut active_with_planned = active.clone();
    active_with_planned.units.push(planned_repair);
    let mut planned_row = repair.clone();
    planned_row.key = "repair-1".into();
    planned_row.spec.key = "repair-1".into();
    assert!(!is_daemon_added_work_unit(
        &active_with_planned,
        &planned_row
    ));
    // superseded の daemon の行の key は D3 の対象外（退役した key は R7-3 の検証が扱う）。
    let mut retired = repair.clone();
    retired.status = WorkUnitStatus::Superseded;
    assert!(daemon_added_key_errors(&validated.spec, &v2.spec, &[retired]).is_empty());
    assert!(DAEMON_ADDED_HINT.contains("配送"));
}
