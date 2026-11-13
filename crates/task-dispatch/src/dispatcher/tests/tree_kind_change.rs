//! ADR 2026-11-11-replan-unit-kind-change: replan で unit の kind を task と leaf の間で変えたとき、始まっていない行は新しい版の
//! kind で走り（task → leaf は WU の run、leaf → task は子 task）、子 task を持つ行の kind の変更は採用の前に planner の再試行に
//! 返る。どれも採用の後に `child_create_failed`（「is not a task unit」）で落ちない。
//!
//! 守る不変条件: replan の後の生きた未完の行の `kind` は、有効な版の unit の kind と同じ。既存の木の試験は replan で kind を
//! 変えないので、行の kind が古いまま残る不具合（本番 R4 の change-map）を検出できなかった。
//! すべて偽のアダプタと一時 DB だけで、外部ネットワークに出ない。

use super::tree::{
    TreeAdapter, assert_replay_is_clean, child_of, leaf, stage, task_unit, tree_dispatcher, unit,
    unit_reasons, v3_plan,
};
use super::*;

/// root（人の明示の compound）。子が継ぐ予算を 30 turns にする（tree_approval の `root_task` と同じ）。
fn root(dir: &std::path::Path) -> Task {
    let mut root = compound_task(dir);
    root.budget.max_turns = 30;
    root
}

/// 本番 R4 の流れ: v1（`review: human` の段階で承認待ち。行は作られるが unit は起きない）を人が replan に回し、planner の v2
/// （承認の要らない計画）を採用して最後まで走らせる。root の id と最初の版の行を返す。
async fn hold_v1_then_adopt_v2(
    store: &Arc<dyn TaskStore>,
    v1: String,
    v2: String,
) -> (TaskId, Vec<task_core::WorkUnitRow>) {
    let dir = tempfile::tempdir().unwrap();
    let root = root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let adapter = Arc::new(TreeAdapter::new(vec![v1, v2], Duration::ZERO));
    let mut d = tree_dispatcher(store, adapter);
    run_until_idle(&mut d, 400).await;
    let task = store.get(root_id).unwrap().unwrap();
    let events = store.events_for(root_id).unwrap();
    assert!(
        task_ops::plan_gate::latest_plan_approval(&task, &events).is_some(),
        "v1 must wait for the human approval: {:?}",
        task.status
    );
    let v1_rows = store.work_units_for(root_id).unwrap();
    let r = task_ops::plan_gate::plan_gate(
        store.as_ref(),
        root_id,
        task_ops::plan_gate::PlanGateAction::Replan,
        Some("rewrite unit m".into()),
        "human",
    )
    .unwrap();
    assert_eq!(r.reason, "plan_replan");
    let report = run_until_idle(&mut d, 1200).await;
    assert!(report.idle, "{report:?}");
    (root_id, v1_rows)
}

/// task → leaf（本番 R4 の change-map の再現）: v1 で kind task の行になった unit m を、v2 で leaf に書き直して採用すると、
/// 行の kind は leaf になり、m は WU の run で done になる。子 task は作られず、`child_create_failed` も出ない。
#[tokio::test]
async fn replan_turns_a_held_task_unit_into_a_leaf_that_runs() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let v1 = v3_plan(
        vec![stage("s1", true)],
        vec![task_unit("m", "s1", &[], "true"), leaf("a", "s1", &[])],
    );
    let v2 = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("m", "s1", &[]), leaf("a", "s1", &[])],
    );
    let (root_id, v1_rows) = hold_v1_then_adopt_v2(&store, v1, v2).await;
    assert_eq!(unit(&v1_rows, "m").kind, task_core::WorkUnitKind::Task);

    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );
    let units = store.work_units_for(root_id).unwrap();
    let m = unit(&units, "m");
    assert_eq!(m.kind, task_core::WorkUnitKind::Implement);
    assert_eq!(m.status, task_core::WorkUnitStatus::Done);
    assert_eq!(m.child_task_id, None);
    assert!(m.runs >= 1, "{m:?}");
    let reasons = unit_reasons(&events, "m");
    assert!(
        !reasons.iter().any(|r| r == "child_create_failed"),
        "{reasons:?}"
    );
    assert!(store.children(root_id).unwrap().is_empty());
    assert_eq!(store.execution_plan_list(root_id).unwrap().len(), 2);
    assert_replay_is_clean(&store);
}

