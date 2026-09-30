//! ADR-0079 §7 R2a: 再帰の gate（深さの閾値、木の子は shadow でも採用）、計画の採用時の unit の gate
//! （上げる・下げる・決定の要求と `UnitGateOverridden`）、木の上限（計画・木の leaf・run・replan・深さ）の
//! 超過は `kind: limit` の決定の要求にしてその仕事だけを止める。すべて偽のアダプタと一時ディレクトリだけで、
//! 外部ネットワークに出ない。

use super::tree::{
    TreeAdapter, approve_root_plan, assert_replay_is_clean, leaf, stage, task_unit,
    tick_until_child_runs, tree_dispatcher, unit, unit_reasons, v3_plan,
};
use super::*;

/// 決定を待って止めた仕事がある木は `ready` の task を残すので `idle` にならない（子待ちの親と同じ）。
/// run・判定・統合が 50 tick（約 1 秒）続けて無ければ落ち着いたとみなす。
async fn run_until_quiet(d: &mut Dispatcher, store: &Arc<dyn TaskStore>, max_ticks: usize) {
    let mut quiet = 0;
    for _ in 0..max_ticks {
        let r = d.tick().unwrap();
        if r.idle {
            return;
        }
        let busy = r.reclaimed + r.dispatched + r.finished + r.reviewed + r.in_flight > 0
            || !store.list(Some(Status::Running)).unwrap().is_empty()
            || !store.list(Some(Status::Reviewing)).unwrap().is_empty();
        if busy {
            quiet = 0;
        } else {
            quiet += 1;
            if quiet >= 50 {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the dispatcher never settled");
}

/// Phase R3b（ADR-0079 D8）: 落ち着くまで回し、root の計画が承認を待っていれば承認してもう一度回す（決定を含む・
/// 上限に近い計画は承認を挟む）。承認の理由（承認を待たなかったなら空）を返す。
async fn run_until_quiet_approving(
    d: &mut Dispatcher,
    store: &Arc<dyn TaskStore>,
    root_id: TaskId,
    max_ticks: usize,
) -> Vec<String> {
    run_until_quiet(d, store, max_ticks).await;
    let task = store.get(root_id).unwrap().unwrap();
    let events = store.events_for(root_id).unwrap();
    if task_ops::plan_gate::latest_plan_approval(&task, &events).is_none() {
        return Vec::new();
    }
    let reasons = approve_root_plan(store, root_id);
    run_until_quiet(d, store, max_ticks).await;
    reasons
}

/// root（`compound_task`: 人の明示の compound）。子が継ぐ予算を 30 turns にする（`new_task` の 1 turn の
/// ままだと、子の gate が強制規則 `atomic/small` に当たり深さの閾値を試せない）。
fn gate_root(dir: &std::path::Path) -> Task {
    let mut root = compound_task(dir);
    root.budget.max_turns = 30;
    root
}

/// 規則表の強制規則 `compound/long-and-broad`（深さに関わらず compound）。
fn broad() -> serde_json::Value {
    serde_json::json!({"expected_length": "high", "cross_cutting": "high"})
}

/// F1 high(2) + F2 high(2) = 4、kind task の unit の手掛かり H(+2。R5b-fix3) を足して score 6
/// （F3 は tool_intensity medium、F4 は cross_cutting low、F5 は judgment low で当てない）。
fn score_six() -> serde_json::Value {
    serde_json::json!({
        "context_size": "high", "expected_length": "high", "tool_intensity": "medium",
        "cross_cutting": "low", "judgment": "low",
    })
}

fn with_features(mut unit: serde_json::Value, features: serde_json::Value) -> serde_json::Value {
    unit["features"] = features;
    unit
}

/// ADR-0079 付記「R6-2」: fixture の `gate: atomic` を外す（kind task の既定 = 明示の compound）。
fn without_gate(mut unit: serde_json::Value) -> serde_json::Value {
    unit.as_object_mut().unwrap().remove("gate");
    unit
}

fn gate_decision(task: &Task) -> task_core::ExecutionGateDecision {
    task.routing
        .as_ref()
        .and_then(|r| r.execution.clone())
        .unwrap_or_else(|| panic!("no gate decision on {}", task.id))
}

fn child_task(store: &Arc<dyn TaskStore>, root: TaskId, key: &str) -> Task {
    let id = super::tree::child_of(store, root, key);
    store.get(id).unwrap().unwrap()
}

fn overrides(events: &[(u64, Event)]) -> Vec<(String, task_core::UnitGateAction, i32, String)> {
    events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::UnitGateOverridden {
                unit_key,
                action,
                score,
                reason,
                ..
            } => Some((unit_key.clone(), *action, *score, reason.clone())),
            _ => None,
        })
        .collect()
}

/// ADR-0079 §7 R2a (a) / (b) `tree_nodes_adopt_gate_even_in_shadow`: `gate = "shadow"`。
/// - root（人の明示の compound）は従来どおり planner に進み、判定は `shadow = true`・深さなし。
/// - 深さ 2 の子は閾値 7 で判定し直し、**shadow でも採用**する（`shadow = false`、`depth = 2`）:
///   score 6 の子は root なら compound の点だが深さ 2 では atomic（1 run）、強制規則の compound の子は
///   自分の planner に進む（深さ 3 の閾値で自分の unit に gate をかける）。
/// - 木でない task の規則表の compound は shadow のまま記録だけ（planner に進まない）。
#[tokio::test]
async fn tree_nodes_adopt_gate_even_in_shadow() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = gate_root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let root_plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            with_features(task_unit("c6", "s1", &[], "true"), score_six()),
            // ADR-0079 付記「R6-2」: `gate` を省いた kind task の unit は明示の compound（fixture の `gate: atomic` を外す）。
            without_gate(with_features(task_unit("cb", "s1", &[], "true"), broad())),
        ],
    );
    let child_plan = v3_plan(vec![stage("t1", false)], vec![leaf("l", "t1", &[])]);
    let adapter = Arc::new(TreeAdapter::new(
        vec![root_plan, child_plan],
        Duration::ZERO,
    ));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.gate = task_core::GateMode::Shadow;
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let stored_root = store.get(root_id).unwrap().unwrap();
    assert_eq!(
        stored_root.status,
        Status::Done,
        "{:?}",
        store.events_for(root_id).unwrap()
    );
    let root_gate = gate_decision(&stored_root);
    assert_eq!(root_gate.rule_id, "human/explicit");
    assert!(
        root_gate.shadow,
        "the root's decision is recorded as shadow"
    );
    assert_eq!(root_gate.depth, None);
    assert_eq!(root_gate.threshold, 5);

    // `gate: atomic` の子（ADR-0079 付記「R6-2」: 以前は score 6 が深さ 2 の閾値 7 に届かず atomic だった）:
    // 明示の atomic（`human/explicit`、score は数えない）、shadow でも採用（`shadow = false`）、1 run で done。
    let c6 = child_task(&store, root_id, "c6");
    assert_eq!(c6.status, Status::Done);
    let g6 = gate_decision(&c6);
    assert_eq!(
        (
            g6.mode,
            g6.rule_id.as_str(),
            g6.threshold,
            g6.depth,
            g6.shadow
        ),
        (
            task_core::ExecutionMode::Atomic,
            "human/explicit",
            7,
            Some(2),
            false
        ),
        "{g6:?}"
    );
    assert!(store.execution_plan_active(c6.id).unwrap().is_none());
    let c6_runs = store.runs_for_task(c6.id).unwrap();
    assert!(
        c6_runs
            .iter()
            .all(|r| r.role != task_core::RunIndexRole::Planner),
        "{c6_runs:?}"
    );

    // 強制規則の子: 深さ 2 で compound、shadow でも自分の planner に進み計画を持つ。
    let cb = child_task(&store, root_id, "cb");
    assert_eq!(cb.status, Status::Done);
    let gb = gate_decision(&cb);
    assert_eq!(
        (gb.mode, gb.threshold, gb.depth, gb.shadow),
        (task_core::ExecutionMode::Compound, 7, Some(2), false)
    );
    assert!(store.execution_plan_active(cb.id).unwrap().is_some());
    assert!(
        store
            .runs_for_task(cb.id)
            .unwrap()
            .iter()
            .any(|r| r.role == task_core::RunIndexRole::Planner)
    );
    // 子の計画の unit には深さ 3 の閾値（9）で gate がかかる（leaf l は atomic で一致、記録なし）。
    assert!(overrides(&store.events_for(cb.id).unwrap()).is_empty());
    // root の計画の unit の gate（ADR-0079 付記「R6-2」: kind task の unit の gate は明示なので上書きしない）:
    // c6（明示の atomic）も cb（明示の compound）も記録なし（以前は c6 が kept_task）。
    let root_overrides = overrides(&store.events_for(root_id).unwrap());
    assert!(root_overrides.is_empty(), "{root_overrides:?}");
    assert_replay_is_clean(&store);

    // 木でない task: 規則表の compound（ヒントの強制規則）は shadow のまま記録だけ、1 run（atomic）。
    let store2: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut plain = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    plain.budget.max_turns = 30;
    plain.routing = Some(task_core::TaskRouting {
        features: serde_json::from_value(broad()).ok(),
        ..Default::default()
    });
    let plain_id = plain.id;
    store2.create_task(&plain, vec![]).unwrap();
    let adapter2 = Arc::new(TreeAdapter::new(vec![], Duration::ZERO));
    let mut d2 = tree_dispatcher(&store2, adapter2);
    d2.config.execution.gate = task_core::GateMode::Shadow;
    run_until_idle(&mut d2, 400).await;
    let stored = store2.get(plain_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done);
    let g = gate_decision(&stored);
    assert_eq!(g.mode, task_core::ExecutionMode::Compound);
    assert!(g.shadow);
    assert_eq!(g.depth, None);
    assert!(store2.execution_plan_active(plain_id).unwrap().is_none());
    assert!(
        store2
            .runs_for_task(plain_id)
            .unwrap()
            .iter()
            .all(|r| r.role != task_core::RunIndexRole::Planner)
    );
}

