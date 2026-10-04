use super::*;

/// U-R1: task の層数で数える（root 1 / 子 2 / 孫 3）。既定 3 では root と子が子 task を持て、
/// 孫は持てない。`max_depth = 1` なら root も持てない。
#[test]
fn depth_counts_task_levels_not_leaves() {
    assert!(can_have_child_tasks(1, 3));
    assert!(can_have_child_tasks(2, 3));
    assert!(!can_have_child_tasks(3, 3));
    assert!(!can_have_child_tasks(1, 1));
    assert!(can_have_child_tasks(1, 2));
    assert!(!can_have_child_tasks(2, 2));
    assert_eq!(remaining_depth(1, 3), 2);
    assert_eq!(remaining_depth(2, 3), 1);
    assert_eq!(remaining_depth(3, 3), 0);
    assert_eq!(remaining_depth(4, 3), 0);
}

/// D3: 閾値は `5 + step × (d − 1)`（depth 2・step 2 は 7）。
#[test]
fn gate_threshold_rises_with_depth_formula() {
    assert_eq!(gate_threshold(1, 2), 5);
    assert_eq!(gate_threshold(2, 2), 7);
    assert_eq!(gate_threshold(3, 2), 9);
    assert_eq!(gate_threshold(3, 0), 5);
}

#[test]
fn tree_limits_defaults_follow_d3_and_u_r4() {
    let l = TreeLimits::default();
    assert!(!l.enabled);
    assert_eq!(l.max_depth, 3);
    assert_eq!(l.max_units_per_stage, 6);
    assert_eq!(l.max_stages, 5);
    assert_eq!(l.max_child_tasks_per_plan, 6);
    assert_eq!(l.max_parallel_child_tasks, 2);
    // ADR-0079「R6-2」で 40 / 120 / 10 から上げた。
    assert_eq!(l.max_tree_leaves, 120);
    assert_eq!(l.max_tree_runs, 400);
    assert_eq!(l.max_tree_replans, 30);
    assert_eq!(l.max_tree_tokens, None);
    assert_eq!(l.max_open_decisions_per_tree, 12);
    assert_eq!(l.max_open_decisions_per_plan, 8);
    assert_eq!(l.gate_depth_step, 2);
    assert_eq!(l.approval_near_limit_permille, 800);
}

/// `tree` の JSON は無ければ省略でき、あれば round-trip する（`Task.tree` は serde(default)）。
#[test]
fn tree_info_round_trips_and_child_depth_is_parent_plus_one() {
    let root_id = TaskId::new();
    let root = TreeInfo::root(root_id);
    let json = serde_json::to_string(&root).unwrap();
    assert_eq!(
        json,
        format!("{{\"root_id\":\"{root_id}\",\"depth\":1}}"),
        "optional fields are omitted"
    );
    let back: TreeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back, root);
}

// ---- Phase R2a: unit の gate・木の上限 ----

use crate::execution_gate::ExecutionMode;
use crate::execution_plan::{
    ExecutionLimits, ExecutionPlanSpec, PlanContext, PlanOrigin, PlanUnitSpec, PlanValidationError,
    RunIndexRole, RunIndexStatus, RunRow, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow,
    WorkUnitSpec, WorkUnitStatus, validate_with,
};
use crate::model::{
    ArtifactRef, Budget, Check, Status, TaskCategory, TaskKind, TaskMode, TaskRouting, Tier, Usage,
    WorkerHint, WorkspaceSpec,
};

