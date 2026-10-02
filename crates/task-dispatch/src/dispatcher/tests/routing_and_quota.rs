use super::*;

/// ADR-0069 Phase 118 D4（既定）: `[reviewer] tier` も部署の `review.tier` も無ければ、reviewer の
/// lane は worker run の lane に一致させ、組織の天井（`budget.max_lane`）で丸める。
#[tokio::test]
async fn reviewer_lane_defaults_to_the_worker_lane_capped_by_the_org_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut eng = org_node_of("eng", Some("secretary"), OrgKind::Department, None);
    eng.profile.budget.max_lane = Some(Tier::Standard);
    for n in [
        org_node_of("secretary", None, OrgKind::Secretary, Some("secretary")),
        eng,
    ] {
        store.org_upsert(&n).unwrap();
    }
    let mut r = new_task(dir.path(), Check::Reviewer, 0);
    r.assignee = Some("eng".into());
    r.worker_hint.tier = Tier::Frontier;
    store.insert(&r).unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        delay: Duration::from_millis(5),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    assert_eq!(store.get(r.id).unwrap().unwrap().status, Status::Done);
    let record = reviewer_routing_record(store.as_ref(), r.id);
    assert_eq!(record.harness.as_deref(), Some("reviewer"));
    assert_eq!(record.decision.lane, Tier::Standard, "{record:?}");
    assert_eq!(record.decision.proposed, Tier::Frontier);
    assert_eq!(record.decision.rule_id, "reviewer/matches-worker-lane");
    assert!(record.decision.clamped_by.is_some(), "{record:?}");
    assert_eq!(record.resolution.lane, Some(Tier::Standard));
}

/// ADR-0069 Phase 118 D4: `[reviewer] tier` の明示は既定（worker lane 一致）に勝つ。
#[tokio::test]
async fn explicit_reviewer_tier_config_wins_over_the_worker_lane_default() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    for n in [
        org_node_of("secretary", None, OrgKind::Secretary, Some("secretary")),
        org_node_of("eng", Some("secretary"), OrgKind::Department, None),
    ] {
        store.org_upsert(&n).unwrap();
    }
    let mut r = new_task(dir.path(), Check::Reviewer, 0);
    r.assignee = Some("eng".into());
    r.worker_hint.tier = Tier::Frontier;
    store.insert(&r).unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        delay: Duration::from_millis(5),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.reviewer_tier_override = Some(Tier::Cheap);
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    let record = reviewer_routing_record(store.as_ref(), r.id);
    assert_eq!(record.decision.lane, Tier::Cheap, "{record:?}");
    assert_eq!(record.decision.rule_id, "reviewer/explicit-config");
    assert_eq!(record.decision.clamped_by, None);
}

/// ADR-0069 Phase 118 D4: 部署の `[profile] review.tier`（ADR-0069 D2）は `[reviewer] tier` の
/// 明示より強い（最も具体的な指定）。
#[tokio::test]
async fn department_review_tier_wins_over_the_explicit_reviewer_config() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut eng = org_node_of("eng", Some("secretary"), OrgKind::Department, None);
    eng.profile.review.tier = Some(Tier::Frontier);
    for n in [
        org_node_of("secretary", None, OrgKind::Secretary, Some("secretary")),
        eng,
    ] {
        store.org_upsert(&n).unwrap();
    }
    let mut r = new_task(dir.path(), Check::Reviewer, 0);
    r.assignee = Some("eng".into());
    r.worker_hint.tier = Tier::Standard;
    store.insert(&r).unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        delay: Duration::from_millis(5),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.reviewer_tier_override = Some(Tier::Cheap);
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    let record = reviewer_routing_record(store.as_ref(), r.id);
    assert_eq!(record.decision.lane, Tier::Frontier, "{record:?}");
    assert_eq!(record.decision.rule_id, "reviewer/department-review-tier");
}

/// ADR-0054 D1（Phase 67）: `resolve_node_session` の店じまい（store の読み書き側）。純粋な判断は
/// `crate::sessions::decide` で別途テスト済みなので、ここでは実際に `node_sessions` を作る・続ける・
/// rollover で作り直す・アカウント変更で作り直す、の 4 つが store に正しく反映されることを見る。
#[tokio::test]
async fn resolve_node_session_resumes_under_the_limit_then_rolls_over_then_retires_on_account_change()
 {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = dispatcher(
        store.clone(),
        Arc::new(FileAdapter {
            plan_json: String::new(),
            review_json: String::new(),
            delay: Duration::ZERO,
        }),
        1,
    );
    d.config.session_rollover_tokens = 100;
    let now = OffsetDateTime::now_utc();

    // 1 本目: 現役セッションが無いので新規（要約は乗らない）。
    let (first, diff0) = d
        .resolve_node_session(
            "cos",
            SessionKind::Conversation,
            None,
            "claude-code",
            Some("acct-a"),
            now,
        )
        .unwrap();
    let first = first.unwrap();
    assert!(
        !first.resume,
        "the first run of a session is never a resume"
    );
    assert!(diff0.is_empty(), "a brand-new session has no summary yet");
    assert!(!first.session_id.is_empty());
    let first_id = first.session_id.clone();

    // 累計トークンが閾値未満なら続く（`--resume` 相当）。
    store
        .node_session_touch("cos", SessionKind::Conversation, None, 50, now)
        .unwrap();
    let (second, _) = d
        .resolve_node_session(
            "cos",
            SessionKind::Conversation,
            None,
            "claude-code",
            Some("acct-a"),
            now,
        )
        .unwrap();
    let second = second.unwrap();
    assert!(second.resume);
    assert_eq!(second.session_id, first_id);

    // 閾値を超えたら次の run から新規セッション（前のセッションは引退、前置きには要約が乗る）。
    store
        .node_session_touch("cos", SessionKind::Conversation, None, 100, now)
        .unwrap();
    let (third, summary) = d
        .resolve_node_session(
            "cos",
            SessionKind::Conversation,
            None,
            "claude-code",
            Some("acct-a"),
            now,
        )
        .unwrap();
    let third = third.unwrap();
    assert!(!third.resume, "rollover starts a fresh session");
    assert_ne!(third.session_id, first_id);
    // 要約自体は空（対話履歴が無いテストなので）でも、`needs_summary` 経路（`session_summary` 呼び出し）
    // を通ったことは rollover で `resume = false` になったことから確認できる。
    let _ = summary;
    let after_rollover = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .unwrap()
        .unwrap();
    assert_eq!(after_rollover.session_id, third.session_id);
    assert_eq!(after_rollover.turns, 0, "a fresh session starts at 0 turns");

    // アカウントが変わると（プールが枯渇して別アカウントに倒れた等）、同じトークン量でも作り直す。
    let (fourth, _) = d
        .resolve_node_session(
            "cos",
            SessionKind::Conversation,
            None,
            "claude-code",
            Some("acct-b"),
            now,
        )
        .unwrap();
    let fourth = fourth.unwrap();
    assert!(!fourth.resume, "an account change starts a fresh session");
    assert_ne!(fourth.session_id, third.session_id);
    let after_account_change = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .unwrap()
        .unwrap();
    assert_eq!(after_account_change.session_id, fourth.session_id);
    assert_eq!(after_account_change.account_id.as_deref(), Some("acct-b"));
}