/// ADR-0079 §7 R2a (c): unit の gate が planner の宣言を上書きする。compound な leaf（強制規則）は
/// 子 task に上げ、構造上の理由の無い小さな kind task の unit は leaf に下げる。どちらも
/// `UnitGateOverridden{declared, gate, action, depth, threshold, score, reason}` を残し、採用した計画の spec と
/// `work_units` の行に反映され、仕事は落ちない（上げた unit は子 task として、下げた unit は leaf の run として
/// 走り、root は done）。
#[tokio::test]
async fn unit_gate_promotes_and_demotes_with_override_events() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = gate_root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let mut small = task_unit("small", "s1", &[], "true");
    small
        .as_object_mut()
        .unwrap()
        .remove("skills")
        .expect("the fixture has skills");
    let root_plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            leaf("a", "s1", &[]),
            with_features(leaf("big", "s1", &[]), broad()),
            small,
        ],
    );
    // 上げた big は子 task として自分の gate（compound）→ 自分の planner に進む。
    let big_plan = v3_plan(vec![stage("t1", false)], vec![leaf("bl", "t1", &[])]);
    let adapter = Arc::new(TreeAdapter::new(vec![root_plan, big_plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );
    let plan = store.execution_plan_list(root_id).unwrap().remove(0);
    let mut seen = events.iter().filter_map(|(_, e)| match e {
        Event::UnitGateOverridden {
            plan_id,
            unit_key,
            declared,
            gate,
            action,
            depth,
            threshold,
            score,
            reason,
        } => Some((
            plan_id.clone(),
            unit_key.clone(),
            *declared,
            *gate,
            *action,
            *depth,
            *threshold,
            *score,
            reason.clone(),
        )),
        _ => None,
    });
    let big = seen.next().expect("big overridden");
    assert_eq!(big.0, plan.id);
    assert_eq!(
        (big.1.as_str(), big.2, big.3, big.4, big.5, big.6),
        (
            "big",
            task_core::UnitDeclared::Leaf,
            task_core::ExecutionMode::Compound,
            task_core::UnitGateAction::Promoted,
            2,
            7
        )
    );
    assert!(big.8.contains("compound/long-and-broad"), "{}", big.8);
    // ADR-0079 付記「R6-2」: small（kind task、`gate: atomic`）は明示の gate なので下げない（以前は demoted）。
    assert!(
        seen.next().is_none(),
        "leaf a agrees with the gate, small keeps its explicit gate"
    );

    // 採用した計画の spec と行: big は kind task（子 task）、small は leaf（段階の kind）。
    let spec_kind = |key: &str| {
        plan.spec
            .units
            .iter()
            .find(|u| u.key == key)
            .map(|u| u.kind)
            .unwrap()
    };
    assert_eq!(spec_kind("big"), task_core::WorkUnitKind::Task);
    // R6-2: small は kind task のまま（明示の atomic の子 task として 1 run で走る）。
    assert_eq!(spec_kind("small"), task_core::WorkUnitKind::Task);
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "big").kind, task_core::WorkUnitKind::Task);
    assert!(unit(&units, "big").child_task_id.is_some());
    assert_eq!(unit(&units, "small").kind, task_core::WorkUnitKind::Task);
    assert!(unit(&units, "small").child_task_id.is_some());
    let small_child = child_task(&store, root_id, "small");
    assert_eq!(small_child.status, Status::Done);
    assert_eq!(
        gate_decision(&small_child).mode,
        task_core::ExecutionMode::Atomic
    );
    for key in ["a", "big", "small"] {
        assert_eq!(
            unit(&units, key).status,
            task_core::WorkUnitStatus::Done,
            "{key}"
        );
    }
    let big_child = child_task(&store, root_id, "big");
    assert_eq!(big_child.status, Status::Done);
    assert_eq!(big_child.tree.as_ref().unwrap().depth, 2);
    // 上げた unit の受け入れは leaf の checks（command）、done_when は子の目的の末尾へ。
    assert_eq!(big_child.acceptance.len(), 1, "{:?}", big_child.acceptance);
    assert!(matches!(
        big_child.acceptance[0].check,
        Check::Command { .. }
    ));
    assert!(
        big_child
            .objective
            .contains(task_core::tree::PROMOTED_DONE_WHEN_HEADING),
        "{}",
        big_child.objective
    );
    assert_eq!(
        store.children(root_id).unwrap().len(),
        2,
        "big and small (R6-2: small is not demoted)"
    );
    assert_replay_is_clean(&store);
}