fn parent_task(max_turns: u32) -> Task {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    Task {
        tree: None,
        paused_at: None,
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "親".to_string(),
        objective: "親の目的".to_string(),
        acceptance: vec![],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status: Status::Running,
        priority: 10,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp/x"),
        repos: vec![],
        budget: Budget {
            max_turns,
            max_wall_secs: 1800,
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
        assignee: None,
        labels: vec![],
        category: TaskCategory::Other,
        skills: vec!["rust".into()],
        mode: TaskMode::Production,
        conversation: None,
        routing: Some(TaskRouting::default()),
    }
}

/// 規則表の強制規則 `compound/long-and-broad`（深さに関わらず compound）に当てるヒント。
fn broad() -> serde_json::Value {
    serde_json::json!({"expected_length": "high", "cross_cutting": "high"})
}

fn leaf_spec(key: &str, stage: &str, compound: bool) -> PlanUnitSpec {
    let mut v = serde_json::json!({
        "key": key, "stage": stage, "kind": "implement",
        "title": format!("Leaf {key}"),
        "objective": format!("Implement the {key} piece"),
        "done_when": [format!("{key} works")],
        "checks": [{"cmd": format!("test -f {key}.txt"), "expect_exit": 0}],
        "context": {"paths": ["src/"], "repo": "app"},
    });
    if compound {
        v["features"] = broad();
    }
    serde_json::from_value(v).unwrap()
}

fn task_spec(key: &str, stage: &str, compound: bool) -> PlanUnitSpec {
    let mut v = serde_json::json!({
        "key": key, "stage": stage, "kind": "task",
        "title": format!("Child {key}"),
        "objective": format!("Deliver the {key} part"),
        "acceptance": [{"text": format!("{key} passes"), "check": {"type": "command", "cmd": format!("make {key}"), "expect_exit": 0}}],
    });
    if compound {
        v["features"] = broad();
    }
    serde_json::from_value(v).unwrap()
}

fn plan(stages: &[(&str, &str)], units: Vec<PlanUnitSpec>) -> ExecutionPlanSpec {
    serde_json::from_value(serde_json::json!({
            "schema": crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": "r",
            "stages": stages.iter().map(|(k, kind)| serde_json::json!({"key": k, "kind": kind, "title": k})).collect::<Vec<_>>(),
            "units": units,
        }))
        .unwrap()
}

fn enabled() -> TreeLimits {
    TreeLimits {
        enabled: true,
        ..TreeLimits::default()
    }
}

fn gate_of(parent: &Task, depth: u32, unit: &PlanUnitSpec, needs: &[String]) -> UnitGate {
    let limits = enabled();
    let names = vec!["app".to_string()];
    let ctx = UnitGateContext {
        parent,
        parent_depth: depth,
        parent_repo_names: &names,
        limits: &limits,
        work_unit_max_turns: 80,
        work_unit_max_wall_secs: 3600,
    };
    unit_gate(&ctx, unit, needs)
}

fn v3_limits() -> ExecutionLimits {
    ExecutionLimits {
        tree: enabled(),
        ..ExecutionLimits::default()
    }
}

/// R6-2（R5b-fix3 を改める）: kind task の unit は計画の書き手の明示。`gate` を省けば `human/explicit` の
/// compound（人の計画・planner の計画を問わない）、`gate: atomic` なら `human/explicit` の atomic。どちらも
/// 上書きしない（記録なし）。view は子 task と同じ `execution_hint` と予算を持つ。leaf には何も足さない。
#[test]
fn task_units_carry_the_explicit_gate_of_the_plan_author() {
    use crate::execution_gate::ExecutionHintSpec;
    let mut parent = parent_task(30);
    parent.budget.max_turns = 10;
    parent.budget.max_wall_secs = 600;
    let unit = task_spec("c", "s1", false);
    let view = unit_view(&parent, &unit);
    assert_eq!(view.budget.max_turns, TREE_CHILD_MIN_MAX_TURNS);
    assert_eq!(view.budget.max_wall_secs, TREE_CHILD_MIN_MAX_WALL_SECS);
    let hint = |t: &Task| t.routing.as_ref().and_then(|r| r.execution_hint);
    assert_eq!(
        hint(&view),
        Some(ExecutionHintSpec {
            mode: ExecutionMode::Compound,
            explicit: true
        })
    );
    let g = gate_of(&parent, 1, &unit, &[]);
    assert_eq!(g.decision.mode, ExecutionMode::Compound);
    assert_eq!(g.decision.rule_id, "human/explicit");
    assert_eq!(g.decision.source, crate::execution_gate::GateSource::Human);
    assert_eq!(g.action, None);
    // `gate: compound` も同じ。
    let mut compound = task_spec("c", "s1", false);
    compound.gate = Some(ExecutionMode::Compound);
    let g = gate_of(&parent, 1, &compound, &[]);
    assert_eq!(
        (g.decision.mode, g.decision.rule_id.as_str(), g.action),
        (ExecutionMode::Compound, "human/explicit", None)
    );
    // `gate: atomic` は明示の atomic。task のまま（下げない・記録しない）。
    let mut atomic = task_spec("c", "s1", true);
    atomic.gate = Some(ExecutionMode::Atomic);
    assert_eq!(
        hint(&unit_view(&parent, &atomic)),
        Some(ExecutionHintSpec {
            mode: ExecutionMode::Atomic,
            explicit: true
        })
    );
    let g = gate_of(&parent, 1, &atomic, &[]);
    assert_eq!(
        (g.decision.mode, g.decision.rule_id.as_str()),
        (ExecutionMode::Atomic, "human/explicit")
    );
    assert_eq!((g.declared, g.action), (UnitDeclared::Task, None));
    // leaf の unit には手掛かりを足さない。
    let leaf = leaf_spec("a", "s1", false);
    assert_eq!(hint(&unit_view(&parent, &leaf)), None);
    let g = gate_of(&parent, 1, &leaf, &[]);
    assert!(!g.decision.signals.iter().any(|s| s.name == "H"));
    assert_ne!(g.decision.rule_id, "human/explicit");
}

/// ADR-0079 §7 R2a (c) `unit_gate_table`: D4 (3) の表の 7 行。
#[test]
fn unit_gate_table() {
    let parent = parent_task(30);
    // 1. leaf + atomic → leaf（一致、記録なし）。
    let g = gate_of(&parent, 1, &leaf_spec("a", "s1", false), &[]);
    assert_eq!(g.declared, UnitDeclared::Leaf);
    assert_eq!(g.decision.mode, ExecutionMode::Atomic);
    assert_eq!((g.depth, g.threshold), (2, 7));
    assert_eq!(g.decision.depth, Some(2));
    assert_eq!(g.action, None);

    // 2. leaf + compound、子 task を持てる深さ（親 1 / 2）→ task に上げる。
    for depth in [1, 2] {
        let g = gate_of(&parent, depth, &leaf_spec("a", "s1", true), &[]);
        assert_eq!(g.decision.mode, ExecutionMode::Compound);
        assert_eq!(g.action, Some(UnitGateAction::Promoted), "depth {depth}");
        assert!(g.reason.contains("promoted"), "{}", g.reason);
    }
    // 3. leaf + compound、子 task を持てない深さ（親 3 = max_depth）→ 決定の要求。
    let g = gate_of(&parent, 3, &leaf_spec("a", "s1", true), &[]);
    assert_eq!(g.action, Some(UnitGateAction::Decision));
    assert_eq!((g.depth, g.threshold), (4, 11));
    assert!(g.reason.contains("max_depth 3"), "{}", g.reason);

    // 4. task + compound → task（一致）。
    let g = gate_of(&parent, 1, &task_spec("c", "s1", true), &[]);
    assert_eq!(g.declared, UnitDeclared::Task);
    assert_eq!(g.decision.mode, ExecutionMode::Compound);
    assert_eq!(g.action, None);

    // 5. / 6.（R6-2）: kind task の unit の gate は書き手の明示なので、atomic（`gate: atomic`）でも構造上の
    //    理由の有無・leaf の基準によらず task のまま、記録もしない（`KeptTask` / `Demoted` は出ない）。
    let mut human = task_spec("c", "s1", false);
    human.acceptance.push(crate::model::Criterion {
        text: "人が確かめる".into(),
        check: Check::Human,
    });
    let mut skills = task_spec("c", "s1", false);
    skills.skills = vec!["gpu".into()];
    let mut plain = task_spec("c", "s1", false);
    plain.gate = Some(ExecutionMode::Atomic);
    for unit in [&human, &skills, &task_spec("c", "s1", false)] {
        let g = gate_of(&parent, 1, unit, &[]);
        assert_eq!(g.decision.mode, ExecutionMode::Compound);
        assert_eq!(g.action, None);
    }
    let g = gate_of(&parent, 1, &plain, &[]);
    assert_eq!(g.decision.mode, ExecutionMode::Atomic);
    assert_eq!(g.action, None);
    let big = parent_task(200);
    let g = gate_of(&big, 1, &plain, &["h1".to_string()]);
    assert_eq!(g.action, None);

    // 7. task を子 task を持てない深さ（3）の計画に書く → 検証で拒否（ChildTaskTooDeep）、最後の試行では
    //    `plan_limit_holds` の `MaxDepth`（決定の要求）で止める（下の `plan_limit_holds_*`）。
    let p = plan(&[("s1", "implement")], vec![task_spec("c", "s1", false)]);
    let errs = validate_with(
        &p,
        v3_limits(),
        &[],
        PlanContext {
            origin: PlanOrigin::Planner,
            depth: 3,
        },
    )
    .unwrap_err();
    assert!(errs.contains(&PlanValidationError::ChildTaskTooDeep {
        key: "c".into(),
        depth: 3,
        max_depth: 3
    }));
}

/// D4 (3): 上げた / 下げた unit は仕事を落とさずに形を変え、計画の検証を通る。
#[test]
fn promoted_and_demoted_units_keep_their_work_and_validate() {
    let parent = parent_task(30);
    let p = plan(
        &[("s1", "implement"), ("s2", "test")],
        vec![
            leaf_spec("a", "s1", true),
            task_spec("c", "s2", false),
            leaf_spec("b", "s2", false),
        ],
    );
    let limits = enabled();
    let names = vec!["app".to_string()];
    let ctx = UnitGateContext {
        parent: &parent,
        parent_depth: 1,
        parent_repo_names: &names,
        limits: &limits,
        work_unit_max_turns: 80,
        work_unit_max_wall_secs: 3600,
    };
    let report = apply_unit_gates(&ctx, &p, &Default::default());
    let actions: Vec<(String, Option<UnitGateAction>)> = report
        .gates
        .iter()
        .map(|g| (g.unit_key.clone(), g.action))
        .collect();
    assert_eq!(
        actions,
        vec![
            ("a".into(), Some(UnitGateAction::Promoted)),
            // R6-2: kind task の unit は明示の compound（下げない）。
            ("c".into(), None),
            ("b".into(), None),
        ]
    );
    assert_eq!(report.overridden().count(), 1);
    assert!(report.leaf_too_large.is_empty());
    let a = &report.spec.units[0];
    assert!(a.is_task());
    assert_eq!(a.repos, vec!["app".to_string()]);
    assert!(a.checks.is_empty() && a.budget.is_none() && a.context.paths.is_empty());
    assert_eq!(
        a.acceptance.len(),
        1,
        "checks become the command acceptance"
    );
    assert!(matches!(a.acceptance[0].check, Check::Command { .. }));
    assert!(a.objective.contains("src/"), "{}", a.objective);
    assert!(
        a.objective.contains(PROMOTED_DONE_WHEN_HEADING),
        "{}",
        a.objective
    );
    assert!(a.objective.contains("- a works"), "{}", a.objective);
    assert!(a.done_when.is_empty());
    assert!(report.spec.units[1].is_task(), "kind task stays a task");
    // `demote_to_leaf`（R6-2 から unit の gate は使わない）の形は変えない。
    let mut declared = p.units[1].clone();
    declared.gate = Some(ExecutionMode::Atomic);
    let c = &demote_to_leaf(&declared, WorkUnitKind::Test);
    assert_eq!(c.gate, None);
    assert_eq!(c.kind, WorkUnitKind::Test, "the stage's kind");
    assert_eq!(c.checks.len(), 1);
    assert_eq!(c.checks[0].cmd, "make c");
    assert_eq!(c.done_when, vec!["c passes".to_string()]);
    assert!(c.acceptance.is_empty());
    validate_with(&report.spec, v3_limits(), &[], PlanContext::default())
        .expect("the gated plan validates");
    // skip（replan の done）は gate をかけない。
    let skip: std::collections::BTreeSet<String> = ["a".to_string()].into();
    let report = apply_unit_gates(&ctx, &p, &skip);
    assert_eq!(report.gates.len(), 2);
    assert!(!report.spec.units[0].is_task());
    // 深さ 3 の計画の compound な leaf は上げずに決定の要求へ。
    let ctx3 = UnitGateContext {
        parent_depth: 3,
        ..ctx
    };
    let report = apply_unit_gates(&ctx3, &p, &Default::default());
    assert_eq!(report.leaf_too_large, vec!["a".to_string()]);
    assert!(!report.spec.units[0].is_task(), "not forced into a task");
}

fn many_leaves(stage: &str, n: usize) -> Vec<PlanUnitSpec> {
    (0..n)
        .map(|i| leaf_spec(&format!("{stage}-l{i}"), stage, false))
        .collect()
}

/// D3（Phase R2a）: 計画の上限（段階の数・段階あたり・子 task・max_depth）と木の leaf を超える unit を
/// 決定的に選ぶ。上限の内の unit は止めない。
#[test]
fn plan_limit_holds_select_only_the_excess() {
    // R6-2: 既定の `max_tree_leaves` は 120。この表は 40 のときの値で書いてある。
    let limits = TreeLimits {
        max_tree_leaves: 40,
        ..enabled()
    };
    let none = std::collections::BTreeSet::new();
    // 段階あたり 7 → 7 つ目だけ。
    let p = plan(&[("s1", "implement")], many_leaves("s1", 7));
    let holds = plan_limit_holds(&p, &limits, 1, 0, &none, &none, &none);
    assert_eq!(
        holds,
        vec![LimitHold {
            limit: TreeLimitKind::UnitsPerStage,
            scope: Some("s1".into()),
            units: vec!["s1-l6".into()],
            count: 7,
            max: 6
        }]
    );
    assert_eq!(
        TreeLimitKind::UnitsPerStage.decision_key(Some("s1")),
        "limit:max_units_per_stage:s1"
    );
    // 段階 6 つ → 6 つ目の段階の unit。
    let stages: Vec<(String, &str)> = (1..=6).map(|i| (format!("s{i}"), "implement")).collect();
    let stage_refs: Vec<(&str, &str)> = stages.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let units: Vec<PlanUnitSpec> = (1..=6)
        .map(|i| leaf_spec(&format!("u{i}"), &format!("s{i}"), false))
        .collect();
    let holds = plan_limit_holds(
        &plan(&stage_refs, units),
        &limits,
        1,
        0,
        &none,
        &none,
        &none,
    );
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].limit, TreeLimitKind::Stages);
    assert_eq!(holds[0].units, vec!["u6".to_string()]);
    assert_eq!((holds[0].count, holds[0].max), (6, 5));
    // 子 task 7 つ（段階 2 つに分ける）→ 7 つ目。
    let mut units: Vec<PlanUnitSpec> = (0..4)
        .map(|i| task_spec(&format!("c{i}"), "s1", false))
        .collect();
    units.extend((4..7).map(|i| task_spec(&format!("c{i}"), "s2", false)));
    let p = plan(&[("s1", "implement"), ("s2", "implement")], units);
    let holds = plan_limit_holds(&p, &limits, 1, 0, &none, &none, &none);
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].limit, TreeLimitKind::ChildTasks);
    assert_eq!(holds[0].units, vec!["c6".to_string()]);
    // max_depth: 深さ 3 の計画の kind task はすべて。leaf は止めない。
    let p = plan(
        &[("s1", "implement")],
        vec![leaf_spec("a", "s1", false), task_spec("c", "s1", false)],
    );
    let holds = plan_limit_holds(&p, &limits, 3, 0, &none, &none, &none);
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].limit, TreeLimitKind::MaxDepth);
    assert_eq!(holds[0].units, vec!["c".to_string()]);
    assert!(plan_limit_holds(&p, &limits, 2, 0, &none, &none, &none).is_empty());
    // 木の leaf: 既に 38、この計画の新しい leaf 3 つ（うち 1 つは既存の key）→ 残り 2 に収まる。
    let p = plan(&[("s1", "implement")], many_leaves("s1", 3));
    let existing: std::collections::BTreeSet<String> = ["s1-l0".to_string()].into();
    assert!(plan_limit_holds(&p, &limits, 1, 38, &existing, &none, &none).is_empty());
    // 既に 39 なら 1 つ目の新しい leaf だけ、残りを止める。
    let holds = plan_limit_holds(&p, &limits, 1, 39, &existing, &none, &none);
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].limit, TreeLimitKind::TreeLeaves);
    assert_eq!(holds[0].units, vec!["s1-l2".to_string()]);
    assert_eq!((holds[0].count, holds[0].max), (41, 40));
    // unit の gate が止めた leaf（extra_held）は束に入れず、leaf の数にも数えない。
    let extra: std::collections::BTreeSet<String> = ["s1-l1".to_string()].into();
    assert!(plan_limit_holds(&p, &limits, 1, 39, &existing, &extra, &none).is_empty());
    // 上限の内なら何も止めない・/2 は対象外。
    let p = plan(&[("s1", "implement")], many_leaves("s1", 6));
    assert!(plan_limit_holds(&p, &limits, 1, 0, &none, &none, &none).is_empty());
    let mut v2 = p.clone();
    v2.schema = crate::execution_plan::EXECUTION_PLAN_SCHEMA_V2.to_string();
    assert!(
        plan_limit_holds(
            &v2,
            &TreeLimits {
                max_units_per_stage: 1,
                ..limits
            },
            1,
            0,
            &none,
            &none,
            &none
        )
        .is_empty()
    );
}

