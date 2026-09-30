use super::*;

/// ADR-0018 D5（監査の「確認不能」の解消）: 並列度は「プロバイダ」と「クラスタ」の両方で守る。
/// クラスタの上限（1）が全体の上限（3）とプロバイダの上限（3）より小さいとき、そのクラスタのタスクは
/// 1 件ずつしか走らない。ローカル実行のタスクはクラスタの枠を消費しない。
#[tokio::test]
async fn cluster_and_provider_concurrency_are_both_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 同じクラスタを指すリモートのタスク 2 件と、ローカルのタスク 1 件。
    let mut remote_ids = Vec::new();
    for _ in 0..2 {
        let mut t = new_task(
            dir.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            0,
        );
        t.workspace = WorkspaceSpec::Remote {
            cluster: "slow".into(),
            path: dir.path().to_path_buf(),
            mode: None,
        };
        store.insert(&t).unwrap();
        remote_ids.push(t.id);
    }
    let local = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&local).unwrap();

    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_millis(400),
    });
    let mut d = dispatcher(store.clone(), adapter, 3);
    // Remote のタスクの写しは `workspace_root/<task_id>`（ADR-0018 D1）。テストでは実体のある場所にする。
    d.config.workspace_root = dir.path().to_path_buf();
    d.config.clusters.insert(
        "slow".into(),
        ClusterSpec {
            id: "slow".into(),
            // 実際に ssh はせず、多重接続の確認だけが通ればよいので localhost 向けの Host 名を使う。
            host: "celeris-localhost".into(),
            concurrency: 1,
            sync: SyncMode::None,
            delete_on_push: false,
            setup: vec![],
            env: vec![],
            rsync_excludes: vec![],
            worktree: Default::default(),
            auth: "manual".into(),
            forwards: vec![],
            work_dir: None,
            keepalive_secs: 0,
            liveness_probe_secs: 0,
            job_wait: Default::default(),
        },
    );
    if !control_master_alive_blocking(&["ssh".to_string()], "celeris-localhost") {
        eprintln!("skip: celeris-localhost への多重接続が無い");
        return;
    }

    let report = d.tick().unwrap();
    // クラスタの上限が 1 なので、リモートは 1 件だけ。ローカルの 1 件は別枠で走る。
    assert_eq!(report.dispatched, 2, "{report:?}");
    let running_remote = remote_ids
        .iter()
        .filter(|id| store.get(**id).unwrap().unwrap().status == Status::Running)
        .count();
    assert_eq!(running_remote, 1, "クラスタの上限 1 を超えない");
    assert_eq!(
        store.get(local.id).unwrap().unwrap().status,
        Status::Running,
        "ローカルはクラスタの枠を使わない"
    );

    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);
    for id in &remote_ids {
        assert_eq!(
            store.get(*id).unwrap().unwrap().status,
            Status::Done,
            "{:?}",
            store.events_for(*id).unwrap()
        );
    }
}

/// ADR-0018 実装メモ M1〜M3: 多重接続の無いクラスタは 1 tick に 1 回の `ssh -O check` で分かり、スナップショットの `clusters[]` に
/// `connected: false` と `cooldown_until` で現れる。タスクは ready のまま（attempts 不変）、`ClusterUnavailable` に host が入り、
/// 人待ちなので idle を止めない。ssh 先が無いことを使うので外部ネットワークには出ない。
#[tokio::test]
async fn offline_cluster_is_reported_in_the_snapshot_and_the_event_carries_the_host() {
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
    task.workspace = WorkspaceSpec::Remote {
        cluster: "offline".into(),
        path: PathBuf::from("/remote/project"),
        mode: None,
    };
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.clusters.insert(
        "offline".into(),
        ClusterSpec {
            id: "offline".into(),
            host: "celeris-no-such-host-for-tests".into(),
            concurrency: 1,
            sync: SyncMode::Rsync,
            delete_on_push: false,
            setup: vec![],
            env: vec![],
            rsync_excludes: vec![],
            worktree: Default::default(),
            auth: "manual".into(),
            forwards: vec![],
            work_dir: None,
            keepalive_secs: 0,
            liveness_probe_secs: 0,
            job_wait: Default::default(),
        },
    );
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-09-15T00:00:00Z".into(),
        tick_ms: 50,
        providers: vec![],
        provider_checks: Default::default(),
    });

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 0);
    assert!(
        report.idle,
        "a task waiting for a human login does not keep the daemon from going idle"
    );

    let snap = rx.borrow().clone().expect("snapshot published");
    assert_eq!(snap.clusters.len(), 1, "{snap:?}");
    let live = &snap.clusters[0];
    assert_eq!(
        (
            live.id.as_str(),
            live.host.as_str(),
            live.concurrency,
            live.in_use,
            live.connected
        ),
        ("offline", "celeris-no-such-host-for-tests", 1, 0, false)
    );
    let until = live.cooldown_until.clone().expect("cooldown_until");
    assert!(
        until > snap.last_tick_at,
        "cooldown ends after the tick: {until} vs {}",
        snap.last_tick_at
    );
    assert!(
        !snap.unroutable.contains(&task.id),
        "人待ちは経路なしではない（監査 4-1）: {snap:?}"
    );
    assert!(d.cluster_waiting.contains(&task.id));

    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Ready, 0));
    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ClusterUnavailable { cluster, host, .. } if cluster == "offline" && host == "celeris-no-such-host-for-tests"
        )),
        "{events:?}"
    );
    // 2 tick 目: cooldown 中は再度イベントを足さない（1 件のまま）。
    d.tick().unwrap();
    let again = store.events_for(task.id).unwrap();
    assert_eq!(
        again
            .iter()
            .filter(|(_, e)| matches!(e, Event::ClusterUnavailable { .. }))
            .count(),
        1,
        "{again:?}"
    );
}

/// ADR-0062 B1（Phase 107）: 担当に `cluster:<id>` が無い remote タスクは、設定に無いクラスタ
/// （`ClusterUnavailable` / `unroutable`）とは違い、`blocked` にして人に質問する
/// （「no such cluster in the config」という誤解を招く文言は出さない）。
#[tokio::test]
async fn assignee_without_the_cluster_tool_is_blocked_with_a_question() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let now = OffsetDateTime::now_utc();
    for (id, parent, tools) in [
        ("cos", None, vec![]),
        ("web-research", Some("cos"), vec!["tavily".to_string()]),
    ] {
        store
            .org_upsert(&OrgNode {
                profile: task_core::Profile {
                    tools,
                    ..Default::default()
                },
                id: id.into(),
                parent_id: parent.map(str::to_string),
                name: id.into(),
                kind: if parent.is_none() {
                    OrgKind::Secretary
                } else {
                    OrgKind::Department
                },
                genre: None,
                brief: String::new(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }

    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/remote/project"),
        mode: None,
    };
    task.assignee = Some("web-research".into());
    store.insert(&task).unwrap();

    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.clusters.insert(
        "sirius".into(),
        cluster_spec_with_auth("sirius", "sirius", "manual"),
    );

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 0);

    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Blocked, "{t:?}");
    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, Event::QuestionRaised { text, .. } if text.contains("cluster:sirius"))
        ),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::ClusterUnavailable { .. })),
        "設定の問題ではないので ClusterUnavailable は出さない: {events:?}"
    );

    // warn の dedupe: 2 tick 目でも同じ 1 件のまま増えない（そもそも blocked なので ready_tasks に
    // 出てこない。`warned_cluster_tool` の dedupe 自体は `task_may_use_cluster` の単体でも効く）。
    let events_again = store.events_for(task.id).unwrap();
    assert_eq!(events_again.len(), events.len());
}

