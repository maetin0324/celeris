use super::*;

/// Phase 45（実機バグ、2026-09-19）: `newly_failed_delegated_children` が「一度扱った失敗は数え直さない」
/// （ADR-0021 D3）を判定するのに `events_for`（タスクごとのローカルな `seq`）で親と子を比較していたため、
/// 子の方が親よりイベント数が多い（＝ `seq` が大きい）場合、子の失敗が毎回「新規」と誤判定され、親が
/// resume するたびに同じ質問（`QuestionRaised` と `child_failed` への遷移）が繰り返された
/// （実機の親 `01M2VG4YNG4DD7Z5BYPSB8W8AW` が 20 分で 5 回同じ質問をした事故）。
/// `events_for_with_global_ids`（`events` テーブルのグローバル `id`）で比較すれば、2 回目以降は
/// 「既に扱った失敗」と正しく判定され、親はやり直しの review pass だけで `done` になる。
#[tokio::test]
async fn child_failure_question_is_not_repeated_when_the_child_has_more_events_than_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org_for_reports(&store);

    let mut parent = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    parent.assignee = Some("coding-poc".into());
    store.insert(&parent).unwrap();

    let mut child = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    child.parent_id = Some(parent.id);
    store
        .delegate_children(parent.id, "run-1", vec![child.clone()])
        .unwrap();
    store
        .apply_transition(child.id, Trigger::Dispatch, None)
        .unwrap();
    // 子に、親が今後 2 回の run で積む以上のイベントを積んでから失敗させる（実機の形の再現）。
    // 子の `seq`（タスクごとのローカルな連番）が親のどの `seq` よりも大きくなるようにする。
    for i in 0..200u32 {
        store
            .append_event(
                child.id,
                &Event::worker_progress("child-run", format!("padding {i}")),
            )
            .unwrap();
    }
    store
        .apply_transition(child.id, Trigger::WorkerError { retryable: false }, None)
        .unwrap();
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Failed);

    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);

    // 1 回目: 親が走って review pass するが、委譲した子が失敗しているので `max_retries = 0` によりやり直せず、
    // ディスパッチャが質問を立てて blocked になる。
    let report1 = run_until_idle(&mut d, 200).await;
    assert!(report1.idle);
    let after_first = store.get(parent.id).unwrap().unwrap();
    assert_eq!(
        after_first.status,
        Status::Blocked,
        "max_retries = 0 なのでやり直せず、人に聞く"
    );

    // 人間が答えると ready に戻る。
    store
        .apply_transition(parent.id, Trigger::Answer, None)
        .unwrap();
    assert_eq!(store.get(parent.id).unwrap().unwrap().status, Status::Ready);

    // 2 回目: 親がもう一度走って review pass する。子は同じ失敗のままだが、既に扱った失敗なので
    // 再度質問を出してはいけない（旧実装のバグ: per-task seq を比較すると、子の方が seq が大きいので
    // 「新規」と誤判定して blocked を繰り返した）。
    let report2 = run_until_idle(&mut d, 200).await;
    assert!(report2.idle);
    let after_second = store.get(parent.id).unwrap().unwrap();
    let parent_events = store.events_for(parent.id).unwrap();
    assert_eq!(
        after_second.status,
        Status::Done,
        "既に扱った子の失敗を数え直してはいけない: {parent_events:?}"
    );

    let questions = parent_events
        .iter()
        .filter(|(_, e)| matches!(e, Event::QuestionRaised { .. }))
        .count();
    assert_eq!(questions, 1, "{parent_events:?}");
    let child_failed_transitions = parent_events
        .iter()
        .filter(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == Trigger::ChildFailed.name()))
        .count();
    assert_eq!(child_failed_transitions, 1, "{parent_events:?}");

    // approvals も 1 件のまま（Phase 44）。Phase F7: 親が `done` になったので、その 1 件は
    // `withdrawn` で閉じている（この試験は答えを `apply_transition` で直接渡すので `once` にはならない）。
    let approvals = store.approval_list(None, None, None).unwrap();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(
        approvals[0].decision,
        Some(task_core::approval::Decision::Withdrawn),
        "{approvals:?}"
    );
}

