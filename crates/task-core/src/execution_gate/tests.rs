use super::*;
use crate::model::{
    ArtifactRef, Budget, Criterion, Status, TaskCategory, TaskId, TaskMode, TaskRouting, Tier,
    WorkerHint, WorkspaceSpec,
};

fn base_task() -> Task {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    Task {
        tree: None,
        paused_at: None,
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "小さな修正".to_string(),
        objective: "typo を直す".to_string(),
        acceptance: vec![Criterion {
            text: "直っている".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status: Status::Ready,
        priority: 10,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp/x"),
        repos: vec![],
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
        genre: Some("coding".into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        labels: vec![],
        category: TaskCategory::Other,
        skills: vec![],
        mode: TaskMode::Production,
        conversation: None,
        routing: Some(TaskRouting::default()),
    }
}

fn features(overrides: impl FnOnce(&mut TaskFeatures)) -> TaskFeatures {
    let mut f = TaskFeatures {
        judgment: Level::Low,
        ambiguity: Level::Low,
        verifiability: Level::High,
        reversibility: Level::High,
        consequence: Level::Low,
        context_size: Level::Low,
        tool_intensity: Level::Low,
        expected_length: Level::Low,
        cross_cutting: Level::Low,
    };
    overrides(&mut f);
    f
}

fn no_inputs() -> ExecutionGateInputs {
    ExecutionGateInputs::default()
}

/// ADR-0079 §7 R2a (a) `gate_threshold_rises_with_depth`: 深さ 2 の閾値は 7（`5 + 2 × (2 − 1)`）。
/// score 6 の task は root（深さ 1、閾値 5）では compound、深さ 2 では atomic。score 7 は深さ 2 でも
/// compound、深さ 3（閾値 9）では atomic。root の判定は深さを記録しない（JSON は従来と同じ）。
#[test]
fn gate_threshold_rises_with_depth() {
    let mut task = base_task();
    task.budget.max_turns = 50; // 強制規則 atomic/small に当てない
    // F1 high(2) + F2 high(2) + F3 high(1) + F4 medium(1) = 6（long-and-broad の強制規則には当たらない）。
    let six = features(|f| {
        f.context_size = Level::High;
        f.expected_length = Level::High;
        f.tool_intensity = Level::High;
        f.cross_cutting = Level::Medium;
    });
    let root = decide(&task, &six, None, false, no_inputs(), true);
    assert_eq!((root.score, root.threshold), (6, 5));
    assert_eq!(root.mode, ExecutionMode::Compound);
    assert_eq!(root.depth, None);
    assert!(
        !serde_json::to_string(&root).unwrap().contains("depth"),
        "the root's decision JSON is unchanged"
    );
    let at_root_depth = decide_at(
        &task,
        &six,
        None,
        false,
        no_inputs(),
        true,
        GateThreshold::ROOT,
    );
    assert_eq!(at_root_depth, root, "decide == decide_at(ROOT)");

    let child = decide_at(
        &task,
        &six,
        None,
        false,
        no_inputs(),
        false,
        GateThreshold::at_depth(2, 2),
    );
    assert_eq!((child.score, child.threshold), (6, 7));
    assert_eq!(child.mode, ExecutionMode::Atomic);
    assert_eq!(child.rule_id, "atomic/score");
    assert_eq!(child.depth, Some(2));
    assert!(!child.shadow);

    // F5（judgment high かつ tool_intensity ≥ medium）で +1 → 7。
    let seven = features(|f| {
        f.context_size = Level::High;
        f.expected_length = Level::High;
        f.tool_intensity = Level::High;
        f.cross_cutting = Level::Medium;
        f.judgment = Level::High;
    });
    let child7 = decide_at(
        &task,
        &seven,
        None,
        false,
        no_inputs(),
        false,
        GateThreshold::at_depth(2, 2),
    );
    assert_eq!((child7.score, child7.threshold), (7, 7));
    assert_eq!(child7.mode, ExecutionMode::Compound);
    assert_eq!(child7.rule_id, "compound/score");
    let grandchild7 = decide_at(
        &task,
        &seven,
        None,
        false,
        no_inputs(),
        false,
        GateThreshold::at_depth(3, 2),
    );
    assert_eq!(grandchild7.threshold, 9);
    assert_eq!(grandchild7.mode, ExecutionMode::Atomic);
    // 強制規則（long-and-broad）は深さに関わらず compound。
    let broad = features(|f| {
        f.expected_length = Level::High;
        f.cross_cutting = Level::High;
    });
    let d = decide_at(
        &task,
        &broad,
        None,
        false,
        no_inputs(),
        false,
        GateThreshold::at_depth(3, 2),
    );
    assert_eq!(d.mode, ExecutionMode::Compound);
    assert_eq!(d.rule_id, "compound/long-and-broad");
    assert_eq!(d.threshold, 9);
}

#[test]
fn tiny_task_with_no_signals_is_atomic_by_the_small_rule() {
    let task = base_task();
    let f = features(|_| {});
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert_eq!(d.mode, ExecutionMode::Atomic);
    assert_eq!(d.rule_id, "atomic/small");
    assert_eq!(d.score, 0);
}

/// ADR-0079 R5b-fix3: 木の子（`tree.parent_unit` を持つ）には `atomic/small` を当てない。規則表のスコアで
/// 決まり、planner の計画の子の手掛かり（H +2）はスコアに入り、人の計画の子は `human/explicit`。
#[test]
fn the_small_rule_does_not_apply_to_tree_children() {
    let parent = base_task();
    let mut task = base_task();
    task.tree = Some(crate::tree::TreeInfo::child_of(
        &parent,
        crate::tree::ParentUnit {
            task_id: parent.id,
            plan_id: "plan-1".into(),
            unit_key: "u".into(),
            stage: "s".into(),
            attempt: 1,
        },
        None,
    ));
    assert!(task.budget.max_turns <= 10);
    let f = features(|_| {});
    let at = GateThreshold::at_depth(2, 2);
    let d = decide_at(&task, &f, None, false, no_inputs(), false, at);
    assert_eq!(d.rule_id, "atomic/score");
    let d = decide_at(&task, &f, None, true, no_inputs(), false, at);
    assert_eq!(d.rule_id, "atomic/score");
    assert_eq!(d.score, 2);
    assert_eq!(d.source, GateSource::Hint);
    let d = decide_at(
        &task,
        &f,
        Some(ExecutionMode::Compound),
        false,
        no_inputs(),
        false,
        at,
    );
    assert_eq!(d.mode, ExecutionMode::Compound);
    assert_eq!(d.rule_id, "human/explicit");
    // root（木でない task）は従来どおり。
    let d = decide(&base_task(), &f, None, true, no_inputs(), false);
    assert_eq!(d.rule_id, "atomic/small");
}

#[test]
fn each_weighted_signal_contributes_its_documented_weight() {
    let mut task = base_task();
    task.budget.max_turns = 40; // avoid the atomic/small forced rule
    task.objective = "テスト対象を確かめる".repeat(10); // avoid triggering S1/S5 accidentally beyond expectations

    // F1 high (+2)
    let f = features(|f| f.context_size = Level::High);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F1" && s.weight == 2));

    // F1 medium (+1)
    let f = features(|f| f.context_size = Level::Medium);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F1" && s.weight == 1));

    // F2 (+2)
    let f = features(|f| f.expected_length = Level::High);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F2" && s.weight == 2));

    // F3 (+1)
    let f = features(|f| f.tool_intensity = Level::High);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F3" && s.weight == 1));

    // F4 high (+2) / medium (+1)
    let f = features(|f| f.cross_cutting = Level::High);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F4" && s.weight == 2));
    let f = features(|f| f.cross_cutting = Level::Medium);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F4" && s.weight == 1));

    // F5: judgment high and tool_intensity >= medium (+1)
    let f = features(|f| {
        f.judgment = Level::High;
        f.tool_intensity = Level::Medium;
    });
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "F5" && s.weight == 1));
    // judgment high alone (tool_intensity low) must not trigger F5.
    let f = features(|f| f.judgment = Level::High);
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert!(!d.signals.iter().any(|s| s.name == "F5"));

    // S1: process stage words in title/objective.
    let mut three_stages = task.clone();
    three_stages.objective = "調査してから設計し、実装する".to_string();
    let d = decide(&three_stages, &f, None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "S1" && s.weight == 2));
    let mut two_stages = task.clone();
    two_stages.objective = "調査してから設計する".to_string();
    let d = decide(
        &two_stages,
        &features(|_| {}),
        None,
        false,
        no_inputs(),
        false,
    );
    assert!(d.signals.iter().any(|s| s.name == "S1" && s.weight == 1));

    // S2: acceptance count >= 6.
    let mut many_acceptance = task.clone();
    many_acceptance.acceptance = (0..6)
        .map(|i| Criterion {
            text: format!("cond {i}"),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        })
        .collect();
    let d = decide(
        &many_acceptance,
        &features(|_| {}),
        None,
        false,
        no_inputs(),
        false,
    );
    assert!(d.signals.iter().any(|s| s.name == "S2"));

    // S3: both Human and Command checks present.
    let mut mixed = task.clone();
    mixed.acceptance = vec![
        Criterion {
            text: "a".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: "b".into(),
            check: Check::Human,
        },
    ];
    let d = decide(&mixed, &features(|_| {}), None, false, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "S3" && s.weight == 1));

    // S4: multi environment input.
    let d = decide(
        &task,
        &features(|_| {}),
        None,
        false,
        ExecutionGateInputs {
            multi_environment: true,
            recent_budget_exhausted_ratio: None,
        },
        false,
    );
    assert!(d.signals.iter().any(|s| s.name == "S4" && s.weight == 1));

    // S5: objective length > 2000.
    let mut long_objective = task.clone();
    long_objective.objective = "x".repeat(2001);
    let d = decide(
        &long_objective,
        &features(|_| {}),
        None,
        false,
        no_inputs(),
        false,
    );
    assert!(d.signals.iter().any(|s| s.name == "S5" && s.weight == 1));
    let mut short_objective = task.clone();
    short_objective.objective = "x".repeat(2000);
    let d = decide(
        &short_objective,
        &features(|_| {}),
        None,
        false,
        no_inputs(),
        false,
    );
    assert!(!d.signals.iter().any(|s| s.name == "S5"));

    // S6: recent budget_exhausted ratio threshold (0.3).
    let d = decide(
        &task,
        &features(|_| {}),
        None,
        false,
        ExecutionGateInputs {
            multi_environment: false,
            recent_budget_exhausted_ratio: Some(0.3),
        },
        false,
    );
    assert!(d.signals.iter().any(|s| s.name == "S6" && s.weight == 2));
    let d = decide(
        &task,
        &features(|_| {}),
        None,
        false,
        ExecutionGateInputs {
            multi_environment: false,
            recent_budget_exhausted_ratio: Some(0.29),
        },
        false,
    );
    assert!(!d.signals.iter().any(|s| s.name == "S6"));

    // H: CoS hint.
    let d = decide(&task, &features(|_| {}), None, true, no_inputs(), false);
    assert!(d.signals.iter().any(|s| s.name == "H" && s.weight == 2));
    assert_eq!(d.source, GateSource::Hint);
}