/// 決定の要求の行（`decisions`）と `DecisionRequested` を確かめる。`(key, kind, needed_before, raised_by)`。
fn open_decisions(
    store: &Arc<dyn TaskStore>,
    root: TaskId,
) -> Vec<(String, task_core::DecisionKind, Vec<String>, TaskId)> {
    store
        .decisions_list(Some(root))
        .unwrap()
        .into_iter()
        .map(|r| {
            assert_eq!(r.status, task_core::DecisionStatus::Open);
            assert_eq!(
                r.request.raised_by.origin,
                task_core::DecisionOrigin::Daemon
            );
            assert_eq!(r.request.path.first().map(|p| p.task_id), Some(root));
            (r.key, r.kind, r.needed_before, r.task_id)
        })
        .collect()
}

/// 「その仕事を止めた以外に副作用が無い」: root は `ready` のまま（failed / blocked / done でない）、質問も
/// approvals も無く、atomic に倒していない（gate は compound のまま）。
fn assert_only_held(store: &Arc<dyn TaskStore>, root: TaskId) {
    let stored = store.get(root).unwrap().unwrap();
    assert_eq!(stored.status, Status::Ready, "{:?}", store.events_for(root));
    let gate = gate_decision(&stored);
    assert_eq!(gate.mode, task_core::ExecutionMode::Compound, "{gate:?}");
    assert_ne!(gate.rule_id, "atomic/planner-invalid");
    assert!(
        store.approval_list(None, None, None).unwrap().is_empty(),
        "no questions / approvals"
    );
    assert!(
        !store
            .events_for(root)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::QuestionRaised { .. })),
    );
}