#[tokio::test]
async fn concurrency_limit_is_respected() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    for _ in 0..3 {
        store
            .insert(&new_task(
                dir.path(),
                Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
                0,
            ))
            .unwrap();
    }
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_millis(200),
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    let first = d.tick().unwrap();
    assert_eq!(first.dispatched, 2);
    assert_eq!(store.list(Some(Status::Running)).unwrap().len(), 2);
    let second = d.tick().unwrap();
    assert_eq!(second.dispatched, 0);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.list(Some(Status::Done)).unwrap().len(), 3);
}

/// ADR-0036 D1: 単独タスク（親なし）は従来どおり `<workspace>/artifacts`。挙動もパスも変わらない。
#[tokio::test]
async fn a_standalone_task_keeps_the_plain_artifacts_dir() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::ArtifactExists {
            name: "report.md".into(),
        },
        0,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(SiblingAdapter {
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    assert!(run_until_idle(&mut d, 200).await.idle);

    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert!(
        !dir.path().join(".taskd").exists(),
        "単独タスクは `.taskd/artifacts/` を使わない"
    );
    let result = std::fs::read_to_string(dir.path().join("artifacts/result.json")).unwrap();
    assert!(result.contains(&task.id.to_string()), "{result}");
    let produced: Vec<String> = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ArtifactProduced { artifact, .. } => Some(artifact.path),
            _ => None,
        })
        .collect();
    assert_eq!(produced, vec!["artifacts/report.md".to_string()]);
}

/// 実機の事故（2026-09-18、ADR-0036 §1）: 計画 run が作った兄弟 2 件が親の workspace を共有し、
/// 両方が `<workspace>/artifacts/` に書いたので `sources.json` / `result.json` が混ざった。
/// 同時に走らせても、結果ファイルと成果物がタスクごとに分かれていること。
#[tokio::test]
async fn siblings_sharing_one_workspace_do_not_mix_their_result_files_or_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut parent = new_task(dir.path(), Check::Human, 0);
    parent.kind = TaskKind::Plan;
    parent.status = Status::Done;
    store.insert(&parent).unwrap();
    let children: Vec<Task> = (0..2)
        .map(|_| {
            // plan / delegate の子は親の workspace をそのまま継ぐ（`plan::materialize`）。
            let mut c = new_task(
                dir.path(),
                Check::ArtifactExists {
                    name: "report.md".into(),
                },
                0,
            );
            c.parent_id = Some(parent.id);
            store.insert(&c).unwrap();
            c
        })
        .collect();

    let adapter = Arc::new(SiblingAdapter {
        delay: Duration::from_millis(150),
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    // 2 件が同じ tick で走り出す（並列度 2）。
    assert_eq!(d.tick().unwrap().dispatched, 2);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);

    assert!(
        !dir.path().join("artifacts").exists(),
        "共有の `artifacts/` は作られない"
    );
    for c in &children {
        let t = store.get(c.id).unwrap().unwrap();
        assert_eq!(
            t.status,
            Status::Done,
            "{:?}",
            store.events_for(c.id).unwrap()
        );
        let own = dir.path().join(".taskd/artifacts").join(c.id.to_string());
        let result = std::fs::read_to_string(own.join("result.json")).unwrap();
        assert!(result.contains(&c.id.to_string()), "{result}");
        assert_eq!(
            std::fs::read_to_string(own.join("report.md")).unwrap(),
            c.id.to_string()
        );
        // 申告された成果物のパスは workspace 相対のタスクごとの形（GUI がそのまま読める）。
        let produced: Vec<String> = store
            .events_for(c.id)
            .unwrap()
            .into_iter()
            .filter_map(|(_, e)| match e {
                Event::ArtifactProduced { artifact, .. } => Some(artifact.path),
                _ => None,
            })
            .collect();
        assert_eq!(
            produced,
            vec![format!(".taskd/artifacts/{}/report.md", c.id)]
        );
    }
}

