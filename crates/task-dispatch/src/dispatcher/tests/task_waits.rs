//! ADR 2026-10-11-task-wait-reasons: dispatcher の見送りの理由（メモリの M1〜M9 は snapshot の `task_waits`、
//! DB の理由は task-core の `db_waits` / `resolve_children` と同じ関数）。偽のアダプタと一時ディレクトリだけで、
//! 外部ネットワークに出ない。

use super::*;

fn slow_adapter() -> Arc<InstantAdapter> {
    Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_secs(30),
    })
}

fn true_task(dir: &std::path::Path) -> Task {
    new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    )
}

fn publish(d: &mut Dispatcher) -> tokio::sync::watch::Receiver<Option<DaemonSnapshot>> {
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-10-11T00:00:00Z".into(),
        tick_ms: 50,
        providers: vec![],
        provider_checks: Default::default(),
    });
    rx
}

fn snapshot_wait(
    rx: &tokio::sync::watch::Receiver<Option<DaemonSnapshot>>,
    id: TaskId,
) -> Option<task_core::TaskWait> {
    rx.borrow()
        .as_ref()
        .expect("snapshot")
        .task_waits
        .iter()
        .find(|w| w.task_id == id)
        .map(|w| w.wait.clone())
}

/// 枠満杯の見送り（M1/M9）と経路なし（M2）が tick ごとに snapshot の `task_waits` に task ごとに載る。
#[tokio::test]
async fn task_wait_full_capacity_and_unroutable_skips_are_in_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut no_route = true_task(dir.path());
    no_route.priority = 10;
    no_route.worker_hint.adapter = Some("nonexistent".into());
    store.insert(&no_route).unwrap();
    let mut first = true_task(dir.path());
    first.priority = 5;
    store.insert(&first).unwrap();
    let second = true_task(dir.path());
    store.insert(&second).unwrap();

    let mut d = dispatcher(store.clone(), slow_adapter(), 1);
    let rx = publish(&mut d);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(
        store.get(first.id).unwrap().unwrap().status,
        Status::Running
    );

    assert!(matches!(
        snapshot_wait(&rx, no_route.id),
        Some(task_core::TaskWait::Unroutable { .. })
    ));
    assert_eq!(
        snapshot_wait(&rx, second.id),
        Some(task_core::TaskWait::Capacity {
            scope: task_core::CapacityScope::Global,
            id: None
        })
    );
    assert_eq!(snapshot_wait(&rx, first.id), None, "dispatched: no wait");

    // 次の tick（枠は埋まったまま）: 照合しない ready にも全体の枠待ちが付く。
    d.tick().unwrap();
    assert_eq!(
        snapshot_wait(&rx, second.id),
        Some(task_core::TaskWait::Capacity {
            scope: task_core::CapacityScope::Global,
            id: None
        })
    );
}

/// depends_on 待ちと祖先の一時停止: dispatcher は `ready_tasks`（`db_waits`）で見送り、一覧の API 用の
/// `db_waits_for` も同じ理由を返す。
#[tokio::test]
async fn task_wait_depends_on_and_paused_ancestor_are_not_dispatched() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let mut paused_parent = true_task(dir.path());
    paused_parent.status = Status::Running;
    paused_parent.paused_at = Some(OffsetDateTime::now_utc());
    store.insert(&paused_parent).unwrap();
    let mut under_pause = true_task(dir.path());
    under_pause.parent_id = Some(paused_parent.id);
    store.insert(&under_pause).unwrap();
    let mut pred = true_task(dir.path());
    pred.status = Status::Running;
    store.insert(&pred).unwrap();
    let mut dependent = true_task(dir.path());
    dependent.depends_on = vec![pred.id];
    store.insert(&dependent).unwrap();

    let mut d = dispatcher(store.clone(), slow_adapter(), 4);
    assert_eq!(d.tick().unwrap().dispatched, 0);
    for id in [under_pause.id, dependent.id] {
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Ready);
    }
    let waits = sqlite
        .db_waits_for(&[under_pause.clone(), dependent.clone()])
        .unwrap();
    assert_eq!(
        waits[&under_pause.id],
        vec![task_core::TaskWait::Paused {
            by: paused_parent.id
        }]
    );
    assert_eq!(
        waits[&dependent.id],
        vec![task_core::TaskWait::DependsOn {
            task_ids: vec![pred.id]
        }]
    );
}

/// 子待ちの親: dispatcher の `resolve_child_dependencies` と一覧・木の `plan_waits` が同じ
/// `task_core::resolve_children` で「未完の子を待つ unit」を決める。
#[tokio::test]
async fn task_wait_parent_waiting_on_child_uses_the_shared_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let parent = true_task(dir.path());
    store.insert(&parent).unwrap();
    let mut child = true_task(dir.path());
    child.parent_id = Some(parent.id);
    child.status = Status::Running;
    child.labels = vec!["child-report".into()];
    store.insert(&child).unwrap();

    let units = vec![task_core::WorkUnitRow::new(
        "wu-merge".into(),
        parent.id.to_string(),
        "plan".into(),
        0,
        wu_spec("merge", &["child:report"]),
        task_core::WorkUnitStatus::Pending,
        "2026-10-11T00:00:00Z".into(),
    )];
    let d = dispatcher(store.clone(), slow_adapter(), 1);
    let (changed, waiting) = d.resolve_child_dependencies(parent.id, &units).unwrap();
    assert!(!changed);
    assert!(waiting, "the dispatcher holds the parent for its child");

    let children = vec![task_core::ChildFacts {
        task_id: child.id,
        status: child.status,
        labels: child.labels.clone(),
    }];
    assert_eq!(
        task_core::plan_waits(&units, &children, &[]).first(),
        Some(&task_core::TaskWait::Children {
            task_ids: vec![child.id],
            unit_keys: vec!["merge".into()],
        })
    );
}