fn held(units: &[task_core::WorkUnitRow], key: &str) -> bool {
    let u = unit(units, key);
    u.status == task_core::WorkUnitStatus::Blocked
        && u.blocked_reason == Some(task_core::WorkUnitBlockedReason::Decision)
        && u.runs == 0
        && u.child_task_id.is_none()
}

/// ADR-0079 §7 R2a (c) `leaf_too_large_at_max_depth_raises_decision`: 子 task を持てない深さ（`max_depth = 1`
/// なら root）の計画の compound な leaf は、子 task に上げられず `kind: leaf_too_large` の決定の要求になり、
/// その unit は `blocked(decision)`。同じ段階の他の unit は進む。
#[tokio::test]
async fn leaf_too_large_at_max_depth_raises_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = gate_root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            leaf("a", "s1", &[]),
            with_features(leaf("big", "s1", &[]), broad()),
        ],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits.tree.max_depth = 1;
    let reasons = run_until_quiet_approving(&mut d, &store, root_id, 400).await;
    assert_eq!(reasons, vec!["decisions:leaf_too_large:big".to_string()]);
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(unit(&units, "a").status, task_core::WorkUnitStatus::Done);
    assert!(held(&units, "big"), "{units:?}");
    assert_eq!(unit(&units, "big").kind, task_core::WorkUnitKind::Implement);
    assert_eq!(
        open_decisions(&store, root_id),
        vec![(
            "leaf_too_large:big".to_string(),
            task_core::DecisionKind::LeafTooLarge,
            vec!["big".to_string()],
            root_id
        )]
    );
    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        overrides(&events)
            .iter()
            .map(|o| (o.0.clone(), o.1))
            .collect::<Vec<_>>(),
        vec![("big".to_string(), task_core::UnitGateAction::Decision)]
    );
    assert_eq!(unit_reasons(&events, "big"), vec!["decision"]);
    assert!(store.children(root_id).unwrap().is_empty());
    // planner の 1 本と a の 1 本だけ（replan も big の run も無い）。
    assert_eq!(store.runs_for_task(root_id).unwrap().len(), 2);
    assert_only_held(&store, root_id);
    // さらに tick しても何も増えない。
    for _ in 0..5 {
        d.tick().unwrap();
    }
    assert_eq!(open_decisions(&store, root_id).len(), 1);
    assert_eq!(store.runs_for_task(root_id).unwrap().len(), 2);
    assert_replay_is_clean(&store);
}

