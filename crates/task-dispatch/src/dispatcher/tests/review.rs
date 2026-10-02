use super::*;

#[tokio::test]
async fn draining_worker_completion_leaves_review_to_the_active_dispatcher() {
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
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut old = dispatcher(store.clone(), adapter.clone(), 1);
    assert_eq!(old.tick().unwrap().dispatched, 1);
    old.set_accepting_new_work(false);
    for _ in 0..100 {
        old.tick().unwrap();
        if old.in_flight() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(old.in_flight(), 0);
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    assert!(old.reviewing.is_empty());
    let mut active = dispatcher(store.clone(), adapter, 1);
    assert!(run_until_idle(&mut active, 100).await.idle);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
}

#[tokio::test]
async fn overlapping_dispatchers_share_review_ownership_until_verdict_is_saved() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: ":".into(),
            expect_exit: 0,
        },
        0,
    );
    task.status = Status::Reviewing;
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut old = dispatcher(store.clone(), adapter.clone(), 1);
    let mut active = dispatcher(store.clone(), adapter, 1);
    assert!(
        old.spawn_review(task.id, "subject".into(), &ReviewSubject::default())
            .unwrap()
    );
    old.set_accepting_new_work(false);
    assert!(
        !active
            .spawn_review(task.id, "subject".into(), &ReviewSubject::default())
            .unwrap()
    );
    // The completed review keeps its lock until the dispatcher persists the verdict.
    // Await the actual review task, leaving its completion queued and verdict unsaved.
    (&mut old.reviewing.get_mut(&task.id).unwrap().handle)
        .await
        .unwrap();
    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Reviewing
    );
    assert!(
        !store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, event)| matches!(event, Event::ReviewVerdict { .. }))
    );
    assert!(old.reviewing.contains_key(&task.id));
    assert!(
        !active
            .spawn_review(task.id, "subject".into(), &ReviewSubject::default())
            .unwrap()
    );
    let report = old.tick().unwrap();
    assert_eq!(report.reviewed, 1);
    assert!(report.idle);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    active.tick().unwrap();
    assert!(active.reviewing.is_empty());
    assert_eq!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .filter(|(_, e)| matches!(e, Event::ReviewVerdict { .. }))
            .count(),
        1
    );
    // A later explicit review can reuse the lock; no stale lock file blocks recovery.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(
            dir.path()
                .join("runs")
                .join(format!(".review-{}.lock", task.id)),
        )
        .unwrap();
    file.try_lock().unwrap();
}

#[tokio::test]
async fn done_then_command_review_passes_and_events_are_recorded() {
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
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![Evidence {
                criterion: 0,
                command: Some("x".into()),
                exit: Some(0),
                stdout_tail: None,
            }],
            usage: None,
        },
        delay: Duration::from_millis(10),
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Done);
    assert_eq!(t.attempts, 0);
    assert!(t.lease.is_none());
    let kinds: Vec<String> = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| match e {
            Event::Transitioned { from, to, reason } => format!("{from:?}->{to:?}:{reason}"),
            Event::WorkerStarted { .. } => "started".into(),
            Event::WorkerProgress { .. } => "progress".into(),
            Event::WorkerFinished { outcome, .. } => format!("finished:{outcome}"),
            Event::ReviewVerdict { pass, .. } => format!("verdict:{pass}"),
            // ADR-0074 D4（Phase F3 quota）: `finish_worker_result` が毎 run 出す
            // `QuotaEstimated`（このテストの provider `p1` はアカウントプールを使わないので
            // `free`）。中身は quota 専用のテストで確かめるので、ここでは種別だけを見る。
            Event::QuotaEstimated { .. } => "quota_estimated".into(),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "Ready->Running:dispatch",
            "started",
            "progress",
            "Running->Reviewing:worker_done",
            "finished:done: ok",
            "quota_estimated",
            "Reviewing->Done:review_pass",
            "verdict:true",
        ]
    );
}

/// Phase 44（実機 2026-09-18）: 委譲した子タスクが失敗し `retry_then_ask`（max_retries 到達）で親が
/// `blocked` に落ちる質問は、ワーカーの `Question` ではなくディスパッチャ自身が立てるものだが、
/// Phase 26 と同じく `approvals` にも 1 件残る（そうしないと認可画面に出ず、`approval_pending` の
/// Discord 通知も飛ばない）。答えれば（`Trigger::Answer`）親は `ready` に戻る。
#[tokio::test]
async fn a_dispatcher_raised_child_failure_question_also_becomes_an_approval() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org_for_reports(&store);

    // 子: 委譲され、走って失敗する（max_retries = 0 なので 1 回で failed）。
    let mut parent = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    parent.status = Status::Reviewing;
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
    let handled = d
        .escalate_failed_children(&parent, "run-1", &mut Vec::new())
        .unwrap();
    assert!(handled);
    let after = store.get(parent.id).unwrap().unwrap();
    assert_eq!(
        after.status,
        Status::Blocked,
        "max_retries = 0 なのでやり直せず、人に聞く"
    );

    // ADR-0033 D5（Phase 26）と同じく `approvals` に 1 件残る。
    let approvals = store.approval_list(Some(true), None, None).unwrap();
    assert_eq!(approvals.len(), 1, "{approvals:?}");
    assert_eq!(approvals[0].node_id, "coding-poc");
    assert_eq!(approvals[0].task_id, Some(parent.id));
    assert!(
        approvals[0].question.contains("委譲した子タスクが失敗し"),
        "{}",
        approvals[0].question
    );

    // 答えれば ready に戻る（既存の answers[] の経路。Phase 29 の一本化はここでは検証しない）。
    store
        .apply_transition(parent.id, Trigger::Answer, None)
        .unwrap();
    assert_eq!(store.get(parent.id).unwrap().unwrap().status, Status::Ready);
}

#[tokio::test]
async fn question_blocks_task_and_review_fail_retries_until_budget() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let q = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&q).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "which?".into(),
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 100).await;
    assert!(report.idle);
    assert_eq!(store.get(q.id).unwrap().unwrap().status, Status::Blocked);

    // レビュー失敗（存在しないファイル）は max_retries=1 で 2 回実行して failed。
    let dir2 = tempfile::tempdir().unwrap();
    let store2: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let f = new_task(
        dir2.path(),
        Check::Command {
            cmd: "test -f never".into(),
            expect_exit: 0,
        },
        1,
    );
    store2.insert(&f).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "claimed".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d2 = dispatcher(store2.clone(), adapter, 1);
    let report = run_until_idle(&mut d2, 200).await;
    assert!(report.idle);
    let t = store2.get(f.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Failed);
    assert_eq!(t.attempts, 2);
    let events = store2.events_for(f.id).unwrap();
    let starts = events
        .iter()
        .filter(|(_, e)| matches!(e, Event::WorkerStarted { .. }))
        .count();
    assert_eq!(starts, 2);
    // 2 回目の run には 1 回目のレビュー結果が prior_review として渡る。
    let prior = prior_review_from_events(&events[..events.len() - 2]);
    assert_eq!(prior.len(), 1);
    assert!(!prior[0].pass);
}