/// ADR-0062 A（Phase 107）: celeris が保持していた master が明示的な切断を経ずに自分で終了した
/// ことを `set_cluster_master_watcher` で知らせると、次に `mark_cluster_unavailable` がそのクラスタを
/// 拾ったときに `Event::ClusterMasterExited`（stderr の末尾・exit code 付き）を 1 回だけ残す。
#[tokio::test]
async fn a_master_that_exited_on_its_own_is_reported_once_with_its_stderr_tail() {
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
    task.workspace = WorkspaceSpec::Remote {
        cluster: "sirius".into(),
        path: PathBuf::from("/remote/project"),
        mode: None,
    };
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.clusters.insert(
        "sirius".into(),
        cluster_spec_with_auth("sirius", "celeris-no-such-host-for-tests", "manual"),
    );
    d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| false));
    let watcher_calls = Arc::new(AtomicUsize::new(0));
    let calls = watcher_calls.clone();
    d.set_cluster_master_watcher(Arc::new(move || {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            vec![ClusterMasterExit {
                cluster: "sirius".into(),
                exit_code: Some(255),
                stderr_tail: "mux_client_request_session: read from master failed: Broken pipe"
                    .into(),
            }]
        } else {
            vec![]
        }
    }));

    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    let exits: Vec<_> = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::ClusterMasterExited { .. }))
        .collect();
    assert_eq!(exits.len(), 1, "{events:?}");
    let Event::ClusterMasterExited {
        cluster,
        exit_code,
        stderr_tail,
    } = &exits[0].1
    else {
        unreachable!()
    };
    assert_eq!(cluster, "sirius");
    assert_eq!(*exit_code, Some(255));
    assert!(stderr_tail.contains("Broken pipe"), "{stderr_tail}");
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ClusterUnavailable { reason, .. } if reason.contains("Broken pipe") && reason.contains("255")
        )),
        "reason に exit code と stderr が入る: {events:?}"
    );

    // 2 tick 目: watcher はもう新しい終了を返さないので、`ClusterMasterExited` は増えない
    // （cooldown 中でもあるので `mark_cluster_unavailable` 自体もう呼ばれない）。
    d.tick().unwrap();
    let events_again = store.events_for(task.id).unwrap();
    let exits_again = events_again
        .iter()
        .filter(|(_, e)| matches!(e, Event::ClusterMasterExited { .. }))
        .count();
    assert_eq!(exits_again, 1, "{events_again:?}");
}

/// ADR-0078 D3-3（ADR-0062 D2 を改める）: 実通信 probe が 1〜2 回失敗しても接続中のまま（`-O exit`
/// の片付けフックはもう無い）。3 回続けて初めて「lost（probe_failed）」になり、その後も失敗が続く間は
/// 数を増やさない。probe が 1 回成功すれば接続に戻る。
#[test]
fn probe_failures_keep_the_master_until_three_in_a_row() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = instant_done_dispatcher(store.clone());
    let mut spec = cluster_spec_with_auth("sirius", "sirius", "manual");
    spec.liveness_probe_secs = 30;
    d.config.clusters.insert("sirius".into(), spec);
    d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| true));
    let probe_ok = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe_calls = Arc::new(AtomicUsize::new(0));
    let (ok_hook, calls_hook) = (probe_ok.clone(), probe_calls.clone());
    d.set_cluster_command_probe(Arc::new(
        move |_ssh_command: &[String], _host: &str, _timeout: Duration| {
            calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ok_hook.load(std::sync::atomic::Ordering::SeqCst)
        },
    ));

    for round in 1..=2 {
        liveness_round(&mut d, "sirius");
        assert_eq!(
            d.cluster_connected.get("sirius"),
            Some(&true),
            "round {round}: 1〜2 回の失敗では接続中のまま"
        );
    }
    liveness_round(&mut d, "sirius");
    assert_eq!(d.cluster_connected.get("sirius"), Some(&false));
    assert!(
        d.cluster_disconnect_info
            .get("sirius")
            .is_some_and(|info| info.detail.contains("3 times in a row")),
        "{:?}",
        d.cluster_disconnect_info
    );
    liveness_round(&mut d, "sirius");
    assert_eq!(d.cluster_connected.get("sirius"), Some(&false));
    assert_eq!(probe_calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    let log = connection_log(&store);
    let kinds: Vec<(&str, Option<&str>)> = log
        .iter()
        .map(|r| (r.kind.as_str(), r.cause.as_deref().or(r.method.as_deref())))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("connected", Some("borrowed")),
            ("lost", Some("probe_failed"))
        ]
    );

    probe_ok.store(true, std::sync::atomic::Ordering::SeqCst);
    liveness_round(&mut d, "sirius");
    // この巡の頭ではまだ `probe_dead`（結果を拾う前）なので false のまま。結果を拾った次の巡で戻る。
    liveness_round(&mut d, "sirius");
    assert_eq!(d.cluster_connected.get("sirius"), Some(&true));
    let stats = &d.cluster_conn["sirius"].stats;
    assert_eq!(stats.losses, 1);
    assert_eq!(stats.losses_by_cause.get("probe_failed"), Some(&1));
    assert_eq!(stats.connects_borrowed, 2);
}

/// ADR-0078 D3-4: 実通信 probe は tick を塞がない（probe が眠っていても `refresh_cluster_liveness`
/// はすぐ戻り、結果は後の tick で拾う）。
#[test]
fn a_slow_probe_does_not_block_the_liveness_refresh() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = instant_done_dispatcher(store);
    let mut spec = cluster_spec_with_auth("sirius", "sirius", "manual");
    spec.liveness_probe_secs = 30;
    d.config.clusters.insert("sirius".into(), spec);
    d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| true));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release_hook = release.clone();
    d.set_cluster_command_probe(Arc::new(
        move |_ssh_command: &[String], _host: &str, _timeout: Duration| {
            // 放されるまで（最大 10 秒）眠る = timeout 寸前まで返らない遅い probe。
            let deadline = Instant::now() + Duration::from_secs(10);
            while !release_hook.load(std::sync::atomic::Ordering::SeqCst)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            true
        },
    ));
    let started = Instant::now();
    d.refresh_cluster_liveness();
    d.last_cluster_liveness = None;
    d.refresh_cluster_liveness();
    let elapsed = started.elapsed();
    // probe はまだ走っている（結果を拾っていない）ことを先に確かめてから時間を判定する。
    assert!(d.cluster_conn["sirius"].probe_inflight.is_some());
    assert!(
        elapsed < Duration::from_secs(5),
        "refresh waited for the probe: {elapsed:?}"
    );
    assert_eq!(d.cluster_connected.get("sirius"), Some(&true));
    release.store(true, std::sync::atomic::Ordering::SeqCst);
    liveness_round(&mut d, "sirius");
    assert!(d.cluster_conn["sirius"].probe_inflight.is_none());
}