/// ADR-0079 §7 R2a / D3: 計画の上限（段階あたりの unit・段階の数・計画あたりの子 task・`max_depth`）を超える
/// /3 は、1 回目は従来どおり不正な試行（planner に理由を渡して再試行）、同じ計画の最後の試行では採用し、
/// 超えた分の unit だけを `kind: limit` の決定の要求で止める（黙って切らない・leaf に押し込まない・atomic に
/// 倒さない）。上限の内の unit は走る。木の leaf の上限は計画の検証ではないので 1 回目で採用して止める。
#[tokio::test]
async fn plan_limits_raise_limit_decisions_and_hold_only_the_excess() {
    struct Case {
        name: &'static str,
        set: fn(&mut task_core::TreeLimits),
        stages: Vec<serde_json::Value>,
        units: Vec<serde_json::Value>,
        planner_runs: usize,
        held: Vec<&'static str>,
        ran: Vec<&'static str>,
        key: &'static str,
    }
    let cases = vec![
        Case {
            name: "units per stage",
            set: |t| t.max_units_per_stage = 2,
            stages: vec![stage("s1", false)],
            units: vec![
                leaf("a", "s1", &[]),
                leaf("b", "s1", &[]),
                leaf("x", "s1", &[]),
            ],
            planner_runs: 2,
            held: vec!["x"],
            ran: vec!["a", "b"],
            key: "limit:max_units_per_stage:s1",
        },
        Case {
            name: "stages",
            set: |t| t.max_stages = 1,
            stages: vec![stage("s1", false), stage("s2", false)],
            units: vec![leaf("a", "s1", &[]), leaf("x", "s2", &[])],
            planner_runs: 2,
            held: vec!["x"],
            ran: vec!["a", "integrate-s1"],
            key: "limit:max_stages",
        },
        Case {
            name: "child tasks per plan",
            set: |t| t.max_child_tasks_per_plan = 1,
            stages: vec![stage("s1", false)],
            units: vec![
                task_unit("c1", "s1", &[], "true"),
                task_unit("x", "s1", &[], "true"),
            ],
            planner_runs: 2,
            held: vec!["x"],
            ran: vec!["c1"],
            key: "limit:max_child_tasks_per_plan",
        },
        Case {
            name: "max_depth",
            set: |t| t.max_depth = 1,
            stages: vec![stage("s1", false)],
            units: vec![leaf("a", "s1", &[]), task_unit("x", "s1", &[], "true")],
            planner_runs: 2,
            held: vec!["x"],
            ran: vec!["a"],
            key: "limit:max_depth",
        },
        Case {
            name: "leaves per tree",
            set: |t| t.max_tree_leaves = 2,
            stages: vec![stage("s1", false)],
            units: vec![
                leaf("a", "s1", &[]),
                leaf("b", "s1", &[]),
                leaf("x", "s1", &[]),
            ],
            planner_runs: 1,
            held: vec!["x"],
            ran: vec!["a", "b"],
            key: "limit:max_tree_leaves",
        },
    ];
    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let root = gate_root(dir.path());
        let root_id = root.id;
        store.create_task(&root, vec![]).unwrap();
        let plan = v3_plan(case.stages.clone(), case.units.clone());
        let adapter = Arc::new(TreeAdapter::new(vec![plan.clone(), plan], Duration::ZERO));
        let mut d = tree_dispatcher(&store, adapter);
        (case.set)(&mut d.config.execution.limits.tree);
        // Phase R3b（ADR-0079 D8）: 上限を超える計画は決定を含み上限にも近いので承認を挟む。
        let reasons = run_until_quiet_approving(&mut d, &store, root_id, 1500).await;
        assert!(
            reasons.iter().any(|r| r.starts_with("decisions:")),
            "{}: {reasons:?}",
            case.name
        );
        let planner_runs = store
            .runs_for_task(root_id)
            .unwrap()
            .iter()
            .filter(|r| r.role == task_core::RunIndexRole::Planner)
            .count();
        assert_eq!(planner_runs, case.planner_runs, "{}", case.name);
        let plans = store.execution_plan_list(root_id).unwrap();
        assert_eq!(plans.len(), 1, "{}: adopted once", case.name);
        assert_eq!(
            plans[0].spec.units.len(),
            case.units.len(),
            "{}: nothing cut from the plan",
            case.name
        );
        let units = store.work_units_for(root_id).unwrap();
        for key in &case.held {
            assert!(held(&units, key), "{}: {key} {units:?}", case.name);
        }
        for key in &case.ran {
            assert_eq!(
                unit(&units, key).status,
                task_core::WorkUnitStatus::Done,
                "{}: {key}",
                case.name
            );
        }
        let decisions = open_decisions(&store, root_id);
        assert_eq!(decisions.len(), 1, "{}: {decisions:?}", case.name);
        assert_eq!(decisions[0].0, case.key, "{}", case.name);
        assert_eq!(decisions[0].1, task_core::DecisionKind::Limit);
        assert_eq!(
            decisions[0].2,
            case.held.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
            "{}",
            case.name
        );
        assert_eq!(decisions[0].3, root_id);
        assert_only_held(&store, root_id);
        assert_replay_is_clean(&store);
    }
}