/// ADR-0074 §6 F1 (g): 決定的な検査が `command timed out after` で不合格になっても、daemon が
/// 2 倍の timeout で 1 回だけ黙って再実行して直す（attempts は不変、repair WU も作らない）。
/// マーカーファイルが無ければ sleep して timeout し、あれば即座に exit 0 になるコマンドで、
/// 「1 回目は本当に時間切れ、2 回目は速い」を決定的に再現する。
#[tokio::test]
async fn command_timeout_check_reruns_once_with_double_timeout_and_passes() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f review-marker && exit 0 || (touch review-marker && sleep 5)".into(),
            expect_exit: 0,
        },
        1,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_millis(1),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.review_timeout = Duration::from_secs(1);
    let report = run_until_idle(&mut d, 400).await;
    let events = store.events_for(task_id).unwrap();
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{events:?}");
    assert_eq!(stored.attempts, 0, "the retry must not consume attempts");
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::RepairScheduled { .. })),
        "a successful retry must not create a repair WU: {events:?}"
    );
    // 判定は差し替えられ、記録される `ReviewVerdict` は 2 回目（合格）の結果だけ
    // （1 回目の timeout は再試行の中間結果で、別の Event としては残さない設計）。
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: true, .. })),
        "{events:?}"
    );
}

/// 監査の指摘（ADR-0012 D2）: 取得窓（max_concurrency*4+16）を優先度の高い経路なしタスクが埋めても、窓の外の実行可能な
/// タスクが dispatch され、それが終わるまで idle にならない。
#[tokio::test]
async fn unroutable_tasks_do_not_starve_or_hide_routable_tasks_outside_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut unroutable = Vec::new();
    for _ in 0..25 {
        let mut t = new_task(
            dir.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            0,
        );
        t.priority = 10;
        t.worker_hint.adapter = Some("nonexistent".into());
        store.insert(&t).unwrap();
        unroutable.push(t.id);
    }
    let routable = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f touched".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&routable).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let first = d.tick().unwrap();
    assert!(
        !first.idle,
        "a routable task is still waiting beyond the window"
    );
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(
        store.get(routable.id).unwrap().unwrap().status,
        Status::Done
    );
    for id in unroutable {
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Ready);
    }
}