#[tokio::test]
async fn delegate_can_select_a_different_genre_and_available_genres_reach_the_prompt_context() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut parent = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    parent.role = Some("lead".into());
    parent.genre = Some("coding".into());
    store.insert(&parent).unwrap();

    let mut child_proposal = proposal("investigate prior art", vec![]);
    child_proposal.role = Some("literature-reader".into());
    child_proposal.genre = Some("literature".into());
    let adapter = Arc::new(GenreDelegatingAdapter {
        proposal: child_proposal,
        seen_available_genres: std::sync::Mutex::new(None),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 4);
    d.config.roles = vec![
        RoleSpec {
            id: "lead".into(),
            ..RoleSpec::default()
        },
        RoleSpec {
            id: "literature-reader".into(),
            ..RoleSpec::default()
        },
    ];
    d.config.genres = vec![
        task_core::GenreSpec {
            id: "coding".into(),
            description: "write and fix code".into(),
            default_role: Some("lead".into()),
            roles: vec!["lead".into()],
            ..task_core::GenreSpec::default()
        },
        task_core::GenreSpec {
            id: "literature".into(),
            description: "related work survey".into(),
            default_role: Some("literature-reader".into()),
            roles: vec!["literature-reader".into()],
            ..task_core::GenreSpec::default()
        },
    ];
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);

    let p = store.get(parent.id).unwrap().unwrap();
    assert_eq!(
        p.status,
        Status::Done,
        "{:?}",
        store.events_for(parent.id).unwrap()
    );
    let children = store.children(parent.id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].role.as_deref(), Some("literature-reader"));
    assert_eq!(
        children[0].genre.as_deref(),
        Some("literature"),
        "explicit genre wins"
    );

    let available = adapter
        .seen_available_genres
        .lock()
        .unwrap()
        .clone()
        .expect("available_genres seen");
    let ids: Vec<&str> = available.iter().map(|g| g.id.as_str()).collect();
    assert!(ids.contains(&"coding"), "{ids:?}");
    assert!(ids.contains(&"literature"), "{ids:?}");
}

/// ADR-0028 D3: `run_extras` は Plan run にも `available_genres` を渡す（今までは Execute/Approval だけ）。
/// これで `build_plan_prompt` にも「使える専門家」節が出る（`claude_code` 側のテストで確認済み）。
#[test]
fn run_extras_fills_available_genres_for_plan_runs() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let plan = plan_task(dir.path(), 0);
    store.insert(&plan).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FileAdapter {
        plan_json: VALID_PLAN.into(),
        review_json: r#"{"verdicts":[]}"#.into(),
        delay: Duration::from_millis(0),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.genres = vec![task_core::GenreSpec {
        id: "coding".into(),
        description: "write and fix code".into(),
        ..task_core::GenreSpec::default()
    }];
    let extras = d.run_extras(&plan, None, None, "claude-code").unwrap();
    let ids: Vec<&str> = extras
        .available_genres
        .iter()
        .map(|g| g.id.as_str())
        .collect();
    assert_eq!(ids, vec!["coding"]);

    // 分野が無い設定では空のまま。
    d.config.genres = Vec::new();
    let extras = d.run_extras(&plan, None, None, "claude-code").unwrap();
    assert!(extras.available_genres.is_empty());
}

/// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: ディスパッチャの配線 2 つ —
/// (1) `available_genres[].harness` が `default_role` の役割のアダプタから決定的に埋まる、
/// (2) 計画がハーネス系の担当に別名のファイルを要求していたら、子を作る前にその条件を落として
///     `objective` に本当の成果物の名前を注記する（LLM は呼ばない）。
#[test]
fn harness_genres_are_marked_and_the_plan_is_fixed_before_children_are_created() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let plan_parent = plan_task(dir.path(), 0);
    store.insert(&plan_parent).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FileAdapter {
        plan_json: VALID_PLAN.into(),
        review_json: r#"{"verdicts":[]}"#.into(),
        delay: Duration::from_millis(0),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.roles = vec![task_core::RoleSpec {
        id: "literature-reader".into(),
        adapter: Some("paperqa".into()),
        ..task_core::RoleSpec::default()
    }];
    d.config.genres = vec![task_core::GenreSpec {
        id: "literature".into(),
        description: "関連研究の調査".into(),
        output_artifacts: vec!["answer.md: 引用付きの答え".into(), "papers.json".into()],
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..task_core::GenreSpec::default()
    }];

    let extras = d
        .run_extras(&plan_parent, None, None, "claude-code")
        .unwrap();
    assert_eq!(
        extras.available_genres[0].harness.as_deref(),
        Some("paperqa")
    );
    assert!(extras.available_genres[0].is_harness());

    let mut plan = task_core::PlanOutput {
        tasks: vec![task_core::NewTask {
            harness: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            title: "候補テーマの抽出".into(),
            objective: "候補テーマを candidates.json にまとめよ".into(),
            acceptance: vec![Criterion {
                text: "candidates.json に候補テーマがある".into(),
                check: Check::ArtifactExists {
                    name: "candidates.json".into(),
                },
            }],
            depends_on: vec![],
            kind: task_core::NewTaskKind::Execute,
            tier: None,
            role: None,
            genre: Some("literature".into()),
            assignee: None,
            workspace: None,
            category: None,
            labels: Vec::new(),
            partial_ok: None,
        }],
    };
    d.fix_plan_for_harness(&plan_parent, &mut plan, &[]);
    assert_eq!(
        plan.tasks[0].acceptance[0].check,
        Check::Reviewer,
        "落とすと 0 件になるので内容はレビュアーが見る"
    );
    assert!(
        plan.tasks[0].objective.ends_with(
            "（注: この担当の成果物は answer.md / papers.json に固定。要求した内容は answer.md の中で述べる）"
        ),
        "{}",
        plan.tasks[0].objective
    );
}