/// ADR-0079 D3: 同じ親で同時に非終端の子 task の数（`max_parallel_child_tasks`）は、決定の要求ではなく
/// 「作らずに待つ（pending / ready のまま）」（D3 の表）。待っている unit は子を持たず、決定も質問も出ない。
#[tokio::test]
async fn concurrent_children_limit_waits_without_a_decision() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = gate_root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            task_unit("c1", "s1", &[], "true"),
            task_unit("c2", "s1", &[], "true"),
        ],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::from_secs(600)));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits.tree.max_parallel_child_tasks = 1;
    tick_until_child_runs(&mut d, &store, root_id).await;
    for _ in 0..5 {
        d.tick().unwrap();
    }
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(
        unit(&units, "c1").status,
        task_core::WorkUnitStatus::Running
    );
    let c2 = unit(&units, "c2");
    assert_eq!(c2.status, task_core::WorkUnitStatus::Ready);
    assert!(c2.child_task_id.is_none());
    assert_eq!(store.children(root_id).unwrap().len(), 1);
    assert!(store.decisions_list(Some(root_id)).unwrap().is_empty());
    assert!(store.approval_list(None, None, None).unwrap().is_empty());
}

/// ADR-0079 §7 R2a (d) `tree_limit_breach_stops_only_that_subtree`: 木の run の数（`max_tree_runs`、reviewer を
/// 除く）が上限に達したら、次の run を起こそうとした節点（子 c2）は run を起こさず、`kind: limit` の決定が
/// 木に 1 件だけ出る（tick を重ねても増えない）。先に走った兄弟 c1 の subtree は done まで走る。
#[tokio::test]
async fn tree_limit_breach_stops_only_that_subtree() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = gate_root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![
            task_unit("c1", "s1", &[], "true"),
            task_unit("c2", "s1", &[], "true"),
        ],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    // root の planner の 1 本 + c1 の 1 本で 2 本。c2 の run は 3 本目になる。
    d.config.execution.limits.tree.max_tree_runs = 2;
    // Phase R3b（ADR-0079 D8）: 木の run の見込み（planner 1 + 子 2 × 5）が上限 2 の 0.8 を超えるので承認を挟む。
    let reasons = run_until_quiet_approving(&mut d, &store, root_id, 1500).await;
    assert_eq!(reasons, vec!["near_limit:max_tree_runs:11/2".to_string()]);
    for _ in 0..5 {
        d.tick().unwrap();
    }
    let decisions = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(decisions.len(), 1, "one decision per tree: {decisions:?}");
    let dec = &decisions[0];
    // 同じ tick に ready になった兄弟のどちらが先に dispatch されるかは作成順の同順位で決まるので、決定を
    // 出した方を「止まった子」、もう一方を「走った兄弟」として確かめる。
    let c1 = child_task(&store, root_id, "c1");
    let c2 = child_task(&store, root_id, "c2");
    let ((ran, ran_key), (stopped, stopped_key)) = if dec.task_id == c1.id {
        ((c2, "c2"), (c1, "c1"))
    } else {
        ((c1, "c1"), (c2, "c2"))
    };
    assert_eq!(ran.status, Status::Done, "the sibling subtree ran");
    assert_eq!(
        stopped.status,
        Status::Ready,
        "{:?}",
        store.events_for(stopped.id)
    );
    assert!(store.runs_for_task(stopped.id).unwrap().is_empty());
    let counters = task_ops::tree::tree_counters(store.as_ref(), root_id).unwrap();
    assert_eq!(counters.runs, 2, "{counters:?}");
    assert_eq!(counters.nodes, 3);
    assert_eq!(dec.key, "limit:max_tree_runs");
    assert_eq!(dec.kind, task_core::DecisionKind::Limit);
    assert_eq!(
        dec.task_id, stopped.id,
        "raised on the node that would breach"
    );
    assert_eq!(dec.status, task_core::DecisionStatus::Open);
    assert_eq!(dec.needed_before, vec!["self".to_string()]);
    let path: Vec<(TaskId, Option<String>, Option<String>)> = dec
        .request
        .path
        .iter()
        .map(|p| (p.task_id, p.stage.clone(), p.unit.clone()))
        .collect();
    assert_eq!(
        path,
        vec![
            (root_id, Some("s1".to_string()), None),
            (stopped.id, None, Some(stopped_key.to_string())),
        ]
    );
    // root は子を待ったまま（止まった子の unit は running = 子の写し）、失敗も質問も無い。
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(
        unit(&units, ran_key).status,
        task_core::WorkUnitStatus::Done
    );
    assert_eq!(
        unit(&units, stopped_key).status,
        task_core::WorkUnitStatus::Running
    );
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    assert!(store.approval_list(None, None, None).unwrap().is_empty());
    assert_replay_is_clean(&store);
}