/// ADR-0013 D4: tick の最後にメモリ上のスナップショットが `watch` に送られる（実行中の run、プロバイダの使用数、cooldown）。
#[tokio::test]
async fn tick_publishes_daemon_snapshot_to_watch() {
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
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::from_millis(300),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-09-14T00:00:00Z".into(),
        tick_ms: 50,
        providers: vec![ProviderLive {
            credential_refs: Default::default(),
            tier_models: Default::default(),
            account_id: None,
            id: "p1".into(),
            adapter: "instant".into(),
            tiers: vec![Tier::Standard],
            concurrency: 1,
            model: Some("m".into()),
            env_keys: vec![],
            in_use: 0,
            last_check: None,
            account_pool: false,
        }],
        provider_checks: Default::default(),
    });
    assert!(
        rx.borrow().is_none(),
        "nothing is published before the first tick"
    );

    d.tick().unwrap();
    let snap = rx.borrow().clone().expect("snapshot after the first tick");
    assert_eq!(
        (snap.ticks, snap.instance_id.as_str(), snap.tick_ms),
        (1, "inst-1", 50)
    );
    assert_eq!(snap.pid, std::process::id());
    assert_eq!(snap.in_flight.len(), 1);
    assert_eq!(snap.in_flight[0].task_id, task.id);
    assert_eq!(snap.in_flight[0].kind, InFlightKind::Worker);
    assert_eq!(snap.in_flight[0].provider, "p1");
    assert_eq!(snap.providers[0].in_use, 1);
    assert!(snap.cooldowns.is_empty());

    d.policy.report(
        "p1".into(),
        &ProviderOutcome::Throttled {
            retry_after: Duration::from_secs(60),
        },
    );
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let snap = rx.borrow().clone().unwrap();
    assert!(snap.ticks > 1);
    assert!(snap.in_flight.is_empty());
    assert_eq!(snap.providers[0].in_use, 0);
    assert_eq!(snap.cooldowns.len(), 1);
    assert_eq!(
        (
            snap.cooldowns[0].provider.as_str(),
            snap.cooldowns[0].reason.as_str()
        ),
        ("p1", "throttled")
    );
    assert!(
        snap.cooldowns[0].until > snap.last_tick_at,
        "until is in the future"
    );

    // ADR-0022 D2: 疎通確認の結果はスナップショットにだけ載る（DB には書かない）。
    assert!(snap.providers[0].last_check.is_none(), "確認する前は空");
    d.set_provider_check(
        "p1",
        ProviderCheckView {
            at: "2026-09-16T02:00:00Z".into(),
            result: "ok".into(),
            detail: None,
        },
    );
    d.tick().unwrap();
    let snap = rx.borrow().clone().unwrap();
    assert_eq!(
        snap.providers[0].last_check,
        Some(ProviderCheckView {
            at: "2026-09-16T02:00:00Z".into(),
            result: "ok".into(),
            detail: None
        })
    );

    // reload でプロバイダ表を差し替えても、残った id の記録は保つ。消えた id の記録は落とす。
    d.set_snapshot_providers(vec![
        ProviderLive {
            credential_refs: Default::default(),
            tier_models: Default::default(),
            account_id: None,
            id: "p1".into(),
            adapter: "instant".into(),
            tiers: vec![Tier::Standard],
            concurrency: 2,
            model: Some("m2".into()),
            env_keys: vec![],
            in_use: 0,
            last_check: None,
            account_pool: false,
        },
        ProviderLive {
            credential_refs: Default::default(),
            tier_models: Default::default(),
            account_id: None,
            id: "p2".into(),
            adapter: "instant".into(),
            tiers: vec![Tier::Standard],
            concurrency: 1,
            model: None,
            env_keys: vec![],
            in_use: 0,
            last_check: None,
            account_pool: false,
        },
    ]);
    d.tick().unwrap();
    let snap = rx.borrow().clone().unwrap();
    assert_eq!(
        snap.providers[0]
            .last_check
            .as_ref()
            .map(|c| c.result.as_str()),
        Some("ok"),
        "p1 の記録は残る"
    );
    assert!(
        snap.providers[1].last_check.is_none(),
        "p2 はまだ確認していない"
    );

    d.set_snapshot_providers(vec![ProviderLive {
        credential_refs: Default::default(),
        tier_models: Default::default(),
        account_id: None,
        id: "p2".into(),
        adapter: "instant".into(),
        tiers: vec![Tier::Standard],
        concurrency: 1,
        model: None,
        env_keys: vec![],
        in_use: 0,
        last_check: None,
        account_pool: false,
    }]);
    d.tick().unwrap();
    let snap = rx.borrow().clone().unwrap();
    assert_eq!(snap.providers.len(), 1);
    assert!(
        snap.providers[0].last_check.is_none(),
        "消えた p1 の記録は残さない"
    );
}

/// (d) run の途中で受け取った `rate_limit_event` の観測値が `AccountBook` とスナップショットに反映される。
#[tokio::test]
async fn mid_run_rate_limit_observation_lands_in_the_book_and_the_snapshot() {
    let dir = accounts_fixture();
    let ws_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        ws_dir.path(),
        Check::Command {
            cmd: "test -f touched".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();

    let captured = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::from_millis(20),
        observation: Some(usage_window(0.42, 90_000)),
        env: Vec::new(),
        captured,
        spawn_failure: false,
    });
    let mut d = pool_dispatcher(store.clone(), adapter, None, dir.path().to_path_buf(), 2, 2);
    let (tx, mut rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst".into(),
        hostname: "h".into(),
        started_at: "t".into(),
        tick_ms: 1,
        providers: Vec::new(),
        provider_checks: HashMap::new(),
    });
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    d.tick().unwrap(); // 完了後もう 1 tick 回し、最新のスナップショットを送らせる。
    rx.changed().await.ok();
    let snapshot = rx.borrow().clone().unwrap();
    assert_eq!(
        snapshot.accounts_root.as_deref(),
        Some(dir.path().to_string_lossy().as_ref())
    );
    assert_eq!(snapshot.max_runs_per_account, Some(2));
    let a_or_b = snapshot
        .accounts
        .iter()
        .find(|a| a.usage.is_some())
        .unwrap_or_else(|| {
            panic!(
                "no account carries the observation: {:?}",
                snapshot.accounts
            )
        });
    let usage = a_or_b.usage.as_ref().unwrap();
    assert_eq!(usage.five_hour.map(|w| w.utilization), Some(0.42));
    assert_eq!(usage.source, "run");
}

