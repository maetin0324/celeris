//! ADR-0089（Phase R6-5）: CoS の対話 run（Console の一言）は `max_concurrency` とアカウントプールの
//! プロバイダの `concurrency` に数えず、`[execution] max_cos_runs` だけで待つ。アカウントは消費する
//! （走っている run の最も少ないもの、`max_runs_per_account` は +1 まで）。偽のアダプタと一時ディレクトリだけで、
//! 外部ネットワークに出ない。

use super::*;

/// run を長く走らせ続ける（この試験は tick を数回回すだけで、終わるのを待たない）。
fn slow_pool_adapter() -> Arc<PoolAdapter> {
    Arc::new(PoolAdapter {
        terminal_or_throttled: Ok(Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        }),
        delay: Duration::from_secs(30),
        observation: None,
        env: Vec::new(),
        captured: Arc::new(StdMutex::new(Vec::new())),
        spawn_failure: false,
    })
}

fn seed_org(store: &Arc<dyn TaskStore>) {
    let now = OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("cos", None, OrgKind::Secretary),
        ("eng", Some("cos"), OrgKind::Department),
    ] {
        store
            .org_upsert(&OrgNode {
                id: id.into(),
                parent_id: parent.map(str::to_owned),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                profile: task_core::Profile::default(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
}

/// 葉（部署に割り当てた通常のタスク）。
fn leaf(dir: &std::path::Path) -> Task {
    let mut t = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    t.assignee = Some("eng".into());
    t
}

/// CoS の対話用タスク（`POST /console/instruct` が作るものと同じ印: 対話 + 担当が秘書）。
fn cos_chat(dir: &std::path::Path) -> Task {
    let mut t = leaf(dir);
    t.assignee = Some("cos".into());
    t.conversation = Some(task_core::MessageId::new());
    t
}

fn started_account(store: &Arc<dyn TaskStore>, id: TaskId) -> Option<String> {
    store
        .events_for(id)
        .unwrap()
        .iter()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted { account, .. } => Some(account.clone()),
            _ => None,
        })
        .flatten()
}

fn status(store: &Arc<dyn TaskStore>, id: TaskId) -> Status {
    store.get(id).unwrap().unwrap().status
}

/// 受け入れ条件 1 / 4: `max_concurrency = 1` で葉が 1 本走っていても CoS run は起きる。CoS が走っていても
/// 葉は `max_concurrency` を超えて起きない（CoS は葉の枠を食わず、葉も CoS の例外に乗らない）。
#[tokio::test]
async fn cos_run_bypasses_max_concurrency_but_leaves_do_not() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    // プロバイダの concurrency は 4（効かせない）、全体の max_concurrency は 1。
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        4,
    );
    d.config.max_concurrency = 1;

    let first = leaf(ws.path());
    store.insert(&first).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(status(&store, first.id), Status::Running);

    let chat = cos_chat(ws.path());
    let second = leaf(ws.path());
    store.insert(&chat).unwrap();
    store.insert(&second).unwrap();
    let report = d.tick().unwrap();
    assert_eq!(
        report.dispatched, 1,
        "only the CoS run goes over max_concurrency"
    );
    assert_eq!(status(&store, chat.id), Status::Running);
    assert_eq!(status(&store, second.id), Status::Ready);
    assert_eq!((d.workers_in_flight(), d.cos_in_flight()), (1, 1));
    // 何 tick 回しても、葉は CoS の走っている間も max_concurrency = 1 を超えない。
    for _ in 0..3 {
        assert_eq!(d.tick().unwrap().dispatched, 0);
    }
    assert_eq!(status(&store, second.id), Status::Ready);
}

