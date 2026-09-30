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

    // 葉 3 本でプールの concurrency (3) が埋まる（a に 2 本、b に 1 本）。4 本目の葉は待つ。
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

    // 2 本目の CoS は、どちらのアカウントも max (2) に達していても +1 の 3 本目として載る。
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