/// ADR-0074 §6 F1 (h): `git merge-base --is-ancestor main HEAD` が不成立（main が worktree の
/// 作業中に先行した）でも、衝突が無ければ daemon が worktree の中で決定的に merge し、再実行して
/// 合格に差し替える（repair WU は作らない）。
#[tokio::test]
async fn merge_base_check_merges_main_deterministically_when_there_is_no_conflict() {
    let repo_dir = tempfile::tempdir().unwrap();
    init_test_repo(repo_dir.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = git_task(
        repo_dir.path(),
        None,
        Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        },
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let adapter = Arc::new(AdvancesMainWhileWorkingAdapter {
        repo_dir: repo_dir.path().to_path_buf(),
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, root.path(), None);
    let report = run_until_idle(&mut d, 400).await;
    let events = store.events_for(task_id).unwrap();
    assert!(report.idle, "{report:?}");
    assert_eq!(
        store.get(task_id).unwrap().unwrap().status,
        Status::Done,
        "{events:?}"
    );
    // repair WU は作られない（衝突が無いので merge だけで直った）。
    let units = store.work_units_for(task_id).unwrap();
    assert!(
        units
            .iter()
            .all(|u| u.kind != task_core::WorkUnitKind::Repair),
        "{units:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::RepairScheduled { .. })),
        "{events:?}"
    );
}

/// ADR-0044 D2 / D8: 走っている run に人がコメントすると、
/// 1. タスクは `ready` に戻り（attempts 据え置き、`Transitioned{reason:"comment"}`）、
/// 2. `WorkerFinished{outcome:"interrupted: comment"}` が残り（失敗ではないので報告は作らない）、
/// 3. ディスパッチャが次の tick でその run を止め（cancel と同じ `abort_stale_runs` の経路）、
/// 4. **次の run の前置きの先頭**にそのコメントが「人からの割り込み」として載る。
#[tokio::test]
async fn a_human_comment_interrupts_the_running_run_and_the_next_run_carries_it() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Human, 2);
    task.attempts = 1;
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(InterruptProbeAdapter {
        seen: seen.clone(),
        hold: Duration::from_secs(30),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);

    // 1 tick で dispatch し、run が**実際に走り出す**まで待つ（spawn されただけでは前置きは組まれない）。
    d.tick().unwrap();
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Running);
    assert_eq!(d.running.len(), 1, "run が手元で走っている");
    for _ in 0..50 {
        if seen.lock().map(|s| !s.is_empty()).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(seen.lock().unwrap().len(), 1, "1 回目の run が始まっている");

    // 人がコメントする（API と同じ経路）。
    let result = task_ops::comment::post_human_comment(
        store.as_ref(),
        task.id,
        "方針を変えたい。まず設計を書いて".into(),
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    assert_eq!(result.effect, task_ops::comment::CommentEffect::Interrupted);
    let after = store.get(task.id).unwrap().unwrap();
    assert_eq!(after.status, Status::Ready);
    assert_eq!(after.attempts, 1, "割り込みは試行を消費しない");
    let events = store.events_for(task.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { outcome, .. } if outcome == "interrupted: comment"))
    );

    // 次の tick で走っていた run が止まり、同じ tick で走り直す。
    d.tick().unwrap();
    assert!(
        d.running_for_task(task.id) == 0
            || store.get(task.id).unwrap().unwrap().status == Status::Running,
        "古い run は捨てられている"
    );
    // 走り直した run の前置きに割り込みが載るまで回す。
    for _ in 0..40 {
        if seen.lock().map(|s| s.len() >= 2).unwrap_or(false) {
            break;
        }
        d.tick().unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let contexts = seen.lock().unwrap().clone();
    assert!(
        contexts.len() >= 2,
        "2 回目の run が始まっていない: {}",
        contexts.len()
    );
    let second = &contexts[1];
    assert_eq!(
        second.interrupt.as_deref(),
        Some("方針を変えたい。まず設計を書いて"),
        "次の run に割り込みが渡る"
    );
    assert_eq!(second.comments.len(), 1);
    assert_eq!(
        second.comments[0].author_kind,
        task_core::CommentAuthorKind::Human
    );
    assert!(second.comments_enabled, "ワーカー run はコメントを書ける");
    assert!(contexts[0].interrupt.is_none(), "1 回目には割り込みが無い");

    // 前置きの先頭に「人からの割り込み」として出る。
    let preamble = task_worker::preamble::render(second, "artifacts");
    assert!(preamble.starts_with("## コメント"), "{preamble}");
    assert!(
        preamble.contains("**人からの割り込み**: 方針を変えたい。まず設計を書いて"),
        "{preamble}"
    );
    assert!(
        preamble.contains("短い進捗や判断の記録はコメントに書け"),
        "{preamble}"
    );
}

/// ADR-0044 D2: ワーカーの `{"type":"comment"}` 行は `author_kind = node` で残り、状態は変えない。
#[tokio::test]
async fn a_worker_comment_is_recorded_as_a_node_comment_without_touching_the_state() {
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
    task.assignee = Some("impl".into());
    store.insert(&task).unwrap();
    let adapter = Arc::new(CommentingAdapter);
    let mut d = dispatcher(store.clone(), adapter, 1);
    run_until_idle(&mut d, 20).await;

    let comments = store.comments_for(task.id).unwrap();
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert_eq!(comments[0].author_kind, task_core::CommentAuthorKind::Node);
    assert_eq!(comments[0].author.as_deref(), Some("impl"));
    assert_eq!(comments[0].body, "ビルドが通った");
    assert!(comments[0].run_id.is_some(), "run に紐づく");
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Done,
        "状態は変えない"
    );
}

