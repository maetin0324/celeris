use super::*;

/// ADR-0010 D5（P-21）: 供給側失敗は attempts を消費せず requeue され、cooldown 中は再 dispatch されず、明けたら done。
#[tokio::test]
async fn provider_failure_requeues_without_consuming_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f touched".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(FlakyProviderAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher_with_test_clock(store.clone(), adapter.clone(), 1, true);
    assert_eq!(d.tick().unwrap().dispatched, 1);
    await_worker_completion(&mut d, task.id).await;
    let second = d.tick().unwrap();
    assert_eq!(second.finished, 1);
    assert_eq!(second.dispatched, 0, "provider is cooling down");
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Ready, 0));

    if let Some(clock) = &d.test_now {
        *clock.lock().unwrap() += time::Duration::hours(2);
    }
    if let Some(clock) = &d.test_policy_clock {
        *clock.lock().unwrap() += Duration::from_secs(7200);
    }
    assert_eq!(d.tick().unwrap().dispatched, 1);
    finish_worker_and_review(&mut d, task.id).await;
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Done, 0));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e, Event::Transitioned { from: Status::Running, to: Status::Ready, reason } if reason == "requeue")));
    assert!(events.iter().any(|(_, e)| matches!(e, Event::WorkerFinished { outcome, .. } if outcome.starts_with("requeue: "))));
    // ADR-0013 D9: cooldown の開始が期限と種別つきで残る。
    let recorded_until = events.iter().find_map(|(_, e)| match e {
        Event::ProviderThrottled {
            provider,
            until,
            reason,
        } if provider == "p1" && reason.as_deref() == Some("throttled") => Some(*until),
        _ => None,
    });
    assert!(recorded_until.is_some(), "{events:?}");
    assert!(
        recorded_until.unwrap() > d.now_utc() - time::Duration::hours(2),
        "{events:?}"
    );
}

/// ADR-0010 D6（P-3）: attempts > 0 の ready タスクはバックオフが明けるまで dispatch されず、idle にもならない。
#[tokio::test]
async fn retry_backoff_delays_redispatch() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f never".into(),
            expect_exit: 0,
        },
        1,
    );
    task.attempts = 1;
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Error {
            message: "done".into(),
            retryable: false,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher_with_test_clock(store.clone(), adapter, 1, true);
    d.config.retry_backoff_base = Duration::from_secs(3600);
    d.config.retry_backoff_max = Duration::from_secs(3600);
    let frozen_now = d.now_utc();
    if let Some(clock) = &d.test_now {
        *clock.lock().unwrap() = frozen_now;
    }
    task.updated_at = frozen_now - time::Duration::minutes(59);
    store.insert(&task).unwrap();
    for _ in 0..5 {
        let r = d.tick().unwrap();
        assert_eq!(r.dispatched, 0);
        assert!(!r.idle, "a task waiting for its backoff is not idle");
    }
    if let Some(clock) = &d.test_now {
        *clock.lock().unwrap() =
            store.get(task.id).unwrap().unwrap().updated_at + time::Duration::hours(2);
    }
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Running);

    let (base, max) = (Duration::from_secs(10), Duration::from_secs(300));
    assert_eq!(retry_backoff(base, max, 0), Duration::ZERO);
    assert_eq!(retry_backoff(base, max, 1), Duration::from_secs(10));
    assert_eq!(retry_backoff(base, max, 3), Duration::from_secs(40));
    assert_eq!(retry_backoff(base, max, 40), max);
}