/// ADR-0078 D3-1 / D4: totp のクラスタでは、鍵認証の再接続は切断 1 回につき 1 回だけ。`-O check` が
/// false のまま 100 tick 回しても connector は 1 回、「ログインが要る」の通知も 1 件。人が繋ぎ直して
/// から再び切れたら、そこでもう 1 回だけ試す。
#[test]
fn totp_key_auth_reconnect_is_tried_once_per_outage() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = instant_done_dispatcher(store.clone());
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19001",
            "bnode150:19001",
        ),
    );
    let master_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let up_hook = master_up.clone();
    d.set_cluster_liveness_probe(Arc::new(move |_ssh_command: &[String], _host: &str| {
        up_hook.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let connector_calls = Arc::new(AtomicUsize::new(0));
    let calls_hook = connector_calls.clone();
    d.set_cluster_connector(Arc::new(move |_id: &str, _host: &str| {
        calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("Permission denied (keyboard-interactive)".to_string())
    }));
    let run_ticks = |d: &mut Dispatcher, n: usize| {
        for _ in 0..n {
            d.last_cluster_liveness = None;
            d.last_cluster_tunnel_refresh = None;
            d.refresh_cluster_liveness();
            d.refresh_cluster_tunnels();
        }
    };
    let login_needed_events = |d: &mut Dispatcher| {
        d.take_tunnel_events()
            .iter()
            .filter(|e| e.kind == TunnelEventKind::LoginNeeded)
            .count()
    };

    run_ticks(&mut d, 100);
    assert_eq!(connector_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(login_needed_events(&mut d), 1);
    assert_eq!(d.clusters_needing_login(), vec!["pegasus".to_string()]);

    // 人が GUI で TOTP を入れて繋いだ。
    d.set_cluster_connect_pending("pegasus", true);
    d.set_cluster_connect_pending("pegasus", false);
    master_up.store(true, std::sync::atomic::Ordering::SeqCst);
    run_ticks(&mut d, 3);
    assert!(d.clusters_needing_login().is_empty());
    // 再び切れた: 鍵認証はもう 1 回だけ、通知ももう 1 件だけ。
    master_up.store(false, std::sync::atomic::Ordering::SeqCst);
    run_ticks(&mut d, 100);
    assert_eq!(connector_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(login_needed_events(&mut d), 1);

    let log = connection_log(&store);
    let summary: Vec<(&str, Option<&str>)> = log
        .iter()
        .map(|r| (r.kind.as_str(), r.method.as_deref().or(r.cause.as_deref())))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("key_auth_attempt", Some("failed")),
            ("connected", Some("totp")),
            ("lost", Some("check_failed")),
            ("key_auth_attempt", Some("failed")),
        ]
    );
    let stats = &d.cluster_conn["pegasus"].stats;
    assert_eq!(
        (stats.connects_totp, stats.losses, stats.key_auth_attempts),
        (1, 1, 2)
    );
    assert_eq!(stats.last_lost_cause.as_deref(), Some("check_failed"));
}

/// ADR-0078 D3-2: publickey のクラスタの鍵認証の再接続は 6 秒から倍々で 5 分に頭打ち。間隔の間は
/// 呼ばない。3 回続けて失敗したら報告済みの印が立つ（切断 1 回につき 1 回）。
#[test]
fn publickey_key_auth_reconnect_backs_off_up_to_five_minutes() {
    let mut secs = Vec::new();
    let mut current = Duration::ZERO;
    for _ in 0..8 {
        current = next_key_auth_backoff(current);
        secs.push(current.as_secs());
    }
    assert_eq!(secs, vec![6, 12, 24, 48, 96, 192, 300, 300]);

    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = instant_done_dispatcher(store);
    let spec = cluster_spec_with_forward(
        "fern03",
        "fern03",
        "publickey",
        "127.0.0.1:19002",
        "localhost:8000",
    );
    d.config.clusters.insert("fern03".into(), spec.clone());
    d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| false));
    let connector_calls = Arc::new(AtomicUsize::new(0));
    let calls_hook = connector_calls.clone();
    d.set_cluster_connector(Arc::new(move |_id: &str, _host: &str| {
        calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("Connection timed out".to_string())
    }));
    for _ in 0..50 {
        assert!(!d.ensure_cluster_master_for_tunnel(&spec));
    }
    assert_eq!(
        connector_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "バックオフの間は呼ばない"
    );
    assert_eq!(
        d.cluster_conn["fern03"].key_auth_backoff,
        Duration::from_secs(6)
    );
    // 期限が来たことにして、さらに 2 回失敗させる。
    for _ in 0..2 {
        d.cluster_conn.get_mut("fern03").unwrap().next_key_auth_at = None;
        assert!(!d.ensure_cluster_master_for_tunnel(&spec));
    }
    let state = &d.cluster_conn["fern03"];
    assert_eq!(state.key_auth_backoff, Duration::from_secs(24));
    assert_eq!(state.key_auth_failures, 3);
    assert!(state.unavailable_reported);
    assert!(
        d.clusters_needing_login().is_empty(),
        "publickey は TOTP を求めない"
    );
}