/// ADR-0043 D3 / D4: `[commands] setup` が落ちたら run を始めず、既存の質問の経路で `blocked` にする。
#[tokio::test]
async fn a_failing_setup_blocks_the_task_with_a_question_instead_of_starting_the_run() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("benchfs");
    init_test_repo(&code);
    std::fs::create_dir_all(code.join(".config/celeris")).unwrap();
    std::fs::write(
        code.join(".config/celeris/workspace.toml"),
        b"[commands]\nsetup = [\"exit 3\"]\n",
    )
    .unwrap();
    for args in [vec!["add", "-A"], vec!["commit", "-q", "-m", "setup"]] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&code)
            .args(&args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let ws_root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project_id, repos) = project_with_repos(
        &store,
        &[("benchfs", code.as_path(), task_core::RepoKind::Git)],
    );
    let mut task = new_task(
        &code,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.project_id = Some(project_id);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    store.insert(&task).unwrap();

    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        seen: seen.clone(),
        files: vec![],
    });
    let mut d = worktree_dispatcher(store.clone(), adapter, ws_root.path(), None);
    run_until_idle(&mut d, 60).await;

    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    assert!(seen.lock().unwrap().is_empty(), "ワーカーは起こさない");
    let task_dir = ws_root.path().join(task.id.to_string());
    let log = std::fs::read_to_string(task_dir.join("runs/setup.log")).expect("setup.log");
    assert!(log.contains("$ (benchfs) exit 3"), "{log}");
    let events = store.events_for(task.id).unwrap();
    let question = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished { outcome, .. } if outcome.starts_with("question:") => {
                Some(outcome.clone())
            }
            _ => None,
        })
        .expect("question outcome");
    assert!(question.contains("setup が失敗しました"), "{question}");
    assert!(question.contains("workspace.toml"), "{question}");
}