/// ADR-0010 D7（P-7）: ワーカーの heartbeat でリースが `idle_timeout + lease_grace` に更新される
/// （取得時の `max_wall_secs + grace` より短くなり、デーモン停止時に早く回収できる）。
#[tokio::test]
async fn heartbeat_renews_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f touched".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), Arc::new(HeartbeatAdapter), 1);
    d.config.lease_grace = Duration::from_millis(400);
    let before = OffsetDateTime::now_utc();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    let initial = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .unwrap()
        .expires_at;
    assert!(
        initial > before + time::Duration::seconds(25),
        "acquired with max_wall_secs + grace"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let renewed = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .expect("still running")
        .expires_at;
    assert!(renewed < initial, "renewed={renewed} initial={initial}");
    assert!(
        renewed > OffsetDateTime::now_utc() + time::Duration::seconds(4),
        "ttl = idle_timeout(5s) + grace"
    );
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

/// ADR-0070 D5（Phase 116。D6(c)）: lease が期限切れでも、dispatcher が抱えている run の
/// プロセスがまだ生きていれば（`process_group::group_alive`）、`reclaim_expired_leases` は
/// reclaim せず lease を延長する（DB busy で数 tick 更新できなかっただけ、という実際の事故を
/// 再現する）。「偽の子プロセス」はテスト自身の pid を run_id に登録するだけで作る（本物の
/// 子プロセスを起こす必要はない。`process_group::group_alive` は「その pid が生きているか」しか
/// 見ないため）。
#[tokio::test]
async fn a_lease_past_its_expiry_is_extended_instead_of_reclaimed_while_the_process_is_alive() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.budget.max_wall_secs = 0;
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), Arc::new(SlowNoHeartbeatAdapter), 1);
    d.config.lease_grace = Duration::from_millis(20);
    assert_eq!(d.tick().unwrap().dispatched, 1);

    let run_id = store
        .get(task.id)
        .unwrap()
        .unwrap()
        .lease
        .expect("running with a lease")
        .worker_run_id;
    assert!(
        !task_worker::process_group::group_alive(&run_id),
        "the fake in-process adapter never registers a real child"
    );
    // 「偽の子プロセス」を自分の pid で登録する（本物の子プロセスは要らない。生死しか見ないため）。
    let guard =
        task_worker::process_group::ProcessGroup::register(&run_id, Some(std::process::id()));
    assert!(task_worker::process_group::group_alive(&run_id));

    // lease が確実に切れるまで待つ（max_wall_secs(0) + lease_grace(20ms)）。
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        store
            .get(task.id)
            .unwrap()
            .unwrap()
            .lease
            .unwrap()
            .expires_at
            < OffsetDateTime::now_utc(),
        "the lease must actually be expired before reclaim runs"
    );

    let reclaimed = d.reclaim_expired_leases().unwrap();
    assert_eq!(
        reclaimed, 0,
        "a still-alive process is extended, not reclaimed"
    );
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Running, 0));
    assert!(
        t.lease.unwrap().expires_at > OffsetDateTime::now_utc(),
        "the lease was extended instead of reclaimed"
    );
    assert!(
        d.running_for_task(task.id) > 0,
        "the run entry is still tracked (not removed by reclaim)"
    );

    drop(guard);
    // 死んでいれば（登録を外せば `group_alive` が偽になる）今度こそ reclaim される。
    tokio::time::sleep(Duration::from_millis(30)).await;
    let reclaimed = d.reclaim_expired_leases().unwrap();
    assert_eq!(reclaimed, 1, "a dead process is reclaimed as usual");
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(
        (t.status, t.attempts),
        (Status::Ready, 0),
        "InfraRequeue does not consume attempts"
    );
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::Transitioned { from: Status::Running, to: Status::Ready, reason } if reason == "infra_requeue"
    )));
}

/// ADR-0011（P-38）: 同じ試行での連続 requeue が max_requeues に達したら通常の失敗として attempts を消費し、
/// 次の試行ではまた 0 から数える。最悪 (max_retries + 1) × (max_requeues + 1) 回で `failed` になる。
#[tokio::test]
async fn requeue_limit_turns_persistent_provider_failures_into_ordinary_failures() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        1,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(AlwaysThrottledAdapter {
        calls: AtomicUsize::new(0),
        review_only: false,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.max_requeues = 2;
    let report = run_until_idle(&mut d, 500).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Failed, 2));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 6);
    let one_attempt = [
        "dispatch",
        "requeue",
        "dispatch",
        "requeue",
        "dispatch",
        "worker_error",
    ];
    let expected: Vec<&str> = one_attempt
        .iter()
        .chain(one_attempt.iter())
        .copied()
        .collect();
    assert_eq!(transition_reasons(&store, task.id), expected);
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e, Event::WorkerFinished { outcome, .. } if outcome.contains("requeue limit (2) reached"))));

    // max_requeues = 0 なら最初の供給側失敗から attempts を消費する。
    let task0 = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task0).unwrap();
    d.config.max_requeues = 0;
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(
        transition_reasons(&store, task0.id),
        vec!["dispatch", "worker_error"]
    );
}