/// ADR-0078 D4/D5: true→false の遷移 1 回につき lost の記録 1 件・通知 1 件。false のままの tick では
/// 増えない。totp で forward が無い（鍵認証の出番が無い）クラスタは、遷移の時点で「ログインが要る」を
/// 1 件出し、本文に切れた時刻と理由を書く。
#[test]
fn a_lost_master_is_recorded_and_notified_once_per_transition() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = instant_done_dispatcher(store.clone());
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_auth("pegasus", "pegasus", "totp"),
    );
    let master_up = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let up_hook = master_up.clone();
    d.set_cluster_liveness_probe(Arc::new(move |_ssh_command: &[String], _host: &str| {
        up_hook.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let tick_liveness = |d: &mut Dispatcher| {
        d.last_cluster_liveness = None;
        d.refresh_cluster_liveness();
    };
    tick_liveness(&mut d);
    tick_liveness(&mut d);
    master_up.store(false, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..10 {
        tick_liveness(&mut d);
    }
    let log = connection_log(&store);
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!(log[0].method.as_deref(), Some("borrowed"));
    assert_eq!(log[1].kind, "lost");
    assert_eq!(log[1].cause.as_deref(), Some("check_failed"));
    assert!(log[1].uptime_secs.is_some());
    let login_needed: Vec<_> = d
        .take_tunnel_events()
        .into_iter()
        .filter(|e| e.kind == TunnelEventKind::LoginNeeded)
        .collect();
    assert_eq!(login_needed.len(), 1);
    let detail = d.cluster_conn["pegasus"]
        .last_lost_detail
        .clone()
        .unwrap_or_default();
    assert!(
        detail.contains("切れた時刻") && detail.contains("check_failed"),
        "{detail}"
    );
    // master の終了（celeris が抱えた子）でも同じ経路で数える。
    master_up.store(true, std::sync::atomic::Ordering::SeqCst);
    tick_liveness(&mut d);
    d.set_cluster_master_watcher(Arc::new(|| {
        vec![ClusterMasterExit {
            cluster: "pegasus".into(),
            exit_code: Some(255),
            stderr_tail: "Broken pipe".into(),
        }]
    }));
    d.refresh_cluster_master_exits();
    let stats = &d.cluster_conn["pegasus"].stats;
    assert_eq!(stats.losses, 2);
    assert_eq!(stats.losses_by_cause.get("master_exited"), Some(&1));
    assert_eq!(stats.connects_borrowed, 2);
}

/// ADR-0078 D1/D5: daemon の再起動を模す。前の dispatcher が繋いでいた master は（`ControlPersist=yes`
/// で）残っているので、作り直した dispatcher は最初の `-O check` でそれを見つけ `borrowed` として
/// 数え、鍵認証も TOTP も求めない。直近 24 時間の回数（DB から数える）は再起動をまたいで同じ。
#[test]
fn a_restarted_dispatcher_borrows_the_surviving_master_and_keeps_the_counts() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let connector_calls = Arc::new(AtomicUsize::new(0));
    let make = |store: Arc<dyn TaskStore>, calls: Arc<AtomicUsize>| {
        let mut d = instant_done_dispatcher(store);
        d.config.clusters.insert(
            "pegasus".into(),
            cluster_spec_with_forward(
                "pegasus",
                "pegasus",
                "totp",
                "127.0.0.1:19001",
                "bnode150:19001",
            ),
        );
        // master は生き続けている（ControlPersist=yes。celeris の停止と無関係）。
        d.set_cluster_liveness_probe(Arc::new(|_ssh_command: &[String], _host: &str| true));
        d.set_cluster_connector(Arc::new(move |_id: &str, _host: &str| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("must not be called".to_string())
        }));
        d.set_tunnel_listener_probe(Arc::new(|_listen: &str| true));
        d.set_tunnel_probe(Arc::new(|_listen: &str| Ok(())));
        d
    };
    let mut before = make(store.clone(), connector_calls.clone());
    before.set_cluster_connect_pending("pegasus", true);
    before.set_cluster_connect_pending("pegasus", false);
    before.refresh_cluster_liveness();
    before.refresh_cluster_tunnels();
    let counts_before =
        task_core::ClusterConnectionStats::from_records(&connection_log(&store), "pegasus");
    assert_eq!(counts_before.connects_totp, 1);
    drop(before);

    let mut after = make(store.clone(), connector_calls.clone());
    for _ in 0..5 {
        after.last_cluster_liveness = None;
        after.last_cluster_tunnel_refresh = None;
        after.refresh_cluster_liveness();
        after.refresh_cluster_tunnels();
    }
    assert_eq!(after.cluster_connected.get("pegasus"), Some(&true));
    assert!(after.clusters_needing_login().is_empty());
    assert_eq!(connector_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let counts_after =
        task_core::ClusterConnectionStats::from_records(&connection_log(&store), "pegasus");
    assert_eq!(counts_after.connects_totp, 1, "TOTP は増えない");
    assert_eq!(counts_after.connects_borrowed, 1);
    assert_eq!(counts_after.losses, 0);
    let since_start = &after.cluster_conn["pegasus"].stats;
    assert_eq!(
        (since_start.connects_totp, since_start.connects_borrowed),
        (0, 1)
    );
}

/// ADR-0059 D1（Phase 99）: `mode: Shared` は、クラスタの `sync` 設定に関わらず `SyncMode::None`
/// を強制する。`mode` 省略（既定の `Worktree`）は従来どおりクラスタの `sync` に従う。
#[test]
fn ssh_settings_shared_mode_forces_sync_none_regardless_of_cluster_sync() {
    let mut spec = cluster_spec_with_auth("pegasus", "pegasus", "manual");
    spec.sync = SyncMode::Worktree;
    let task_id = TaskId::new();

    let worktree_settings = spec.ssh_settings(
        std::path::Path::new("/work/x"),
        task_id,
        WorkspaceMode::Worktree,
    );
    assert_eq!(worktree_settings.sync, SyncMode::Worktree);

    let shared_settings = spec.ssh_settings(
        std::path::Path::new("/work/x"),
        task_id,
        WorkspaceMode::Shared,
    );
    assert_eq!(shared_settings.sync, SyncMode::None);

    // rsync クラスタでも同じ: 省略（既定）は従来どおり、shared は None を強制する。
    spec.sync = SyncMode::Rsync;
    assert_eq!(
        spec.ssh_settings(
            std::path::Path::new("/work/x"),
            task_id,
            WorkspaceMode::Worktree
        )
        .sync,
        SyncMode::Rsync
    );
    assert_eq!(
        spec.ssh_settings(
            std::path::Path::new("/work/x"),
            task_id,
            WorkspaceMode::Shared
        )
        .sync,
        SyncMode::None
    );
}

/// ADR-0059 D6（Phase 99）: `cluster_of` の実効 `work_dir` は DB の上書き（`cluster_settings`）
/// > 設定ファイルの `[[clusters]] work_dir` > 無し、の順。相対パスはそこからの相対に解決する。
#[test]
fn cluster_of_resolves_relative_paths_against_the_db_override_then_the_config_work_dir() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let mut spec = cluster_spec_with_auth("pegasus", "pegasus", "manual");
    spec.work_dir = Some(PathBuf::from("/work/NBB/config-default"));
    d.config.clusters.insert("pegasus".into(), spec);

    let mut task = new_task(
        std::path::Path::new("/unused"),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("benchfs"),
        mode: None,
    };

    // DB の上書きがまだ無ければ設定ファイルの `work_dir` を使う。
    let (_, resolved, mode) = d.cluster_of(&task).expect("cluster configured");
    assert_eq!(resolved, PathBuf::from("/work/NBB/config-default/benchfs"));
    assert_eq!(mode, WorkspaceMode::Worktree);

    // DB の上書きがあればそちらが勝つ。
    store
        .cluster_settings_set(
            "pegasus",
            Some("/work/NBB/db-override"),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    let (_, resolved, _) = d.cluster_of(&task).expect("cluster configured");
    assert_eq!(resolved, PathBuf::from("/work/NBB/db-override/benchfs"));

    // 絶対パス・`~` はどちらの `work_dir` からも独立にそのまま使う。
    task.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/scratch/x"),
        mode: Some(WorkspaceMode::Shared),
    };
    let (_, resolved, mode) = d.cluster_of(&task).expect("cluster configured");
    assert_eq!(resolved, PathBuf::from("/scratch/x"));
    assert_eq!(mode, WorkspaceMode::Shared);
}

/// ADR-0059 D6: `work_dir` がどこにも無ければ、相対・空の `path` は解決できず、`cluster_of` は
/// 受け取った `path` をそのまま返す（`remote_dir_is_resolved` で「解決できなかった」と判定できる形）。
#[test]
fn cluster_of_leaves_the_path_unresolved_when_there_is_no_work_dir_anywhere() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_auth("pegasus", "pegasus", "manual"),
    );
    let mut task = new_task(
        std::path::Path::new("/unused"),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.workspace = WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::new(),
        mode: None,
    };
    let (_, resolved, _) = d.cluster_of(&task).expect("cluster configured");
    assert_eq!(resolved, PathBuf::new());
    assert!(!task_worker::remote_dir_is_resolved(&resolved));
}