/// (a) 観測値の異なる 2 アカウントがあれば、スコアの高い方（残量が多い方）に run が割り当てられ、
/// `WorkerStarted.account` とアダプタが実際に受け取った env が一致する（ADR-0024 D2/D3、受け入れ条件 1）。
#[tokio::test]
async fn pool_run_goes_to_the_account_with_more_headroom_and_sets_the_env() {
    let dir = accounts_fixture();
    let book_path = dir.path().join(".celeris-usage.json");
    {
        let mut book = AccountBook::load(&book_path);
        book.record_observation("a", usage_window(0.8, 90_000), ObservationSource::Run);
        book.record_observation("b", usage_window(0.1, 90_000), ObservationSource::Run);
        book.save().unwrap();
    }

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
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: false,
    });
    let mut d = pool_dispatcher(store.clone(), adapter, None, dir.path().to_path_buf(), 2, 2);
    // `usage_window` は `observed_at = 10_000` 基準なので、評価もその時刻で行う（実時計だと `resets_at` が
    // とっくに過ぎていて両方とも実効使用率 0 になってしまうため）。
    d.set_now_unix_fn(Arc::new(|| 10_000));
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);

    let events = store.events_for(task.id).unwrap();
    let account = events.iter().find_map(|(_, e)| match e {
        Event::WorkerStarted { account, .. } => account.clone(),
        _ => None,
    });
    assert_eq!(account.as_deref(), Some("b"));

    let envs = captured.lock().unwrap();
    assert_eq!(envs.len(), 1);
    let dir_value = envs[0]
        .iter()
        .find(|(k, _)| k == "CLAUDE_SECURESTORAGE_CONFIG_DIR")
        .map(|(_, v)| v.clone());
    assert_eq!(
        dir_value.as_deref(),
        Some(dir.path().join("b").to_string_lossy().as_ref())
    );
}

/// ADR-0074 D4/§6 F3 (k)（Phase F3 quota）: quota の bookkeeping（`QuotaActivity`/
/// `QuotaCalibrationBook`）はアカウント選択にも lane 決定にも影響しない（観測と記録だけ、
/// D4「quota で dispatch・選択を変えない」）。同じ観測値・同じ設定で 2 回走らせ、片方だけ事前に
/// 「無関係な別 run がまだ重なっている」quota の状態と較正材料を仕込んでおいても、選ばれる
/// アカウントと `RoutingDecided` の記録内容（lane）は変わらない。
#[tokio::test]
async fn quota_bookkeeping_does_not_change_account_or_lane_selection() {
    async fn run_once(prime_quota_state: bool) -> (Option<String>, Option<task_core::Tier>) {
        let dir = accounts_fixture();
        let book_path = dir.path().join(".celeris-usage.json");
        {
            let mut book = AccountBook::load(&book_path);
            book.record_observation("a", usage_window(0.8, 90_000), ObservationSource::Run);
            book.record_observation("b", usage_window(0.1, 90_000), ObservationSource::Run);
            book.save().unwrap();
        }
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
            delay: Duration::ZERO,
            observation: None,
            env: Vec::new(),
            captured,
            spawn_failure: false,
        });
        let mut d = pool_dispatcher(store.clone(), adapter, None, dir.path().to_path_buf(), 2, 2);
        d.set_now_unix_fn(Arc::new(|| 10_000));
        if prime_quota_state {
            // 「無関係な別 run がまだ走っている」状態を作る（この run の完了時に apportion 側へ
            // 倒れうる状態。選択そのものには関わらないはず）。
            d.quota_activity.begin(
                AccountAdapter::ClaudeCode,
                "b",
                "unrelated-run",
                None,
                false,
            );
            d.quota_calibration.record(
                "claude-oauth",
                task_core::QuotaWindow::FiveHour,
                50.0,
                1_000.0,
            );
        }
        let report = run_until_idle(&mut d, 200).await;
        assert!(report.idle);
        let events = store.events_for(task.id).unwrap();
        let account = events.iter().find_map(|(_, e)| match e {
            Event::WorkerStarted { account, .. } => account.clone(),
            _ => None,
        });
        let event_list: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
        let task_row = store.get(task.id).unwrap().unwrap();
        let lane = task_core::routing_audit(&task_row, &event_list)
            .iter()
            .rev()
            .find_map(|a| a.lane);
        (account, lane)
    }

    let baseline = run_once(false).await;
    let primed = run_once(true).await;
    assert_eq!(
        baseline, primed,
        "quota state must not change account/lane selection"
    );
    assert_eq!(
        baseline.0.as_deref(),
        Some("b"),
        "sanity: the higher-headroom account still wins"
    );
}

/// (b) 片方が throttled で終わると、そのアカウントだけが cooldown になり、次の run はもう片方に行く。
/// プロバイダ自体は cooldown にならない（受け入れ条件 2）。
#[tokio::test]
async fn throttled_account_cools_down_without_cooling_the_provider() {
    let dir = accounts_fixture();
    let ws_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // `max_requeues = 0` にして、1 回失敗したらすぐ通常の失敗（`max_retries = 0` で即 `failed`）にする。
    // そうしないと供給側失敗は requeue され続け、"a" だけでなく "b" も使い切って cooldown にしてしまう。
    let task1 = new_task(
        ws_dir.path(),
        Check::Command {
            cmd: "test -f touched".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task1).unwrap();

    let captured = Arc::new(StdMutex::new(Vec::new()));
    // "a" は id の昇順タイブレークで最初に選ばれ、throttled で失敗する。
    let adapter = Arc::new(PoolAdapter {
        terminal_or_throttled: Err(Duration::from_secs(120)),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: false,
    });
    let mut d = pool_dispatcher_with_requeues(
        store.clone(),
        adapter,
        None,
        dir.path().to_path_buf(),
        1,
        1,
        0,
    );
    // 1 tick で dispatch → 完了まで待つ。
    for _ in 0..50 {
        d.tick().unwrap();
        if !store.events_for(task1.id).unwrap().is_empty()
            && store
                .events_for(task1.id)
                .unwrap()
                .iter()
                .any(|(_, e)| matches!(e, Event::WorkerFinished { .. }))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = store.events_for(task1.id).unwrap();
    let first_account = events.iter().find_map(|(_, e)| match e {
        Event::WorkerStarted { account, .. } => account.clone(),
        _ => None,
    });
    assert_eq!(first_account.as_deref(), Some("a"));
    // プロバイダ自体は cooldown にならない（ADR-0024 D4）。
    assert!(d.policy.cooldowns(Instant::now()).is_empty());
    // `ProviderThrottled` イベントは記録されない（アカウントの cooldown として扱われるため）。
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::ProviderThrottled { .. }))
    );

    // 次に投入したタスクは、cooldown 中の "a" を避けて "b" に行く。
    let task2 = new_task(
        ws_dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task2).unwrap();
    for _ in 0..50 {
        d.tick().unwrap();
        let events2 = store.events_for(task2.id).unwrap();
        if events2
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. }))
        {
            let acct = events2.iter().find_map(|(_, e)| match e {
                Event::WorkerStarted { account, .. } => account.clone(),
                _ => None,
            });
            assert_eq!(acct.as_deref(), Some("b"));
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task2 was never dispatched to account b");
}