#[tokio::test]
async fn invalid_plan_is_retried_with_prior_review_then_children_auto_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let plan = plan_task(dir.path(), 1);
    store.insert(&plan).unwrap();
    // ADR-0067 D2: `human` チェックには artifacts か知識ベースの参照が要る（この plan.json は
    // `depends_on` の範囲外エラーだけを狙っているので、`human` 側の違反を出さないよう
    // `artifact_exists` を添えておく）。
    let adapter = Arc::new(FileAdapter {
        plan_json: r#"{"tasks":[{"title":"a","objective":"o","acceptance":[{"text":"c","check":{"type":"human"}},{"text":"d","check":{"type":"artifact_exists","name":"result.md"}}],"depends_on":[9]}]}"#.into(),
        review_json: String::new(),
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    d.config.plan_auto_accept = true;
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    let p = store.get(plan.id).unwrap().unwrap();
    assert_eq!(p.status, Status::Done);
    assert_eq!(p.attempts, 1);
    let events = store.events_for(plan.id).unwrap();
    let verdicts: Vec<(bool, String)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ReviewVerdict { pass, reason, .. } => Some((*pass, reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts.len(), 2);
    assert!(
        !verdicts[0].0 && verdicts[0].1.contains("out of range"),
        "{:?}",
        verdicts[0]
    );
    assert!(verdicts[1].0);
    // auto_accept=true: 子は ready で挿入され、その後 done まで進む（a, b は Command、c は Reviewer で review.json 無し→ fail → failed）。
    let children: Vec<Task> = store
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|t| t.parent_id == Some(plan.id))
        .collect();
    assert_eq!(children.len(), 3);
    for c in &children {
        let ev = store.events_for(c.id).unwrap();
        assert!(matches!(&ev[0].1, Event::Created { task, .. } if task.status == Status::Draft));
        assert!(
            matches!(&ev[1].1, Event::Transitioned { from: Status::Draft, to: Status::Ready, reason } if reason == "accept")
        );
    }
    let by_title = |t: &str| {
        children
            .iter()
            .find(|c| c.title == t)
            .map(|c| store.get(c.id).unwrap().unwrap())
            .unwrap()
    };
    assert_eq!(by_title("a").status, Status::Done);
    assert_eq!(by_title("b").status, Status::Done);
    let c = by_title("c");
    assert_eq!(
        c.status,
        Status::Failed,
        "{:?}",
        store.events_for(c.id).unwrap()
    );
    assert!(store.events_for(c.id).unwrap().iter().any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: false, reason, .. } if reason.contains("review.json"))));
}

/// ADR-0074 §6 F1 (k): reviewer run の `runs` 索引の `usage` が欠けない（以前は
/// `finish_reviewer_run_index` が常に `usage: None` を書いていた）。`celerisctl replay --check`
/// と同じ突き合わせ（events から再構築した `runs` と、store に確定した `runs` の diff）で
/// 食い違いが無いことを確かめる（E6 report 問題 3 の fixture）。
#[tokio::test]
async fn reviewer_run_usage_is_recorded_in_the_runs_index_and_survives_replay_check() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    let task_id = task.id;
    store.insert(&task).unwrap();
    let adapter = Arc::new(ReviewerUsageAdapter {
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);

    let stored_runs = store.runs_for_task(task_id).unwrap();
    let reviewer_row = stored_runs
        .iter()
        .find(|r| r.role == task_core::RunIndexRole::Reviewer)
        .expect("a reviewer run in the runs index");
    assert!(
        reviewer_row.usage.is_some(),
        "reviewer run usage must not be dropped: {reviewer_row:?}"
    );
    assert_eq!(
        reviewer_row.usage.as_ref().unwrap().input_tokens,
        Some(1000)
    );
    assert!(reviewer_row.finished_at.is_some());

    // `replay --check` と同じ突き合わせ（`task_ops::replay::diff_execution`）: events だけから
    // 再構築した runs と、store に確定した runs が reviewer run について食い違わない。
    // （WU の無いこの atomic Task では `work_units` は両方とも空でよい。worker run 自身の
    // `seq` は本 Phase の対象外の別の既知のずれ〈E2b の run_index_start が
    // `current_run_seq` を二重に +1 している〉があるため、ここでは reviewer run だけを見る。
    // PROGRESS.md に申し送り済み）。
    let event_rows = store.event_rows_for(task_id, None, usize::MAX).unwrap();
    let (_, replayed_runs) = task_ops::replay::rebuild_work_units_and_runs(task_id, &event_rows);
    let stored_units = store.work_units_for(task_id).unwrap();
    let reviewer_only = |runs: &[task_core::RunRow]| -> Vec<task_core::RunRow> {
        runs.iter()
            .filter(|r| r.role == task_core::RunIndexRole::Reviewer)
            .cloned()
            .collect()
    };
    let (wu_mismatches, run_mismatches) = task_ops::replay::diff_execution(
        task_id,
        &stored_units,
        &stored_units,
        &reviewer_only(&replayed_runs),
        &reviewer_only(&stored_runs),
    );
    assert!(wu_mismatches.is_empty(), "{wu_mismatches:?}");
    assert!(run_mismatches.is_empty(), "{run_mismatches:?}");
}