// ---- ADR-0053 D3: `refresh_cluster_tunnels` の状態機械（偽の ssh/probe で完全に決定的） ----

/// down → 鍵認証 ok → forward up: master が死んでいても `cluster_connector` が繋ぎ直し、
/// 続けて forward が(再)確立されて `Up` イベントが立つ。
#[tokio::test]
async fn tunnel_down_then_key_auth_ok_brings_the_forward_up() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19001",
            "bnode150:19001",
        ),
    );
    // master は死んでいる（cluster_connected に何も入っていない）。
    d.set_cluster_connector(Arc::new(|_id: &str, _host: &str| Ok(())));
    let ensure_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let forward_present = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ensure_calls_hook = ensure_calls.clone();
    let forward_present_hook = forward_present.clone();
    d.set_tunnel_forward_ensurer(Arc::new(
        move |_host: &str, _listen: &str, _target: &str| {
            ensure_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            forward_present_hook.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
    ));
    // Phase 85: listener と target の健康は別のフック。この試験では両方を同じフラグに束ねる
    // （ensure が listener を立て、立った listener はそのまま target も健全とみなす）。
    let forward_present_listener = forward_present.clone();
    d.set_tunnel_listener_probe(Arc::new(move |_listen: &str| {
        forward_present_listener.load(std::sync::atomic::Ordering::SeqCst)
    }));
    let forward_present_probe = forward_present.clone();
    d.set_tunnel_probe(Arc::new(move |_listen: &str| {
        if forward_present_probe.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err("GET /models failed: could not connect".to_string())
        }
    }));

    d.refresh_cluster_tunnels();

    assert_eq!(d.cluster_connected.get("pegasus"), Some(&true));
    assert!(d.clusters_needing_login().is_empty());
    assert_eq!(ensure_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(d.tunnel_reachable("pegasus", "127.0.0.1:19001"));
    let events = d.take_tunnel_events();
    assert!(
        events.iter().any(|e| e.cluster == "pegasus"
            && e.listen == "127.0.0.1:19001"
            && e.kind == TunnelEventKind::Up),
        "{events:?}"
    );
}

/// down → 鍵認証も失敗 → login_needed が立つ。同じ outage の間は 2 回目の tick で再度立てない
/// （イベントも報告も 1 回だけ）。
#[tokio::test]
async fn tunnel_down_and_key_auth_fails_marks_login_needed_once() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19002",
            "bnode150:19002",
        ),
    );
    d.set_cluster_connector(Arc::new(|_id: &str, _host: &str| {
        Err("permission denied (keyboard-interactive)".to_string())
    }));

    d.refresh_cluster_tunnels();
    assert_eq!(d.clusters_needing_login(), vec!["pegasus".to_string()]);
    let first_events = d.take_tunnel_events();
    assert_eq!(
        first_events
            .iter()
            .filter(|e| e.kind == TunnelEventKind::LoginNeeded)
            .count(),
        1,
        "{first_events:?}"
    );

    // 2 回目の tick（間引きを避けるため、間隔を過ぎさせる）。まだ同じ outage の間なので、
    // login_needed の再検出（新しいイベント）は起きない。
    d.last_cluster_tunnel_refresh =
        Some(Instant::now() - CLUSTER_LIVENESS_INTERVAL - Duration::from_millis(1));
    d.refresh_cluster_tunnels();
    assert_eq!(d.clusters_needing_login(), vec!["pegasus".to_string()]);
    let second_events = d.take_tunnel_events();
    assert!(
        second_events
            .iter()
            .all(|e| e.kind != TunnelEventKind::LoginNeeded),
        "login_needed must not fire twice for the same outage: {second_events:?}"
    );
}

/// listener が消えている（`-O forward` の手元の待ち受けが無い）が master は生きている:
/// `tunnel_forward_ensurer` で張り直しを試みる（呼ばれたことを確認する。「listener missing →
/// re-add」。Phase 85: listener の有無だけで re-add を決める。target の健康は別）。
#[tokio::test]
async fn tunnel_listener_absent_is_re_added() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19003",
            "bnode150:19003",
        ),
    );
    d.cluster_connected.insert("pegasus".into(), true);
    let ensure_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener_present = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ensure_calls_hook = ensure_calls.clone();
    let listener_present_hook = listener_present.clone();
    d.set_tunnel_forward_ensurer(Arc::new(
        move |_host: &str, _listen: &str, _target: &str| {
            ensure_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            listener_present_hook.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
    ));
    let listener_present_probe = listener_present.clone();
    d.set_tunnel_listener_probe(Arc::new(move |_listen: &str| {
        listener_present_probe.load(std::sync::atomic::Ordering::SeqCst)
    }));
    // listener が有る間は target も健全（この試験は listener の有無だけを見たいので単純化する）。
    d.set_tunnel_probe(Arc::new(|_listen: &str| Ok(())));

    // 1 回目: 最初から listener が有る。ensure は呼ばれない。
    d.refresh_cluster_tunnels();
    assert_eq!(ensure_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(d.tunnel_reachable("pegasus", "127.0.0.1:19003"));

    // listener が消える（誰かが master 側を再起動した、等）。
    listener_present.store(false, std::sync::atomic::Ordering::SeqCst);
    d.last_cluster_tunnel_refresh =
        Some(Instant::now() - CLUSTER_LIVENESS_INTERVAL - Duration::from_millis(1));
    d.refresh_cluster_tunnels();
    assert_eq!(
        ensure_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a missing listener must be re-added"
    );
    assert!(d.tunnel_reachable("pegasus", "127.0.0.1:19003"));
}

/// ADR-0053 Phase 85 の本旨: listener は有る（`-O forward` は張れている）が target
/// （先方の vLLM 等）の `/v1/models` probe が失敗する。**re-add はしない**（listener が有るので
/// `tunnel_forward_ensurer` を呼ぶのは無意味）。状態は `TargetUnreachable` として観測され、
/// `up = false` だが `listener = true` / `target_healthy = false` / `last_error` が立つ。
///
/// 本番観測（2026-09-21）: `-O forward` は張れているのに bnode150 が応答せず、旧実装（listener と
/// target を区別しない）は「届かない＝forward が無い」と誤認し、毎 tick `-O forward` を打ち直して
/// いた（5 分で 37 回）。この試験はその回帰を防ぐ。
#[tokio::test]
async fn tunnel_listener_present_target_unhealthy_does_not_re_add() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19004",
            "bnode150:19004",
        ),
    );
    d.cluster_connected.insert("pegasus".into(), true);
    d.set_tunnel_listener_probe(Arc::new(|_listen: &str| true));
    let ensure_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ensure_calls_hook = ensure_calls.clone();
    d.set_tunnel_forward_ensurer(Arc::new(
        move |_host: &str, _listen: &str, _target: &str| {
            ensure_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
    ));
    d.set_tunnel_probe(Arc::new(|_listen: &str| {
        Err("GET /models timed out after 2s".to_string())
    }));

    d.refresh_cluster_tunnels();

    assert_eq!(
        ensure_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a present listener must not be re-added just because the target is unhealthy"
    );
    assert!(!d.tunnel_reachable("pegasus", "127.0.0.1:19004"));
    assert!(d.tunnel_listener_present("pegasus", "127.0.0.1:19004"));
    assert!(!d.tunnel_target_healthy("pegasus", "127.0.0.1:19004"));
    assert_eq!(
        d.tunnel_last_error("pegasus", "127.0.0.1:19004"),
        Some(
            "target bnode150:19004 (as seen from pegasus) did not answer /v1/models through \
             the forward: GET /models timed out after 2s"
                .to_string()
        )
    );
    let events = d.take_tunnel_events();
    assert!(
        events.iter().any(|e| e.cluster == "pegasus"
            && e.listen == "127.0.0.1:19004"
            && e.kind == TunnelEventKind::TargetUnreachable),
        "{events:?}"
    );
}

