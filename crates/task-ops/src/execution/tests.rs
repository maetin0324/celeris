use super::*;
use std::path::PathBuf;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, SqliteStore, Task, TaskKind, Tier, WorkUnitContext,
    WorkUnitKind, WorkUnitSpec, WorkerHint, WorkspaceSpec,
};

fn wu(key: &str, depends_on: &[&str]) -> WorkUnitSpec {
    WorkUnitSpec {
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

    // planner の replan は従来どおり拒む（何も書かない）。
    let err = replan(
        &store,
        task.id,
        v2_spec.clone(),
        "planner".to_string(),
        PlanOrigin::Planner,
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