#[test]
fn score_threshold_boundary_is_five() {
    let mut task = base_task();
    task.budget.max_turns = 40;
    // Craft exactly score 4 (below threshold): F1 high(+2) + F4 medium(+1) + F3(+1) = 4.
    let f = features(|f| {
        f.context_size = Level::High;
        f.cross_cutting = Level::Medium;
        f.tool_intensity = Level::High;
    });
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert_eq!(d.score, 4);
    assert_eq!(d.mode, ExecutionMode::Atomic);
    assert_eq!(d.rule_id, "atomic/score");

    // Add F2 (+2) to cross 5.
    let f = features(|f| {
        f.context_size = Level::High;
        f.cross_cutting = Level::Medium;
        f.tool_intensity = Level::High;
        f.expected_length = Level::High;
    });
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert_eq!(d.score, 6);
    assert_eq!(d.mode, ExecutionMode::Compound);
    assert_eq!(d.rule_id, "compound/score");
}

#[test]
fn forced_rule_long_and_broad_overrides_low_score() {
    let mut task = base_task();
    task.budget.max_turns = 90;
    task.budget.max_wall_secs = 4000;
    let f = features(|f| {
        f.expected_length = Level::High;
        f.cross_cutting = Level::High;
    });
    let d = decide(&task, &f, None, false, no_inputs(), false);
    assert_eq!(d.mode, ExecutionMode::Compound);
    assert_eq!(d.rule_id, "compound/long-and-broad");
}

