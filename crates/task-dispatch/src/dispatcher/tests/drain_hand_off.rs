//! ADR-0040 付記（2026-10-04、WU 検査の引き継ぎ）: draining の旧 instance は worker run の終わりで手を離し、WU の
//! 受け入れ検査・統合は新しい active instance が store の状態から拾って行う。検査は fake のコマンド（`touch` で
//! 走った印を残す）、run は gate で止める fake adapter で決定的に走らせる。
use super::*;

/// 検査 2 件（1 件目は走った印を残す・2 件目は出力を出す）を持つ葉 `a` だけの 1 段の v2 計画。
fn adopt_checked_plan(store: &Arc<dyn TaskStore>, task_id: TaskId, marker: &Path) {
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![
        task_core::WorkUnitCheck {
            cmd: format!("echo ran >> {}", marker.display()),
            expect_exit: 0,
            scope: false,
        },
        task_core::WorkUnitCheck {
            cmd: "echo wu-check-out; echo wu-check-err 1>&2".into(),
            expect_exit: 0,
            scope: false,
        },
    ];
    adopt_v2_plan(store, task_id, &["build"], vec![a]);
}

fn wu_a(store: &Arc<dyn TaskStore>, id: TaskId) -> task_core::WorkUnitRow {
    store
        .work_units_for(id)
        .unwrap()
        .into_iter()
        .find(|u| u.key == "a")
        .unwrap()
}

fn handed_off(store: &Arc<dyn TaskStore>, id: TaskId, run_id: &str) -> usize {
    events_of(store, id)
        .iter()
        .filter(|e| matches!(e, Event::WorkUnitChecksHandedOff { run_id: r, key, .. } if r == run_id && key == "a"))
        .count()
}

fn wu_check_starts(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
) -> Vec<(String, u32, u32, String, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkUnitCheckStarted {
                run_id,
                index,
                total,
                cmd,
                log_path,
                ..
            } => Some((run_id, index, total, cmd, log_path)),
            _ => None,
        })
        .collect()
}