/// ADR-0079 R7-2: 持ち越す done の kind task の unit は子 task の数に入れない（検証と同じ）。done 3 + 生きた 4
/// は上限 6 の内、生きた 7 なら 7 つ目を止める。
#[test]
fn plan_limit_holds_do_not_count_done_task_units() {
    let limits = enabled();
    let none = std::collections::BTreeSet::new();
    // 段階あたり 6 の上限に当たらないよう 2 つの段階に分ける。
    let units: Vec<PlanUnitSpec> = (0..7)
        .map(|i| task_spec(&format!("c{i}"), if i < 4 { "s1" } else { "s2" }, false))
        .collect();
    let p = plan(&[("s1", "implement"), ("s2", "implement")], units);
    let done: std::collections::BTreeSet<String> =
        ["c0".to_string(), "c1".to_string(), "c2".to_string()].into();
    assert!(plan_limit_holds(&p, &limits, 1, 0, &none, &none, &done).is_empty());
    let holds = plan_limit_holds(&p, &limits, 1, 0, &none, &none, &none);
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].limit, TreeLimitKind::ChildTasks);
    assert_eq!(holds[0].units, vec!["c6".to_string()]);
}

/// ADR-0079 付記「R7-3」D5: `max_units_per_stage` の止めは生きた unit だけを数える（持ち越す done の unit と
/// `adopt` の unit を除く。検証の `TooManyUnitsInStage` と同じ）。done を渡さなければ従来どおり 7 つ目を止める。
#[test]
fn plan_limit_holds_count_only_live_units_per_stage() {
    let limits = enabled();
    let none = std::collections::BTreeSet::new();
    let mut units: Vec<PlanUnitSpec> = (0..3)
        .map(|i| leaf_spec(&format!("done-{i}"), "s1", false))
        .collect();
    units.extend((0..6).map(|i| leaf_spec(&format!("live-{i}"), "s1", false)));
    let mut adopted = task_spec("adopted", "s1", false);
    adopted.adopt = Some(TaskId::new());
    units.push(adopted);
    let p = plan(&[("s1", "implement")], units);
    let done: std::collections::BTreeSet<String> = [
        "done-0".to_string(),
        "done-1".to_string(),
        "done-2".to_string(),
    ]
    .into();
    assert!(
        plan_limit_holds(&p, &limits, 1, 0, &none, &none, &done)
            .iter()
            .all(|h| h.limit != TreeLimitKind::UnitsPerStage),
        "3 done + 1 adopt + 6 live: only the 6 live units count"
    );
    let mut p7 = p.clone();
    p7.units.push(leaf_spec("live-6", "s1", false));
    let holds = plan_limit_holds(&p7, &limits, 1, 0, &none, &none, &done);
    let per_stage: Vec<&LimitHold> = holds
        .iter()
        .filter(|h| h.limit == TreeLimitKind::UnitsPerStage)
        .collect();
    assert_eq!(per_stage.len(), 1);
    assert_eq!(per_stage[0].units, vec!["live-6".to_string()]);
    assert_eq!((per_stage[0].count, per_stage[0].max), (7, 6));
}

