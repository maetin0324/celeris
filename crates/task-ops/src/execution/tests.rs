use super::*;
use std::path::PathBuf;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, SqliteStore, Task, TaskKind, Tier, WorkUnitContext,
    WorkUnitKind, WorkUnitSpec, WorkerHint, WorkspaceSpec,
};

fn wu(key: &str, depends_on: &[&str]) -> WorkUnitSpec {
    WorkUnitSpec {
        expected_write_paths: None,
        key: key.to_string(),
        kind: WorkUnitKind::Implement,
        title: format!("title {key}"),
        objective: format!("objective for the {key} step, spelled out plainly"),
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

fn spec() -> ExecutionPlanSpec {
    ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "A -> B -> C".to_string(),
        work_units: vec![wu("a", &[]), wu("b", &["a"]), wu("c", &["b"])],
        phases: Vec::new(),
        children: Vec::new(),
    }
}

fn sample_task() -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        expected_write_paths: None,
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
            text: "x".to_string(),
            check: Check::Human,
        }],
        inputs: vec![ArtifactRef {
            name: "n".to_string(),
            path: "p".to_string(),
            sha256: "s".to_string(),
            kind: "doc".to_string(),
            declared: true,
        }],
        depends_on: vec![],
        status: task_core::Status::Draft,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/tmp/ws"),
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
fn adopt_plan_rejects_a_missing_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    let err = adopt_plan(
        &store,
        TaskId::new(),
        spec(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::NotFound(_)));
}

#[test]
fn adopt_plan_creates_ready_and_pending_work_units_in_topological_order() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let plan = adopt_plan(
        &store,
        task.id,
        spec(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(plan.version, 1);
    assert_eq!(plan.origin, PlanOrigin::Human);

    let view = active_plan(&store, task.id).unwrap().unwrap();
    assert_eq!(view.plan.id, plan.id);
    assert_eq!(view.work_units.len(), 3);
    assert_eq!(view.work_units[0].key, "a");
    assert_eq!(view.work_units[0].status, WorkUnitStatus::Ready);
    assert_eq!(view.work_units[1].key, "b");
    assert_eq!(view.work_units[1].status, WorkUnitStatus::Pending);
    assert_eq!(view.work_units[2].key, "c");
    assert_eq!(view.work_units[2].status, WorkUnitStatus::Pending);
}

fn phase(key: &str, kind: WorkUnitKind) -> task_core::PhaseSpec {
    task_core::PhaseSpec {
        key: key.to_string(),
        kind,
        title: format!("phase {key}"),
    }
}

/// ADR-0074 D1.1（Phase F2）/ D2.1（Phase F3 途中確認）: `design` → `build` の 2 工程 v2 計画。
fn spec_v2_two_phases() -> ExecutionPlanSpec {
    let mut a = wu("a", &[]);
    a.phase = Some("design".to_string());
    let mut b = wu("b", &["a"]);
    b.phase = Some("build".to_string());
    ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA_V2.to_string(),
        rationale: "design -> build".to_string(),
        work_units: vec![a, b],
        phases: vec![
            phase("design", WorkUnitKind::Design),
            phase("build", WorkUnitKind::Implement),
        ],
        children: Vec::new(),
    }
}

/// ADR-0074 D2.1（Phase F3 途中確認、区切り 1 (a)）: v2 の計画を採用すると、`Task.routing.pause_after`
/// が工程の key の集合へ解決され、`ExecutionPlanned` と同じトランザクションで
/// `Event::PausePointsResolved` に残る。
#[test]
fn adopt_plan_resolves_pause_points_from_task_routing_for_v2_plans() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task();
    task.routing = Some(task_core::TaskRouting {
        pause_after: task_core::PausePolicy::EachPhase,
        ..Default::default()
    });
    store.insert(&task).unwrap();

    let plan = adopt_plan(
        &store,
        task.id,
        spec_v2_two_phases(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let events = store.events_for(task.id).unwrap();
    let resolved = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::PausePointsResolved {
                plan_id,
                phases,
                source,
            } if plan_id == &plan.id => Some((phases.clone(), *source)),
            _ => None,
        })
        .expect("PausePointsResolved recorded");
    // `EachPhase` = 最後の工程を除くすべて（"design" だけ）。
    assert_eq!(resolved.0, vec!["design".to_string()]);
    assert_eq!(resolved.1, task_core::PauseSource::Human);
}