#[tokio::test]
async fn reviewer_run_shares_concurrency_and_is_deferred_when_at_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // 並列度 1: 実行中のワーカーがいる間は Reviewer run を開始できず、reviewing のまま待つ。
    let r = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&r).unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        delay: Duration::from_millis(150),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let first = d.tick().unwrap();
    assert_eq!(first.dispatched, 1);
    // ワーカーが終わるのを待ってから、次の tick で reviewing に入る。
    tokio::time::sleep(Duration::from_millis(250)).await;
    // 2 つ目のタスクを ready にしておき、Reviewer run が枠を取っている間は dispatch されないことを見る。
    let other = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&other).unwrap();
    let second = d.tick().unwrap();
    assert_eq!(second.finished, 1);
    assert_eq!(store.get(r.id).unwrap().unwrap().status, Status::Reviewing);
    assert_eq!(second.dispatched, 0, "reviewer run occupies the only slot");
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    assert_eq!(store.get(r.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(store.get(other.id).unwrap().unwrap().status, Status::Done);
}

/// ADR-0054 D1（Phase 67）: 部署の根ノード（`department_of` が返す id）は、Reviewer run のたびに
/// **同じ継続セッション**（`kind = lead`）を使う。1 本目の run で新規セッション（`turns = 1`）ができ、
/// 2 本目の run では **同じ `session_id` のまま** `turns = 2` に進む（`--resume` 相当の継続）。
/// アダプタが継続に対応する id（`claude-code`）のときだけの経路（`crate::sessions::adapter_supports_sessions`）。
#[tokio::test]
async fn department_reviewer_runs_share_and_continue_one_lead_session() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let policy = StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            // ADR-0054 D1: セッション継続に対応するアダプタ id（`crate::sessions::SUPPORTED_ADAPTERS`）。
            adapter: "claude-code".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: 2,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    );
    let file_adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        delay: Duration::from_millis(5),
    });
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert("p1".into(), file_adapter);
    let mut d = Dispatcher::new(
        store.clone(),
        Box::new(policy),
        HashMap::from([("p1".to_string(), "m".to_string())]),
        adapters,
        std::collections::HashSet::new(),
        DispatchConfig {
            delivery: Default::default(),
            max_concurrency: 2,
            lease_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
            review_timeout: Duration::from_secs(5),
            workspace_root: PathBuf::from("/nonexistent"),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: crate::review::reviewer_hint(),
            reviewer_tier_override: None,
            clusters: HashMap::new(),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues: 5,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 5120,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: DelegationLimits::default(),
            accounts: None,
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: ContainersRuntimeConfig::default(),
            knowledge: KnowledgeRuntimeConfig::default(),
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution: ExecutionConfig::default(),
        },
    );

    // 1 本目: `coding-poc`（セクション）の Reviewer run。部署は `department_of` で `coding` に解決する。
    let mut first = new_task(dir.path(), Check::Reviewer, 0);
    first.assignee = Some("coding-poc".into());
    store.insert(&first).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(first.id).unwrap().unwrap().status, Status::Done);

    let after_first = store
        .node_session_active("coding", SessionKind::Lead, None)
        .unwrap()
        .expect("a lead session exists for the department after the first review");
    assert_eq!(after_first.adapter, "claude-code");
    assert!(
        !after_first.session_id.is_empty(),
        "claude-code sessions get a celeris-assigned id up front"
    );
    assert_eq!(after_first.turns, 1);

    // 2 本目: 別のタスクだが同じ部署。セッションは**続く**（同じ id、turns が進む）。
    let mut second = new_task(dir.path(), Check::Reviewer, 0);
    second.assignee = Some("coding-poc".into());
    store.insert(&second).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(second.id).unwrap().unwrap().status, Status::Done);

    let after_second = store
        .node_session_active("coding", SessionKind::Lead, None)
        .unwrap()
        .expect("the lead session is still active");
    assert_eq!(
        after_second.session_id, after_first.session_id,
        "the second review resumes the same lead session"
    );
    assert_eq!(after_second.turns, 2);
    assert!(after_second.retired_at.is_none());

    // 部署の無い（担当なし）Reviewer run はセッションを持たない。
    let plain = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&plain).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(plain.id).unwrap().unwrap().status, Status::Done);
    // `research` 部署は今回のタスクで一度も使っていないので、セッションは無い。
    assert_eq!(
        store
            .node_session_active("research", SessionKind::Lead, None)
            .unwrap(),
        None
    );
}

/// ADR-0054 D1 / Phase 67b 追記: resume が拒否されたら（`EventSink::session_resume_failed`）、
/// その場で Lead セッションを retire し、次の Reviewer run は新しいセッション（別の id、
/// `resume=false`）を作る（「失敗も同じ経路で作り直す」）。Phase 67 の実装は `ReviewerSink` に
/// この配線が無く、部門長のレビュー run では resume 拒否が一切 retire されなかった（この場合の
/// 本番の症状は「同じ壊れた session_id で `--resume` を延々と再試行する」）。
#[tokio::test]
async fn a_rejected_lead_session_resume_retires_it_and_the_next_review_starts_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let policy = StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: "claude-code".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: 2,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    );
    let adapter = Arc::new(ResumeRejectingReviewAdapter {
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
    });
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert("p1".into(), adapter);
    let mut d = Dispatcher::new(
        store.clone(),
        Box::new(policy),
        HashMap::from([("p1".to_string(), "m".to_string())]),
        adapters,
        std::collections::HashSet::new(),
        DispatchConfig {
            delivery: Default::default(),
            max_concurrency: 2,
            lease_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
            review_timeout: Duration::from_secs(5),
            workspace_root: PathBuf::from("/nonexistent"),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: crate::review::reviewer_hint(),
            reviewer_tier_override: None,
            clusters: HashMap::new(),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues: 5,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 5120,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: DelegationLimits::default(),
            accounts: None,
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: ContainersRuntimeConfig::default(),
            knowledge: KnowledgeRuntimeConfig::default(),
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution: ExecutionConfig::default(),
        },
    );

    // 1 本目: `coding-poc` の Reviewer run。新規セッション（resume していないので拒否は起きない）。
    let mut first = new_task(dir.path(), Check::Reviewer, 0);
    first.assignee = Some("coding-poc".into());
    store.insert(&first).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(first.id).unwrap().unwrap().status, Status::Done);
    let after_first = store
        .node_session_active("coding", SessionKind::Lead, None)
        .unwrap()
        .expect("a lead session exists after the first review");
    let first_id = after_first.session_id.clone();

    // 2 本目: 同じ部署 → resume を頼まれる → アダプタが resume 拒否を報告する。
    let mut second = new_task(dir.path(), Check::Reviewer, 0);
    second.assignee = Some("coding-poc".into());
    store.insert(&second).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(second.id).unwrap().unwrap().status, Status::Done);

    // resume 拒否はその場で retire する（次の `resolve_node_session` を待たない。ADR-0054 D1）。
    assert_eq!(
        store
            .node_session_active("coding", SessionKind::Lead, None)
            .unwrap(),
        None,
        "a rejected resume retires the session immediately"
    );

    // 3 本目: 次の run は新しいセッション（別の id、resume=false）を作る。
    let mut third = new_task(dir.path(), Check::Reviewer, 0);
    third.assignee = Some("coding-poc".into());
    store.insert(&third).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(third.id).unwrap().unwrap().status, Status::Done);

    let after_third = store
        .node_session_active("coding", SessionKind::Lead, None)
        .unwrap()
        .expect("the next run starts a fresh lead session");
    assert_ne!(
        after_third.session_id, first_id,
        "a fresh session gets a new id"
    );
    assert_eq!(
        after_third.turns, 1,
        "a fresh session starts at 0 turns, then this run touches it once"
    );
}