/// ADR-0066 D3（Phase 110b）: この forward を初めて観測するときだけ、`refresh_cluster_tunnels` は
/// 同期に 1 回 probe して種を蒔く（listener が有れば必ず 1 回は確かめる、という Phase 85 までの
/// 前提を保つ）。**2 回目以降はもう同期に probe しない** — 専用スレッドが裏で行うので、probe 自体が
/// 遅くても（本番観測: `/v1/models` が 2 秒級）tick は待たない。これが Phase 110b の本旨（`slow tick
/// phases`〈`tunnel_ms`≈2000〉が 1 時間に 118 件出ていた不具合の直し）。
#[tokio::test]
async fn target_probe_seeds_synchronously_once_then_the_tick_stops_waiting_on_it() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19006",
            "bnode150:19006",
        ),
    );
    d.cluster_connected.insert("pegasus".into(), true);
    d.set_tunnel_listener_probe(Arc::new(|_listen: &str| true));
    d.set_tunnel_forward_ensurer(Arc::new(|_host: &str, _listen: &str, _target: &str| Ok(())));
    let probe_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let probe_calls_hook = probe_calls.clone();
    d.set_tunnel_probe(Arc::new(move |_listen: &str| {
        probe_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // 本番観測（無応答の Qwen 先方への `/v1/models`）の遅さを模す。
        std::thread::sleep(Duration::from_millis(150));
        Ok(())
    }));

    let first_started = Instant::now();
    d.refresh_cluster_tunnels();
    assert!(
        first_started.elapsed() >= Duration::from_millis(150),
        "the first observation seeds synchronously"
    );
    assert_eq!(probe_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(d.tunnel_reachable("pegasus", "127.0.0.1:19006"));

    // 次の tick（`CLUSTER_LIVENESS_INTERVAL` を過ぎさせる）。probe はもう同期では呼ばれないので、
    // probe が遅くても tick は速い。
    d.last_cluster_tunnel_refresh =
        Some(Instant::now() - CLUSTER_LIVENESS_INTERVAL - Duration::from_millis(1));
    let second_started = Instant::now();
    d.refresh_cluster_tunnels();
    assert!(
        second_started.elapsed() < Duration::from_millis(100),
        "the tick must not wait on the slow target probe once it has been seeded: {:?}",
        second_started.elapsed()
    );
}

/// ADR-0066 D3: 失敗が続くと probe の間隔が伸び、1 回成功すれば最短間隔に戻る（純粋関数。
/// スレッドも時刻も使わない）。
#[test]
fn next_probe_interval_secs_grows_on_failure_and_resets_on_success() {
    assert_eq!(next_probe_interval_secs(30, 30, false), 60);
    assert_eq!(next_probe_interval_secs(60, 30, false), 120);
    assert_eq!(next_probe_interval_secs(120, 30, false), 240);
    assert_eq!(next_probe_interval_secs(240, 30, false), 480);
    assert_eq!(
        next_probe_interval_secs(480, 30, false),
        600,
        "capped at the max"
    );
    assert_eq!(
        next_probe_interval_secs(600, 30, false),
        600,
        "stays at the max"
    );
    assert_eq!(
        next_probe_interval_secs(600, 30, true),
        30,
        "one success resets to the minimum"
    );
    // 設定変更で `probe_interval_secs`（min）が現在値より大きくなっても、min を下限にする。
    assert_eq!(next_probe_interval_secs(10, 30, false), 60);
}

/// ADR-0053 Phase 85: 同じフェーズ（ここでは `TargetUnreachable`）が続く間、状態遷移のイベント
/// （Console に出る 1 行）は 1 回しか積まない（毎回 probe しても、フェーズが変わらなければスパムしない）。
#[tokio::test]
async fn tunnel_target_unreachable_emits_the_event_once_across_many_ticks() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19007",
            "bnode150:19007",
        ),
    );
    d.cluster_connected.insert("pegasus".into(), true);
    d.set_tunnel_listener_probe(Arc::new(|_listen: &str| true));
    d.set_tunnel_forward_ensurer(Arc::new(|_host: &str, _listen: &str, _target: &str| Ok(())));
    d.set_tunnel_probe(Arc::new(|_listen: &str| {
        Err("GET /models timed out after 2s".to_string())
    }));

    for _ in 0..5 {
        // `CLUSTER_LIVENESS_INTERVAL` の間引きを毎回越えさせる。ADR-0066 D3: target probe 自体は
        // 最初の 1 回だけ同期で種を蒔き、以後は `tunnel_probe_state` の既存の観測を読むだけ
        // （専用スレッドがこのテストの短い実行時間の中で probe をやり直すことは通常無い）。
        // それでもイベントが 1 回しか積まれないことを確かめる。
        d.last_cluster_tunnel_refresh =
            Some(Instant::now() - CLUSTER_LIVENESS_INTERVAL - Duration::from_millis(1));
        d.refresh_cluster_tunnels();
    }
    let events = d.take_tunnel_events();
    let target_unreachable: Vec<_> = events
        .iter()
        .filter(|e| e.kind == TunnelEventKind::TargetUnreachable)
        .collect();
    assert_eq!(
        target_unreachable.len(),
        1,
        "a steady TargetUnreachable state must log the transition once, not every tick: {events:?}"
    );
}