/// 受け入れ条件 2 / 3: プールのプロバイダの concurrency が埋まっていても CoS run は起き、走っている run の
/// 最も少ないアカウントに載る（`max_runs_per_account` は +1 まで）。`max_cos_runs = 2` を超える 3 本目は待つ。
/// `GET /providers` の元のスナップショットは CoS を `in_use_cos` に別に数える。
#[tokio::test]
async fn cos_run_bypasses_pool_concurrency_on_the_least_loaded_account_up_to_max_cos_runs() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    // プロバイダの concurrency 3、全体の max_concurrency 6、1 アカウント 2 本まで。
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        3,
    );
    d.config.max_concurrency = 6;
    assert_eq!(d.config.execution.max_cos_runs, 2, "default max_cos_runs");
    let (tx, rx) = tokio::sync::watch::channel(None);
    d.set_snapshot_publisher(SnapshotPublisher {
        tx,
        instance_id: "inst-1".into(),
        hostname: "host-1".into(),
        started_at: "2026-09-29T00:00:00Z".into(),
        tick_ms: 50,
        providers: vec![ProviderLive {
            kind: Default::default(),
            llm_source: None,
            credential_refs: Default::default(),
            tier_models: Default::default(),
            account_id: None,
            id: "p1".into(),
            adapter: "claude-code".into(),
            tiers: vec![Tier::Standard],
            concurrency: 3,
            model: Some("m".into()),
            env_keys: vec![],
            in_use: 0,
            in_use_cos: 0,
            last_check: None,
            account_pool: true,
        }],
        provider_checks: Default::default(),
    });

    // Individual overrides, rather than provider concurrency, admit a=2 and b=1.
    d.set_account_limit("claude-code", "a", Some(2)).unwrap();
    d.set_account_limit("claude-code", "b", Some(1)).unwrap();
    let leaves: Vec<Task> = (0..4).map(|_| leaf(ws.path())).collect();
    for t in &leaves {
        store.insert(t).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 3);
    assert_eq!(d.provider_in_use(&"p1".to_string()), 3);
    let on_a = leaves[..3]
        .iter()
        .filter(|t| started_account(&store, t.id).as_deref() == Some("a"))
        .count();
    assert_eq!(on_a, 2, "leaves fill a (2) then b (1)");
    assert_eq!(status(&store, leaves[3].id), Status::Ready);

    // CoS run はプールが満杯でも起き、走っている run の少ない b に載る。
    let chat1 = cos_chat(ws.path());
    store.insert(&chat1).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(status(&store, chat1.id), Status::Running);
    assert_eq!(started_account(&store, chat1.id).as_deref(), Some("b"));
    assert_eq!(
        status(&store, leaves[3].id),
        Status::Ready,
        "leaves still wait"
    );

    let snap = rx.borrow().clone().expect("snapshot");
    assert_eq!(snap.providers[0].in_use, 3);
    assert_eq!(snap.providers[0].in_use_cos, 1);

    // 2 本目の CoS は、個別上限に達していても +1 の追加枠として載る。
    let chat2 = cos_chat(ws.path());
    store.insert(&chat2).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(status(&store, chat2.id), Status::Running);
    assert!(started_account(&store, chat2.id).is_some());
    assert_eq!(d.cos_in_flight(), 2);

    // 3 本目の CoS は max_cos_runs (2) を超えるので待つ。
    let chat3 = cos_chat(ws.path());
    store.insert(&chat3).unwrap();
    for _ in 0..3 {
        assert_eq!(d.tick().unwrap().dispatched, 0);
    }
    assert_eq!(status(&store, chat3.id), Status::Ready);
    assert_eq!(status(&store, leaves[3].id), Status::Ready);
    let snap = rx.borrow().clone().expect("snapshot");
    assert_eq!(
        (snap.providers[0].in_use, snap.providers[0].in_use_cos),
        (3, 2)
    );
}

/// `max_cos_runs = 0` は例外の無効化: CoS の対話 run も通常の run と同じく max_concurrency で待つ。
#[tokio::test]
async fn max_cos_runs_zero_disables_the_exemption() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        4,
    );
    d.config.max_concurrency = 1;
    d.config.execution.max_cos_runs = 0;

    let first = leaf(ws.path());
    store.insert(&first).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    let chat = cos_chat(ws.path());
    store.insert(&chat).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(status(&store, chat.id), Status::Ready);
}

/// A pool can run four leaves despite legacy provider concurrency=1, and stops at
/// the two accounts' limits. Clearing a live override immediately restores default.
#[tokio::test]
async fn pool_ignores_provider_concurrency_and_uses_account_overrides() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        1,
    );
    d.config.max_concurrency = 8;
    d.set_account_limit("claude-code", "a", Some(1)).unwrap();
    let leaves: Vec<_> = (0..5).map(|_| leaf(ws.path())).collect();
    for task in &leaves {
        store.insert(task).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 3);
    assert_eq!(d.account_in_use(AccountAdapter::ClaudeCode, "a"), 1);
    assert_eq!(d.account_in_use(AccountAdapter::ClaudeCode, "b"), 2);
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert!(!d.account_usable(AccountAdapter::ClaudeCode, "a", false));
    assert!(d.account_usable(AccountAdapter::ClaudeCode, "a", true));
    assert_eq!(
        d.peek_account(AccountAdapter::ClaudeCode, "p1", false),
        None
    );
    assert!(
        d.legacy_provider_profiles(&leaf(ws.path()).worker_hint)
            .iter()
            .all(|p| p.deployment.concurrency_limit.is_none())
    );
    d.set_account_limit("claude-code", "a", None).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    assert_eq!(d.account_in_use(AccountAdapter::ClaudeCode, "a"), 2);
    assert_eq!(d.tick().unwrap().dispatched, 0);
}