/// (e): v1（`phases` が空）の計画では `pause_after` があっても無害（空集合を解決するだけ）。
#[test]
fn adopt_plan_resolves_no_pause_points_for_v1_plans() {
    let store = SqliteStore::open_in_memory().unwrap();
    let mut task = sample_task();
    task.routing = Some(task_core::TaskRouting {
        pause_after: task_core::PausePolicy::EachPhase,
        ..Default::default()
    });
    store.insert(&task).unwrap();

    let plan = adopt_plan(
        &store,
        task.id,
        spec(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let events = store.events_for(task.id).unwrap();
    let resolved = events.iter().find_map(|(_, e)| match e {
        Event::PausePointsResolved {
            plan_id, phases, ..
        } if plan_id == &plan.id => Some(phases.clone()),
        _ => None,
    });
    assert_eq!(resolved, Some(Vec::<String>::new()));
    // Task 自体は 1 バイトも変わらず（v1 は今までどおり動く）。
    assert_eq!(store.get(task.id).unwrap().unwrap().status, task.status);
}

#[test]
fn adopt_plan_rejects_an_invalid_plan_without_writing_anything() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let mut bad = spec();
    bad.work_units[1].depends_on = vec!["ghost".to_string()];
    let err = adopt_plan(
        &store,
        task.id,
        bad,
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    assert!(active_plan(&store, task.id).unwrap().is_none());
}

#[test]
fn adopt_plan_rejects_a_second_plan_for_the_same_task() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    adopt_plan(
        &store,
        task.id,
        spec(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let err = adopt_plan(
        &store,
        task.id,
        spec(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        matches!(err, OpsError::Store(task_core::StoreError::InUse { .. })),
        "{err:?}"
    );
}

#[test]
fn active_plan_is_none_for_a_task_without_a_plan() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    assert!(active_plan(&store, task.id).unwrap().is_none());
}

// ---- ADR-0072 D17（Phase E4）: replan ----

fn adopt(store: &SqliteStore, task_id: TaskId) -> PlanView {
    adopt_plan(
        store,
        task_id,
        spec(),
        PlanOrigin::Fixture,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    active_plan(store, task_id).unwrap().unwrap()
}

fn mark_done(store: &SqliteStore, task_id: TaskId, key: &str) {
    let view = active_plan(store, task_id).unwrap().unwrap();
    let row = view.work_units.iter().find(|u| u.key == key).unwrap();
    let mut updated = row.clone();
    updated.status = task_core::WorkUnitStatus::Done;
    store
        .work_unit_transition(
            task_id,
            updated,
            Event::WorkUnitTransitioned {
                work_unit_id: row.id.clone(),
                key: row.key.clone(),
                from: row.status,
                to: task_core::WorkUnitStatus::Done,
                reason: "completed".to_string(),
                run_id: None,
            },
        )
        .unwrap();
}

#[test]
fn replan_rejects_when_there_is_no_active_plan() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let err = replan(
        &store,
        task.id,
        spec(),
        "test".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
}

/// D17 の人の依頼の例: A は done、M（migration）を追加、B は blocked by M
/// （`depends_on: ["m"]`）、C は blocked by B。
#[test]
fn replan_keeps_done_work_units_and_applies_the_human_request_example() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let v1 = adopt(&store, task.id);
    mark_done(&store, task.id, "a");

    let mut v2_spec = spec();
    v2_spec.work_units[1].depends_on = vec!["a".to_string(), "m".to_string()]; // b: blocked by m
    v2_spec.work_units.insert(1, wu("m", &[])); // migration, no deps
    let (new_plan, diff) = replan(
        &store,
        task.id,
        v2_spec,
        "human request: add migration m before b".to_string(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(new_plan.version, 2);
    assert_eq!(new_plan.origin, PlanOrigin::Human);
    assert_eq!(diff.added, vec!["m".to_string()]);
    assert_eq!(diff.changed, vec!["b".to_string()]);
    assert!(diff.removed.is_empty(), "{diff:?}");

    // 旧版は superseded、新版が active。
    let plans = store.execution_plan_list(task.id).unwrap();
    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].id, v1.plan.id);
    assert_eq!(plans[0].status, PlanStatus::Superseded);
    assert_eq!(plans[1].id, new_plan.id);
    assert_eq!(plans[1].status, PlanStatus::Active);

    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 4, "{units:?}"); // a, b, c（保持）+ m（追加）
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, WorkUnitStatus::Done, "done の a は保持される");
    assert_eq!(a.plan_id, v1.plan.id, "done の行は元の plan_id のまま");
    let m = units.iter().find(|u| u.key == "m").unwrap();
    assert_eq!(m.status, WorkUnitStatus::Ready, "依存が無い m はすぐ ready");
    assert_eq!(m.plan_id, new_plan.id);
    let b = units.iter().find(|u| u.key == "b").unwrap();
    assert_eq!(
        b.status,
        WorkUnitStatus::Pending,
        "m がまだ done でないので b は pending"
    );
    assert_eq!(b.depends_on, vec!["a".to_string(), "m".to_string()]);
    let c = units.iter().find(|u| u.key == "c").unwrap();
    assert_eq!(
        c.status,
        WorkUnitStatus::Pending,
        "b 経由で m に依存 = pending"
    );

    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned { version: 2, supersedes: Some(s), .. } if *s == v1.plan.id
        )),
        "{events:?}"
    );
}