/// Phase 113 D1+D2+D4(b)（ADR-0054 追記。本番のタスク 01M35X86XTK84F97QW0CN5PGMR /
/// reviewer run 01M388BENASH3JEBWFS03KEQYT の再現）: 1 回目の reviewer run が resume 拒否で
/// `error_during_execution`/`is_error` 終わっても、`fail_all` されず reviewing のまま延期され
/// （D2）、resume 拒否は retire される（D1）。2 回目は新規セッション（`resume=false`）で走り、
/// 判定が出て `Done` になる。Phase 67〜112 時点はこの経路（`result` を観測できた resume 拒否）を
/// 一切 self-heal できず、この形の失敗はそのまま `failed` になっていた。
#[tokio::test]
async fn a_resume_rejection_that_still_produced_a_result_self_heals_and_the_retry_produces_a_verdict()
 {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let policy = StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: "claude-code".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: 2,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    );
    let adapter = Arc::new(ResumeRejectingThenSucceedingReviewAdapter {
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        calls: AtomicUsize::new(0),
    });
    let adapter_handle = adapter.clone();
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert("p1".into(), adapter);
    let mut d = Dispatcher::new(
        store.clone(),
        Box::new(policy),
        HashMap::from([("p1".to_string(), "m".to_string())]),
        adapters,
        std::collections::HashSet::new(),
        DispatchConfig {
            delivery: Default::default(),
            max_concurrency: 2,
            lease_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
            review_timeout: Duration::from_secs(5),
            workspace_root: PathBuf::from("/nonexistent"),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: crate::review::reviewer_hint(),
            reviewer_tier_override: None,
            clusters: HashMap::new(),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues: 5,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 5120,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: DelegationLimits::default(),
            accounts: None,
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: ContainersRuntimeConfig::default(),
            knowledge: KnowledgeRuntimeConfig::default(),
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution: ExecutionConfig::default(),
        },
    );

    // 1 本目: `coding-poc` の Reviewer run。新規セッション（resume していないので拒否は起きない）。
    let mut first = new_task(dir.path(), Check::Reviewer, 0);
    first.assignee = Some("coding-poc".into());
    store.insert(&first).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(first.id).unwrap().unwrap().status, Status::Done);

    // 2 本目: 同じ部署 → resume を頼まれる → resume 拒否で `error_during_execution` になる
    // （D1: self-heal で session を retire）が、`fail_all` はされず（D2）、3 本目として
    // 新規セッションでやり直して判定が出る。
    let mut second = new_task(dir.path(), Check::Reviewer, 0);
    second.assignee = Some("coding-poc".into());
    store.insert(&second).unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(
        store.get(second.id).unwrap().unwrap().status,
        Status::Done,
        "D2: the infra failure did not fail_all; the retried run produced a passing verdict"
    );
    assert_eq!(
        store.get(second.id).unwrap().unwrap().attempts,
        0,
        "the deferred retry did not consume an attempt"
    );
    assert!(
        adapter_handle.calls.load(Ordering::SeqCst) >= 2,
        "the adapter ran at least twice for the second task"
    );

    let events = store.events_for(second.id).unwrap();
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: true, .. })),
        "{events:?}"
    );
    // resume 拒否は D1 どおりその場で retire された（`node_sessions` の履歴で確認できる: 3 本目の
    // 部署は同じ `coding` だが、resume 拒否のあとは新規セッションで走った——2 本目のタスクが
    // `Done` になったこと自体が、resume 拒否の run 単体では止まらず先に進んだ証拠。ここでは加えて
    // 有効なセッションが（retire→新規作成で）存在することも確認する）。
    assert!(
        store
            .node_session_active("coding", SessionKind::Lead, None)
            .unwrap()
            .is_some(),
        "a fresh lead session exists after the self-heal"
    );
}

/// ADR-0054 Phase 67b 追記: 本番事故の再現と自己修復。`node_sessions` に Phase 67 が残した
/// ULID の `session_id`（`--session-id`/`--resume` を Claude Code CLI 2.1.278 に拒否される）を持つ
/// `claude-code` の行が既にあっても、`resolve_node_session` はそれを resume させず、retire した上で
/// UUID の新しいセッションを作る。
#[tokio::test]
async fn resolve_node_session_self_heals_a_non_uuid_claude_code_session_id() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let d = dispatcher(
        store.clone(),
        Arc::new(FileAdapter {
            plan_json: String::new(),
            review_json: String::new(),
            delay: Duration::ZERO,
        }),
        1,
    );
    let now = OffsetDateTime::now_utc();

    // 本番で観測された壊れた行を直接作る（Phase 67 が `ulid::Ulid::new().to_string()` を
    // `--session-id` に渡していた事故。2026-09-21 13:53 UTC 観測、`turns=1`）。
    let broken = NodeSession::new(
        "cos",
        SessionKind::Conversation,
        None,
        "claude-code",
        Some("claude_max_lab".to_string()),
        "01M323X6TJQSFEP0MKXABWVY78",
        now,
    );
    store.node_session_create(&broken).unwrap();
    store
        .node_session_touch("cos", SessionKind::Conversation, None, 10, now)
        .unwrap();

    let (fresh, _summary) = d
        .resolve_node_session(
            "cos",
            SessionKind::Conversation,
            None,
            "claude-code",
            Some("claude_max_lab"),
            now,
        )
        .unwrap();
    let fresh = fresh.unwrap();
    assert!(
        !fresh.resume,
        "an invalid stored session id must never be resumed"
    );
    assert_ne!(fresh.session_id, "01M323X6TJQSFEP0MKXABWVY78");
    assert!(
        task_worker::provider::is_valid_uuid(&fresh.session_id),
        "{}",
        fresh.session_id
    );

    // 壊れた行は retire され、新しい（0 turns の）行に置き換わっている。
    let after = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .unwrap()
        .unwrap();
    assert_eq!(after.session_id, fresh.session_id);
    assert_eq!(after.turns, 0, "a fresh session starts at 0 turns");
    assert_eq!(after.account_id.as_deref(), Some("claude_max_lab"));
}

/// ADR-0008 D2: `Check::Human` はディスパッチャが `Approval` 子タスクを生成して待つ。承認前は
/// `reviewing` のまま（`attempts` を消費しない）、承認後に `Done` になる。
#[tokio::test]
async fn human_check_creates_approval_child_and_completes_after_approval() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Human, 1);
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);

    let approval = wait_for_approval_child(&mut d, &store, task.id).await;
    assert_eq!(approval.status, Status::Ready);
    assert_eq!(approval.parent_id, Some(task.id));
    // 未決の間は reviewing のまま、attempts は消費しない。
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Reviewing);
    assert_eq!(t.attempts, 0);

    store
        .apply_transition(
            approval.id,
            Trigger::Approve,
            Some(Event::ApprovalDecided {
                by: "human".into(),
                approved: true,
                note: Some("looks good".into()),
            }),
        )
        .unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Done);
    assert_eq!(t.attempts, 0);
    assert!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: true, reason, .. } if reason.contains("approved")))
    );
}