/// S10: `Spawn` 失敗（起動できない）はアカウントの責任ではないので、プールの run でもプロバイダを
/// cooldown にする（アカウントは cooldown にしない）。
#[tokio::test]
async fn spawn_failure_on_pool_run_cools_the_provider_not_the_account() {
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
            summary: "unused".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: captured.clone(),
        spawn_failure: true,
    });
    let mut d = pool_dispatcher_with_requeues(
        store.clone(),
        adapter,
        None,
        dir.path().to_path_buf(),
        1,
        1,
        0,
    );
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
    for _ in 0..50 {
        d.tick().unwrap();
        if store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerFinished { .. }))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = store.events_for(task.id).unwrap();
    let account = events.iter().find_map(|(_, e)| match e {
        Event::WorkerStarted { account, .. } => account.clone(),
        _ => None,
    });
    assert!(
        account.is_some(),
        "run should have used a pooled account: {events:?}"
    );
    // `ProviderThrottled` イベントが記録される（アカウントの cooldown としては扱わない）。
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::ProviderThrottled { .. }))
    );

    // プロバイダは cooldown になる。アカウント自体は cooldown にならない。
    d.tick().unwrap(); // もう 1 tick 回し、最新のスナップショットを送らせる。
    rx.changed().await.ok();
    let snapshot = rx.borrow().clone().unwrap();
    assert!(
        !snapshot.cooldowns.is_empty(),
        "provider should be cooling down: {snapshot:?}"
    );
    assert!(
        snapshot.accounts.iter().all(|a| a.cooldown.is_none()),
        "no account should be cooling down: {:?}",
        snapshot.accounts
    );
}