#[test]
fn replan_supersedes_work_units_that_are_dropped_from_the_new_plan() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    adopt(&store, task.id);
    mark_done(&store, task.id, "a");

    // c を落とす（b で終わる 2 段の計画に縮める）。
    let mut v2_spec = spec();
    v2_spec.work_units.truncate(2); // a, b だけ
    let (_, diff) = replan(
        &store,
        task.id,
        v2_spec,
        "drop c".to_string(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(diff.removed, vec!["c".to_string()]);

    let units = store.work_units_for(task.id).unwrap();
    let c = units.iter().find(|u| u.key == "c").unwrap();
    assert_eq!(c.status, WorkUnitStatus::Superseded);
}

/// ADR-0079 R5b-fix1: 人の replan は done の WU の spec（本番の task では `baseline` の check）を上書きできる。
/// 行は `done` のまま（`plan_id` / 依存も元のまま）spec だけが新しい版になり、`WorkUnitSpecOverridden` と
/// `ReplanDiff.overridden_done` に残る。events だけから作り直しても同じ行になる（replay）。
#[test]
fn human_replan_overrides_the_spec_of_a_done_work_unit() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let mut v1_spec = spec_v2_two_phases();
    v1_spec.work_units[0].checks = vec![task_core::WorkUnitCheck {
        cmd: "git diff --quiet 06e9a03cffe8 -- gui".to_string(),
        expect_exit: 0,
    }];
    let v1 = adopt_plan(
        &store,
        task.id,
        v1_spec.clone(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    mark_done(&store, task.id, "a");
    let before = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();

    let mut v2_spec = v1_spec.clone();
    let fixed_cmd =
        "git diff --quiet 06e9a03cffe8 -- gui \":!gui/docs/adr/0002-frontend-stack.md\"";
    v2_spec.work_units[0].checks[0].cmd = fixed_cmd.to_string();

    // repair の計画は従来どおり拒む（何も書かない）。planner は ADR-0079 R7-3 D1 から `checks` だけなら書き換え
    // られる（`planner_replan_rewrites_the_checks_of_a_done_unit`）。
    let err = replan(
        &store,
        task.id,
        v2_spec.clone(),
        "repair".to_string(),
        PlanOrigin::Repair,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, OpsError::Validation(m) if m.contains("done work unit a must not change")),
        "{err:?}"
    );

    let (new_plan, diff) = replan(
        &store,
        task.id,
        v2_spec,
        "human: correct the check of a".to_string(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(new_plan.version, 2);
    assert_eq!(diff.overridden_done, vec!["a".to_string()]);
    assert!(diff.added.is_empty(), "{diff:?}");
    assert!(diff.removed.is_empty(), "{diff:?}");

    let a = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap();
    assert_eq!(a.status, WorkUnitStatus::Done, "done のまま");
    assert_eq!(a.id, before.id);
    assert_eq!(a.plan_id, v1.id, "plan_id は元のまま");
    assert_eq!(a.depends_on, before.depends_on);
    assert_eq!(a.spec.checks[0].cmd, fixed_cmd);
    assert_eq!(a.phase.as_deref(), Some("design"));

    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkUnitSpecOverridden { work_unit_id, key, plan_id, plan_version: 2, changed_fields }
                if *work_unit_id == a.id && key == "a" && *plan_id == new_plan.id
                    && changed_fields == &vec!["checks".to_string()]
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned { version: 2, reason: Some(r), .. } if r.contains("overridden_done=a")
        )),
        "{events:?}"
    );

    // events だけから作り直しても同じ（spec の上書きを含む）。
    let (wu_mm, _, plan_mm, _) = crate::replay::check_and_apply_execution(&store, false).unwrap();
    assert!(wu_mm.is_empty(), "{wu_mm:?}");
    assert!(plan_mm.is_empty(), "{plan_mm:?}");
}