/// Phase 113 D3/D4(d)（ADR-0054 追記。本番のタスク 01M35X86XTK84F97QW0CN5PGMR の障害を踏まえた
/// 回復手段）: `Human` 条件が承認済みのまま `Reviewer` 条件だけが不合格で `failed`
/// （直前の遷移は `review_fail`）になったタスクを `task_ops::comment::rereview` で再判定すると、
/// 承認済みの `Approval` 子タスクを**再利用**し（人に二度承認させない）、`Reviewer` 条件だけを
/// やり直す。2 回目は合格して `Done` になる。
#[tokio::test]
async fn rereview_from_failed_reuses_the_approved_human_child_and_only_reruns_the_reviewer() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Human, 0);
    task.acceptance.push(Criterion {
        text: "r".into(),
        check: Check::Reviewer,
    });
    store.insert(&task).unwrap();
    let adapter = Arc::new(TogglingReviewAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);

    let approval = wait_for_approval_child(&mut d, &store, task.id).await;
    store
        .apply_transition(
            approval.id,
            Trigger::Approve,
            Some(Event::ApprovalDecided {
                by: "human".into(),
                approved: true,
                note: Some("looks good".into()),
            }),
        )
        .unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(
        t.status,
        Status::Failed,
        "the reviewer criterion genuinely failed the first time"
    );
    assert_eq!(t.attempts, 1);

    let approvals_before: Vec<_> = store
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|c| c.parent_id == Some(task.id) && c.kind == TaskKind::Approval)
        .collect();
    assert_eq!(approvals_before.len(), 1);
    assert_eq!(approvals_before[0].id, approval.id);
    assert_eq!(approvals_before[0].status, Status::Done);

    // `failed` からの再レビュー（最後の遷移が `review_fail` なので許される。D3）。
    let result = task_ops::comment::rereview(store.as_ref(), task.id, None).unwrap();
    assert_eq!(result.to, Status::Reviewing);
    assert_eq!(
        store.get(task.id).unwrap().unwrap().attempts,
        0,
        "attempts rolled back to what it was when the approved child was created"
    );

    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Done, "the retried reviewer run passed");

    let approvals_after: Vec<_> = store
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|c| c.parent_id == Some(task.id) && c.kind == TaskKind::Approval)
        .collect();
    assert_eq!(
        approvals_after.len(),
        1,
        "no new Approval child was created; the human was not asked again"
    );
    assert_eq!(approvals_after[0].id, approval.id);
    assert_eq!(
        adapter.calls.load(Ordering::SeqCst),
        2,
        "the reviewer ran exactly twice"
    );
}

/// ADR-0033 D2（監査 D-3）: 承認子タスクは親の `project_id` / `milestone_id` / `assignee` を継ぐ
/// （案件の仕事の木から子が消えないように）。
#[tokio::test]
async fn human_check_approval_child_inherits_the_parents_project_milestone_and_assignee() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Human, 1);
    task.project_id = Some(ProjectId::new());
    task.milestone_id = Some(MilestoneId::new());
    task.assignee = Some("research-survey".into());
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);

    let approval = wait_for_approval_child(&mut d, &store, task.id).await;
    assert_eq!(approval.project_id, task.project_id);
    assert_eq!(approval.milestone_id, task.milestone_id);
    assert_eq!(approval.assignee, task.assignee);
}

/// ADR-0008 D2: 承認児タスクが reject されると、対象タスクの `Human` criterion は fail になる
/// （`max_retries=0` なので即 `Failed`）。
#[tokio::test]
async fn human_check_fails_task_after_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Human, 0);
    store.insert(&task).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);

    let approval = wait_for_approval_child(&mut d, &store, task.id).await;
    store
        .apply_transition(
            approval.id,
            Trigger::Reject,
            Some(Event::ApprovalDecided {
                by: "human".into(),
                approved: false,
                note: Some("not ready".into()),
            }),
        )
        .unwrap();
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Failed);
    assert!(
        store
            .events_for(task.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: false, reason, .. } if reason.contains("rejected") && reason.contains("not ready")))
    );
}

/// DESIGN §6 Phase 6 受け入れ: 承認前に子が `ready` にならないこと（dispatch されないこと）、
/// `reject` で子が `cancelled` になること。`ready_tasks` の除外は task-core 側で検証済みなので、
/// ここではディスパッチャの実際の tick を通して「dispatch されない」ことまで確認する。
#[tokio::test]
async fn approval_gate_blocks_child_dispatch_and_reject_cancels_it() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());

    let now = OffsetDateTime::now_utc();
    let mut approval = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    approval.kind = TaskKind::Approval;
    approval.status = Status::Ready;
    store.insert(&approval).unwrap();

    let mut child = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    child.parent_id = Some(approval.id);
    child.created_at = now;
    store.insert(&child).unwrap();

    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);

    // 承認前: 何 tick 回しても子は dispatch されず Ready のまま。
    for _ in 0..5 {
        let report = d.tick().unwrap();
        assert_eq!(
            report.dispatched, 0,
            "child must not be dispatched while its Approval parent is pending"
        );
    }
    assert_eq!(store.get(child.id).unwrap().unwrap().status, Status::Ready);

    // reject すると子は cancelled になり、以降も dispatch されない。
    store
        .apply_transition(
            approval.id,
            Trigger::Reject,
            Some(Event::ApprovalDecided {
                by: "human".into(),
                approved: false,
                note: None,
            }),
        )
        .unwrap();
    assert_eq!(
        store.get(approval.id).unwrap().unwrap().status,
        Status::Failed
    );
    assert_eq!(
        store.get(child.id).unwrap().unwrap().status,
        Status::Cancelled
    );
    for _ in 0..5 {
        let report = d.tick().unwrap();
        assert_eq!(report.dispatched, 0);
    }
    assert_eq!(
        store.get(child.id).unwrap().unwrap().status,
        Status::Cancelled
    );
}

/// ADR-0010 D8（P-35）: Human 条件は再レビュー（attempt が進んだ後）で新しい Approval 子を要求する。
/// 承認待ちで延期中の reviewing しか無ければ idle になる。
#[tokio::test]
async fn human_check_requests_a_new_approval_for_each_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Human, 1);
    task.acceptance.push(Criterion {
        text: "second run".into(),
        check: Check::Command {
            cmd: "test -f second".into(),
            expect_exit: 0,
        },
    });
    store.insert(&task).unwrap();
    let task_id = task.id;
    let approvals = |store: &Arc<dyn TaskStore>| -> Vec<Task> {
        let mut v: Vec<Task> = store
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|t| t.parent_id == Some(task_id) && t.kind == TaskKind::Approval)
            .collect();
        v.sort_by_key(|t| t.created_at);
        v
    };
    let approve = |store: &Arc<dyn TaskStore>, id: TaskId| {
        store
            .apply_transition(
                id,
                Trigger::Approve,
                Some(Event::ApprovalDecided {
                    by: "human".into(),
                    approved: true,
                    note: None,
                }),
            )
            .unwrap();
    };
    let mut d = dispatcher(
        store.clone(),
        Arc::new(CountingAdapter {
            calls: AtomicUsize::new(0),
        }),
        2,
    );

    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle, "only a human can make progress now");
    let first = approvals(&store);
    assert_eq!(first.len(), 1);
    assert!(
        first[0].title.ends_with("(attempt 1)"),
        "{}",
        first[0].title
    );
    assert_eq!(
        store.get(task_id).unwrap().unwrap().status,
        Status::Reviewing
    );

    // 承認 → Command 条件が fail → attempts 1 → 2 回目の run → 新しい Approval 子を待って idle。
    approve(&store, first[0].id);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);
    let all = approvals(&store);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0].status, Status::Done);
    assert!(all[1].title.ends_with("(attempt 2)"), "{}", all[1].title);
    assert_eq!(all[1].status, Status::Ready);
    let t = store.get(task_id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Reviewing, 1));

    approve(&store, all[1].id);
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(store.get(task_id).unwrap().unwrap().status, Status::Done);
}

