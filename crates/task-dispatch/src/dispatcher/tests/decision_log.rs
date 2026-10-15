//! 判断・実行・成果ログ（`dispatcher/decision_log_wire.rs`）: 実行時に新欄が埋まる。

use super::*;

use task_core::decision_log::RunEndLabel;

/// 実際に dispatch して run が終わると、`decision_log_runs` に提案・実行・成果が別欄で入る。
#[tokio::test]
async fn dispatch_fills_decision_log_fields_at_run_time() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        ws.path(),
        Check::Command {
            cmd: "cargo test".into(),
            expect_exit: 0,
        },
        2,
    );
    task.objective = "crates/task-core/src/model.rs の typo を直す".into();
    task.genre = Some("coding".into());
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Hint,
        ..Default::default()
    });
    store.insert(&task).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "not needed".into(),
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();
    let run_id = store
        .events_for(task.id)
        .unwrap()
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted { run_id, .. } => Some(run_id.clone()),
            _ => None,
        })
        .expect("worker_started");
    let mut finished = None;
    for _ in 0..400 {
        if let Some(run) = store.decision_log_run_get(&run_id).unwrap()
            && run.outcome.run_end.is_some()
        {
            finished = Some(run);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        d.tick().unwrap();
    }
    let run = finished.expect("decision log outcome is written when the run ends");
    assert_eq!(
        run.proposal.task_id.as_deref(),
        Some(task.id.to_string().as_str())
    );
    assert!(run.proposal.decided_lane.is_some(), "{run:?}");
    assert!(run.execution.exec_model.is_some());
    assert!(run.execution.exec_source_id.is_some());
    assert_eq!(run.outcome.run_end, Some(RunEndLabel::Question));
    // 再処理しても行は 1 つのまま。
    d.record_decision_log(task.id);
    assert_eq!(store.decision_log_run_get(&run_id).unwrap().unwrap(), run);
}