#[test]
fn forced_rule_small_overrides_score() {
    let mut task = base_task();
    task.budget.max_turns = 10;
    task.objective = "短い".to_string();
    // Even with a hint that would otherwise push toward compound, the atomic/small
    // forced rule fires first because objective is short and max_turns is tiny.
    let d = decide(&task, &features(|_| {}), None, true, no_inputs(), false);
    assert_eq!(d.mode, ExecutionMode::Atomic);
    assert_eq!(d.rule_id, "atomic/small");
}

#[test]
fn out_of_scope_tasks_are_always_atomic() {
    let mut plan_kind = base_task();
    plan_kind.kind = TaskKind::Plan;
    assert_eq!(out_of_scope_rule(&plan_kind), Some("atomic/out-of-scope"));

    let mut conversation = base_task();
    conversation.conversation = Some(crate::message::MessageId::new());
    assert_eq!(
        out_of_scope_rule(&conversation),
        Some("atomic/out-of-scope")
    );

    let mut no_routing = base_task();
    no_routing.routing = None;
    assert_eq!(out_of_scope_rule(&no_routing), Some("atomic/out-of-scope"));

    let mut fixed_pipeline = base_task();
    fixed_pipeline.genre = Some("literature".to_string());
    assert_eq!(
        out_of_scope_rule(&fixed_pipeline),
        Some("atomic/out-of-scope")
    );

    let mut shared_ws = base_task();
    shared_ws.workspace = WorkspaceSpec::Local {
        path: "/tmp/x".into(),
        mode: Some(WorkspaceMode::Shared),
    };
    assert_eq!(out_of_scope_rule(&shared_ws), Some("atomic/out-of-scope"));

    // An ordinary in-scope task has no out-of-scope rule.
    assert_eq!(out_of_scope_rule(&base_task()), None);

    // out_of_scope always wins even with a very high score / compound hint.
    let d = decide(
        &plan_kind,
        &features(|f| {
            f.expected_length = Level::High;
            f.cross_cutting = Level::High;
        }),
        Some(ExecutionMode::Compound),
        true,
        ExecutionGateInputs {
            multi_environment: true,
            recent_budget_exhausted_ratio: Some(1.0),
        },
        false,
    );
    assert_eq!(d.mode, ExecutionMode::Atomic);
    assert_eq!(d.rule_id, "atomic/out-of-scope");
}