/// ADR-0010 D5（P-29）: Reviewer run の供給側失敗は ReviewFail にならず、reviewing のまま延期され後で判定される。
#[tokio::test]
async fn reviewer_run_provider_failure_defers_review_without_consuming_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let adapter = Arc::new(FlakyReviewerAdapter {
        review_calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Done, 0));
    assert_eq!(adapter.review_calls.load(Ordering::SeqCst), 2);
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e, Event::WorkerProgress { msg, .. } if msg.starts_with("reviewer run requeued"))));
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::ProviderThrottled { provider, reason, .. } if provider == "p1" && reason.as_deref() == Some("throttled")
    )), "{events:?}");
    let verdicts: Vec<bool> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ReviewVerdict { pass, .. } => Some(*pass),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts, vec![true]);
    // ADR-0014 D1: 延期した Reviewer run も、成功した Reviewer run も WorkerFinished{role: reviewer} を残す。
    let reviewer_outcomes: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                outcome,
                role: Some(RunRole::Reviewer),
                ..
            } => Some(outcome.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reviewer_outcomes.len(), 2, "{events:?}");
    assert!(
        reviewer_outcomes[0].starts_with("requeue: "),
        "{reviewer_outcomes:?}"
    );
    assert!(
        reviewer_outcomes[1].starts_with("done: "),
        "{reviewer_outcomes:?}"
    );
}

/// ADR-0014 D1（P-G14）: Reviewer run も対象タスクに WorkerStarted / WorkerFinished（role: reviewer、provider つき）を残す。
/// ReviewVerdict はワーカー run に付き、ワーカー run を前提にする `last_run_id` は Reviewer run を見ない。
#[tokio::test]
async fn reviewer_run_records_worker_started_and_finished_with_reviewer_role() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let r = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&r).unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#.into(),
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 2);
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    assert_eq!(store.get(r.id).unwrap().unwrap().status, Status::Done);
    let events = store.events_for(r.id).unwrap();
    let started: Vec<(String, Option<RunRole>, Option<String>)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerStarted {
                run_id,
                role,
                provider,
                ..
            } => Some((run_id.clone(), *role, provider.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 2, "{events:?}");
    assert_eq!(
        (started[0].1, started[1].1),
        (None, Some(RunRole::Reviewer))
    );
    assert_eq!(started[1].2.as_deref(), Some("p1"));
    let reviewer_finished: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerFinished {
                run_id,
                outcome,
                role: Some(RunRole::Reviewer),
                ..
            } if *run_id == started[1].0 => Some(outcome.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reviewer_finished.len(), 1, "{events:?}");
    assert!(
        reviewer_finished[0].starts_with("done: "),
        "{reviewer_finished:?}"
    );
    assert_eq!(last_run_id(&events).as_deref(), Some(started[0].0.as_str()));
    assert!(events.iter().any(|(_, e)| matches!(e, Event::ReviewVerdict { run_id, pass: true, .. } if *run_id == started[0].0)));
}

/// 監査 M-1/M-2: 1 回目の「できました」がレビューで差し戻され、2 回目で通っても、`result` 報告は 1 件だけ。
#[tokio::test]
async fn a_review_retry_that_eventually_passes_produces_exactly_one_done_report() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org_for_reports(&store);
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f ready".into(),
            expect_exit: 0,
        },
        1,
    );
    task.assignee = Some("coding-poc".into());
    task.project_id = Some(ProjectId::new());
    store.insert(&task).unwrap();
    let adapter = Arc::new(AttemptGatedAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(t.status, Status::Done);
    assert_eq!(
        t.attempts, 1,
        "1 回目のレビュー差し戻しで attempts を消費し、2 回目で done になる"
    );
    let reports = store.report_list(&ReportFilter::default()).unwrap();
    let done_reports: Vec<_> = reports
        .iter()
        .filter(|r| r.kind == ReportKind::Result)
        .collect();
    assert_eq!(
        done_reports.len(),
        1,
        "差し戻された 1 回目は報告にせず、done の報告は 1 件だけ: {reports:?}"
    );
}

/// ADR-0011（P-38）: Reviewer run の供給側失敗による延期も max_requeues までで、超えたら Reviewer 条件を fail にして判定する。
#[tokio::test]
async fn reviewer_requeue_limit_fails_reviewer_criteria() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let adapter = Arc::new(AlwaysThrottledAdapter {
        calls: AtomicUsize::new(0),
        review_only: true,
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    d.config.max_requeues = 2;
    let report = run_until_idle(&mut d, 500).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Failed, 1));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
    let events = store.events_for(task.id).unwrap();
    // 最後の遷移（review_fail）の直前までで数える（その後ろには ReviewVerdict と ProviderThrottled が続く）。
    let last_transition = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::Transitioned { .. }))
        .unwrap();
    assert_eq!(consecutive_reviewer_requeues(&events[..last_transition]), 2);
    assert!(events.iter().any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: false, reason, .. } if reason.starts_with("requeue limit (2) reached"))));
}

/// Phase 113 D2/D4(c)（ADR-0054 追記。実機障害 2026-09-23、タスク 01M35X86XTK84F97QW0CN5PGMR）:
/// reviewer run **自身のインフラ都合の失敗**（`is_error`/クラッシュ相当。`AlwaysThrottledAdapter`
/// が使う「プロバイダが分類できる供給側失敗」とは別カウンタ）は `max_reviewer_retries` までは
/// `fail_all` せず reviewing のままやり直す。上限に達したときだけ「reviewer infra failure ×N」で
/// 不合格にする（`consecutive_reviewer_requeues` とは別に `consecutive_reviewer_infra_failures` で
/// 数える）。
#[tokio::test]
async fn reviewer_infra_failure_retry_limit_fails_reviewer_criteria() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let adapter = Arc::new(AlwaysInfraFailingReviewAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 2);
    d.config.max_reviewer_retries = 2;
    let report = run_until_idle(&mut d, 500).await;
    assert!(report.idle);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Failed, 1));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 3);
    let events = store.events_for(task.id).unwrap();
    // 最後の遷移（review_fail）の直前までで数える（その後ろには ReviewVerdict が続く）。
    let last_transition = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::Transitioned { .. }))
        .unwrap();
    assert_eq!(
        consecutive_reviewer_infra_failures(&events[..last_transition]),
        2
    );
    // プロバイダが分類できる供給側失敗のカウンタ（`REVIEWER_REQUEUED_PREFIX`）は動かない
    // （別軸であることの裏取り）。
    assert_eq!(consecutive_reviewer_requeues(&events[..last_transition]), 0);
    assert!(events.iter().any(|(_, e)| matches!(e, Event::ReviewVerdict { pass: false, reason, .. } if reason.starts_with("reviewer infra failure ×2"))));
}

