//! ADR 2026-10-22-stalled-auto-replan: 進展の無い自動 replan を止める。本番 2026-10-22 moshi-brewed の task 01M5HV2YTM
//! （統合 WU integrate-kcore の検査が毎回同じ compile error で落ち、planner は done の unit の check の書き方だけを変えて
//! 19 回 replan し、`max_replans` を使い切った）の再現。実 git の一時リポジトリと偽のアダプタだけで、外部に出ない。

use super::*;

fn replan_delta(base_version: u32, body: serde_json::Value) -> String {
    let mut delta = serde_json::json!({
        "schema": task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA,
        "base_version": base_version,
        "rationale": "retry the integration",
    });
    if let (Some(d), Some(b)) = (delta.as_object_mut(), body.as_object()) {
        d.extend(b.clone());
    }
    delta.to_string()
}

fn auto_replans(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::AutoReplanRequested { .. }))
        .count()
}

/// 統合の検査が常に同じ出力で落ち、偽の planner が done の unit `b` の check の書き方（`bash -c` の包み・`set -e`・
/// env の前置き）だけを変える計画を返す。自動 replan は 2 回以内で止まり、task は人への質問になる（D4 / D7）。
#[tokio::test]
async fn an_integration_failure_with_check_only_replans_stops_within_two_replans() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    // 分類に当たらない（repair を作らない）同じ stderr。compile error の repair は下の試験（D6）。
    let failing = "if test -f bad.txt; then echo 'FAILURE: Build failed with an exception (stale binding)' >&2; exit 1; fi";
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: failing.into(),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let mut adapter =
        ParallelWuAdapter::new(Duration::from_millis(5)).with_file("a", "bad.txt", "x");
    let wrappers = [
        format!("bash -c '{}'", failing.replace('\'', "'\\''")),
        format!("set -e; {failing}"),
        format!("export JAVA_HOME=/opt/jdk17; {failing}"),
        format!(
            "JAVA_HOME=/opt/jdk17 bash -c 'set -e; {}'",
            failing.replace('\'', "'\\''")
        ),
    ];
    for version in 1..=4u32 {
        for w in &wrappers {
            adapter = adapter.with_planner_output(replan_delta(
                version,
                serde_json::json!({"modify": [{"key": "b", "checks": [{"cmd": w, "expect_exit": 0}]}]}),
            ));
        }
    }
    let adapter = Arc::new(adapter);
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.max_replans = 20;
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let raw = store.events_for(task.id).unwrap();
    let counted = task_ops::plan_gate::counted_replans(&raw);
    assert!(counted <= 2, "counted_replans = {counted}");
    let events = events_of(&store, task.id);
    assert!(auto_replans(&events) <= 2, "{}", auto_replans(&events));
    // 人への質問（`WorkerQuestion`）で止まっている。
    assert!(
        events.iter().rev().find_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason.as_str()),
            _ => None,
        }) == Some("worker_question"),
        "{:#?}",
        &events[events.len().saturating_sub(6)..]
    );
    // 拒まれた done の check だけの計画（D7）は採用されない。
    assert!(
        events.iter().any(|e| format!("{e:?}")
            .contains("no progress: the replan after the failed integration integrate-build")),
        "the check-only plans were rejected as no progress"
    );
}

/// D4: planner が毎回、何も変えない葉を足す（D7 には当たらない）が、統合は同じ失敗を繰り返す。同じ統合 WU・同じ失敗の
/// 自動 replan が commit なしに 2 回続いたら、3 回目の失敗で `max_replans` に余りがあっても止めて人に聞く。
#[tokio::test]
async fn the_same_integration_failure_without_commits_asks_a_human_after_two_replans() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: "test ! -f bad.txt".into(),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let mut adapter =
        ParallelWuAdapter::new(Duration::from_millis(5)).with_file("a", "bad.txt", "x");
    for version in 1..=5u32 {
        adapter = adapter.with_planner_output(replan_delta(
            version,
            serde_json::json!({"add": [serde_json::to_value(v2_wu(&format!("noop-{version}"), "build", &[])).unwrap()]}),
        ));
    }
    let adapter = Arc::new(adapter);
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.max_replans = 20;
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    let raw = store.events_for(task.id).unwrap();
    assert_eq!(task_ops::plan_gate::counted_replans(&raw), 2);
    let events = events_of(&store, task.id);
    assert_eq!(auto_replans(&events), 2);
    let question = task_ops::derive::latest_question(&raw);
    assert!(
        question.starts_with(task_ops::plan_gate::REPLAN_STALLED_QUESTION_PREFIX),
        "{question}"
    );
    assert!(question.contains("integrate-build"), "{question}");
    assert_eq!(
        wu_status(&store, task.id, "integrate-build"),
        Some(task_core::WorkUnitStatus::Blocked)
    );
    // 回答は人の replan の依頼（上限に数えない）。
    task_ops::gate::answer(
        store.as_ref(),
        task.id,
        "binding を作り直した".to_string(),
        None,
    )
    .unwrap();
    assert!(task_ops::plan_gate::answered_replan_exhausted(
        &store.events_for(task.id).unwrap()
    ));
}

/// D6: 統合の検査の compile error（rustc の `error[E…]`）は repair WU（class `build`）になり、直れば task は終わる。
#[tokio::test]
async fn a_compile_error_in_the_integration_check_is_repaired() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![task_core::WorkUnitCheck {
        cmd: "if test -f bad.txt; then echo 'error[E0425]: cannot find function `f` in this scope' >&2; exit 101; fi".into(),
        expect_exit: 0,
        scope: false,
    }];
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![v2_wu("a", "build", &[]), b],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5))
            .with_file("a", "bad.txt", "x")
            .with_action("repair-build-1", |cwd| {
                std::fs::remove_file(cwd.join("bad.txt")).unwrap();
            }),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task.id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    let events = events_of(&store, task.id);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::RepairScheduled { key, class, origin, .. }
                if key == "repair-build-1"
                    && class == "build"
                    && *origin == task_core::execution::RepairOrigin::Integration
        )),
        "{events:#?}"
    );
    assert_eq!(auto_replans(&events), 0, "no replan");
}
