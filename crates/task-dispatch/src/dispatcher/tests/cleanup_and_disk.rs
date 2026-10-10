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

/// ADR-0125 (b): 削除は背景スレッドなので、`target/` の消滅と期待の `WorkspacePruned` event の両方が
/// 観測できるまで待つ（固定回数のループではなく出来事待ち）。[`STATE_WAIT_GUARD`] は壊れたときに止まる保険。
async fn wait_for_prune(
    store: &Arc<dyn TaskStore>,
    task_id: TaskId,
    target_dir: &std::path::Path,
    expected_removed: &[String],
) -> Vec<(u64, Event)> {
    let started = Instant::now();
    loop {
        let events = store.events_for(task_id).unwrap();
        let pruned = !target_dir.exists()
            && events.iter().any(|(_, e)| {
                matches!(
                    e,
                    Event::WorkspacePruned { removed } if removed == expected_removed
                )
            });
        if pruned {
            return events;
        }
        if started.elapsed() >= STATE_WAIT_GUARD {
            panic!(
                "workspace prune not observed within {STATE_WAIT_GUARD:?}: target_dir exists={}, events={events:?}",
                target_dir.exists(),
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `tick()` は、終端になってから `workspace_prune_after_secs` 経った作業場所の `target/` を消し、
/// `workspace_pruned` イベントを積む（削除は背景スレッド。出来事が届くまで待つ）。
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
    let expected_removed = vec!["repos/benchfs/target".to_string()];
    let events = wait_for_prune(&store, task.id, &target_dir, &expected_removed).await;
    assert!(!target_dir.exists(), "target/ should have been pruned");
    assert!(
        task_dir.join("repos").join("benchfs").is_dir(),
        "the repo dir itself is kept"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkspacePruned { removed } if removed == &expected_removed
        )),
        "{events:?}"
    );
}

/// `workspace_prune_after_secs == 0` は無効（何も消さない）。
///
/// この 50ms sleep は直さない: `prune_one_workspace` は `workspace_prune_after_secs == 0` のとき
/// 削除スレッドを一切立てずに即 return する（housekeeping.rs の `if ... == 0 { return; }`）ので、
/// 負荷で遅れて偽の失敗を生む非同期処理が無い（待っても届かない出来事を待つ形には書き直せない）。
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

/// ADR 2026-10-07-build-tmp-hygiene D4: tick の段 `disk_watch` は注入した probe と時計で 60 秒ごとに測り、
/// warn で通知・critical で受信箱に出し、新しい coding run を保留する。
#[tokio::test]
async fn disk_watch_tick_phase_holds_coding_until_pressure_recovers() {
    use crate::disk_watch::{DiskProbe, DiskUsage, DiskWatchEntry};

    /// 使用率 = `pct` の偽の filesystem（試験の途中で値を変える）。
    struct Probe(Arc<StdMutex<u64>>);
    impl DiskProbe for Probe {
        fn usage(&self, _: &std::path::Path) -> Result<DiskUsage, String> {
            let used = *self.0.lock().unwrap();
            Ok(DiskUsage {
                blocks: 100,
                bfree: 100 - used,
                bavail: 100 - used,
            })
        }
    }

    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let dir = tempfile::tempdir().unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: String::new(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let clock = Arc::new(StdMutex::new(
        OffsetDateTime::from_unix_timestamp(1_791_331_200).unwrap(),
    ));
    d.test_now = Some(clock.clone());
    let pct = Arc::new(StdMutex::new(85));
    d.set_disk_watch_with_probe(
        vec![DiskWatchEntry {
            path: "/local".into(),
            warn_pct: 80.0,
            critical_pct: 95.0,
        }],
        Box::new(Probe(pct.clone())),
    );
    let disk_notices = || {
        store
            .notice_list(&task_core::feed::NoticeQuery {
                kinds: vec![task_core::feed::NoticeKind::Disk],
                ..Default::default()
            })
            .unwrap()
            .items
    };
    d.tick().unwrap();
    assert_eq!(disk_notices().len(), 1);
    // 60 秒経つまでは測らない（使用率を上げても critical にならない）。
    *pct.lock().unwrap() = 97;
    *clock.lock().unwrap() += time::Duration::seconds(30);
    d.tick().unwrap();
    assert_eq!(
        sqlite.disk_watch_states().unwrap()[0].level,
        task_core::DiskLevel::Warn
    );
    *clock.lock().unwrap() += time::Duration::seconds(30);
    d.tick().unwrap();
    assert_eq!(
        sqlite.disk_watch_states().unwrap()[0].level,
        task_core::DiskLevel::Critical
    );
    let ctx = task_ops::view::ViewContext {
        workspace_root: dir.path().to_path_buf(),
        retry_backoff_base: Duration::ZERO,
        retry_backoff_max: Duration::ZERO,
        max_requeues: 5,
        clusters: Default::default(),
    };
    let inbox = task_ops::human_inbox::human_inbox(
        store.as_ref(),
        None,
        &ctx,
        OffsetDateTime::now_utc(),
        &|_, _| vec![],
    )
    .unwrap();
    assert!(
        inbox.items.iter().any(|i| i.id == "disk_full-local"),
        "{:?}",
        inbox.items
    );
    assert_eq!(disk_notices().len(), 1, "critical goes to the inbox only");

    // critical 中は attempts を消費せず ready に留める。
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 0);
    let held = store.get(task.id).unwrap().unwrap();
    assert_eq!(held.status, Status::Ready);
    assert_eq!(held.attempts, task.attempts);
    assert!(d.disk_watch_critical());
    let mut conversation = task.clone();
    conversation.genre = Some("conversation".into());
    assert!(!d.held_by_disk_critical(&conversation));
    let review = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&review).unwrap();
    store
        .apply_transition(review.id, Trigger::Dispatch, None)
        .unwrap();
    store
        .apply_transition(review.id, Trigger::WorkerDone, None)
        .unwrap();
    assert!(
        !d.spawn_review(
            review.id,
            "test-review".into(),
            &ReviewSubject {
                summary: "ok".into(),
                evidence: vec![]
            }
        )
        .unwrap()
    );
    assert_eq!(
        store.get(review.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    // Remove this fixture from scheduling before testing the held worker's recovery.
    store
        .apply_transition(review.id, Trigger::ReviewPass, None)
        .unwrap();
    *pct.lock().unwrap() = 89;
    *clock.lock().unwrap() += time::Duration::seconds(60);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert!(!d.disk_watch_critical());
}

/// ADR 2026-10-10-local-disk-growth-paths D2: cron の `target_sweep` が無くても、tick が終端の木の repo 直下
/// cargo target を消して `workspace_pruned` を積む（workspace prune は無効にして経路を分ける）。
#[tokio::test]
async fn repo_target_gc_tick_removes_a_finished_task_target_without_cron() {
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
    for t in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        store
            .apply_transition_with_events(task.id, t, vec![])
            .unwrap();
    }
    let mut current = store.get(task.id).unwrap().unwrap();
    current.updated_at = OffsetDateTime::now_utc() - time::Duration::hours(7);
    store
        .update_task(&current, Event::worker_progress("x", "backdated"))
        .unwrap();
    let repo = root
        .path()
        .join(task.id.to_string())
        .join("repos")
        .join("agent-platform");
    let target_dir = repo.join("target");
    std::fs::create_dir_all(target_dir.join("debug")).unwrap();
    std::fs::write(
        target_dir.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    std::fs::write(target_dir.join("debug").join(".cargo-lock"), "").unwrap();

    d.tick().unwrap();
    let expected_removed = vec!["repos/agent-platform/target".to_string()];
    wait_for_prune(&store, task.id, &target_dir, &expected_removed).await;
    assert!(repo.is_dir(), "the checkout itself is kept");
}