/// (c) プールに選べるアカウントが無ければ、プールのプロバイダは満杯として扱われ、
/// 非プールのプロバイダにフォールバックする（ADR-0012 D2、ADR-0024 D2）。
#[tokio::test]
async fn no_eligible_account_falls_back_to_a_non_pool_provider() {
    // アカウントは 1 つも作らない（ディレクトリはあるが空 = 選べるアカウント無し）。
    let dir = tempfile::tempdir().unwrap();
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
    let pool_adapter = Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "should not run".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured,
        spawn_failure: false,
    });
    let fallback_adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = pool_dispatcher(
        store.clone(),
        pool_adapter,
        Some(("p2", fallback_adapter)),
        dir.path().to_path_buf(),
        2,
        2,
    );
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Done);
    let events = store.events_for(task.id).unwrap();
    let (provider, account) = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted {
                provider, account, ..
            } => Some((provider.clone(), account.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(provider.as_deref(), Some("p2"));
    assert_eq!(account, None);
}

#[test]
fn portable_work_uses_headroom_across_pools_then_falls_back() {
    let claude = accounts_fixture();
    let codex = tempfile::tempdir().unwrap();
    std::fs::create_dir(codex.path().join("gpt")).unwrap();
    std::fs::write(codex.path().join("gpt/auth.json"), "{}").unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = pool_dispatcher(store, adapter, None, claude.path().into(), 2, 2);
    d.now_unix_fn = Arc::new(|| 10_000);
    d.config
        .accounts
        .as_mut()
        .unwrap()
        .roots
        .insert(AccountAdapter::Codex, codex.path().into());
    d.account_books.insert(
        AccountAdapter::Codex,
        Arc::new(StdMutex::new(AccountBook::new_in_memory())),
    );
    d.account_pool_providers.insert("gpt".into());
    d.policy = Box::new(StaticPolicy::new(
        [
            ("p1", "claude-code"),
            ("local", "acp"),
            ("gpt", "codex"),
            ("research", "paperqa"),
        ]
        .into_iter()
        .map(|(id, adapter)| ProviderSpec {
            id: id.into(),
            adapter: adapter.into(),
            tiers: vec![Tier::Standard],
            concurrency: 2,
            model: String::new(),
        })
        .collect(),
        Duration::from_secs(5),
    ));
    let hint = WorkerHint {
        tier: Tier::Standard,
        adapter: None,
    };
    let now = Instant::now();
    let task = TaskId::new();
    let mut full = std::collections::HashSet::new();
    for id in ["a", "b"] {
        d.record_account_check(
            AccountAdapter::ClaudeCode,
            id,
            "ok",
            None,
            Some(usage_window(0.8, 3600)),
        );
    }
    d.record_account_check(
        AccountAdapter::Codex,
        "gpt",
        "ok",
        None,
        Some(usage_window(0.2, 3600)),
    );
    assert_eq!(
        d.select_provider(&hint, now, task, &mut full, None)
            .unwrap()
            .1,
        "gpt"
    );
    assert!(
        full.is_empty(),
        "enumeration must not exclude eligible providers for other tasks"
    );
    // 同点なら設定順。Codex を優先する固定ではない。
    d.record_account_check(
        AccountAdapter::Codex,
        "gpt",
        "ok",
        None,
        Some(usage_window(0.8, 3600)),
    );
    assert_eq!(
        d.select_provider(&hint, now, task, &mut full, None)
            .unwrap()
            .1,
        "p1"
    );
    for id in ["a", "b"] {
        d.record_account_check(
            AccountAdapter::ClaudeCode,
            id,
            "ok",
            None,
            Some(usage_window(1.0, 3600)),
        );
    }
    assert_eq!(
        d.select_provider(&hint, now, task, &mut full, None)
            .unwrap()
            .1,
        "gpt"
    );
    let pinned = WorkerHint {
        tier: Tier::Standard,
        adapter: Some("claude-code".into()),
    };
    assert!(
        d.select_provider(&pinned, now, task, &mut full, None)
            .is_none()
    );
    d.record_account_check(AccountAdapter::Codex, "gpt", "auth_failed", None, None);
    assert_eq!(
        d.select_provider(&hint, now, task, &mut full, None)
            .unwrap()
            .1,
        "local"
    );
    // 再確認が成功すれば cooldown を解除して復帰する。
    d.record_account_check(
        AccountAdapter::Codex,
        "gpt",
        "ok",
        None,
        Some(usage_window(0.1, 3600)),
    );
    full.clear();
    assert_eq!(
        d.select_provider(&hint, now, task, &mut full, None)
            .unwrap()
            .1,
        "gpt"
    );
}

/// ADR-0054 Phase 67c: 継続セッションが `select_provider` の入口に渡ると、ADR-0049 のランキングが
/// 別のアカウントを勧めていても、そのセッションのアカウントに留まる（本番 2026-09-21 の事故の直接の
/// 再現: スコアの逆転だけで account_change 扱いにならないことを確かめる）。使えなくなれば
/// （cooldown）ランキングへフォールバックすることも確認する。
#[tokio::test]
async fn select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one() {
    let dir = accounts_fixture();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = pool_dispatcher(store, adapter, None, dir.path().into(), 2, 2);
    d.now_unix_fn = Arc::new(|| 10_000);
    // "b" の残量が多い（スコアが高い）ので、sticky が無ければ "b" が勝つ。
    d.record_account_check(
        AccountAdapter::ClaudeCode,
        "a",
        "ok",
        None,
        Some(usage_window(0.8, 3600)),
    );
    d.record_account_check(
        AccountAdapter::ClaudeCode,
        "b",
        "ok",
        None,
        Some(usage_window(0.1, 3600)),
    );
    let hint = WorkerHint {
        tier: Tier::Standard,
        adapter: None,
    };
    let now = Instant::now();
    let task = TaskId::new();

    // ランキングだけなら "b" が勝つ（前提の確認）。
    let mut full = std::collections::HashSet::new();
    assert_eq!(
        d.select_provider(&hint, now, task, &mut full, None)
            .unwrap()
            .2,
        Some((AccountAdapter::ClaudeCode, "b".to_string()))
    );

    let active = NodeSession::new(
        task_core::COS_ID,
        SessionKind::Conversation,
        None,
        "claude-code",
        Some("a".to_string()),
        "550e8400-e29b-41d4-a716-446655440000",
        OffsetDateTime::now_utc(),
    );

    // 継続セッションが "a" にあれば、"b" の方がスコアが高くても "a" に留まる。
    let mut full = std::collections::HashSet::new();
    let (adapter_id, provider_id, selected_account) = d
        .select_provider(&hint, now, task, &mut full, Some(&active))
        .unwrap();
    assert_eq!(adapter_id, "claude-code");
    assert_eq!(provider_id, "p1");
    assert_eq!(
        selected_account,
        Some((AccountAdapter::ClaudeCode, "a".to_string()))
    );

    // "a" が cooldown（例: 認証失敗）に落ちたら、留まれないので通常のランキング（"b"）へ戻る。
    d.record_account_check(AccountAdapter::ClaudeCode, "a", "auth_failed", None, None);
    let mut full = std::collections::HashSet::new();
    let (_, _, selected_account) = d
        .select_provider(&hint, now, task, &mut full, Some(&active))
        .unwrap();
    assert_eq!(
        selected_account,
        Some((AccountAdapter::ClaudeCode, "b".to_string())),
        "unusable account must fall back to the normal ranking"
    );
}

/// (e) 帳簿（`AccountBook`）はファイルに保存され、celeris の再起動（新しい `Dispatcher`）後も残る。
#[tokio::test]
async fn account_book_is_persisted_and_reloaded_after_restart() {
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
        delay: Duration::ZERO,
        observation: Some(usage_window(0.33, 90_000)),
        env: Vec::new(),
        captured,
        spawn_failure: false,
    });
    let mut d1 = pool_dispatcher(store.clone(), adapter, None, dir.path().to_path_buf(), 2, 2);
    let report = run_until_idle(&mut d1, 200).await;
    assert!(report.idle);
    assert!(dir.path().join(".celeris-usage.json").exists());
    drop(d1);

    // "celeris を再起動" = 新しい Dispatcher（同じ store・同じ accounts root）を作る。
    let never_used = Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "unused".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::ZERO,
        observation: None,
        env: Vec::new(),
        captured: Arc::new(StdMutex::new(Vec::new())),
        spawn_failure: false,
    });
    let mut d2 = pool_dispatcher(
        store.clone(),
        never_used,
        None,
        dir.path().to_path_buf(),
        2,
        2,
    );
    let (tx, mut rx) = tokio::sync::watch::channel(None);
    d2.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst2".into(),
        hostname: "h".into(),
        started_at: "t".into(),
        tick_ms: 1,
        providers: Vec::new(),
        provider_checks: HashMap::new(),
    });
    d2.tick().unwrap();
    rx.changed().await.ok();
    let snapshot = rx.borrow().clone().unwrap();
    let used_account = snapshot.accounts.iter().find(|a| a.usage.is_some());
    let usage = used_account
        .expect("observation survives restart")
        .usage
        .as_ref()
        .unwrap();
    assert_eq!(usage.five_hour.map(|w| w.utilization), Some(0.33));
}