/// ADR-0131 付記（2026-10-04、Complexity Gate 例外）: cron が作った knowledge-curation task
/// （harness = genre `knowledge-curation` かつ label `cron`）は、objective がどれだけ長くても
/// 強制規則・規則表のスコアを評価する前に atomic が決まる。rule_id は専用の
/// `atomic/knowledge-curation`（event から理由が読める）。
#[test]
fn knowledge_curation_cron_tasks_are_always_atomic() {
    let mut curation = base_task();
    curation.genre = Some(crate::cron::KNOWLEDGE_CURATION_HARNESS.to_string());
    curation.labels = vec![crate::cron::CRON_TASK_LABEL.to_string()];
    assert_eq!(
        out_of_scope_rule(&curation),
        Some("atomic/knowledge-curation")
    );

    // genre だけ・label だけでは当たらない（両方そろって初めて判別する。`is_curation_task` と同じ条件）。
    let mut genre_only = base_task();
    genre_only.genre = Some(crate::cron::KNOWLEDGE_CURATION_HARNESS.to_string());
    assert_eq!(out_of_scope_rule(&genre_only), None);

    let mut label_only = base_task();
    label_only.labels = vec![crate::cron::CRON_TASK_LABEL.to_string()];
    assert_eq!(out_of_scope_rule(&label_only), None);

    // 2000 文字を超える長い objective（S5 の強制シグナル）と cross_cutting=high/expected_length=high
    // （強制規則 compound/long-and-broad）を両方乗せても、cron の knowledge-curation task は常に atomic。
    curation.objective = "x".repeat(3000);
    let d = decide(
        &curation,
        &features(|f| {
            f.expected_length = Level::High;
            f.cross_cutting = Level::High;
        }),
        None,
        false,
        ExecutionGateInputs::default(),
        false,
    );
    assert_eq!(d.mode, ExecutionMode::Atomic);
    assert_eq!(d.rule_id, "atomic/knowledge-curation");
    assert_eq!(d.source, GateSource::Policy);
}

#[test]
fn human_explicit_beats_the_rule_table_and_the_hint() {
    let mut task = base_task();
    task.budget.max_turns = 40;
    // Score would be high (compound by score), but a human explicit atomic wins.
    let f = features(|f| {
        f.expected_length = Level::High;
        f.context_size = Level::High;
    });
    let d = decide(
        &task,
        &f,
        Some(ExecutionMode::Atomic),
        true,
        no_inputs(),
        false,
    );
    assert_eq!(d.mode, ExecutionMode::Atomic);
    assert_eq!(d.source, GateSource::Human);
    assert_eq!(d.rule_id, "human/explicit");
    assert!(d.signals.is_empty());

    task.title = "小さな修正".to_string();
    task.objective = "typo".to_string();
    let d = decide(
        &task,
        &features(|_| {}),
        Some(ExecutionMode::Compound),
        false,
        no_inputs(),
        false,
    );
    assert_eq!(d.mode, ExecutionMode::Compound);
    assert_eq!(d.source, GateSource::Human);
}

#[test]
fn shadow_flag_is_recorded_without_changing_the_decision() {
    let task = base_task();
    let d = decide(&task, &features(|_| {}), None, false, no_inputs(), true);
    assert!(d.shadow);
    let d2 = decide(&task, &features(|_| {}), None, false, no_inputs(), false);
    assert_eq!(d.mode, d2.mode);
    assert_eq!(d.rule_id, d2.rule_id);
    assert!(!d2.shadow);
}

#[test]
fn gate_mode_parses_and_round_trips() {
    assert_eq!(GateMode::parse("off"), Some(GateMode::Off));
    assert_eq!(GateMode::parse("shadow"), Some(GateMode::Shadow));
    assert_eq!(GateMode::parse("on"), Some(GateMode::On));
    assert_eq!(GateMode::parse("bogus"), None);
    assert_eq!(GateMode::default(), GateMode::Shadow);
}