/// ADR-0079 付記「R7-3」D4（08:18Z「score 7 ≥ 閾値 11」）: `leaf_too_large` の決定文は gate の根拠を正しく書く。
/// 強制規則の compound は「score は閾値未満だが規則で compound」、score による compound だけ「score ≥ 閾値」。
#[test]
fn leaf_too_large_text_states_the_real_gate_basis() {
    let raised_by = crate::decision::DecisionRaisedBy {
        task_id: TaskId::new(),
        run_id: None,
        origin: crate::decision::DecisionOrigin::Daemon,
    };
    let parent = parent_task(30);
    let mut g = gate_of(&parent, 3, &leaf_spec("a", "s1", true), &[]);
    g.decision.rule_id = "compound/long-and-broad".into();
    g.decision.score = 7;
    g.threshold = 11;
    let d = leaf_too_large_decision(&g, "Leaf a", vec![], raised_by.clone());
    assert!(!d.question.contains('≥'), "{}", d.question);
    assert!(
        d.question.contains(
            "gate compound/long-and-broad: score 7 は閾値 11 未満だが、この規則は score によらず compound と判定する（expected_length=high かつ cross_cutting=high）"
        ),
        "{}",
        d.question
    );
    g.decision.rule_id = "compound/score".into();
    g.decision.score = 12;
    let d = leaf_too_large_decision(&g, "Leaf a", vec![], raised_by);
    assert!(
        d.question
            .contains("（gate compound/score、score 12 ≥ 閾値 11）"),
        "{}",
        d.question
    );
}