/// ADR-0070 D3（Phase 116。D6(b)）: ワーカー run 自身のインフラ都合の失敗（プロバイダが分類でき
/// ない `Err`）は `max_infra_retries` まで `attempts` を消費せず `Trigger::InfraRequeue` で
/// 再試行し、上限に達したときだけ `WorkerError{retryable:false}` で `"infra failure ×N"` として
/// 打ち切る（そのときだけ `attempts` が 1 進む）。`infra_backoff` はテストのために毎 tick 手で
/// 解除する（本番の 30秒/2分/5分のタイミングそのものは `derive::infra_backoff_delay` の単体
/// テストで確かめている）。
#[tokio::test]
async fn infra_failure_retry_limit_fails_the_task_without_consuming_attempts_until_then() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(AlwaysInfraFailingWorkerAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.max_infra_retries = 2;

    let mut report = TickReport::default();
    for _ in 0..200 {
        report = d.tick().unwrap();
        // インフラ再試行のバックオフ（30秒〜）を待たない。カウント・分類のテストなので、
        // タイミングそのものは触らない（別途 derive::infra_backoff_delay の単体テストが確かめる）。
        d.infra_backoff.clear();
        if report.idle {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(
        (t.status, t.attempts),
        (Status::Failed, 1),
        "2回のinfra_requeueはattemptsを消費せず、3回目で初めてfailed（attempts+1）"
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        transition_reasons(&store, task.id),
        vec![
            "dispatch",
            "infra_requeue",
            "dispatch",
            "infra_requeue",
            "dispatch",
            "worker_error"
        ]
    );
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerFinished { outcome, .. } if outcome.starts_with("infra failure ×3: ")
    )));
    let (class, reason) = task_ops::derive::classify_task_failure(&events);
    assert_eq!(class, task_ops::derive::FailureClass::Infra);
    assert!(reason.starts_with("infra failure ×3: "));
}

/// 監査 M-1: 供給側失敗が requeue の上限に達して通常の失敗（`Status::Failed`）になったら、bad_news を 1 件作る。
#[tokio::test]
async fn requeue_limit_reached_produces_one_bad_news_report() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org_for_reports(&store);
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.assignee = Some("coding-poc".into());
    task.project_id = Some(ProjectId::new());
    store.insert(&task).unwrap();
    let adapter = Arc::new(AlwaysThrottledAdapter {
        calls: AtomicUsize::new(0),
        review_only: false,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.max_requeues = 1;
    let report = run_until_idle(&mut d, 500).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Failed);
    let reports = store.report_list(&ReportFilter::default()).unwrap();
    let bad_news_at_source: Vec<_> = reports
        .iter()
        .filter(|r| r.kind == ReportKind::BadNews && r.node_id == "coding-poc")
        .collect();
    assert_eq!(
        bad_news_at_source.len(),
        1,
        "requeue 上限で failed になったら bad_news は 1 件だけ: {reports:?}"
    );
}

/// 監査 M-1: `max_retries` に余裕があるうちの retryable な失敗は `Status::Ready` に戻るだけで、
/// bad_news をリトライのたびに作らない(以前は `WorkerError` のたびに bad_news が飛んでいた)。
#[tokio::test]
async fn three_retryable_worker_errors_in_a_row_produce_no_bad_news_report() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org_for_reports(&store);
    // max_retries は大きめに取り、3 回失敗するまでの間は確実に `ready` に戻るだけにする。
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        50,
    );
    task.assignee = Some("coding-poc".into());
    task.project_id = Some(ProjectId::new());
    store.insert(&task).unwrap();
    let adapter = Arc::new(RetryableErrorAdapter);
    let mut d = dispatcher(store.clone(), adapter, 1);
    // 少なくとも 3 回、retryable な失敗を経験するまで tick する(タイミングにより 3 を超えても構わない。
    // ここで確かめたいのは「failed になる前に bad_news が作られないこと」)。
    for _ in 0..200 {
        d.tick().unwrap();
        let t = store.get(task.id).unwrap().unwrap();
        if t.attempts >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let t = store.get(task.id).unwrap().unwrap();
    assert!(
        t.attempts >= 3,
        "少なくとも 3 回は retryable な失敗を経たはず: attempts={}",
        t.attempts
    );
    assert_ne!(
        t.status,
        Status::Failed,
        "max_retries=50 なのでまだ failed にならない"
    );
    let reports = store.report_list(&ReportFilter::default()).unwrap();
    assert_eq!(
        reports
            .iter()
            .filter(|r| r.kind == ReportKind::BadNews)
            .count(),
        0,
        "途中の retryable な失敗では bad_news を作らない: {reports:?}"
    );
}

/// ADR-0074 §6 F2 (c): provider の `concurrency = 1` なら 1 本ずつ。
#[tokio::test]
async fn provider_concurrency_one_runs_units_one_at_a_time() {
    let (max_active, task, _units, _events, _task_dir, _repo, _root) =
        run_three_independent_units(3, 1).await;
    assert_eq!(task.status, Status::Done, "{task:?}");
    assert_eq!(max_active, 1);
}