/// ADR-0072 (a)(b)(e)(g): 予算切れは `Trigger::Continue` で `Ready` に戻り attempts を消費しない。
/// 進捗のない checkpoint が連続 2 回続くと `blocked`（`QuestionRaised` + `Approval`）。人の回答
/// （`Trigger::Answer`）で窓が戻り、また continuation が進む。
#[tokio::test]
async fn budget_exhausted_run_continues_without_consuming_attempts_then_blocks_on_no_progress() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), Arc::new(AlwaysBudgetExhaustedAdapter), 1);

    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked, "{stored:?}");
    assert_eq!(
        stored.attempts, 0,
        "continuation は attempts を消費しない (g)"
    );

    let events = store.events_for(task_id).unwrap();
    let checkpoint_events = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::CheckpointSaved { .. }))
        .count();
    assert!(checkpoint_events >= 2, "{checkpoint_events}");
    let ends: Vec<_> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                role: None, end, ..
            } => Some(*end),
            _ => None,
        })
        .collect();
    assert!(
        ends.iter().all(|e| matches!(
            e,
            Some(task_core::RunEnd::BudgetExhausted {
                kind: task_core::BudgetKind::Turns
            })
        )),
        "{ends:?}"
    );
    // D18/D21: 上限到達・進捗なしは（cross_department の質問などと同じ扱いで）`WorkerFinished.outcome`
    // が `"question: ..."` になり、`Trigger::WorkerQuestion` の一般経路で `blocked` になる。
    let last_outcome = events
        .iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::WorkerFinished {
                role: None,
                outcome,
                ..
            } => Some(outcome.clone()),
            _ => None,
        })
        .expect("some WorkerFinished");
    assert!(last_outcome.starts_with("question: "), "{last_outcome}");
    assert!(last_outcome.contains("進捗なし"), "{last_outcome}");

    // (g): continue は試行に数えない（`retry_policy::attempt_history`）。
    let events_only: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
    let history = task_core::retry_policy::attempt_history(&stored, &events_only);
    assert!(history.is_empty(), "{history:?}");
    // (g): `classify_task_failure` は blocked のタスクを見ても Failed 前提の分類を返すだけなので
    // ここでは呼ばない。`stats::classify_outcome` が continue を Error に数えないことは
    // `task-api::stats::tests::outcome_prefixes_are_classified` で確認済み。

    // D18: 人の回答で窓が戻る。
    store
        .apply_transition(
            task_id,
            Trigger::Answer,
            Some(Event::Answered {
                question: "実行が進みません".into(),
                answer: "続けてください".into(),
            }),
        )
        .unwrap();
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Ready);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle, "{report:?}");
    // 同じ（進捗を示さない）アダプタなので、また 2 回の無進捗 continuation の後 blocked に戻る。
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Blocked);
    assert_eq!(stored.attempts, 0);
}

/// ADR-0072 (c)(d): result.json の `yield` は `Terminal::Yielded` → `Trigger::Continue` になり、
/// 次の run の `RunContext.continuation` に checkpoint と Run 番号が載る（会話の全文は載らない —
/// そもそも `ContinuationContext` は checkpoint の JSON しか運ばない）。1 回目の `RunContext` は
/// continuation を持たない。
#[tokio::test]
async fn yielded_run_continues_and_the_next_run_context_carries_the_checkpoint() {
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
    let task_id = task.id;
    store.insert(&task).unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let mut d = dispatcher(
        store.clone(),
        Arc::new(YieldThenDoneAdapter { seen: seen.clone() }),
        1,
    );

    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.attempts, 0,
        "continuation は attempts を消費しない (g)"
    );
    assert_eq!(stored.status, Status::Done, "{stored:?}");

    let contexts = seen.lock().unwrap().clone();
    assert_eq!(contexts.len(), 2, "{contexts:?}");
    assert!(
        contexts[0].continuation.is_none(),
        "最初の run には continuation は無い: {:?}",
        contexts[0].continuation
    );
    let cont = contexts[1]
        .continuation
        .as_ref()
        .expect("2 回目の run は continuation を持つ");
    assert_eq!(cont.run_seq, 2);
    assert_eq!(cont.previous_end, "yielded");
    assert_eq!(cont.checkpoint["next_action"], "B のテストを書く");
    assert_eq!(cont.prior_runs, vec!["Run #1 yielded".to_string()]);

    let events = store.events_for(task_id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::CheckpointSaved { .. })),
        "{events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerFinished {
                role: None,
                end: Some(task_core::RunEnd::Yielded),
                ..
            }
        )),
        "{events:?}"
    );
}