#[test]
fn dispatcher_account_limits_load_on_start_and_reload() {
    let accounts = accounts_fixture();
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("limits.db");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        1,
    );
    d.set_account_limit("claude-code", "a", Some(3)).unwrap();
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "a", false),
        3
    );
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "a", true),
        4
    );
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "b", false),
        2
    );
    assert!(d.set_account_limit("claude-code", "a", Some(0)).is_err());
    assert_eq!(d.account_max_runs("claude-code", "a"), Some(3));
    drop(d);
    drop(store);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        1,
    );
    assert_eq!(d.account_max_runs("claude-code", "a"), Some(3));
    store
        .set_account_limit("claude-code", "a", Some(1))
        .unwrap();
    d.reload_account_limits().unwrap();
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "a", true),
        2
    );
    d.set_account_limit("claude-code", "a", None).unwrap();
    d.reload_account_limits().unwrap();
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "a", false),
        2
    );
}

/// A CoS run using its extra slot does not consume the remaining leaf slot.
#[tokio::test]
async fn cos_account_usage_does_not_consume_leaf_capacity() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        1,
        1,
    );
    d.config.max_concurrency = 8;
    let chat = cos_chat(ws.path());
    store.insert(&chat).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    let leaves: Vec<_> = (0..3).map(|_| leaf(ws.path())).collect();
    for task in &leaves {
        store.insert(task).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 2);
    assert_eq!(d.tick().unwrap().dispatched, 0);
}

/// A non-pool row (`account_pool = false`) as in production Qwen: `p1` with the
/// deprecated `concurrency` written as `legacy` (`None` = not written).
fn non_pool_dispatcher(store: Arc<dyn TaskStore>, legacy: Option<usize>) -> Dispatcher {
    let mut d = dispatcher(store, slow_pool_adapter(), 8);
    // 旧 `concurrency` の値を暗黙アカウントの保存値として置く（None なら保存値に触らない）。
    if let Some(n) = legacy {
        d.set_account_limit("implicit", "p1", Some(n)).unwrap();
    }
    d.policy = Box::new(StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: "instant".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: legacy,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    ));
    d
}

/// ADR 2026-10-08 §(e): a non-pool row's implicit account is limited by its own stored
/// value (default `max_runs_per_account`). A row's written `concurrency = 1` does not limit
/// it: with the default 2 the row admits two runs. A stored override of 1 holds it to one.
#[tokio::test]
async fn non_pool_row_is_limited_by_its_implicit_account() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = non_pool_dispatcher(store.clone(), None);
    assert_eq!(d.implicit_account_ids(), vec!["p1".to_string()]);
    assert_eq!(
        d.implicit_account_limit("p1"),
        Some((2, crate::capacity::MaxRunsSource::Default))
    );
    let leaves: Vec<_> = (0..4).map(|_| leaf(ws.path())).collect();
    for task in &leaves {
        store.insert(task).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 2);
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(d.implicit_account_in_use("p1"), 2);
    // provider_select・routing の候補には暗黙アカウントの上限が渡る（旧 concurrency は渡らない）。
    assert!(
        d.legacy_provider_profiles(&leaf(ws.path()).worker_hint)
            .iter()
            .all(|p| p.deployment.concurrency_limit == Some(2))
    );

    d.set_account_limit("implicit", "p1", Some(1)).unwrap();
    assert_eq!(
        d.implicit_account_limit("p1"),
        Some((1, crate::capacity::MaxRunsSource::Account))
    );
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(d.implicit_account_in_use("p1"), 2);

    // 解除すると既定（2）に戻る。走っている 2 本は止めず、残りの run が待つ。
    d.set_account_limit("implicit", "p1", None).unwrap();
    assert_eq!(
        d.implicit_account_limit("p1"),
        Some((2, crate::capacity::MaxRunsSource::Default))
    );
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(d.implicit_account_in_use("p1"), 2);
}