/// ADR-0041 D5 / Phase 66c（実機 2026-09-21）: `--mode verify` の celeris（`set_eligible_tasks` で
/// 「`smoke` の煙試験だけ」に絞られたインスタンス）は「migrations、API と煙試験だけ。他の裏方の仕事は
/// 無い」はずなのに、`[[clusters.forwards]]` を持つクラスタが設定されていると
/// `refresh_cluster_tunnels` が forward の(再)確立と `cluster_login_needed` の報告書き込みを行い、
/// staging と本番のコピーで `reports` の件数がずれて `verify.sh` check 2 が落ちた
/// （`reports(snapshot=135 staging=136)`）。`set_eligible_tasks` を呼んだ（＝ verify）インスタンスは
/// `refresh_cluster_tunnels` を丸ごと素通りし、フック（`cluster_connector` / `tunnel_forward_ensurer`
/// / `tunnel_probe`）を一切呼ばず、報告も書かないことを確かめる。
#[tokio::test]
async fn verify_mode_does_not_refresh_cluster_tunnels_or_write_a_report() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_forward(
            "pegasus",
            "pegasus",
            "totp",
            "127.0.0.1:19005",
            "bnode150:19005",
        ),
    );
    // master は死んでいる（`cluster_connected` に何も入っていない）ので、verify でなければ
    // `cluster_connector` → 失敗 → `cluster_login_needed` の報告書き込みに至るはずの状態。
    let connector_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connector_calls_hook = connector_calls.clone();
    d.set_cluster_connector(Arc::new(move |_id: &str, _host: &str| {
        connector_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("permission denied (keyboard-interactive)".to_string())
    }));
    let ensure_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ensure_calls_hook = ensure_calls.clone();
    d.set_tunnel_forward_ensurer(Arc::new(
        move |_host: &str, _listen: &str, _target: &str| {
            ensure_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        },
    ));
    let probe_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let probe_calls_hook = probe_calls.clone();
    d.set_tunnel_probe(Arc::new(move |_listen: &str| {
        probe_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }));
    let listener_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let listener_calls_hook = listener_calls.clone();
    d.set_tunnel_listener_probe(Arc::new(move |_listen: &str| {
        listener_calls_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        true
    }));

    let reports_before = store
        .report_list(&task_core::ReportFilter::default())
        .unwrap()
        .len();

    // ADR-0041 D5: `--mode verify` の目印（celeris は verify のときだけこれを呼ぶ）。
    d.set_eligible_tasks(Arc::new(|task: &Task| {
        task.genre.as_deref() == Some("smoke")
    }));

    d.refresh_cluster_liveness();
    d.refresh_cluster_tunnels();

    assert_eq!(
        connector_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "verify mode must not touch the cluster connector"
    );
    assert_eq!(
        ensure_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "verify mode must not (re-)establish forwards"
    );
    assert_eq!(
        probe_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "verify mode must not probe forwards"
    );
    assert_eq!(
        listener_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "verify mode must not probe forward listeners"
    );
    assert!(d.clusters_needing_login().is_empty());
    assert!(d.take_tunnel_events().is_empty());
    assert_eq!(d.cluster_connected.get("pegasus"), None);

    let reports_after = store
        .report_list(&task_core::ReportFilter::default())
        .unwrap()
        .len();
    assert_eq!(
        reports_after, reports_before,
        "verify mode must not write a cluster_login_needed report"
    );
}

/// ADR-0032 D3: `auth = "publickey"` かつ接続フックが刺さっていれば、未接続のクラスタは cooldown にする前に
/// 1 回だけ自動接続を試みる。成功したら `cluster_connected` が true になり、そのまま dispatch が続く
/// （`ClusterUnavailable` は残らない）。
#[tokio::test]
async fn publickey_cluster_auto_connects_and_dispatch_continues_on_success() {
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
    task.workspace = WorkspaceSpec::Remote {
        cluster: "auto".into(),
        path: PathBuf::from("/remote/project"),
        mode: None,
    };
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.clusters.insert(
        "auto".into(),
        cluster_spec_with_auth(
            "auto",
            "celeris-no-such-host-for-tests-auto-ok",
            "publickey",
        ),
    );
    let calls: Arc<StdMutex<Vec<(String, String)>>> = Arc::new(StdMutex::new(Vec::new()));
    let calls_for_hook = calls.clone();
    d.set_cluster_connector(Arc::new(move |id: &str, host: &str| {
        calls_for_hook
            .lock()
            .unwrap()
            .push((id.to_string(), host.to_string()));
        Ok(())
    }));

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 1, "{report:?}");
    assert_eq!(
        *calls.lock().unwrap(),
        vec![(
            "auto".to_string(),
            "celeris-no-such-host-for-tests-auto-ok".to_string()
        )]
    );
    assert_eq!(d.cluster_connected.get("auto"), Some(&true));
    assert!(
        !d.cluster_cooldown.contains_key("auto"),
        "success does not cool the cluster down"
    );
    let events = store.events_for(task.id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::ClusterUnavailable { .. })),
        "no ClusterUnavailable when the auto-connect succeeded: {events:?}"
    );
}

