//! 2026-10-04 統合の検査の進み具合: 段の統合の検査 1 件ごとに `IntegrationCheckStarted` / `IntegrationCheckFinished`
//! が残り、出力がログファイルへ逐次書かれる（実行中でも末尾が読める）。検査は fake のコマンドで決定的に走らせる。
use super::*;

/// 統合 WU の検査（unit の checks と workspace.toml の check）が、1 件ずつ開始・終了の event（cmd・exit・所要時間）と
/// ログファイルを残し、終了は `PhaseIntegrated` より前に積まれる。
#[tokio::test]
async fn integration_checks_record_start_finish_events_and_log_files() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![
        task_core::WorkUnitCheck {
            cmd: "echo first-out; echo first-err 1>&2".into(),
            expect_exit: 0,
        },
        task_core::WorkUnitCheck {
            cmd: "printf 'second-out\\n'".into(),
            expect_exit: 0,
        },
    ];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![a, v2_wu("b", "build", &[])],
    );
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)));
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let events = events_of(&store, task.id);
    let integ = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "integrate-build")
        .expect("integrate-build");
    let started: Vec<(u32, u32, String, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::IntegrationCheckStarted {
                work_unit_id,
                key,
                index,
                total,
                cmd,
                log_path,
                ..
            } if *work_unit_id == integ.id && key == "integrate-build" => {
                Some((*index, *total, cmd.clone(), log_path.clone()))
            }
            _ => None,
        })
        .collect();
    // unit a の 2 件と workspace の既定の check（task の acceptance の `true`）。
    let total = u32::try_from(started.len()).unwrap();
    assert!(total >= 2, "{started:?}");
    assert_eq!(started[0].2, "echo first-out; echo first-err 1>&2");
    assert_eq!(started[1].2, "printf 'second-out\\n'");
    for (i, s) in started.iter().enumerate() {
        assert_eq!(s.0, u32::try_from(i).unwrap());
        assert_eq!(s.1, total);
    }
    let first_log = std::fs::read_to_string(&started[0].3).expect("first log");
    assert!(first_log.contains("first-out"), "{first_log}");
    assert!(first_log.contains("first-err"), "{first_log}");
    let second_log = std::fs::read_to_string(&started[1].3).expect("second log");
    assert_eq!(second_log, "second-out\n");
    assert!(
        started[0]
            .3
            .contains("/integration-checks/integrate-build/"),
        "{}",
        started[0].3
    );
    let finished: Vec<(u32, String, bool, Option<i32>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::IntegrationCheckFinished {
                work_unit_id,
                index,
                cmd,
                pass,
                exit,
                timed_out,
                ..
            } if *work_unit_id == integ.id => {
                assert!(!timed_out);
                Some((*index, cmd.clone(), *pass, *exit))
            }
            _ => None,
        })
        .collect();
    assert_eq!(finished.len(), started.len());
    for (f, s) in finished.iter().zip(&started) {
        assert_eq!((f.0, &f.1), (s.0, &s.2));
        assert!(f.2);
        assert_eq!(f.3, Some(0));
    }
    let pos = |pred: &dyn Fn(&Event) -> bool| events.iter().position(pred).unwrap();
    let last_finished = events
        .iter()
        .rposition(|e| matches!(e, Event::IntegrationCheckFinished { .. }))
        .unwrap();
    assert!(last_finished < pos(&|e| matches!(e, Event::PhaseIntegrated { .. })));
    assert!(
        pos(&|e| matches!(e, Event::IntegrationCheckStarted { index: 0, .. }))
            < pos(&|e| matches!(e, Event::IntegrationCheckFinished { index: 0, .. }))
    );
}

/// 実行中の検査は Started だけが積まれ、その時点のログに出力の途中までが読める。終われば exit・不合格が残る。
#[tokio::test]
async fn a_running_integration_check_has_a_started_event_and_a_readable_partial_log() {
    let ws_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(ws_dir.path(), "true");
    store.insert(&task).unwrap();
    let gate = ws_dir.path().join("go");
    let observed = super::super::phase_integration::ObservedIntegration {
        work_unit_id: "wu-int".into(),
        key: "integrate-p1".into(),
        log_dir: ws_dir
            .path()
            .join("integration-checks")
            .join("integrate-p1"),
    };
    let checks = vec![
        task_core::WorkUnitCheck {
            // 出力の前半を出してから、試験が `go` を置くまで待つ（出来事待ち。CPU は焼かない）。
            cmd: format!(
                "echo before-gate; while [ ! -f '{}' ]; do sleep 0.02; done; echo after-gate; exit 3",
                gate.display()
            ),
            expect_exit: 0,
        },
        task_core::WorkUnitCheck {
            cmd: "echo ok".into(),
            expect_exit: 0,
        },
    ];
    let ws = task_worker::LocalWorkspace::new(ws_dir.path());
    let store2 = store.clone();
    let task_id = task.id;
    let handle = tokio::spawn(async move {
        super::super::phase_integration::run_integration_checks(
            store2.as_ref(),
            task_id,
            &observed,
            &ws,
            &checks,
            Duration::from_secs(30),
        )
        .await
    });
    // Started が積まれ、ログに前半が出るまで待つ。
    let mut log_path = None;
    for _ in 0..1000 {
        let events = events_of(&store, task.id);
        if let Some(Event::IntegrationCheckStarted { log_path: p, .. }) = events
            .iter()
            .find(|e| matches!(e, Event::IntegrationCheckStarted { index: 0, .. }))
            && std::fs::read_to_string(p).is_ok_and(|t| t.contains("before-gate"))
        {
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, Event::IntegrationCheckFinished { .. })),
                "まだ終わっていない"
            );
            log_path = Some(p.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let log_path = log_path.expect("the running check's log has its first line");
    let partial = std::fs::read_to_string(&log_path).unwrap();
    assert!(!partial.contains("after-gate"), "{partial}");
    std::fs::write(&gate, "").unwrap();
    let results = handle.await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(!results[0].0);
    assert!(results[1].0);
    let full = std::fs::read_to_string(&log_path).unwrap();
    assert!(full.contains("after-gate"), "{full}");
    let finished: Vec<(u32, u32, bool, Option<i32>)> = events_of(&store, task.id)
        .into_iter()
        .filter_map(|e| match e {
            Event::IntegrationCheckFinished {
                index,
                total,
                pass,
                exit,
                ..
            } => Some((index, total, pass, exit)),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        vec![(0, 2, false, Some(3)), (1, 2, true, Some(0))]
    );
}