/// ADR-0079 D3: 木の replan の版（`max_tree_replans`）。上限に達した木で replan の planner run を起こそうと
/// すると（人の「分けて」= `ExecutionHintSet{replan: true}`）、run を起こさず `limit:max_tree_replans` の決定が
/// 1 件出る。計画は版 1 のまま、子は走り続ける。
#[tokio::test]
async fn tree_replan_limit_raises_a_decision_instead_of_a_planner_run() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = gate_root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(TreeAdapter::new(vec![plan], Duration::from_secs(600)));
    let mut d = tree_dispatcher(&store, adapter);
    d.config.execution.limits.tree.max_tree_replans = 0;
    tick_until_child_runs(&mut d, &store, root_id).await;
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Ready);
    task_ops::regate::set_execution_mode(
        store.as_ref(),
        root_id,
        task_core::ExecutionMode::Compound,
        "human",
        Some("split it differently".into()),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    for _ in 0..5 {
        d.tick().unwrap();
    }
    let planner_runs = store
        .runs_for_task(root_id)
        .unwrap()
        .iter()
        .filter(|r| r.role == task_core::RunIndexRole::Planner)
        .count();
    assert_eq!(planner_runs, 1, "no replan planner run");
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 1);
    let decisions = store.decisions_list(Some(root_id)).unwrap();
    assert_eq!(decisions.len(), 1, "{decisions:?}");
    assert_eq!(decisions[0].key, "limit:max_tree_replans");
    assert_eq!(decisions[0].task_id, root_id);
    let stored = store.get(root_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Ready);
    assert!(stored.lease.is_none());
    let child = child_task(&store, root_id, "c");
    assert_eq!(child.status, Status::Running, "the child keeps running");
}