/// ADR-0079 R6-4: replan で別の段階（工程）へ移した未完了の unit は `work_units.phase` も新しい版の段階に
/// 書き換わる（R4a から既知の食い違い）。移していない unit はそのまま。events だけから作り直しても同じ。
#[test]
fn replan_rewrites_the_phase_of_a_unit_moved_to_another_phase() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let mut v1 = spec_v2_two_phases();
    let mut c = wu("c", &[]);
    c.phase = Some("build".to_string());
    v1.work_units.push(c.clone());
    adopt_plan(
        &store,
        task.id,
        v1.clone(),
        PlanOrigin::Fixture,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let phase_of = |key: &str| {
        store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == key)
            .and_then(|u| u.phase)
    };
    assert_eq!(phase_of("b").as_deref(), Some("build"));

    // b を build → design へ移す（c は build に残る）。
    let mut v2 = v1;
    v2.rationale = "move b to design".to_string();
    for w in v2.work_units.iter_mut() {
        if w.key == "b" {
            w.phase = Some("design".to_string());
        }
    }
    replan(
        &store,
        task.id,
        v2,
        "move b".to_string(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(phase_of("b").as_deref(), Some("design"));
    assert_eq!(phase_of("a").as_deref(), Some("design"));
    assert_eq!(phase_of("c").as_deref(), Some("build"));

    let (wu_mm, run_mm, plan_mm, applied) =
        crate::replay::check_and_apply_execution(&store, false).unwrap();
    assert!(wu_mm.is_empty(), "{wu_mm:?}");
    assert!(run_mm.is_empty(), "{run_mm:?}");
    assert!(plan_mm.is_empty(), "{plan_mm:?}");
    assert_eq!(applied, 0);
}

/// ADR-0079 R6-4（本番 01M3QGRC542ZC23996DNCTHZF5 の再現）: /3 の planner replan v2 が unit `x` を段階 `relay`
/// から `verify` へ移し、`verify` の unit `p` に依存させた。行の `phase` が `relay` のまま（`seq` も v1 の並び）
/// だと、`relay` に pending の unit が残るので `integrate-relay` が走れず、`x` は後の段階を待つ → 何も走れない
/// （`stall_detected{nothing_runnable}`）。replan は `phase` と `seq` を新しい版に直し、`relay` の残りが done に
/// なれば今の段階（seq 最小の未終端の行の段階）は `relay` のままで、その統合 WU が走れる。replay も同じ行を作る。
#[test]
fn replan_moving_a_unit_to_a_later_stage_does_not_strand_the_earlier_stage() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let limits = ExecutionLimits {
        tree: task_core::TreeLimits {
            enabled: true,
            ..task_core::TreeLimits::default()
        },
        ..ExecutionLimits::default()
    };
    let leaf = |key: &str, stage: &str, deps: &[&str]| {
        serde_json::json!({
            "key": key,
            "stage": stage,
            "kind": "implement",
            "title": format!("leaf {key}"),
            "objective": format!("objective of leaf {key} that is distinct"),
            "depends_on": deps,
            "checks": [{"cmd": "true", "expect_exit": 0}],
        })
    };
    let plan = |rationale: &str, units: Vec<serde_json::Value>| -> ExecutionPlanSpec {
        serde_json::from_value(serde_json::json!({
            "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": rationale,
            "stages": [
                {"key": "relay", "kind": "implement", "title": "relay"},
                {"key": "verify", "kind": "implement", "title": "verify"},
            ],
            "units": units,
        }))
        .unwrap()
    };
    let v1 = plan(
        "v1",
        vec![
            leaf("r1", "relay", &[]),
            leaf("x", "relay", &[]),
            leaf("p", "verify", &[]),
        ],
    );
    adopt_plan(
        &store,
        task.id,
        v1,
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let v2 = plan(
        "v2: x needs p",
        vec![
            leaf("r1", "relay", &[]),
            leaf("p", "verify", &[]),
            leaf("x", "verify", &["p"]),
        ],
    );
    let (_, diff) = replan(
        &store,
        task.id,
        v2,
        "x needs the launch".to_string(),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(diff.moved, vec!["x(relay→verify)".to_string()]);

    let rows = |store: &SqliteStore| {
        let mut rows: Vec<WorkUnitRow> = store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .filter(|u| u.status.is_active())
            .collect();
        rows.sort_by_key(|u| u.seq);
        rows
    };
    let order: Vec<(String, Option<String>)> =
        rows(&store).into_iter().map(|u| (u.key, u.phase)).collect();
    let s = |k: &str, p: &str| (k.to_string(), Some(p.to_string()));
    assert_eq!(
        order,
        vec![
            s("r1", "relay"),
            s("integrate-relay", "relay"),
            s("p", "verify"),
            s("x", "verify"),
            s("integrate-verify", "verify"),
        ]
    );

    // relay の残り（r1）が done → 今の段階は relay のまま、relay の unit はすべて done、統合 WU が待っている
    // （= scheduler の `settle_phase` が `Integrate(integrate-relay)` を返す形）。verify の unit はまだ上がらない。
    mark_done(&store, task.id, "r1");
    let now_rows = rows(&store);
    let current = now_rows
        .iter()
        .find(|u| !u.status.is_terminal())
        .and_then(|u| u.phase.clone());
    assert_eq!(current.as_deref(), Some("relay"));
    assert!(
        now_rows
            .iter()
            .filter(|u| u.phase.as_deref() == Some("relay")
                && u.kind != task_core::WorkUnitKind::Integrate)
            .all(|u| u.status == WorkUnitStatus::Done)
    );
    let integ = now_rows
        .iter()
        .find(|u| u.key == "integrate-relay")
        .unwrap();
    assert!(matches!(
        integ.status,
        WorkUnitStatus::Pending | WorkUnitStatus::Ready
    ));
    assert!(task_core::newly_ready(&now_rows).is_empty());

    // 段階の移動は `ExecutionPlanned.reason` に残る（タイムラインで見える）。
    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned { version: 2, reason: Some(r), .. }
                if r.contains("phase: x(relay→verify)")
        )),
        "{events:?}"
    );

    let (wu_mm, run_mm, plan_mm, applied) =
        crate::replay::check_and_apply_execution(&store, false).unwrap();
    assert!(wu_mm.is_empty(), "{wu_mm:?}");
    assert!(run_mm.is_empty(), "{run_mm:?}");
    assert!(plan_mm.is_empty(), "{plan_mm:?}");
    assert_eq!(applied, 0);
}

/// ADR-0079 R5b-fix1: planner の replan は done の WU の spec を変えられない（人の replan は上書きできる。
/// `human_replan_overrides_the_spec_of_a_done_work_unit`）。
#[test]
fn replan_rejects_a_changed_done_work_unit() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    adopt(&store, task.id);
    mark_done(&store, task.id, "a");

    let mut v2_spec = spec();
    v2_spec.work_units[0].objective = "a completely different objective now".to_string();
    let err = replan(
        &store,
        task.id,
        v2_spec,
        "test".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
    // 何も書き込まれていない。
    let units = store.work_units_for(task.id).unwrap();
    assert_eq!(units.len(), 3);
}