fn run(task: TaskId, role: RunIndexRole, tokens: (u64, u64), cost: Option<f64>) -> RunRow {
    RunRow {
        run_id: TaskId::new().to_string(),
        task_id: task.to_string(),
        work_unit_id: None,
        role,
        seq: 1,
        status: RunIndexStatus::Completed,
        adapter: None,
        model: None,
        account: None,
        session_id: None,
        checkpoint: None,
        usage: Some(Usage {
            input_tokens: Some(tokens.0),
            output_tokens: Some(tokens.1),
            cache_read_tokens: Some(999),
            cache_creation_tokens: None,
            cost_usd: cost,
            duplicate_reads: None,
            session_resumed: None,
        }),
        metrics: None,
        started_at: String::new(),
        finished_at: None,
    }
}

fn wu(task: TaskId, key: &str, kind: WorkUnitKind, status: WorkUnitStatus) -> WorkUnitRow {
    let spec: WorkUnitSpec = serde_json::from_value(serde_json::json!({
        "key": key, "kind": kind.as_str(), "title": key, "objective": key,
    }))
    .unwrap();
    let mut row = WorkUnitRow::new(
        key.into(),
        task.to_string(),
        "p".into(),
        0,
        spec,
        status,
        String::new(),
    );
    if status == WorkUnitStatus::Blocked {
        row.blocked_reason = Some(WorkUnitBlockedReason::Decision);
    }
    row
}