/// 受け入れ 3: `aggregate = false` の親は子が終わるまで reviewing のまま、終わったら run を増やさず done。
#[tokio::test]
async fn non_aggregate_parent_stays_reviewing_until_children_finish_then_completes() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let parent = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&parent).unwrap();
    let adapter = Arc::new(DelegatingAdapter {
        proposals: vec![proposal("slow", vec![])],
        child_delay: Duration::from_millis(400),
        seen_role: std::sync::Mutex::new(None),
        aggregate_children: AtomicUsize::new(0),
        write_summary: false,
    });
    let mut d = dispatcher(store.clone(), adapter, 4);
    // ADR-0023 D3: 子待ちの親はスナップショットの `awaiting_children` にも出る。
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-09-16T00:00:00Z".into(),
        tick_ms: 10,
        providers: vec![],
        provider_checks: Default::default(),
    });
    // 親の run と判定が終わり、子がまだ走っている間に観察する。
    let mut observed_waiting = false;
    for _ in 0..200 {
        let r = d.tick().unwrap();
        let p = store.get(parent.id).unwrap().unwrap();
        let child_running = store
            .children(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.status == Status::Running);
        if p.status == Status::Reviewing
            && child_running
            && d.awaiting_children.contains_key(&parent.id)
        {
            observed_waiting = true;
            break;
        }
        if r.idle {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        observed_waiting,
        "parent should be reviewing while its delegated child runs"
    );
    assert_eq!(
        rx.borrow().as_ref().map(|s| s.awaiting_children.clone()),
        Some(vec![parent.id]),
        "ADR-0023 D3: 子待ちの親がスナップショットに出る（GUI が「判定中」と区別できる）"
    );
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle);
    let p = store.get(parent.id).unwrap().unwrap();
    assert_eq!(p.status, Status::Done);
    assert_eq!(
        rx.borrow().as_ref().map(|s| s.awaiting_children.clone()),
        Some(vec![]),
        "子が終われば待ちも消える"
    );
    assert_eq!(
        transition_reasons(&store, parent.id),
        vec!["dispatch", "worker_done", "review_pass"]
    );
    let events = store.events_for(parent.id).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|(_, e)| matches!(e, Event::WorkerStarted { role: None, .. }))
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|(_, e)| matches!(e, Event::Delegated { .. }))
    );
}

/// ADR-0033 D5（Phase 26）: `Question` で終わった run は既存の `Blocked` / `answers[]` に加えて、
/// `approvals` にも担当ノード宛ての 1 件を残す。
#[tokio::test]
async fn a_question_from_an_assigned_run_creates_a_pending_approval() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let task = assigned_task(&workspace_root, "t3", "research-survey");
    store.create_task(&task, vec![]).unwrap();

    let adapter = Arc::new(person_adapter(Terminal::Question {
        text: "どのクラスタを使いますか".into(),
    }));
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    assert_eq!(
        store.get(task.id).unwrap().unwrap().status,
        Status::Blocked,
        "既存の質問の終端はそのまま"
    );
    let pending = store.approval_list(Some(true), None, None).unwrap();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_id, "research-survey");
    assert_eq!(pending[0].task_id, Some(task.id));
    assert_eq!(pending[0].question, "どのクラスタを使いますか");
    assert!(pending[0].is_pending());
}

/// ADR-0072 §6 E4 (b): 大きな実装の後に「`cargo fmt --check` 相当」だけが不合格になった atomic な
/// Task が `ReviewRepair` → repair WU（最小の context）→ 再レビュー → `done` になる。attempts は
/// 不変で、repair WU の objective に元の（長い）objective の全文が含まれない。
#[tokio::test]
async fn a_format_only_review_failure_is_repaired_without_consuming_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let long_tail = "ORIGINAL_OBJECTIVE_TAIL_MARKER_".repeat(30); // 960 文字。600 文字の上限より長い。
    let mut task = new_task(
        dir.path(),
        Check::Command {
            // 実行される内容は `test -f .repair-done` だけ（`echo` は /dev/null に捨てる）。
            // cmd の文字列に "cargo fmt --check" を含めておき、classify_review_failure の
            // 分類（D16 の Format）に載せる。
            cmd: "echo 'cargo fmt --check' >/dev/null; test -f .repair-done".into(),
            expect_exit: 0,
        },
        2,
    );
    task.objective = format!(
        "{}{}",
        "big multi-step implementation. ".repeat(20),
        long_tail
    );
    let task_id = task.id;
    store.insert(&task).unwrap();

    let adapter = Arc::new(RepairFsAdapter(WuScriptAdapter::new(HashMap::new())));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    let report = run_until_idle(&mut d, 400).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(stored.status, Status::Done, "{stored:?}");
    assert_eq!(stored.attempts, 0, "repair must not consume Task.attempts");

    let units = store.work_units_for(task_id).unwrap();
    assert_eq!(units.len(), 2, "{units:?}");
    let main = units.iter().find(|u| u.key == "main").expect("main wu");
    assert_eq!(main.status, task_core::WorkUnitStatus::Done);
    let repair = units
        .iter()
        .find(|u| u.key == "repair-1")
        .expect("repair wu");
    assert_eq!(repair.status, task_core::WorkUnitStatus::Done);
    assert_eq!(repair.kind, task_core::WorkUnitKind::Repair);
    // request.json 相当（WU の objective）に元の objective の全文は無い（最小の context、D16）。
    assert!(
        !repair.spec.objective.contains(&long_tail),
        "{}",
        repair.spec.objective
    );
    assert!(repair.spec.objective.contains("cargo fmt --check"));

    let active = store.execution_plan_active(task_id).unwrap().unwrap();
    assert_eq!(active.origin, task_core::PlanOrigin::Repair);

    let events = store.events_for(task_id).unwrap();
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "review_repair")
        ),
        "{events:?}"
    );
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::ExecutionPlanned {
                origin: task_core::PlanOrigin::Repair,
                ..
            }
        )),
        "{events:?}"
    );
    // ADR-0074 §6 F1 (i): `RepairScheduled` を残し、`execution_metrics` の `repairs_by_class` に
    // `unknown` が出ない（title の接頭辞に頼らない）。
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::RepairScheduled { key, class, origin, .. }
                if key == "repair-1"
                    && class == "format"
                    && *origin == task_core::execution::RepairOrigin::Review
        )),
        "{events:?}"
    );
    let event_list: Vec<Event> = events.iter().map(|(_, e)| e.clone()).collect();
    let metrics = task_core::execution_metrics::summarize(&stored, &event_list);
    assert_eq!(
        metrics.repairs_by_class.get("format"),
        Some(&1),
        "{metrics:?}"
    );
    assert!(
        !metrics.repairs_by_class.contains_key("unknown"),
        "{metrics:?}"
    );
}