/// leaf → task: v1 で leaf の行になった unit m を、v2 で kind task に書き直して採用すると、行の kind は task になり、m から子
/// task が作られて done になる（以前は行が leaf のまま WU の run で走り、子 task は作られなかった）。
#[tokio::test]
async fn replan_turns_a_held_leaf_into_a_task_unit_with_a_child() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let v1 = v3_plan(
        vec![stage("s1", true)],
        vec![leaf("m", "s1", &[]), leaf("a", "s1", &[])],
    );
    let v2 = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("m", "s1", &[], "true"), leaf("a", "s1", &[])],
    );
    let (root_id, v1_rows) = hold_v1_then_adopt_v2(&store, v1, v2).await;
    assert_eq!(unit(&v1_rows, "m").kind, task_core::WorkUnitKind::Implement);

    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );
    let units = store.work_units_for(root_id).unwrap();
    let m = unit(&units, "m");
    assert_eq!(m.kind, task_core::WorkUnitKind::Task);
    assert_eq!(m.status, task_core::WorkUnitStatus::Done);
    assert_eq!(m.runs, 0, "a task unit does not run as a work unit: {m:?}");
    let child = child_of(&store, root_id, "m");
    assert_eq!(m.child_task_id, Some(child.to_string()));
    assert_eq!(store.get(child).unwrap().unwrap().status, Status::Done);
    assert_replay_is_clean(&store);
}

/// 子 task がある行: 子が failed になった kind task の unit c を、親の replan の planner が leaf に書き直すと、計画は採用の前
/// （設計 review・root の承認の前）に検証の理由「has already started (child task …)」で planner の再試行に返る。再試行の計画
/// （c は kind task のまま）が採用され、同じ unit から 2 回目の子が作られて done になる。c の行は kind task のまま。
#[tokio::test]
async fn replan_cannot_turn_a_task_unit_with_a_child_into_a_leaf() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = root(dir.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let v1 = v3_plan(
        vec![stage("s1", false)],
        vec![leaf("a", "s1", &[]), task_unit("c", "s1", &[], "false")],
    );
    // 親の replan: done の leaf a は書かない（daemon が持ち越す）。1 回目は c を leaf にし、2 回目は kind task のまま直す。
    let to_leaf = v3_plan(vec![stage("s1", false)], vec![leaf("c", "s1", &[])]);
    let keep_task = v3_plan(
        vec![stage("s1", false)],
        vec![task_unit("c", "s1", &[], "true")],
    );
    let adapter = Arc::new(TreeAdapter::new(
        vec![v1, to_leaf, keep_task],
        Duration::ZERO,
    ));
    let mut d = tree_dispatcher(&store, adapter);
    let report = run_until_idle(&mut d, 2500).await;
    assert!(report.idle, "{report:?}");

    let events = store.events_for(root_id).unwrap();
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );
    let rejected: Vec<String> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. }
                if msg.contains("unit c has already started (child task ") =>
            {
                Some(msg.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(rejected.len(), 1, "{events:?}");
    let plans = store.execution_plan_list(root_id).unwrap();
    assert_eq!(plans.len(), 2, "the leaf rewrite is never adopted");
    let units = store.work_units_for(root_id).unwrap();
    let c = unit(&units, "c");
    assert_eq!(c.kind, task_core::WorkUnitKind::Task);
    assert_eq!(c.status, task_core::WorkUnitStatus::Done);
    let reasons = unit_reasons(&events, "c");
    assert!(
        !reasons.iter().any(|r| r == "child_create_failed"),
        "{reasons:?}"
    );
    let children: Vec<Task> = store
        .children(root_id)
        .unwrap()
        .into_iter()
        .filter(|t| t.title == "Child c")
        .collect();
    assert_eq!(children.len(), 2, "{children:?}");
    assert!(children.iter().any(|t| t.status == Status::Failed));
    let done = children
        .iter()
        .find(|t| t.status == Status::Done)
        .expect("the second child is done");
    assert_eq!(c.child_task_id, Some(done.id.to_string()));
    assert_replay_is_clean(&store);
}