#[tokio::test]
async fn routing_applies_quota_tier_and_explicit_account_to_the_executed_model() {
    use task_core::model_routing::ModelBinding;
    for (utilization, expected, known) in [
        (0.1, "frontier-id", true),
        (0.8, "standard-id", true),
        (0.95, "cheap-id", true),
        (0.8, "frontier-id", false),
    ] {
        let accounts = accounts_fixture();
        let mut book = AccountBook::load(&accounts.path().join(".celeris-usage.json"));
        let mut obs = usage_window(utilization, 90_000);
        obs.seven_day = if known { obs.five_hour } else { None };
        book.record_observation("a", obs, ObservationSource::Run);
        book.save().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let mut task = new_task(
            ws.path(),
            Check::Command {
                cmd: "test -f touched".into(),
                expect_exit: 0,
            },
            0,
        );
        task.worker_hint.tier = Tier::Frontier;
        store.insert(&task).unwrap();
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let adapter = Arc::new(task_worker::tiered::TieredAdapter {
            base: Arc::new(PoolAdapter {
                terminal_or_throttled: Ok(Terminal::Done {
                    summary: "ok".into(),
                    evidence: vec![],
                    usage: None,
                }),
                delay: Duration::ZERO,
                observation: None,
                env: vec![],
                captured: captured.clone(),
                spawn_failure: false,
            }),
            models: [
                (Tier::Frontier, "frontier-id"),
                (Tier::Standard, "standard-id"),
                (Tier::Cheap, "cheap-id"),
            ]
            .into_iter()
            .map(|(tier, id)| {
                (
                    tier,
                    ModelBinding {
                        name: id.into(),
                        model_id: Some(id.into()),
                        unavailable_reason: None,
                        reasoning_effort: None,
                    },
                )
            })
            .collect(),
            account_id: Some("a".into()),
            credential_error: None,
        });
        let mut d = pool_dispatcher(
            store.clone(),
            adapter,
            None,
            accounts.path().to_path_buf(),
            2,
            2,
        );
        d.set_now_unix_fn(Arc::new(|| 10_000));
        assert!(run_until_idle(&mut d, 200).await.idle);
        let envs = captured.lock().unwrap();
        assert_eq!(envs.len(), 1);
        assert!(
            envs[0].contains(&("TEST_MODEL".into(), expected.into())),
            "{envs:?}"
        );
        let events = store.events_for(task.id).unwrap();
        assert!(events.iter().any(|(_,e)| matches!(e,Event::WorkerStarted {model,account,..} if model == expected && account.as_deref() == Some("a"))));
        assert!(events.iter().any(|(_,e)| matches!(e,Event::WorkerProgress {msg,..} if msg.contains(if known { "measured quota remaining" } else { "quota remaining unknown" }))));
    }
}

#[tokio::test]
async fn unavailable_tier_blocks_before_starting_any_worker() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(ws.path(), Check::Human, 0);
    store.insert(&task).unwrap();
    let adapter = Arc::new(task_worker::tiered::TieredAdapter {
        base: Arc::new(InstantAdapter {
            terminal: Terminal::Question {
                text: "must not run".into(),
            },
            delay: Duration::ZERO,
        }),
        models: [(
            task.worker_hint.tier,
            task_core::model_routing::ModelBinding {
                name: "fable".into(),
                model_id: None,
                unavailable_reason: Some("unverified executable ID".into()),
                reasoning_effort: None,
            },
        )]
        .into(),
        account_id: None,
        credential_error: None,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.tick().unwrap();
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    let events = store.events_for(task.id).unwrap();
    assert!(
        !events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { .. }))
    );
    assert!(events.iter().any(|(_, e)| matches!(e, Event::WorkerProgress { msg, .. } if msg.contains("unverified executable ID"))));
}

/// ADR-0069 D3 / D5: `routing` を持つ execute タスクは、LLM のヒント（frontier）ではなく
/// TaskFeatures の規則表で lane が決まり（機械的・検証可能・戻せる → cheap）、その lane の model が
/// 走り、`RoutingDecided` に features・規則・版・reasoning effort が残る。
#[tokio::test]
async fn lane_policy_decides_the_tier_and_records_the_routing_decision() {
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
    task.worker_hint.tier = Tier::Frontier;
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Hint,
        ..Default::default()
    });
    store.insert(&task).unwrap();
    let mut d = dispatcher(store.clone(), three_lane_adapter(), 1);
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerStarted { model, .. } if model == "cheap-id")),
        "{events:?}"
    );
    let record = routing_record(&events).expect("routing_decided");
    assert_eq!(record.decision.lane, Tier::Cheap);
    assert_eq!(record.decision.hint, Some(Tier::Frontier));
    assert_eq!(
        record.decision.rule_id,
        "cheap/mechanical-verifiable-reversible"
    );
    assert_eq!(
        record.decision.policy_version,
        task_core::LANE_POLICY_VERSION
    );
    assert_eq!(record.harness.as_deref(), Some("coding"));
    assert_eq!(record.resolution.model_id, "cheap-id");
    // ADR-0069 Phase 118 D1: この記録は「設定した」値ではなく「実際に CLI へ渡った」値。
    // `InstantAdapter`（このテストの基盤アダプタ）は `WorkerAdapter::supports_reasoning_effort`
    // の既定（`false`）のままなので `None`（`crates/task-worker/src/codex.rs` の
    // `tier_reasoning_effort_reaches_cli_as_a_dash_c_config_override` が実際に codex へ渡る
    // ことを別途検証する）。
    assert_eq!(record.resolution.reasoning_effort, None);
    // 監査の集計に乗る。
    let audit = task_ops::routing_audit::task_routing_audit(store.as_ref(), task.id).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].lane, Some(Tier::Cheap));
    assert_eq!(
        audit[0].rule_id.as_deref(),
        Some("cheap/mechanical-verifiable-reversible")
    );

    // 人の明示 tier は policy が触らない。`routing` の無い既存タスクは記録も出さない。
    let store2: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut human = task.clone();
    human.id = TaskId::new();
    human.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Human,
        ..Default::default()
    });
    store2.insert(&human).unwrap();
    let mut legacy = task.clone();
    legacy.id = TaskId::new();
    legacy.routing = None;
    store2.insert(&legacy).unwrap();
    let mut d2 = dispatcher(store2.clone(), three_lane_adapter(), 2);
    d2.tick().unwrap();
    let h = store2.events_for(human.id).unwrap();
    assert!(
        h.iter().any(
            |(_, e)| matches!(e, Event::WorkerStarted { model, .. } if model == "frontier-id")
        )
    );
    assert_eq!(
        routing_record(&h).map(|r| r.decision.rule_id),
        Some("explicit/human".to_string())
    );
    let l = store2.events_for(legacy.id).unwrap();
    assert!(
        l.iter().any(
            |(_, e)| matches!(e, Event::WorkerStarted { model, .. } if model == "frontier-id")
        )
    );
    assert!(routing_record(&l).is_none());
}