/// ADR-0074 F5-fix（不具合 2）: 全体形式の replan で planner が統合 WU を書かなくても、done の
/// `integrate-design`（daemon の統合 WU）と done の統合の repair WU は不変条件の対象にならず、
/// 行は base のまま持ち越される。新しい版の工程の統合 WU は daemon が補う。
#[test]
fn replan_carries_daemon_added_units_without_the_planner_restating_them() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let v1 = adopt_plan(
        &store,
        task.id,
        spec_v2_two_phases(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    mark_done(&store, task.id, "a");
    mark_done(&store, task.id, "integrate-design");
    let mut repair_spec = wu("integ-repair-design-1", &[]);
    repair_spec.kind = WorkUnitKind::Repair;
    repair_spec.phase = Some("design".to_string());
    let mut repair = task_core::WorkUnitRow::new(
        "wu-repair".to_string(),
        task.id.to_string(),
        v1.id.clone(),
        1,
        repair_spec,
        WorkUnitStatus::Done,
        "2026-09-27T00:00:00Z".to_string(),
    );
    repair.status = WorkUnitStatus::Done;
    store
        .work_units_apply(task.id, vec![repair], vec![], vec![])
        .unwrap();

    let mut new_spec = spec_v2_two_phases();
    new_spec.work_units[1].objective = "do b again, now with the quota fix".to_string();
    replan(
        &store,
        task.id,
        new_spec,
        "retry b".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("daemon-added done units must not block the replan");

    let units = store.work_units_for(task.id).unwrap();
    let get = |k: &str| units.iter().find(|u| u.key == k).unwrap();
    assert_eq!(get("integrate-design").status, WorkUnitStatus::Done);
    assert_eq!(get("integrate-design").plan_id, v1.id);
    assert_eq!(get("integ-repair-design-1").status, WorkUnitStatus::Done);
    assert_eq!(get("integ-repair-design-1").plan_id, v1.id);
    assert_eq!(get("b").status, WorkUnitStatus::Ready);
    assert_eq!(get("integrate-build").kind, WorkUnitKind::Integrate);
    assert_eq!(get("integrate-build").status, WorkUnitStatus::Pending);
}

#[test]
fn replan_rejects_reusing_a_superseded_key() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    adopt(&store, task.id);
    mark_done(&store, task.id, "a");

    let mut v2_spec = spec();
    v2_spec.work_units.truncate(2); // c を落とす（superseded になる）
    replan(
        &store,
        task.id,
        v2_spec,
        "drop c".to_string(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    // v3 で c を「新しい」key として使い回そうとすると拒否される（UNIQUE(task_id, key)）。
    let v3_spec = spec(); // a, b, c 全部（c は superseded 済みの key）
    let err = replan(
        &store,
        task.id,
        v3_spec,
        "reintroduce c".to_string(),
        PlanOrigin::Human,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(matches!(err, OpsError::Validation(_)), "{err:?}");
}

/// ADR-0079 付記「R7-3」D1（リファクタ retry の子の `HEAD^2`）: planner の replan は done の WU の `checks` だけを
/// 書き換えられる。行は done のまま（`plan_id` も元のまま）spec の `checks` だけが新しい版のものになり、
/// `WorkUnitSpecOverridden{changed_fields: ["checks"]}` が残る（段階の統合はこの行の check を走らせる）。replay も一致。
#[test]
fn planner_replan_rewrites_the_checks_of_a_done_unit() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let mut v1_spec = spec_v2_two_phases();
    v1_spec.work_units[0].checks = vec![task_core::WorkUnitCheck {
        cmd: "git rev-parse HEAD^2".to_string(),
        expect_exit: 0,
    }];
    let v1 = adopt_plan(
        &store,
        task.id,
        v1_spec.clone(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    mark_done(&store, task.id, "a");

    let mut v2_spec = v1_spec;
    let fixed = vec![task_core::WorkUnitCheck {
        cmd: "! git grep -n '<<<<<<<'".to_string(),
        expect_exit: 0,
    }];
    v2_spec.work_units[0].checks = fixed.clone();
    let (v2, diff) = replan(
        &store,
        task.id,
        v2_spec,
        "the HEAD^2 check cannot hold after the integration".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("a planner may rewrite the checks of a done unit");
    assert_eq!(diff.overridden_done, vec!["a".to_string()]);
    let units = store.work_units_for(task.id).unwrap();
    let a = units.iter().find(|u| u.key == "a").unwrap();
    assert_eq!(a.status, WorkUnitStatus::Done);
    assert_eq!(a.plan_id, v1.id, "the done row keeps its plan");
    assert_eq!(a.spec.checks, fixed);
    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkUnitSpecOverridden { key, plan_id, changed_fields, .. }
                if key == "a" && *plan_id == v2.id && changed_fields == &vec!["checks".to_string()]
        )),
        "{events:?}"
    );
    let (wu_mm, run_mm, plan_mm, applied) =
        crate::replay::check_and_apply_execution(&store, false).unwrap();
    assert!(wu_mm.is_empty(), "{wu_mm:?}");
    assert!(run_mm.is_empty(), "{run_mm:?}");
    assert!(plan_mm.is_empty(), "{plan_mm:?}");
    assert_eq!(applied, 0);
}

/// ADR-0079 付記「R7-3」D3（08:15Z の `UNIQUE constraint failed: work_units.task_id, key`）: 前の版で消した段階の key
/// を戻すと、その段階の統合 WU の key `integrate-<stage>` が退役した行と重なる。sqlite のエラーではなく検証の理由
/// （新しい段階の key を選ぶ）で拒否し、何も書かない。
#[test]
fn replan_rejects_a_removed_stage_key_as_a_validation_error() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    adopt_plan(
        &store,
        task.id,
        spec_v2_two_phases(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    mark_done(&store, task.id, "a");
    // v2: build の段階を消す（b と integrate-build は superseded）。
    let mut design_only = spec_v2_two_phases();
    design_only.work_units.truncate(1);
    design_only.phases.truncate(1);
    replan(
        &store,
        task.id,
        design_only,
        "drop build".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(
        store
            .work_units_for(task.id)
            .unwrap()
            .iter()
            .find(|u| u.key == "integrate-build")
            .map(|u| u.status),
        Some(WorkUnitStatus::Superseded)
    );
    let before = store.work_units_for(task.id).unwrap().len();
    // v3: build の段階を新しい unit の key（b2）で戻す。
    let mut back = spec_v2_two_phases();
    back.work_units[1].key = "b2".to_string();
    let err = replan(
        &store,
        task.id,
        back,
        "bring build back".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    match &err {
        OpsError::Validation(msg) => {
            assert!(msg.contains("stage key \"build\""), "{msg}");
            assert!(msg.contains("choose a new stage key"), "{msg}");
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
    assert_eq!(store.work_units_for(task.id).unwrap().len(), before);
}

/// ADR-0074 D6.2/§6 F1 (i)（Phase F1）: replan（planner）が新しい `kind = repair` の WU を書いたら
/// `Event::RepairScheduled{class: "planner"}` が残る（`execution_metrics` の `unknown` を無くす）。
#[test]
fn replan_records_repair_scheduled_for_a_planner_authored_repair_unit() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    adopt(&store, task.id);
    mark_done(&store, task.id, "a");

    let mut v2_spec = spec();
    let mut repair = wu("repair-1", &[]);
    repair.kind = task_core::WorkUnitKind::Repair;
    v2_spec.work_units.push(repair);
    let (_, diff) = replan(
        &store,
        task.id,
        v2_spec,
        "add a repair unit".to_string(),
        PlanOrigin::Planner,
        None,
        ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(diff.added, vec!["repair-1".to_string()]);

    let events = store.events_for(task.id).unwrap();
    let scheduled = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::RepairScheduled {
                key, class, origin, ..
            } if key == "repair-1" => Some((class.clone(), *origin)),
            _ => None,
        })
        .expect("a RepairScheduled event for repair-1");
    assert_eq!(scheduled.0, "planner");
    assert_eq!(scheduled.1, task_core::execution::RepairOrigin::Planner);
}

/// ADR-0079 付記「R7-9」D2（本番 01M3PAX6RVE7AX8Z6118KADME3 の再現）: /3 の 2 段階（`p4` と `inject`）がどちらも
/// 統合済み（統合 WU が done）の後、planner の replan が `p4` に `land2`、`inject` に `gaps` → `closeout` を足す。
/// 修正前は done の統合 WU をそのまま持ち越したので、足した unit は統合されないまま計画が完了した。修正後は両方の
/// 統合 WU を `pending` に戻し、依存に足した unit を入れ、`replan v2: stage_reopened` の遷移と
/// `stage_reopened: p4,inject` の reason を残す。段階の順も保つ（`inject` の新しい unit は `p4` の統合を待つ）。
/// replay は同じ行を作る。
#[test]
fn replan_adding_units_to_integrated_stages_reopens_their_integrations() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let limits = ExecutionLimits {
        tree: task_core::TreeLimits {
            enabled: true,
            ..task_core::TreeLimits::default()
        },
        ..ExecutionLimits::default()
    };
    let leaf = |key: &str, stage: &str, deps: &[&str]| {
        serde_json::json!({
            "key": key,
            "stage": stage,
            "kind": "implement",
            "title": format!("leaf {key}"),
            "objective": format!("objective of leaf {key} that is distinct"),
            "depends_on": deps,
            "checks": [{"cmd": "true", "expect_exit": 0}],
        })
    };
    let plan = |rationale: &str, units: Vec<serde_json::Value>| -> ExecutionPlanSpec {
        serde_json::from_value(serde_json::json!({
            "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": rationale,
            "stages": [
                {"key": "p4", "kind": "implement", "title": "phase 4"},
                {"key": "inject", "kind": "implement", "title": "phase 4 inject"},
            ],
            "units": units,
        }))
        .unwrap()
    };
    adopt_plan(
        &store,
        task.id,
        plan(
            "v1",
            vec![leaf("p4a", "p4", &[]), leaf("p4b", "inject", &["p4a"])],
        ),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    for key in ["p4a", "integrate-p4", "p4b", "integrate-inject"] {
        mark_done(&store, task.id, key);
    }

    let (_, diff) = replan(
        &store,
        task.id,
        plan(
            "v2: land the phase and close the gaps",
            vec![
                leaf("p4a", "p4", &[]),
                leaf("land2", "p4", &[]),
                leaf("p4b", "inject", &["p4a"]),
                leaf("gaps", "inject", &[]),
                leaf("closeout", "inject", &["gaps"]),
            ],
        ),
        "replan (planner run)".to_string(),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(diff.added, vec!["land2", "gaps", "closeout"]);
    assert_eq!(diff.reopened_stages, vec!["p4", "inject"]);

    let rows = store.work_units_for(task.id).unwrap();
    let row = |key: &str| rows.iter().find(|u| u.key == key).unwrap().clone();
    let p4 = row("integrate-p4");
    assert_eq!(p4.status, WorkUnitStatus::Pending);
    assert_eq!(p4.depends_on, vec!["p4a", "land2"]);
    assert_eq!(p4.spec.depends_on, p4.depends_on);
    let inject = row("integrate-inject");
    assert_eq!(inject.status, WorkUnitStatus::Pending);
    assert_eq!(inject.depends_on, vec!["p4b", "gaps", "closeout"]);
    // 段階の順: p4 の新しい unit は走れる、inject の新しい unit は p4 の統合を待つ。
    assert_eq!(row("land2").status, WorkUnitStatus::Ready);
    assert_eq!(row("gaps").status, WorkUnitStatus::Pending);
    assert_eq!(row("closeout").status, WorkUnitStatus::Pending);
    assert!(task_core::stale_stage_integrations(&rows).is_empty());

    let events = store.events_for(task.id).unwrap();
    let reopened: Vec<(String, String)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkUnitTransitioned {
                key,
                from: WorkUnitStatus::Done,
                to: WorkUnitStatus::Pending,
                reason,
                ..
            } => Some((key.clone(), reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        reopened,
        vec![
            (
                "integrate-p4".to_string(),
                "replan v2: stage_reopened".to_string()
            ),
            (
                "integrate-inject".to_string(),
                "replan v2: stage_reopened".to_string()
            ),
        ]
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned { version: 2, reason: Some(r), .. }
                if r.contains("(stage_reopened: p4,inject)")
        )),
        "{events:?}"
    );
    let replay_is_clean = |store: &SqliteStore| {
        let (wu_mm, run_mm, plan_mm, applied) =
            crate::replay::check_and_apply_execution(store, false).unwrap();
        assert!(wu_mm.is_empty(), "{wu_mm:?}");
        assert!(run_mm.is_empty(), "{run_mm:?}");
        assert!(plan_mm.is_empty(), "{plan_mm:?}");
        assert_eq!(applied, 0);
    };
    replay_is_clean(&store);

    // 何も足さない replan（done の統合 WU の依存が段階の unit をすべて持つ）は統合 WU に触れない。
    for key in [
        "land2",
        "integrate-p4",
        "gaps",
        "closeout",
        "integrate-inject",
    ] {
        mark_done(&store, task.id, key);
    }
    let (_, diff) = replan(
        &store,
        task.id,
        plan(
            "v3: nothing to add",
            vec![
                leaf("p4a", "p4", &[]),
                leaf("land2", "p4", &[]),
                leaf("p4b", "inject", &["p4a"]),
                leaf("gaps", "inject", &[]),
                leaf("closeout", "inject", &["gaps"]),
            ],
        ),
        "replan (planner run)".to_string(),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert!(diff.reopened_stages.is_empty());
    let rows = store.work_units_for(task.id).unwrap();
    assert!(
        rows.iter()
            .filter(|u| u.kind == task_core::WorkUnitKind::Integrate)
            .all(|u| u.status == WorkUnitStatus::Done)
    );
    replay_is_clean(&store);
}

/// ADR-0079 付記「R7-12」（本番 01M3WZ1GEPC670GED0TYAXSGDF の再現）: /3 の計画（design / impl / verify）がすべて done
/// → 配送が main の移動で task を開き直し、配送の repair WU `repair-2`（`kind = repair`、`phase: None`、計画の spec に
/// 無い。`crates/celeris/src/delivery.rs` と同じ行・event）を足して done → 再レビューの不合格 → planner の replan が
/// repair-2 を書かない。修正前は「done work unit repair-2 must not change on replan」で毎回拒否された。
/// 修正後は採用され、repair-2 の行は触れられずに残り（done、元の `plan_id` / `seq`）、replay も planner の行を同じに作る。
/// 人の PUT（origin human）の replan も同じく通る。repair-2 を計画に書いた replan は検証の理由で拒む。
#[test]
fn replan_after_a_done_delivery_repair_unit_without_a_phase_is_adopted() {
    let store = SqliteStore::open_in_memory().unwrap();
    let task = sample_task();
    store.insert(&task).unwrap();
    let limits = ExecutionLimits {
        tree: task_core::TreeLimits {
            enabled: true,
            ..task_core::TreeLimits::default()
        },
        ..ExecutionLimits::default()
    };
    let leaf = |key: &str, stage: &str, deps: &[&str]| {
        serde_json::json!({
            "key": key,
            "stage": stage,
            "kind": "implement",
            "title": format!("leaf {key}"),
            "objective": format!("objective of leaf {key} that is distinct"),
            "depends_on": deps,
            "checks": [{"cmd": "true", "expect_exit": 0}],
        })
    };
    let plan = |rationale: &str, units: Vec<serde_json::Value>| -> ExecutionPlanSpec {
        serde_json::from_value(serde_json::json!({
            "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": rationale,
            "stages": [
                {"key": "design", "kind": "design", "title": "design"},
                {"key": "impl", "kind": "implement", "title": "implement"},
                {"key": "verify", "kind": "test", "title": "verify"},
            ],
            "units": units,
        }))
        .unwrap()
    };
    let v1_units = || {
        vec![
            leaf("adr", "design", &[]),
            leaf("daemon-gate", "impl", &[]),
            leaf("web-unit", "impl", &[]),
            leaf("verify-all", "verify", &[]),
        ]
    };
    let v1 = adopt_plan(
        &store,
        task.id,
        plan("v1", v1_units()),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    for key in [
        "adr",
        "integrate-design",
        "daemon-gate",
        "web-unit",
        "integrate-impl",
        "verify-all",
        "integrate-verify",
    ] {
        mark_done(&store, task.id, key);
    }

    // 配送の repair WU（delivery.rs の「計画がある」枝と同じ: active な計画の id、seq は続き、ready、
    // `WorkUnitTransitioned{pending → ready, "delivery_repair"}`）。その後 done。
    let units = store.work_units_for(task.id).unwrap();
    let seq = units.iter().map(|u| u.seq).max().unwrap_or(0) + 1;
    let mut repair_spec = wu("repair-2", &[]);
    repair_spec.kind = WorkUnitKind::Repair;
    repair_spec.title = "repair (other): 配送の局所修復".to_string();
    repair_spec.budget = Some(task_core::WorkUnitBudget {
        max_turns: Some(20),
        max_wall_secs: Some(1800),
    });
    let repair = task_core::WorkUnitRow::new(
        task_core::new_id(),
        task.id.to_string(),
        v1.id.clone(),
        seq,
        repair_spec,
        WorkUnitStatus::Ready,
        "2026-10-02T00:00:00Z".to_string(),
    );
    assert_eq!(repair.phase, None);
    store
        .work_units_apply(
            task.id,
            vec![repair.clone()],
            vec![],
            vec![Event::WorkUnitTransitioned {
                work_unit_id: repair.id.clone(),
                key: repair.key.clone(),
                from: WorkUnitStatus::Pending,
                to: WorkUnitStatus::Ready,
                reason: "delivery_repair".into(),
                run_id: None,
            }],
        )
        .unwrap();
    mark_done(&store, task.id, "repair-2");

    // planner の replan（done の unit を書き写し、verify に flaky test を直す unit を足す。repair-2 は書かない）。
    let mut v2_units = v1_units();
    v2_units.push(leaf("fix-flake", "verify", &[]));
    let (_, diff) = replan(
        &store,
        task.id,
        plan("v2: fix the flaky browser test", v2_units.clone()),
        "replan (planner run)".to_string(),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .expect("the delivery repair unit must not block the planner's replan");
    assert_eq!(diff.added, vec!["fix-flake"]);
    assert!(diff.removed.is_empty(), "{diff:?}");
    assert_eq!(diff.reopened_stages, vec!["verify"]);

    let rows = store.work_units_for(task.id).unwrap();
    let row = |key: &str| rows.iter().find(|u| u.key == key).unwrap().clone();
    let kept = row("repair-2");
    assert_eq!(kept.status, WorkUnitStatus::Done);
    assert_eq!(kept.plan_id, v1.id, "the row is carried untouched");
    assert_eq!(kept.seq, seq);
    assert_eq!(kept.phase, None);
    assert_eq!(row("fix-flake").status, WorkUnitStatus::Ready);
    assert_eq!(row("integrate-verify").status, WorkUnitStatus::Pending);

    // replay: planner の行・統合 WU・計画の版は stored と一致する。daemon の repair WU の行は spec を運ぶ event が無いので
    // replay は作らない（従来からの既知の差。ADR-0079 R7-12「残したもの」）。差はそれだけで、replan はその行に触れない。
    let check_replay = |store: &SqliteStore| {
        let (wu_mm, run_mm, plan_mm, applied) =
            crate::replay::check_and_apply_execution(store, false).unwrap();
        assert!(
            wu_mm
                .iter()
                .all(|m| m.key == "repair-2" && m.field == "presence"),
            "{wu_mm:?}"
        );
        assert!(run_mm.is_empty(), "{run_mm:?}");
        assert!(plan_mm.is_empty(), "{plan_mm:?}");
        assert_eq!(applied, 0);
    };
    check_replay(&store);

    // planner が repair-2 を計画に書いた（段階を付けて写した）replan は、done の行を ready に戻さず検証の理由で拒む。
    let mut writes_it = v2_units.clone();
    let mut copied = leaf("repair-2", "verify", &[]);
    copied["kind"] = serde_json::json!("repair");
    writes_it.push(copied);
    let err = replan(
        &store,
        task.id,
        plan("v3: copy the delivery repair", writes_it),
        "replan (planner run)".to_string(),
        PlanOrigin::Planner,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, OpsError::Validation(m) if m.contains("repair-2") && m.contains("R7-12")),
        "{err:?}"
    );
    assert_eq!(
        store
            .work_units_for(task.id)
            .unwrap()
            .iter()
            .find(|u| u.key == "repair-2")
            .unwrap()
            .status,
        WorkUnitStatus::Done
    );

    // 人の PUT（origin human）の replan も repair-2 を書かずに通る。
    let mut v3_units = v2_units;
    v3_units.push(leaf("note", "verify", &[]));
    replan(
        &store,
        task.id,
        plan("v3: human adds a note", v3_units),
        "PUT /tasks/{id}/execution-plan".to_string(),
        PlanOrigin::Human,
        None,
        limits,
        OffsetDateTime::now_utc(),
    )
    .expect("a human replan omitting the delivery repair unit is adopted");
    let rows = store.work_units_for(task.id).unwrap();
    let kept = rows.iter().find(|u| u.key == "repair-2").unwrap();
    assert_eq!(kept.status, WorkUnitStatus::Done);
    assert_eq!(kept.plan_id, v1.id);
    check_replay(&store);
}
