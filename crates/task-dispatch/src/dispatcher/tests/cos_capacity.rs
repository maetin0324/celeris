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
    d.set_config_max_concurrency(1);

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
    d.set_config_max_concurrency(1);
    d.config.execution.max_cos_runs = 0;

    let first = leaf(ws.path());
    store.insert(&first).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 1);
    let chat = cos_chat(ws.path());
    store.insert(&chat).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(status(&store, chat.id), Status::Ready);
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

#[test]
fn account_limits_source_account_and_source_limits_are_independent() {
    let accounts = accounts_fixture();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = pool_dispatcher(
        store,
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        2,
        1,
    );

    // Account overrides are indexed by the migrated (source_id, account_id) key.
    d.set_account_limit("claude-code", "a", Some(3)).unwrap();
    d.set_account_limit("claude-code", "b", Some(1)).unwrap();
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "a", false),
        3
    );
    assert_eq!(
        d.account_run_limit(AccountAdapter::ClaudeCode, "b", false),
        1
    );

    // A source-owned limit has no fabricated shared unknown account: it uses the
    // explicit source key and the source's stable implicit account identity.
    d.set_account_limit("implicit", "shared-source", Some(4))
        .unwrap();
    assert_eq!(
        d.account_limits
            .as_ref()
            .and_then(|limits| limits.get(&(
                task_core::llm_route::SourceId::from("implicit"),
                task_core::llm_route::AccountId::from("shared-source"),
            )))
            .copied(),
        Some(4)
    );
    assert_eq!(d.account_max_runs("claude-code", "shared-source"), None);
}

#[test]
fn account_limits_source_cooldown_excludes_only_the_cooled_account() {
    let mut book = AccountBook::new_in_memory();
    book.set_cooldown(
        "a",
        crate::accounts::AccountCooldown {
            reason: crate::accounts::AccountCooldownReason::Throttled,
            until: 2_000,
        },
        1_000,
    );
    let a = evaluate(
        &AccountCandidate {
            id: "a",
            logged_in: true,
            in_use: 0,
        },
        book.state("a"),
        2,
        1_500,
    );
    let b = evaluate(
        &AccountCandidate {
            id: "b",
            logged_in: true,
            in_use: 0,
        },
        book.state("b"),
        2,
        1_500,
    );
    assert_eq!(a.excluded, Some(ExcludedReason::Cooldown));
    assert_eq!(b.excluded, None);
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
    d.set_config_max_concurrency(8);
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

/// A source without an account pool as in production Qwen: `p1` with a stored implicit
/// limit `legacy` (`None` = not stored).
fn non_pool_dispatcher(store: Arc<dyn TaskStore>, legacy: Option<usize>) -> Dispatcher {
    let mut d = dispatcher(store, slow_pool_adapter(), 8);
    // 旧 `concurrency` の値を暗黙アカウントの保存値として置く（None なら保存値に触らない）。
    if let Some(n) = legacy {
        d.set_account_limit("implicit", "p1", Some(n)).unwrap();
    }
    d
}

/// Overrides survive a reload that keeps the source; a reload that drops the source (or
/// gives it an account pool) prunes the implicit override. Real-account rows are never touched,
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

    // 同じ宣言のままの reload（＋ daemon 再起動）を跨いで残る。
    d.reload_account_limits().unwrap();
    let declared = d.llm_route_registry().clone();
    d.set_llm_route_registry(declared.clone());
    assert!(d.prune_implicit_account_limits().unwrap().is_empty());
    assert_eq!(d.implicit_account_limit("p1").unwrap().0, 2);
    drop(d);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let mut d = non_pool_dispatcher(store.clone(), None);
    assert_eq!(
        d.implicit_account_limit("p1"),
        Some((2, crate::capacity::MaxRunsSource::Account))
    );

    // source が account pool の種類に変わると暗黙アカウントは無くなり、保存値も消える。実アカウントの値は残る。
    let mut pooled = declared;
    if let Some(spec) = pooled.sources.get_mut(&"p1".into()) {
        spec.kind = "claude_oauth".into();
    }
    d.set_llm_route_registry(pooled);
    assert!(d.implicit_account_ids().is_empty());
    assert_eq!(d.implicit_account_limit("p1"), None);
    assert_eq!(d.prune_implicit_account_limits().unwrap(), vec!["p1"]);
    assert_eq!(store.account_limits().unwrap().len(), 1);
    assert_eq!(d.account_max_runs("claude-code", "p1"), Some(5));
}

/// ADR 2026-10-14-local-machine-settings D3・D4: store の `max_concurrency` は設定ファイルの値に勝ち、次の tick
/// から効く。上げれば次の tick で枠が増え、下げても走行中の run は止めず（`Running` のまま）、超過が解けるまで
/// 新しい run を起こさない。
#[tokio::test]
async fn store_max_concurrency_applies_next_tick_without_stopping_running_runs() {
    let accounts = accounts_fixture();
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_org(&store);
    let mut d = pool_dispatcher(
        store.clone(),
        slow_pool_adapter(),
        None,
        accounts.path().to_path_buf(),
        8,
        8,
    );
    d.set_config_max_concurrency(1);
    let set_store_max = |n: Option<u32>| {
        store
            .local_settings_set(
                &task_core::LocalSettingsOverrides {
                    workspace_root: None,
                    max_concurrency: n,
                },
                "admin",
                OffsetDateTime::now_utc(),
            )
            .unwrap();
    };

    let leaves: Vec<Task> = (0..4).map(|_| leaf(ws.path())).collect();
    for t in &leaves {
        store.insert(t).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 1, "config value 1");
    assert_eq!(d.max_concurrency(), 1);

    set_store_max(Some(3));
    assert_eq!(d.max_concurrency(), 1, "not before the next tick");
    assert_eq!(
        d.tick().unwrap().dispatched,
        2,
        "store value 3 from the next tick"
    );
    assert_eq!(d.max_concurrency(), 3);
    assert_eq!(d.workers_in_flight(), 3);

    set_store_max(Some(1));
    for _ in 0..3 {
        assert_eq!(d.tick().unwrap().dispatched, 0, "over the new limit");
    }
    assert_eq!(d.max_concurrency(), 1);
    assert_eq!(d.workers_in_flight(), 3, "running runs are not stopped");
    let running = leaves
        .iter()
        .filter(|t| status(&store, t.id) == Status::Running)
        .count();
    assert_eq!(running, 3);
    assert_eq!(
        leaves
            .iter()
            .filter(|t| status(&store, t.id) == Status::Ready)
            .count(),
        1
    );

    // 上書きを消すと設定ファイルの値（reload で入れ直した値）に戻る。
    set_store_max(None);
    d.set_config_max_concurrency(4);
    assert_eq!(d.max_concurrency(), 4);
    assert_eq!(d.tick().unwrap().dispatched, 1);
}
