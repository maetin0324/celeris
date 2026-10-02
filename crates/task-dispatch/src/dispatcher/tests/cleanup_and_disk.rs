use super::*;

#[tokio::test]
async fn disk_gate_notifies_once_and_recovers_without_a_run() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: String::new(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.workspace_root = dir.path().join("not-created");
    assert!(free_disk_mb(&d.config.workspace_root).is_ok());
    d.config.min_free_disk_mb = u64::MAX;
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Ready);
    let notices = store
        .notice_list(&task_core::feed::NoticeQuery::default())
        .unwrap();
    assert_eq!(notices.total, 1);
    assert_eq!(notices.items[0].kind, task_core::feed::NoticeKind::BadNews);
    assert!(notices.items[0].title.contains("ディスク不足"));
    d.config.min_free_disk_mb = 0;
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert!(!d.disk_low);
    d.config.min_free_disk_mb = u64::MAX;
    assert!(!d.check_disk_space());
    assert_eq!(
        store
            .notice_list(&task_core::feed::NoticeQuery::default())
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        store
            .notice_list(&task_core::feed::NoticeQuery::default())
            .unwrap()
            .items[0]
            .count,
        2
    );
}

// ---- ADR-0066 D2（Phase 110b）: 終端タスクの作業場所からビルド生成物を刈る ----

/// `tick()` は、終端になってから `workspace_prune_after_secs` 経った作業場所の `target/` を消し、
/// `workspace_pruned` イベントを積む（削除は背景スレッド。少し待てば反映される）。
#[tokio::test]
async fn tick_prunes_the_oldest_terminal_workspace_and_records_an_event() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = tempfile::tempdir().unwrap();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    d.config.workspace_root = root.path().to_path_buf();
    d.config.workspace_prune_after_secs = 1;

    let task = new_task(
        root.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    store
        .apply_transition_with_events(task.id, Trigger::Dispatch, vec![])
        .unwrap();
    store
        .apply_transition_with_events(task.id, Trigger::WorkerDone, vec![])
        .unwrap();
    store
        .apply_transition_with_events(task.id, Trigger::ReviewPass, vec![])
        .unwrap();
    let mut current = store.get(task.id).unwrap().unwrap();
    current.updated_at = OffsetDateTime::now_utc() - time::Duration::days(2);
    store
        .update_task(&current, Event::worker_progress("x", "backdated"))
        .unwrap();

    let task_dir = root.path().join(task.id.to_string());
    let target_dir = task_dir.join("repos").join("benchfs").join("target");
    std::fs::create_dir_all(&target_dir).unwrap();

    d.tick().unwrap();
    // 削除は背景スレッドなので、少し待って反映を確かめる。
    for _ in 0..100 {
        if !target_dir.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!target_dir.exists(), "target/ should have been pruned");
    assert!(
        task_dir.join("repos").join("benchfs").is_dir(),
        "the repo dir itself is kept"
    );
    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkspacePruned { removed }
                if removed == &vec!["repos/benchfs/target".to_string()]
        )),
        "{events:?}"
    );
}

/// `workspace_prune_after_secs == 0` は無効（何も消さない）。
#[tokio::test]
async fn workspace_prune_after_secs_zero_disables_pruning() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = tempfile::tempdir().unwrap();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    d.config.workspace_root = root.path().to_path_buf();
    d.config.workspace_prune_after_secs = 0;

    let task = new_task(
        root.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    store
        .apply_transition_with_events(task.id, Trigger::Dispatch, vec![])
        .unwrap();
    store
        .apply_transition_with_events(task.id, Trigger::WorkerDone, vec![])
        .unwrap();
    store
        .apply_transition_with_events(task.id, Trigger::ReviewPass, vec![])
        .unwrap();
    let mut current = store.get(task.id).unwrap().unwrap();
    current.updated_at = OffsetDateTime::now_utc() - time::Duration::days(30);
    store
        .update_task(&current, Event::worker_progress("x", "backdated"))
        .unwrap();

    let task_dir = root.path().join(task.id.to_string());
    let target_dir = task_dir.join("repos").join("benchfs").join("target");
    std::fs::create_dir_all(&target_dir).unwrap();

    d.tick().unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        target_dir.exists(),
        "prune_after_secs = 0 must disable pruning"
    );
}