/// D3 / U-R7（Phase R2a）: 3 段の木（root → 子 → 孫）の数。run は reviewer を除き、reviewer の run と
/// 定価は深さごとに出る。leaf は repair・統合・kind task・決定待ちの行を数えない。replan は版 − 1 の和。
#[test]
fn tree_counters_across_a_three_level_tree() {
    let (root, child, grandchild) = (TaskId::new(), TaskId::new(), TaskId::new());
    let nodes = vec![
        TreeNodeFacts {
            task_id: root,
            depth: 1,
            runs: vec![
                run(root, RunIndexRole::Planner, (100, 10), Some(0.5)),
                run(root, RunIndexRole::Worker, (200, 20), Some(1.0)),
                run(root, RunIndexRole::Reviewer, (50, 5), Some(0.25)),
            ],
            work_units: vec![
                wu(root, "a", WorkUnitKind::Implement, WorkUnitStatus::Done),
                wu(root, "c", WorkUnitKind::Task, WorkUnitStatus::Running),
                wu(
                    root,
                    "integrate-s1",
                    WorkUnitKind::Integrate,
                    WorkUnitStatus::Pending,
                ),
                wu(
                    root,
                    "held",
                    WorkUnitKind::Implement,
                    WorkUnitStatus::Blocked,
                ),
            ],
            plan_versions: 2,
        },
        TreeNodeFacts {
            task_id: child,
            depth: 2,
            runs: vec![
                run(child, RunIndexRole::Planner, (10, 1), Some(0.1)),
                run(child, RunIndexRole::Worker, (20, 2), None),
                run(child, RunIndexRole::Reviewer, (5, 1), Some(0.05)),
            ],
            work_units: vec![
                wu(child, "g", WorkUnitKind::Task, WorkUnitStatus::Done),
                wu(child, "l1", WorkUnitKind::Test, WorkUnitStatus::Ready),
                wu(child, "fix", WorkUnitKind::Repair, WorkUnitStatus::Done),
            ],
            plan_versions: 1,
        },
        TreeNodeFacts {
            task_id: grandchild,
            depth: 3,
            runs: vec![
                run(grandchild, RunIndexRole::WrapUp, (1, 1), Some(0.01)),
                run(grandchild, RunIndexRole::Reviewer, (2, 2), Some(0.02)),
            ],
            work_units: vec![],
            plan_versions: 0,
        },
    ];
    let c = tree_counters(Some(root), &nodes);
    assert_eq!(c.root_id, Some(root));
    assert_eq!(c.nodes, 3);
    assert_eq!(c.leaves, 2, "a and l1");
    assert_eq!(c.runs, 5);
    assert_eq!(c.reviewer_runs, 3);
    assert_eq!(c.replans, 1);
    assert_eq!(
        c.tokens,
        110 + 220 + 55 + 11 + 22 + 6 + 2 + 4,
        "input + output, no cache"
    );
    assert!(
        !c.cost_usd_complete,
        "the child's worker has tokens but no cost"
    );
    assert!((c.cost_usd - (0.5 + 1.0 + 0.25 + 0.1 + 0.05 + 0.01 + 0.02)).abs() < 1e-9);
    let depths: Vec<u32> = c.by_depth.iter().map(|d| d.depth).collect();
    assert_eq!(depths, vec![1, 2, 3]);
    let d1 = &c.by_depth[0];
    assert_eq!((d1.nodes, d1.runs, d1.reviewer_runs), (1, 2, 1));
    assert!((d1.reviewer_cost_usd - 0.25).abs() < 1e-9);
    assert!(d1.cost_usd_complete);
    assert_eq!(d1.runs_by_role.get("planner"), Some(&1));
    let d2 = &c.by_depth[1];
    assert_eq!((d2.runs, d2.reviewer_runs, d2.tokens), (2, 1, 11 + 22 + 6));
    assert!(!d2.cost_usd_complete);
    assert!((d2.reviewer_cost_usd - 0.05).abs() < 1e-9);
    let d3 = &c.by_depth[2];
    assert_eq!((d3.runs, d3.reviewer_runs), (1, 1));
    assert_eq!(
        d3.runs_by_role
            .get("wrap_up")
            .or(d3.runs_by_role.get("wrapup")),
        Some(&1)
    );

    // 上限: run 5 本は max 5 で超過（もう 1 本起こすと 6）、4 本の上限でも。6 なら通る。
    let limits = TreeLimits {
        max_tree_runs: 5,
        ..enabled()
    };
    assert_eq!(
        run_limit_breach(&limits, &c, NextRun::Worker),
        Some(RunLimitBreach {
            limit: TreeLimitKind::TreeRuns,
            count: 5,
            max: 5
        })
    );
    let limits = TreeLimits {
        max_tree_runs: 6,
        ..enabled()
    };
    assert_eq!(run_limit_breach(&limits, &c, NextRun::Worker), None);
    // replan は replan の planner run だけが見る。
    let limits = TreeLimits {
        max_tree_replans: 1,
        ..enabled()
    };
    assert_eq!(run_limit_breach(&limits, &c, NextRun::Worker), None);
    assert_eq!(
        run_limit_breach(&limits, &c, NextRun::Planner { replan: false }),
        None
    );
    assert_eq!(
        run_limit_breach(&limits, &c, NextRun::Planner { replan: true }).map(|b| b.limit),
        Some(TreeLimitKind::TreeReplans)
    );
    // トークンは設定したときだけ。
    let limits = TreeLimits {
        max_tree_tokens: Some(c.tokens),
        ..enabled()
    };
    assert_eq!(
        run_limit_breach(&limits, &c, NextRun::Worker).map(|b| b.limit),
        Some(TreeLimitKind::TreeTokens)
    );
    assert_eq!(run_limit_breach(&enabled(), &c, NextRun::Worker), None);
}