/// ADR-0069 D6: 同じ lane でレビュー不合格が 2 回続いたタスクのやり直しは 1 段だけ上がり
/// （cheap → standard）、理由が `RoutingDecided.decision.escalation` に残る。
#[tokio::test]
async fn repeated_review_failures_escalate_the_retry_lane_one_step() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        ws.path(),
        Check::Command {
            cmd: "cargo test".into(),
            expect_exit: 0,
        },
        3,
    );
    task.objective = "crates/task-core/src/model.rs の typo を直す".into();
    task.genre = Some("coding".into());
    task.routing = Some(task_core::TaskRouting::default());
    task.attempts = 2;
    task.updated_at = OffsetDateTime::now_utc() - time::Duration::days(1);
    store.insert(&task).unwrap();
    let decision = task_core::model_policy::decide_for_task(&task, &Default::default()).unwrap();
    assert_eq!(decision.lane, Tier::Cheap);
    for run in ["r1", "r2"] {
        let record = task_core::RoutingRecord {
            org_node: None,
            harness: Some("coding".into()),
            decision: decision.clone(),
            resolution: task_core::model_routing::LaneResolution {
                lane: Some(Tier::Cheap),
                ..Default::default()
            },
            quota_reason: None,
            work_unit_id: None,
        };
        for event in [
            Event::RoutingDecided {
                run_id: run.into(),
                record: Box::new(record),
            },
            Event::ReviewVerdict {
                run_id: format!("{run}-review"),
                criterion_idx: 0,
                pass: false,
                reason: "tests fail".into(),
            },
            Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Ready,
                reason: "review_fail".into(),
            },
        ] {
            store.append_event(task.id, &event).unwrap();
        }
    }
    let mut d = dispatcher(store.clone(), three_lane_adapter(), 1);
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, Event::WorkerStarted { model, .. } if model == "standard-id")
        ),
        "{events:?}"
    );
    let record = routing_record(&events).expect("routing_decided");
    assert_eq!(record.decision.lane, Tier::Standard);
    assert_eq!(record.decision.proposed, Tier::Cheap);
    assert!(
        record
            .decision
            .escalation
            .as_deref()
            .is_some_and(|e| e.starts_with("escalate Cheap -> Standard")),
        "{:?}",
        record.decision.escalation
    );
}

/// `skills_context` は KB にある skill を `SkillMount` に解決し、無い名前は 2 つ目の戻り値
/// （`missing`）に回す（run は落とさない）。
#[test]
fn skills_context_resolves_mounted_skills_and_reports_missing_ones() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = dispatcher(store, adapter, 1);
    let kb = tempfile::tempdir().unwrap();
    d.config.knowledge.root = kb.path().to_path_buf();
    write_kb_skill(kb.path(), "writing", "文章の書き方", "本文");

    let (skills, missing) = d.skills_context(&["writing".to_string(), "ghost".to_string()]);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "writing");
    assert_eq!(skills[0].description, "文章の書き方");
    assert_eq!(
        skills[0].path,
        kb.path()
            .join("skills")
            .join("writing")
            .display()
            .to_string()
    );
    assert_eq!(missing, vec!["ghost".to_string()]);
}

/// ADR-0046 D1（継承）+ ADR-0056 D3: `run_extras` は担当ノードの実効 profile が継いだ
/// `skills_mounts`（親と子の和、重複は落ちる）を KB から解決して `RunExtras.skills` に積む。
#[test]
fn run_extras_resolves_the_assigned_nodes_effective_skills_mounts() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let now = OffsetDateTime::now_utc();
    store
        .org_upsert(&OrgNode {
            id: "cos".into(),
            parent_id: None,
            name: "cos".into(),
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            profile: task_core::Profile {
                skills_mounts: vec!["writing".into()],
                ..task_core::Profile::default()
            },
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    store
        .org_upsert(&OrgNode {
            id: "engineering".into(),
            parent_id: Some("cos".into()),
            name: "engineering".into(),
            kind: OrgKind::Department,
            genre: None,
            brief: String::new(),
            // `writing` は親と重複（落ちる）、`ghost` は KB に無い（missing に回る）。
            profile: task_core::Profile {
                skills_mounts: vec!["rust-review".into(), "writing".into(), "ghost".into()],
                ..task_core::Profile::default()
            },
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let mut task = new_task(dir.path(), Check::Human, 0);
    task.assignee = Some("engineering".into());
    store.insert(&task).unwrap();

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = dispatcher(store, adapter, 1);
    let kb = tempfile::tempdir().unwrap();
    d.config.knowledge.root = kb.path().to_path_buf();
    write_kb_skill(kb.path(), "writing", "d", "body");
    write_kb_skill(kb.path(), "rust-review", "d", "body");

    let extras = d.run_extras(&task, None, None, "claude-code").unwrap();
    let names: Vec<&str> = extras.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["writing", "rust-review"],
        "根→葉の和で重複が落ちる: {names:?}"
    );
    assert_eq!(extras.missing_skills, vec!["ghost".to_string()]);
}

/// 実際の dispatch（`run_until_idle`）で `RunContext.skills` がアダプタに届き、mount 名にあったが
/// KB に無かった skill は `status` の進行イベントを 1 行残すだけで run を失敗させない。
#[tokio::test]
async fn dispatch_delivers_mounted_skills_and_reports_missing_ones_without_failing_the_run() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let now = OffsetDateTime::now_utc();
    store
        .org_upsert(&OrgNode {
            id: "engineering".into(),
            parent_id: None,
            name: "engineering".into(),
            // 根は secretary だけ（ADR-0033 D1）。skills mount の解決そのものは対話・非対話を
            // 区別しないので、根で 1 段だけの組織にする。
            kind: OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            profile: task_core::Profile {
                skills_mounts: vec!["writing".into(), "ghost".into()],
                ..task_core::Profile::default()
            },
            position: 0,
            created_at: now,
            updated_at: now,
        })
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let mut task = new_task(dir.path(), Check::Human, 0);
    task.assignee = Some("engineering".into());
    store.insert(&task).unwrap();

    let seen = Arc::new(StdMutex::new(None));
    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        seen: seen.clone(),
        memory: None,
        proposals: Vec::new(),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let kb = tempfile::tempdir().unwrap();
    d.config.knowledge.root = kb.path().to_path_buf();
    write_kb_skill(kb.path(), "writing", "d", "body");

    assert!(run_until_idle(&mut d, 20).await.idle);
    let context = seen.lock().unwrap().clone().expect("the adapter ran");
    assert_eq!(context.skills.len(), 1);
    assert_eq!(context.skills[0].name, "writing");

    let events = store.events_for(task.id).unwrap();
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerProgress { msg, kind, .. }
                if msg == "skill ghost not found" && *kind == Some(task_core::ProgressKind::Status)
        )),
        "{events:?}"
    );
    let status = store.get(task.id).unwrap().unwrap().status;
    assert!(
        matches!(status, Status::Reviewing | Status::Done),
        "a missing skill mount must not fail the run: {status:?}"
    );
}