/// ADR-0072 §6 E4 (c): repair の上限（`max_repairs_per_class` = 2）を超えると、修復を試みずに
/// 従来の `ReviewFail`（attempts を消費する）に戻る。
#[tokio::test]
async fn exceeding_the_per_class_repair_limit_falls_back_to_review_fail() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // このコマンドは常に失敗する（`.repair-done` を作っても、次に必要な `.repair-done-2` は
    // 作られないので repair 後も不合格のまま = 何度でも repair が起きようとする）。
    // `max_retries = 0`: repair が尽きた後の `ReviewFail` 1 回で即 `failed` になる（scheduler の
    // 既知の制約 — repair で実体化した計画は全 WU が done のまま Task だけ `ready` に戻っても
    // 次に走らせる WU が無い。ADR の「Phase E4 実装時の逸脱・明確化」に記録した）。
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "echo 'cargo fmt --check' >/dev/null; false".into(),
            expect_exit: 0,
        },
        0,
    );
    let task_id = task.id;
    store.insert(&task).unwrap();

    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.config.execution.max_repairs_per_class = 2;
    d.config.execution.max_repairs = 5;
    let report = run_until_idle(&mut d, 800).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Failed,
        "repairs are exhausted; must fall back to review_fail: {stored:?}"
    );
    assert_eq!(stored.attempts, 1, "{stored:?}");

    let units = store.work_units_for(task_id).unwrap();
    let repairs: Vec<_> = units
        .iter()
        .filter(|u| u.kind == task_core::WorkUnitKind::Repair)
        .collect();
    assert_eq!(
        repairs.len(),
        2,
        "no more than max_repairs_per_class repair work units: {units:?}"
    );

    let events = store.events_for(task_id).unwrap();
    let repair_reasons = events
        .iter()
        .filter(
            |(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "review_repair"),
        )
        .count();
    assert_eq!(repair_reasons, 2, "{events:?}");
    assert!(
        events.iter().any(
            |(_, e)| matches!(e, Event::Transitioned { reason, .. } if reason == "review_fail")
        ),
        "{events:?}"
    );
}

/// ADR-0072 D5（E2b の指摘、Phase E3 で配線）: 計画の無い Task（暗黙の WorkUnit）の worker run と
/// reviewer run の両方が `runs` 索引に行を書く（(g)「全タスクの run について書かれる」は WU の run
/// だけでなく atomic/reviewer にも及ぶ）。
#[tokio::test]
async fn plain_task_writes_both_a_worker_and_a_reviewer_row_to_the_runs_index() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    // `Check::Reviewer` を使うので reviewer run が実際に起動する。`review.json` を書かない
    // `InstantAdapter` では reviewer の判定は不合格になるが、`runs` の行は起動した時点で作られる。
    let task = new_task(dir.path(), Check::Reviewer, 0);
    let task_id = task.id;
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
    let report = run_until_idle(&mut d, 300).await;
    assert!(report.idle, "{report:?}");

    let stored = store.get(task_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Failed,
        "no review.json was written, so the reviewer criterion fails: {stored:?}"
    );

    let runs = store.runs_for_task(task_id).unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(
        runs.iter()
            .any(|r| r.role == task_core::RunIndexRole::Worker && r.work_unit_id.is_none()),
        "{runs:?}"
    );
    assert!(
        runs.iter()
            .any(|r| r.role == task_core::RunIndexRole::Reviewer),
        "{runs:?}"
    );
    for r in &runs {
        assert!(r.finished_at.is_some(), "{r:?}");
    }
}

/// ADR-0117 D1: reviewer に渡す人の決定・回答は、対象 task とその祖先のものだけ（兄弟・未回答・取り下げは
/// 除く）。決定は回答の時刻順、選んだ選択肢の label と note が入る。回答は root 側から順に並ぶ。
#[test]
fn review_human_inputs_collect_answered_decisions_and_answers_of_the_task_and_its_ancestors() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let check = Check::Command {
        cmd: ":".into(),
        expect_exit: 0,
    };
    let root = new_task(dir.path(), check.clone(), 0);
    let mut child = new_task(dir.path(), check.clone(), 0);
    child.parent_id = Some(root.id);
    let mut sibling = new_task(dir.path(), check, 0);
    sibling.parent_id = Some(root.id);
    for t in [&root, &child, &sibling] {
        store.insert(t).unwrap();
    }
    let row = |task_id: TaskId, key: &str, answer: Option<(&str, Option<&str>, &str)>| {
        let request = task_core::DecisionRequest {
            id: format!("d-{key}"),
            key: key.into(),
            kind: task_core::DecisionKind::Choice,
            question: format!("question {key}"),
            options: vec![
                task_core::DecisionOption {
                    key: "a".into(),
                    label: format!("option a of {key}"),
                    consequence: None,
                },
                task_core::DecisionOption {
                    key: "b".into(),
                    label: format!("option b of {key}"),
                    consequence: None,
                },
            ],
            recommended: "a".into(),
            cost_of_reversal: task_core::CostOfReversal::Low,
            cost_note: None,
            needed_before: vec!["self".into()],
            path: vec![task_core::DecisionPathEntry {
                task_id: root.id,
                title: root.title.clone(),
                stage: None,
                unit: None,
            }],
            raised_by: task_core::DecisionRaisedBy {
                task_id,
                run_id: None,
                origin: task_core::DecisionOrigin::Planner,
            },
            status: task_core::DecisionStatus::Open,
            answer: None,
            withdrawn_reason: None,
        };
        let mut r = task_core::DecisionRow::from_request(task_id, &request, "2026-10-02T00:00:00Z");
        if let Some((option, note, ts)) = answer {
            r.apply_answer(option, note, "human", ts);
        }
        r
    };
    store
        .decisions_replace(vec![
            // 子の決定の方が先に答えられた。
            row(
                root.id,
                "root-later",
                Some(("a", None, "2026-10-02T02:00:00Z")),
            ),
            row(
                child.id,
                "child-first",
                Some(("b", Some("範囲を広げる"), "2026-10-02T01:00:00Z")),
            ),
            row(
                sibling.id,
                "sibling",
                Some(("a", None, "2026-10-02T00:30:00Z")),
            ),
            row(child.id, "open", None),
        ])
        .unwrap();
    store
        .append_event(
            root.id,
            &Event::Answered {
                question: "root q".into(),
                answer: "root a".into(),
            },
        )
        .unwrap();
    store
        .append_event(
            child.id,
            &Event::Answered {
                question: "child q".into(),
                answer: "child a".into(),
            },
        )
        .unwrap();
    store
        .append_event(
            sibling.id,
            &Event::Answered {
                question: "sibling q".into(),
                answer: "sibling a".into(),
            },
        )
        .unwrap();

    let d = dispatcher(store.clone(), done_adapter(), 1);
    let (decisions, answers) = d.review_human_inputs(&child).unwrap();
    assert_eq!(
        decisions
            .iter()
            .map(|x| (x.key.as_str(), x.option.as_str(), x.option_label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("child-first", "b", "option b of child-first"),
            ("root-later", "a", "option a of root-later"),
        ]
    );
    assert_eq!(decisions[0].task_id, child.id);
    assert_eq!(decisions[0].note.as_deref(), Some("範囲を広げる"));
    assert_eq!(decisions[0].question, "question child-first");
    assert_eq!(
        answers
            .iter()
            .map(|a| (a.question.as_str(), a.answer.as_str()))
            .collect::<Vec<_>>(),
        vec![("root q", "root a"), ("child q", "child a")]
    );
}