/// D7: daemon の決定の要求の形（選択肢・推奨・key・needed_before）。
#[test]
fn daemon_decisions_have_the_d7_shape() {
    let root = TaskId::new();
    let raised_by = crate::decision::DecisionRaisedBy {
        task_id: root,
        run_id: None,
        origin: crate::decision::DecisionOrigin::Daemon,
    };
    let d = limit_decision(
        TreeLimitKind::TreeRuns,
        None,
        120,
        120,
        vec!["self".into()],
        vec![],
        raised_by.clone(),
    );
    assert_eq!(d.kind, crate::decision::DecisionKind::Limit);
    assert_eq!(d.key, "limit:max_tree_runs");
    assert_eq!(d.recommended, "replan");
    let keys: Vec<&str> = d.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, vec!["raise-once", "replan", "withdraw"]);
    assert_eq!(d.status, crate::decision::DecisionStatus::Open);
    assert_eq!(d.root_id(), root);
    assert_eq!(d.id.len(), 26, "a ULID");
    // 計画の決定と同じ形の検査（key の書式以外）を通る。
    let spec = crate::decision::DecisionSpec {
        key: "x".into(),
        question: d.question.clone(),
        options: d.options.clone(),
        recommended: d.recommended.clone(),
        cost_of_reversal: d.cost_of_reversal,
        cost_note: None,
        needed_before: d.needed_before.clone(),
    };
    assert!(crate::decision::validate_shape(&spec).is_empty());
    let parent = parent_task(30);
    let g = gate_of(&parent, 3, &leaf_spec("a", "s1", true), &[]);
    let d = leaf_too_large_decision(&g, "Leaf a", vec![], raised_by);
    assert_eq!(d.kind, crate::decision::DecisionKind::LeafTooLarge);
    assert_eq!(d.key, "leaf_too_large:a");
    assert_eq!(d.needed_before, vec!["a".to_string()]);
    assert!(
        d.question.contains("compound/long-and-broad"),
        "{}",
        d.question
    );
}