/// ADR-0032 D3: 自動接続が失敗したら、従来どおり cooldown + `Event::ClusterUnavailable` に落ちるが、
/// `reason` は「自動接続を試みて失敗した」と分かる文字列になる（人が受信箱で区別できるように）。
/// 接続の試行はクラスタごとに 1 回だけ（cooldown 中の 2 tick 目では呼ばれない）。
#[tokio::test]
async fn publickey_cluster_auto_connect_failure_gets_a_distinguishable_reason() {
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
    task.workspace = WorkspaceSpec::Remote {
        cluster: "auto".into(),
        path: PathBuf::from("/remote/project"),
        mode: None,
    };
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.clusters.insert(
        "auto".into(),
        cluster_spec_with_auth(
            "auto",
            "celeris-no-such-host-for-tests-auto-fail",
            "publickey",
        ),
    );
    let call_count = Arc::new(StdMutex::new(0u32));
    let call_count_for_hook = call_count.clone();
    d.set_cluster_connector(Arc::new(move |_id: &str, _host: &str| {
        *call_count_for_hook.lock().unwrap() += 1;
        Err("permission denied (publickey)".to_string())
    }));

    let report = d.tick().unwrap();
    assert_eq!(report.dispatched, 0, "{report:?}");
    assert_eq!(*call_count.lock().unwrap(), 1);
    let events = store.events_for(task.id).unwrap();
    let reason = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::ClusterUnavailable { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("ClusterUnavailable event");
    assert!(reason.contains("auto-connect failed"), "{reason}");
    assert!(reason.contains("permission denied (publickey)"), "{reason}");

    // 2 tick 目: cooldown 中なので自動接続は再試行しない（tick ごとに ssh が湧かない）。
    d.tick().unwrap();
    assert_eq!(*call_count.lock().unwrap(), 1, "cooldown 中は 1 回だけ");
}

/// ADR-0032 D3: `auth = "manual"` / `"totp"` は自動接続の対象外。接続フックが刺さっていても呼ばれず、
/// `reason` は従来どおりの文言のまま（自動接続を試みたとは分からない）。
#[tokio::test]
async fn manual_and_totp_clusters_are_not_auto_connected_even_with_a_hook() {
    for auth in ["manual", "totp"] {
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
        task.workspace = WorkspaceSpec::Remote {
            cluster: "auto".into(),
            path: PathBuf::from("/remote/project"),
            mode: None,
        };
        store.insert(&task).unwrap();
        let adapter = Arc::new(InstantAdapter {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            delay: Duration::ZERO,
        });
        let mut d = dispatcher(store.clone(), adapter, 1);
        d.config.clusters.insert(
            "auto".into(),
            cluster_spec_with_auth("auto", "celeris-no-such-host-for-tests-not-auto", auth),
        );
        let call_count = Arc::new(StdMutex::new(0u32));
        let call_count_for_hook = call_count.clone();
        d.set_cluster_connector(Arc::new(move |_id: &str, _host: &str| {
            *call_count_for_hook.lock().unwrap() += 1;
            Ok(())
        }));

        let report = d.tick().unwrap();
        assert_eq!(report.dispatched, 0, "{auth}: {report:?}");
        assert_eq!(
            *call_count.lock().unwrap(),
            0,
            "{auth}: hook must not run for auth={auth:?}"
        );
        let events = store.events_for(task.id).unwrap();
        let reason = events
            .iter()
            .find_map(|(_, e)| match e {
                Event::ClusterUnavailable { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .expect("ClusterUnavailable event");
        assert!(!reason.contains("auto-connect"), "{auth}: {reason}");
    }
}

/// ADR-0032 D1/D4: `ClusterSpec.auth` がスナップショットの `ClusterLive.auth` に写り、
/// `set_cluster_connect_pending` が `ClusterLive.connect_pending` を立てる/降ろす。
#[tokio::test]
async fn cluster_live_carries_auth_and_connect_pending() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "fern03".into(),
        cluster_spec_with_auth("fern03", "celeris-no-such-host-for-tests-live", "publickey"),
    );
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-09-17T00:00:00Z".into(),
        tick_ms: 50,
        providers: vec![],
        provider_checks: Default::default(),
    });

    d.tick().unwrap();
    let snap = rx.borrow().clone().expect("snapshot published");
    let live = snap
        .clusters
        .iter()
        .find(|c| c.id == "fern03")
        .expect("fern03 in snapshot");
    assert_eq!(
        (live.auth.as_str(), live.connect_pending),
        ("publickey", false)
    );

    d.set_cluster_connect_pending("fern03", true);
    d.tick().unwrap();
    let snap = rx.borrow().clone().expect("snapshot published");
    let live = snap
        .clusters
        .iter()
        .find(|c| c.id == "fern03")
        .expect("fern03 in snapshot");
    assert!(live.connect_pending, "connect_pending set");

    d.set_cluster_connect_pending("fern03", false);
    d.tick().unwrap();
    let snap = rx.borrow().clone().expect("snapshot published");
    let live = snap
        .clusters
        .iter()
        .find(|c| c.id == "fern03")
        .expect("fern03 in snapshot");
    assert!(!live.connect_pending, "connect_pending cleared");
}

/// ADR-0018 実装メモ M1: 接続が戻っていれば、その tick で cooldown が解ける。`celeris-localhost` への多重接続が無い環境では skip。
#[tokio::test]
async fn cluster_cooldown_is_cleared_once_the_control_master_is_back() {
    if !control_master_alive_blocking(&["ssh".to_string()], "celeris-localhost") {
        eprintln!("skip: celeris-localhost への多重接続が無い");
        return;
    }
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store, adapter, 1);
    d.config.clusters.insert(
        "local".into(),
        ClusterSpec {
            id: "local".into(),
            host: "celeris-localhost".into(),
            concurrency: 1,
            sync: SyncMode::Rsync,
            delete_on_push: false,
            setup: vec![],
            env: vec![],
            rsync_excludes: vec![],
            worktree: Default::default(),
            auth: "manual".into(),
            forwards: vec![],
            work_dir: None,
            keepalive_secs: 0,
            liveness_probe_secs: 0,
            job_wait: Default::default(),
        },
    );
    d.cluster_cooldown
        .insert("local".into(), Instant::now() + Duration::from_secs(3600));
    d.refresh_cluster_liveness();
    assert_eq!(d.cluster_connected.get("local"), Some(&true));
    assert!(
        !d.cluster_cooldown.contains_key("local"),
        "cooldown is cleared when the connection is back"
    );

    // ADR-0023 D1: 5 秒以内の 2 回目は `ssh -O check` を回さず、前回の結果をそのまま使う。
    d.cluster_connected.insert("local".into(), false);
    d.refresh_cluster_liveness();
    assert_eq!(
        d.cluster_connected.get("local"),
        Some(&false),
        "間引いた回は確認し直さない"
    );
    // 前回の確認を古くすると、次の呼び出しで確認し直す。
    d.last_cluster_liveness =
        Some(Instant::now() - CLUSTER_LIVENESS_INTERVAL - Duration::from_millis(1));
    d.refresh_cluster_liveness();
    assert_eq!(
        d.cluster_connected.get("local"),
        Some(&true),
        "間隔を過ぎたら確認し直す"
    );
}

/// Phase 99b（ADR-0059 追記、実機 2026-09-22 13:19 UTC タスク 01M34MACCEZ032A6YF8R4BMFM1）:
/// クラスタ一覧（`RunExtras::clusters`）は `recent_work` / `knowledge` / `profile` / `role` と同じ
/// 「いまの状態」なので、継続中（resume）の CoS 対話 run でも毎回渡す。Phase 99 まではフル
/// プリアンブルと同じ扱いで継続中は空になっており、CoS はセッションが rollover するまでクラスタと
/// 実効 work_dir を知れなかった。
#[test]
fn cluster_context_is_carried_into_continuing_cos_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let to_secretary = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root.clone(), None);
    d.config.clusters.insert(
        "pegasus".into(),
        cluster_spec_with_auth("pegasus", "pegasus", "manual"),
    );

    // 1 本目（新規セッション）: 従来どおりクラスタ一覧を渡す。
    let first = d
        .run_extras(&to_secretary, None, None, "claude-code")
        .unwrap();
    assert!(!first.session.as_ref().unwrap().resume);
    assert_eq!(first.clusters.len(), 1, "fresh session: クラスタ一覧を渡す");
    assert_eq!(first.clusters[0].id, "pegasus");

    // 2 本目（継続 = resume）: brief などの全量前置きは空になるが、クラスタ一覧は毎回渡す。
    let second = d
        .run_extras(&to_secretary, None, None, "claude-code")
        .unwrap();
    assert!(second.session.as_ref().unwrap().resume);
    assert!(
        second.node.is_none(),
        "continuing session: brief は流し直さない（対比のための確認）"
    );
    assert_eq!(
        second.clusters.len(),
        1,
        "continuing session でもクラスタ一覧は毎回渡す（Phase 99b）"
    );
    assert_eq!(second.clusters[0].id, "pegasus");

    // CoS 以外への対話には（継続の有無に関わらず）クラスタ一覧を渡さない。
    let to_survey = task_ops::conversation::start(
        store.as_ref(),
        "research-survey",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;
    let other = d.run_extras(&to_survey, None, None, "claude-code").unwrap();
    assert!(other.clusters.is_empty(), "CoS 以外の run では常に空");
}