/// Without a written `concurrency`, the implicit account falls back to
/// `max_runs_per_account` (built-in 2 without `[accounts]`).
#[tokio::test]
async fn implicit_account_uses_the_default_without_accounts_section() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = non_pool_dispatcher(store.clone(), None);
    assert!(d.config.accounts.is_none());
    assert_eq!(
        d.implicit_account_limit("p1"),
        Some((
            crate::dispatcher::DEFAULT_MAX_RUNS_PER_ACCOUNT,
            crate::capacity::MaxRunsSource::Default
        ))
    );
    for task in (0..3).map(|_| leaf(ws.path())) {
        store.insert(&task).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 2);
    assert_eq!(d.tick().unwrap().dispatched, 0);
}

/// ADR 2026-10-08 §(e): CoS gets no +1 on an implicit account — a CoS run and a
/// leaf share the single slot of a stored limit of 1 (GPU 1 枚の Qwen に 2 本目を載せない).
#[tokio::test]
async fn implicit_account_counts_cos_runs_without_an_extra_slot() {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = non_pool_dispatcher(store.clone(), None);
    d.set_account_limit("implicit", "p1", Some(1)).unwrap();
    let chat = cos_chat(ws.path());
    store.insert(&chat).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    store.insert(&leaf(ws.path())).unwrap();
    store.insert(&cos_chat(ws.path())).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(d.implicit_account_in_use("p1"), 1);
}

/// Overrides survive a reload that keeps the row; a reload that drops the row (or
/// makes it a pool) prunes the implicit override. Real-account rows are never touched,
/// even when a real account shares the provider id (`implicit` is a reserved adapter).
#[test]
fn implicit_account_overrides_survive_reload_and_are_pruned_with_their_row() {
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("implicit.db");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let mut d = non_pool_dispatcher(store.clone(), Some(1));
    d.set_account_limit("implicit", "p1", Some(2)).unwrap();
    d.set_account_limit("claude-code", "p1", Some(5)).unwrap();
    d.set_account_limit("implicit", "gone", Some(3)).unwrap();
    // 実アカウント `claude-code/p1` と暗黙アカウント `implicit/p1` は別の鍵。
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "p1", false),
        5
    );
    assert_eq!(d.implicit_account_limit("p1").unwrap().0, 2);
    assert!(
        AccountAdapter::parse(crate::capacity::IMPLICIT_ACCOUNT_ADAPTER).is_none(),
        "implicit must stay a reserved adapter name"
    );

    // 起動時の prune: 行の無い `implicit/gone` だけ消える。
    assert_eq!(d.prune_implicit_account_limits().unwrap(), vec!["gone"]);
    assert_eq!(d.account_max_runs("implicit", "gone"), None);

    // 同じ行のままの reload（＋ daemon 再起動）を跨いで残る。
    let same = |legacy| -> Box<dyn ProviderPolicy> {
        Box::new(StaticPolicy::new(
            vec![ProviderSpec {
                id: "p1".into(),
                adapter: "instant".into(),
                tiers: vec![Tier::Standard],
                concurrency: legacy,
                model: "m".into(),
            }],
            Duration::from_secs(1),
        ))
    };
    d.reload_account_limits().unwrap();
    d.reload_providers(
        same(Some(1)),
        HashMap::new(),
        HashMap::new(),
        Default::default(),
    );
    assert!(d.prune_implicit_account_limits().unwrap().is_empty());
    assert_eq!(d.implicit_account_limit("p1").unwrap().0, 2);
    drop(d);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let mut d = non_pool_dispatcher(store.clone(), None);
    assert_eq!(
        d.implicit_account_limit("p1"),
        Some((2, crate::capacity::MaxRunsSource::Account))
    );

    // 行がプールに変わると暗黙アカウントは無くなり、保存値も消える。実アカウントの値は残る。
    d.reload_providers(
        same(Some(1)),
        HashMap::new(),
        HashMap::new(),
        std::collections::HashSet::from(["p1".to_string()]),
    );
    assert!(d.implicit_account_ids().is_empty());
    assert_eq!(d.implicit_account_limit("p1"), None);
    assert_eq!(d.prune_implicit_account_limits().unwrap(), vec!["p1"]);
    assert_eq!(store.account_limits().unwrap().len(), 1);
    assert_eq!(d.account_max_runs("claude-code", "p1"), Some(5));
}
