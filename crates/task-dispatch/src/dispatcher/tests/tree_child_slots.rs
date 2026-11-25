//! ADR 2026-11-24-paused-children-free-child-slots: 同時の子 task の枠（`max_parallel_child_tasks`）と一時停止。
//!
//! 既存の `tree::unit_mirrors_child_status` は止まっていない子で上限を見るだけで、一時停止した子が枠を占めて兄弟を黙って止める
//! 事象（2026-11-24、MemT の root）も、枠の待ちが `waits` に出ないことも検出できないので足す。偽のアダプタと一時ディレクトリだけで、
//! 外部ネットワークに出ない。

use super::tree::{
    TreeAdapter, child_of, drive_child, stage, task_unit, tree_dispatcher, tree_limits, unit,
    v3_plan, view_ctx,
};
use super::*;

/// 段 s1 に kind task の unit を `keys` で持つ root を作り、計画を採用する（unit の seq は key の順になるので、key の先頭に順を書く）。
fn tree_root(store: &Arc<SqliteStore>, dir: &std::path::Path, keys: &[&str]) -> TaskId {
    let root = compound_task(dir);
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let spec: task_core::ExecutionPlanSpec = serde_json::from_str(&v3_plan(
        vec![stage("s1", false)],
        keys.iter()
            .map(|k| task_unit(k, "s1", &[], "true"))
            .collect(),
    ))
    .unwrap();
    task_ops::execution::adopt_plan(
        store.as_ref(),
        root_id,
        spec,
        task_core::PlanOrigin::Human,
        None,
        tree_limits(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    root_id
}

/// `GET /tasks/{id}/execution` と同じ材料（`task_ops::view::task_detail` の `waits`、API と同じく上限を渡した `MemoryWaits`）。
fn detail_waits(store: &Arc<SqliteStore>, id: TaskId) -> Vec<task_core::TaskWait> {
    let memory = task_ops::wait::MemoryWaits::default().with_child_slot_limit(2);
    task_ops::view::task_detail(
        store.as_ref(),
        id,
        &view_ctx(),
        &memory,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .waits
}

fn child_slot(waits: &[task_core::TaskWait]) -> Option<(Vec<String>, Vec<TaskId>, usize)> {
    waits.iter().find_map(|w| match w {
        task_core::TaskWait::ChildSlot {
            unit_keys,
            task_ids,
            limit,
        } => Some((unit_keys.clone(), task_ids.clone(), *limit)),
        _ => None,
    })
}

fn sorted(mut ids: Vec<TaskId>) -> Vec<TaskId> {
    ids.sort();
    ids
}

/// D1・D3・D2: 今回の事象の形。子 2 件が枠を占めている間は残りの unit が `child_slot` で待つ（`waits` に待つ unit の key と
/// 枠を占める子の id が出る）。2 件の子を自分で一時停止すると、上限 2 のまま次の照合で残りの unit の子が作られる。一時停止を
/// 解くと枠を占める子は 4/2 になるが、再開した子は待たせない（`ready_tasks` に入る）。
#[tokio::test]
async fn self_paused_children_free_their_slots() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root_id = tree_root(&store, dir.path(), &["a-e2", "b-e3", "c-e1b", "d-e1c"]);
    let adapter = Arc::new(TreeAdapter::new(Vec::new(), Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);
    assert_eq!(d.config.execution.limits.tree.max_parallel_child_tasks, 2);

    d.reconcile_tree_units().unwrap();
    let e2 = child_of(&store, root_id, "a-e2");
    let e3 = child_of(&store, root_id, "b-e3");
    let units = store.work_units_for(root_id).unwrap();
    assert!(unit(&units, "c-e1b").child_task_id.is_none());
    // D3: 枠の待ちは `children` とは別の待ちとして出る。
    let waits = detail_waits(&store, root_id);
    assert_eq!(
        child_slot(&waits),
        Some((
            vec!["c-e1b".to_string(), "d-e1c".to_string()],
            sorted(vec![e2, e3]),
            2
        )),
        "{waits:?}"
    );
    assert!(
        waits
            .iter()
            .any(|w| matches!(w, task_core::TaskWait::Children { .. }))
    );

    // D1: 2 件の子を自分で一時停止すると枠に数えない → 上限 2 のまま次の照合で e1b・e1c の子が作られる。
    let now = OffsetDateTime::now_utc();
    task_ops::lifecycle::pause_task(store.as_ref(), e2, now).unwrap();
    task_ops::lifecycle::pause_task(store.as_ref(), e3, now).unwrap();
    assert_eq!(child_slot(&detail_waits(&store, root_id)), None);
    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert_eq!(
        unit(&units, "c-e1b").status,
        task_core::WorkUnitStatus::Running
    );
    let e1b = child_of(&store, root_id, "c-e1b");
    let e1c = child_of(&store, root_id, "d-e1c");
    assert_eq!(store.children(root_id).unwrap().len(), 4);
    let ready: Vec<TaskId> = store
        .ready_tasks(1_000)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(ready.contains(&e1b) && ready.contains(&e1c));
    assert!(!ready.contains(&e2) && !ready.contains(&e3));

    // D2: 一時停止を解くと枠を占める子は 4/2 になるが、再開した子は待たせない（ready_tasks に入る）。上限を超えている間の
    // 新しい子の扱いは `resumed_children_may_exceed_the_limit_and_block_new_children`。
    task_ops::lifecycle::resume_task(store.as_ref(), e2, now).unwrap();
    task_ops::lifecycle::resume_task(store.as_ref(), e3, now).unwrap();
    let ready: Vec<TaskId> = store
        .ready_tasks(1_000)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(
        ready.contains(&e2) && ready.contains(&e3),
        "再開した子は枠の空きを待たない"
    );
    d.reconcile_tree_units().unwrap();
    assert_eq!(store.children(root_id).unwrap().len(), 4);
}

/// D2: 再開で上限を超えている間は、新しい子を作らずに `child_slot` で待ち、上限を下回ったら作る。
#[tokio::test]
async fn resumed_children_may_exceed_the_limit_and_block_new_children() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root_id = tree_root(&store, dir.path(), &["a", "b", "c", "d"]);
    let adapter = Arc::new(TreeAdapter::new(Vec::new(), Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);

    d.reconcile_tree_units().unwrap();
    let a = child_of(&store, root_id, "a");
    let b = child_of(&store, root_id, "b");
    let now = OffsetDateTime::now_utc();
    // a を一時停止 → c が作られる（a は数えない: b・c で 2/2）。
    task_ops::lifecycle::pause_task(store.as_ref(), a, now).unwrap();
    d.reconcile_tree_units().unwrap();
    let c = child_of(&store, root_id, "c");
    let units = store.work_units_for(root_id).unwrap();
    assert!(unit(&units, "d").child_task_id.is_none(), "b・c で 2/2");
    // a を再開 → 3/2。a は待たせない。d は作られず、枠の待ちに a・b・c が並ぶ。
    task_ops::lifecycle::resume_task(store.as_ref(), a, now).unwrap();
    assert!(store.ready_tasks(1_000).unwrap().iter().any(|t| t.id == a));
    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert!(unit(&units, "d").child_task_id.is_none());
    assert_eq!(
        child_slot(&detail_waits(&store, root_id)),
        Some((vec!["d".to_string()], sorted(vec![a, b, c]), 2))
    );
    // b が終わっても 2/2（a・c）なので、まだ作らない。
    drive_child(&store, b, Status::Done);
    d.reconcile_tree_units().unwrap();
    assert!(
        unit(&store.work_units_for(root_id).unwrap(), "d")
            .child_task_id
            .is_none()
    );
    // c も終われば 1/2 → d が作られる。
    drive_child(&store, c, Status::Done);
    d.reconcile_tree_units().unwrap();
    assert!(
        unit(&store.work_units_for(root_id).unwrap(), "d")
            .child_task_id
            .is_some()
    );
}

/// D1: 祖先（ここでは親）の一時停止で止まっている子は従来どおり枠に数える。子自身の `paused_at` は無いので、3 件目の unit の
/// 子は作られず、`waits` には親の一時停止と枠の待ちが出る。
#[tokio::test]
async fn children_stopped_by_an_ancestor_pause_keep_their_slots() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<SqliteStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root_id = tree_root(&store, dir.path(), &["a", "b", "c"]);
    let adapter = Arc::new(TreeAdapter::new(Vec::new(), Duration::ZERO));
    let mut d = tree_dispatcher(&store, adapter);

    d.reconcile_tree_units().unwrap();
    let a = child_of(&store, root_id, "a");
    let b = child_of(&store, root_id, "b");
    task_ops::lifecycle::pause_task(store.as_ref(), root_id, OffsetDateTime::now_utc()).unwrap();
    let ready: Vec<TaskId> = store
        .ready_tasks(1_000)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(
        !ready.contains(&a) && !ready.contains(&b),
        "子は祖先の一時停止で止まる"
    );
    d.reconcile_tree_units().unwrap();
    let units = store.work_units_for(root_id).unwrap();
    assert!(unit(&units, "c").child_task_id.is_none());
    assert_eq!(store.children(root_id).unwrap().len(), 2);
    let waits = detail_waits(&store, root_id);
    assert!(
        waits
            .iter()
            .any(|w| matches!(w, task_core::TaskWait::Paused { by } if *by == root_id)),
        "{waits:?}"
    );
    assert_eq!(
        child_slot(&waits),
        Some((vec!["c".to_string()], sorted(vec![a, b]), 2))
    );
}