/// 旧 instance が draining のまま worker run が終わった: 旧 instance は検査を始めず（印も `WorkUnitCheckStarted`
/// も無い）、`WorkUnitChecksHandedOff` を残して in_flight 0 で drain を終える。新しい active instance が検査を
/// 行い（開始・終了の event とログ）、WU は done、`WorkerFinished` は 1 件、run は増えない。
/// 修正前は旧 instance が run の完了を受けてそのまま検査を spawn し（`checking`）、印が残っていた。
#[tokio::test]
async fn a_draining_instance_leaves_the_work_unit_checks_to_the_new_active() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let marker = flags.path().join("checks-ran");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_checked_plan(&store, task.id, &marker);
    let adapter = Arc::new(ParallelWuAdapter::new(Duration::ZERO).holding("a"));
    let mut old = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let s = store.clone();
    assert!(
        run_until_state(&mut old, || wu_a(&s, task.id).status
            == task_core::WorkUnitStatus::Running)
        .await
    );
    let run_id = wu_a(&store, task.id).last_run_id.unwrap();
    assert!(old.in_flight() > 0);

    // live handoff: 旧 instance は draining になり、手元の run が終わるまで tick する（supervisor と同じ）。
    old.set_accepting_new_work(false);
    let mut drained = false;
    for _ in 0..500 {
        adapter.gate.notify_waiters();
        old.tick().unwrap();
        if old.in_flight() == 0 {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(drained, "{:?}", events_of(&store, task.id));
    assert!(old.checking.is_empty());
    assert!(
        !marker.exists(),
        "draining の旧 instance が WU の検査を流した"
    );
    assert!(wu_check_starts(&store, task.id).is_empty());
    assert_eq!(handed_off(&store, task.id, &run_id), 1);
    let u = wu_a(&store, task.id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Running, "{u:?}");
    assert!(worker_finished_outcomes(&store, task.id, &run_id).is_empty());
    // 旧 instance はその後の tick でも検査を始めない。
    old.tick().unwrap();
    assert!(old.checking.is_empty() && !marker.exists());
    drop(old);

    // 新しい active instance が store の状態（印・result.json）から拾って検査を行う。
    let mut active = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    active.tick().unwrap();
    assert!(
        active.checking.contains_key(&run_id),
        "{:?}",
        events_of(&store, task.id)
    );
    run_until_task_terminal(&mut active, &store, task.id).await;
    // WU の検査で 1 回、段の統合（unit の checks を再実行する）で 1 回。
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "ran\nran\n");
    let u = wu_a(&store, task.id);
    assert_eq!(u.status, task_core::WorkUnitStatus::Done, "{u:?}");
    assert_eq!(u.runs, 1);
    let outcomes = worker_finished_outcomes(&store, task.id, &run_id);
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(outcomes[0].starts_with("done: "), "{outcomes:?}");
    let starts = wu_check_starts(&store, task.id);
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!(starts.iter().all(|s| s.0 == run_id && s.2 == 2));
    assert_eq!((starts[0].1, starts[1].1), (0, 1));
    assert!(
        starts[1].4.contains("/work-unit-checks/a/"),
        "{}",
        starts[1].4
    );
    let log = std::fs::read_to_string(&starts[1].4).unwrap();
    assert!(
        log.contains("wu-check-out") && log.contains("wu-check-err"),
        "{log}"
    );
    let finished: Vec<(u32, bool, Option<i32>)> = events_of(&store, task.id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkUnitCheckFinished {
                run_id: r,
                index,
                pass,
                exit,
                ..
            } if r == run_id => Some((index, pass, exit)),
            _ => None,
        })
        .collect();
    assert_eq!(finished, vec![(0, true, Some(0)), (1, true, Some(0))]);
    let row = store.run_index_get(&run_id).unwrap().unwrap();
    assert_eq!(row.status, task_core::RunIndexStatus::Completed, "{row:?}");
}

/// draining の旧 instance で段の最後の WU（検査なし）が終わった: 旧 instance は統合を始めず、新しい active が
/// 統合して Task を進める。
#[tokio::test]
async fn a_draining_instance_does_not_start_the_phase_integration() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(&store, task.id, &["build"], vec![v2_wu("a", "build", &[])]);
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::ZERO)
            .with_file("a", "a.txt", "a\n")
            .holding("a"),
    );
    let mut old = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 2, 2, 2);
    let s = store.clone();
    assert!(
        run_until_state(&mut old, || wu_a(&s, task.id).status
            == task_core::WorkUnitStatus::Running)
        .await
    );
    old.set_accepting_new_work(false);
    let mut drained = false;
    for _ in 0..500 {
        adapter.gate.notify_waiters();
        old.tick().unwrap();
        if old.in_flight() == 0 {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(drained);
    assert!(old.integrating.is_empty());
    assert_eq!(
        wu_a(&store, task.id).status,
        task_core::WorkUnitStatus::Done
    );
    let integrate = |s: &Arc<dyn TaskStore>| {
        s.work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.kind == task_core::WorkUnitKind::Integrate)
            .unwrap()
    };
    assert_ne!(
        integrate(&store).status,
        task_core::WorkUnitStatus::Done,
        "draining の旧 instance が統合した"
    );
    assert!(
        !events_of(&store, task.id).iter().any(|e| matches!(
            e,
            Event::PhaseIntegrated { .. } | Event::IntegrationCheckStarted { .. }
        )),
        "draining の旧 instance が統合を始めた"
    );
    old.tick().unwrap();
    assert!(old.integrating.is_empty());
    drop(old);
    let mut active = parallel_dispatcher(store.clone(), adapter, root.path(), 2, 2, 2);
    run_until_task_terminal(&mut active, &store, task.id).await;
    assert_eq!(integrate(&store).status, task_core::WorkUnitStatus::Done);
}