/// ADR-0074 §6 F1 (d): planner run は既定で `standard` lane（`rule_id = planner/system-standard`）、
/// `[execution.planner] max_turns`/`max_wall_secs`（テストでは既定 24/900）が予算に反映される。
/// ADR-0076: planner / Reviewer run もアカウントプールの run なら `QuotaActivity` を通り、終了で
/// `Event::QuotaEstimated` を残す（worker と同じ）。終わった後に開いたままの run は残らず、
/// `ExecutionMetrics.quota` の `runs_by_role` に worker / planner / reviewer が現れる。
#[tokio::test]
async fn planner_and_reviewer_runs_emit_quota_estimates() {
    struct RolesAdapter;
    #[async_trait]
    impl WorkerAdapter for RolesAdapter {
        fn id(&self) -> &str {
            "claude-code"
        }
        async fn run(
            &self,
            req: RunRequest,
            _run_id: &str,
            _limits: RunLimits,
            _sink: &dyn EventSink,
        ) -> Result<RunOutcome, AdapterError> {
            std::fs::create_dir_all(&req.artifacts_dir).unwrap();
            if req.context.execution_planner.is_some() {
                std::fs::write(
                    req.artifacts_dir.join("execution-plan.json"),
                    plan_json(vec![wu_spec("a", &[])]),
                )
                .unwrap();
            } else if req.task.kind == TaskKind::Review {
                std::fs::write(
                    req.artifacts_dir.join("review.json"),
                    r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#,
                )
                .unwrap();
            }
            Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: "ok".into(),
                    evidence: vec![],
                    usage: Some(task_core::Usage {
                        input_tokens: Some(1000),
                        output_tokens: Some(100),
                        cache_read_tokens: None,
                        cache_creation_tokens: None,
                        cost_usd: None,
                        duplicate_reads: None,
                        session_resumed: None,
                    }),
                },
                exit_code: Some(0),
            })
        }
        // アカウントプールは選んだアカウントの env を `with_env` で足す（`None` だと選べない）。
        fn with_env(&self, _extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
            Some(Arc::new(RolesAdapter))
        }
    }

    let accounts = accounts_fixture();
    {
        let mut book = AccountBook::load(&accounts.path().join(".celeris-usage.json"));
        book.record_observation("a", usage_window(0.2, 90_000), ObservationSource::Run);
        book.record_observation("b", usage_window(0.1, 90_000), ObservationSource::Run);
        book.save().unwrap();
    }
    let ws_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = compound_task(ws_dir.path());
    task.acceptance = vec![task_core::Criterion {
        text: "reviewed".into(),
        check: Check::Reviewer,
    }];
    let task_id = task.id;
    store.insert(&task).unwrap();
    let mut d = pool_dispatcher(
        store.clone(),
        Arc::new(RolesAdapter),
        None,
        accounts.path().to_path_buf(),
        2,
        2,
    );
    d.set_now_unix_fn(Arc::new(|| 10_000));
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "claude-code".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");

    let events: Vec<Event> = store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let started: Vec<(String, Option<RunRole>, Option<String>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorkerStarted {
                run_id,
                role,
                account,
                ..
            } => Some((run_id.clone(), *role, account.clone())),
            _ => None,
        })
        .collect();
    for want in [Some(RunRole::Planner), Some(RunRole::Reviewer), None] {
        let (run_id, _, account) = started
            .iter()
            .find(|(_, role, _)| *role == want)
            .unwrap_or_else(|| panic!("a {want:?} run: {started:?}"));
        let account = account.as_deref().expect("pool account selected");
        let quota = events.iter().find_map(|e| match e {
            Event::QuotaEstimated {
                run_id: r,
                account,
                weighted_tokens,
                ..
            } if r == run_id => Some((account.clone(), *weighted_tokens)),
            _ => None,
        });
        let (quota_account, weighted) =
            quota.unwrap_or_else(|| panic!("QuotaEstimated for {want:?} run {run_id}"));
        assert_eq!(quota_account.as_deref(), Some(account));
        assert!(weighted > 0.0, "usage is weighted for {want:?}");
        assert!(
            !d.quota_activity
                .is_tracked(AccountAdapter::ClaudeCode, account, run_id),
            "{want:?} run {run_id} must not stay open in QuotaActivity"
        );
    }

    let metrics = task_core::execution_metrics::summarize(&stored, &events);
    let mut by_role: std::collections::BTreeMap<String, u32> = Default::default();
    for row in metrics
        .quota
        .iter()
        .filter(|r| r.window == task_core::QuotaWindow::FiveHour)
    {
        for (role, n) in &row.runs_by_role {
            *by_role.entry(role.clone()).or_insert(0) += n;
        }
    }
    assert_eq!(by_role.get("planner"), Some(&1), "{metrics:?}");
    assert_eq!(by_role.get("reviewer"), Some(&1), "{metrics:?}");
    assert!(
        by_role.get("worker").copied().unwrap_or(0) >= 1,
        "{metrics:?}"
    );
}

#[tokio::test]
async fn planner_run_uses_the_standard_lane_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = compound_task(dir.path());
    let task_id = task.id;
    store.insert(&task).unwrap();

    let valid_plan = plan_json(vec![wu_spec("a", &[])]);
    let adapter = Arc::new(PlannerScriptAdapter::new(
        vec![Some(valid_plan)],
        HashMap::new(),
    ));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let events = store.events_for(task_id).unwrap();
    let planner_record = events
        .iter()
        .find_map(|(_, e)| match e {
            Event::RoutingDecided { record, .. }
                if record.decision.rule_id.starts_with("planner/") =>
            {
                Some((**record).clone())
            }
            _ => None,
        })
        .expect("a planner RoutingDecided event");
    assert_eq!(
        planner_record.decision.lane,
        Tier::Standard,
        "{planner_record:?}"
    );
    assert_eq!(planner_record.decision.rule_id, "planner/system-standard");
    assert_eq!(
        planner_record.decision.source,
        task_core::TierSource::System
    );
}