/// R3a: limit の key の読み戻し、run 時の上限、`raise-once` の余裕、plan_invalid の選択肢。
#[test]
fn limit_keys_allowances_and_plan_invalid_options() {
    assert_eq!(
        TreeLimitKind::from_decision_key("limit:max_tree_runs"),
        Some(TreeLimitKind::TreeRuns)
    );
    assert_eq!(
        TreeLimitKind::from_decision_key("limit:max_units_per_stage:s1"),
        Some(TreeLimitKind::UnitsPerStage)
    );
    assert_eq!(TreeLimitKind::from_decision_key("h1"), None);
    assert!(TreeLimitKind::TreeRuns.is_run_time());
    assert!(TreeLimitKind::NodeReplans.is_run_time());
    assert!(!TreeLimitKind::TreeLeaves.is_run_time());
    assert_eq!(limit_allowance_step(TreeLimitKind::TreeRuns, 120), 60);
    assert_eq!(limit_allowance_step(TreeLimitKind::TreeRuns, 1), 1);
    assert_eq!(limit_allowance_step(TreeLimitKind::NodeReplans, 3), 1);
    let mut allowances = std::collections::BTreeMap::new();
    allowances.insert(TreeLimitKind::TreeRuns, 2);
    allowances.insert(TreeLimitKind::TreeTokens, 1);
    let base = TreeLimits {
        max_tree_tokens: Some(1000),
        // R6-2: 既定は 400。この表は 120 のときの値で書いてある。
        max_tree_runs: 120,
        ..TreeLimits::default()
    };
    let raised = limits_with_allowances(&base, &allowances);
    assert_eq!(raised.max_tree_runs, 240);
    assert_eq!(raised.max_tree_tokens, Some(1500));
    assert_eq!(raised.max_tree_replans, base.max_tree_replans);

    let raised_by = crate::decision::DecisionRaisedBy {
        task_id: TaskId::new(),
        run_id: None,
        origin: crate::decision::DecisionOrigin::Daemon,
    };
    let keys = |replan: bool| -> Vec<String> {
        plan_invalid_decision(TaskId::new(), replan, &[], vec![], raised_by.clone())
            .options
            .into_iter()
            .map(|o| o.key)
            .collect()
    };
    assert_eq!(keys(false), vec!["replan", "atomic", "cancel"]);
    assert_eq!(
        keys(true),
        vec!["replan", "cancel"],
        "no atomic once a plan exists"
    );
}