/// ADR-0072 §6 (f): `[execution] continuation = false` なら、予算切れは従来どおり
/// `WorkerError{retryable:true}` になり attempts を消費する（continuation しない）。
#[tokio::test]
async fn continuation_disabled_restores_the_legacy_worker_error() {
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
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), Arc::new(AlwaysBudgetExhaustedAdapter), 1);
    d.config.execution.continuation = false;

    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Failed, "{stored:?}");
    assert_eq!(
        stored.attempts, 2,
        "max_retries=1 なので 2 回試行して失敗 (f)"
    );
    let events = store.events_for(task_id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::CheckpointSaved { .. })),
        "continuation を無効にしたら checkpoint も作らない: {events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::Transitioned { reason, .. } if reason == "worker_error"
        )),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "continue")),
        "{events:?}"
    );
}

/// ADR-0072 (h): daemon の再起動（同じ store で新しい Dispatcher）の後も、
/// 最新の checkpoint から continuation が組まれる（全ての状態は DB/events にあり、
/// dispatcher のメモリには無いため）。
#[tokio::test]
async fn continuation_survives_a_dispatcher_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        2,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();
    // 「daemon 再起動前」の状態を events で直接作る（実際の Dispatcher の tick は使わない — 即完了
    // する fake adapter で 2 つの tick にまたがる非同期の完了を決定的に待つのは難しいため。
    // ここで作る events は、実際の on_worker_finished が積む形とバイト単位で同じにする）。
    store
        .apply_transition_with_events(
            task_id,
            Trigger::Dispatch,
            vec![Event::WorkerStarted {
                run_id: "run-1".into(),
                adapter: "instant".into(),
                model: "m".into(),
                provider: Some("p1".into()),
                account: None,
                role: None,
                task_role: None,
            }],
        )
        .unwrap();
    let checkpoint = task_core::Checkpoint {
        schema: task_core::CHECKPOINT_SCHEMA.into(),
        task_id: task_id.to_string(),
        work_unit: None,
        run_id: "run-1".into(),
        run_seq: 1,
        end: task_core::CheckpointEnd::BudgetExhausted,
        source: task_core::CheckpointSource::Mechanical,
        completed: vec![],
        remaining: vec!["続きの作業".into()],
        decisions: vec![],
        files_changed: vec![],
        tests_run: vec![],
        known_failures: vec![],
        artifact_refs: vec![],
        next_action: "続ける".into(),
        open_questions: vec![],
        plan_issue: None,
        repo_state: None,
        recent_activity: vec![],
        created_at: "2026-09-24T00:00:00Z".into(),
    };
    store
        .apply_transition_with_events(
            task_id,
            Trigger::Continue {
                why: task_core::ContinueWhy::Continue,
            },
            vec![
                Event::WorkerFinished {
                    run_id: "run-1".into(),
                    outcome: "continue: budget_exhausted(turns) の続き（Run #2）".into(),
                    usage: None,
                    role: None,
                    metrics: None,
                    end: Some(task_core::RunEnd::BudgetExhausted {
                        kind: task_core::BudgetKind::Turns,
                    }),
                },
                Event::CheckpointSaved {
                    run_id: "run-1".into(),
                    work_unit_id: None,
                    checkpoint: Box::new(checkpoint),
                },
            ],
        )
        .unwrap();
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Ready, "{stored:?}");
    assert_eq!(stored.attempts, 0);
    // ここまでが「daemon 再起動前」に相当する状態。d1（前のプロセスの Dispatcher）は無く、
    // 全ての状態は DB/events にある（`infra_backoff` 等のプロセス内メモリは何も使っていない）。

    // 新しい Dispatcher（同じ store）で続きを回す。
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let mut d2 = dispatcher(
        store.clone(),
        Arc::new(InterruptProbeAdapter {
            seen: seen.clone(),
            hold: Duration::ZERO,
        }),
        1,
    );
    let report = run_until_idle(&mut d2, 200).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.attempts, 0, "continuation は attempts を消費しない");
    assert_eq!(stored.status, Status::Done, "{stored:?}");

    let contexts = seen.lock().unwrap().clone();
    assert_eq!(contexts.len(), 1, "{contexts:?}");
    let cont = contexts[0]
        .continuation
        .as_ref()
        .expect("再起動後の run も continuation を持つ");
    assert_eq!(cont.run_seq, 2);
    assert!(cont.previous_end.starts_with("budget_exhausted"));
}
