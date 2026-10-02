use super::run_until_idle;
use super::*;
use crate::policy::{ProviderSpec, StaticPolicy};
use async_trait::async_trait;
use std::sync::Mutex as SyncMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 知識整理 run を受ける「汎用」アダプタ（開発用の `fake` と同じ id）。走ったら
/// `artifacts/knowledge-candidates.json` を書いて `done` になる。
struct CandidatesAdapter {
    id: &'static str,
    seen: Arc<SyncMutex<Vec<RunRequest>>>,
}

#[async_trait]
impl WorkerAdapter for CandidatesAdapter {
    fn id(&self) -> &str {
        self.id
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).ok();
        std::fs::write(
            req.artifact_path("knowledge-candidates.json"),
            r#"{"candidates": [{"op": "create", "path": "environment/tools/x.md", "title": "x",
                "tags": [], "scope": "environment", "body": "b", "sources": ["task:x"],
                "confidence": "high"}]}"#,
        )
        .ok();
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(req);
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "1 件の候補を書いた".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn knowledge_task(dir: &std::path::Path) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "知識整理: pegasus".into(),
        objective: "この仕事から知識の候補を抽出せよ".into(),
        acceptance: Vec::new(),
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Cheap,
            adapter: Some("langmem".into()),
        },
        workspace: WorkspaceSpec::Local {
            path: dir.to_path_buf(),
            mode: None,
        },
        budget: Budget {
            max_turns: 4,
            max_wall_secs: 900,
            max_retries: 1,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: Some(task_core::report::KNOWLEDGE_ROLE.to_string()),
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// `langmem`（専用）と `fake`（汎用）の 2 つの供給元を持つディスパッチャ。
/// `generic` が false なら汎用の供給元を置かない（ADR-0052 D2「候補が無ければ従来どおり失敗」）。
fn knowledge_dispatcher(
    store: Arc<dyn TaskStore>,
    seen: Arc<SyncMutex<Vec<RunRequest>>>,
    fallback_tier: Option<Tier>,
    generic: bool,
) -> Dispatcher {
    let mut providers = vec![ProviderSpec {
        id: "qwen".into(),
        adapter: "langmem".into(),
        tiers: vec![Tier::Cheap],
        concurrency: 1,
        model: "qwen".into(),
    }];
    let mut adapters: HashMap<ProviderId, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert(
        "qwen".into(),
        Arc::new(CandidatesAdapter {
            id: "langmem",
            seen: seen.clone(),
        }),
    );
    if generic {
        providers.push(ProviderSpec {
            id: "cheap-generic".into(),
            adapter: "fake".into(),
            tiers: vec![Tier::Cheap],
            concurrency: 1,
            model: "fake".into(),
        });
        adapters.insert(
            "cheap-generic".into(),
            Arc::new(CandidatesAdapter { id: "fake", seen }),
        );
    }
    let policy = StaticPolicy::new(providers, Duration::from_secs(1));
    Dispatcher::new(
        store,
        Box::new(policy),
        HashMap::new(),
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
            knowledge: KnowledgeRuntimeConfig {
                langmem_base_url: Some("http://127.0.0.1:1/v1".to_string()),
                fallback_tier,
                ..KnowledgeRuntimeConfig::default()
            },
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution: ExecutionConfig::default(),
        },
    )
}

fn started_adapter(store: &Arc<dyn TaskStore>, task_id: TaskId) -> Option<String> {
    store
        .events_for(task_id)
        .expect("events")
        .into_iter()
        .rev()
        .find_map(|(_, e)| match e {
            Event::WorkerStarted { adapter, .. } => Some(adapter),
            _ => None,
        })
}

/// ADR-0052 D1 + D2: 接続先に届かなければ tier `cheap` の汎用ハーネスで走り、`status` の進行が
/// 理由つきで残り、前置きは LangMem と同じ抽出の指示 + 出力契約、予算は 8 turn / 600 秒になる。
/// 書かれた `artifacts/knowledge-candidates.json` は `apply_finished` が読む場所にある。
#[tokio::test]
async fn an_unreachable_langmem_endpoint_runs_the_extraction_on_a_cheap_generic_harness() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let task = knowledge_task(dir.path());
    store.insert(&task).expect("insert");
    let seen = Arc::new(SyncMutex::new(Vec::new()));
    let mut d = knowledge_dispatcher(store.clone(), seen.clone(), Some(Tier::Cheap), true);
    d.set_knowledge_probe(Arc::new(|_, _| Reachability::Unreachable {
        reason: "接続できない: Connection refused".into(),
    }));
    run_until_idle(&mut d, 100).await;

    assert_eq!(
        started_adapter(&store, task.id).as_deref(),
        Some("fake"),
        "cheap の汎用ハーネスで走る"
    );
    let events = store.events_for(task.id).expect("events");
    assert!(
        events.iter().any(|(_, e)| matches!(
            e,
            Event::WorkerProgress { msg, kind, .. }
                if *kind == Some(ProgressKind::Status)
                    && msg.contains("langmem の接続先に届かない")
                    && msg.contains("Connection refused")
                    && msg.contains("cheap のハーネスに倒す")
        )),
        "{events:?}"
    );

    let requests = seen.lock().expect("seen");
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.task.budget.max_turns, 8, "ADR-0052 D2 の予算");
    assert_eq!(req.task.budget.max_wall_secs, 600);
    assert_eq!(
        req.task.worker_hint.adapter, None,
        "専用アダプタの固定は外れている"
    );
    assert_eq!(
        req.task.objective, task.objective,
        "依頼文（maintenance_objective）は同じ入力のまま"
    );
    let role = req.context.role.as_ref().expect("role");
    assert!(
        role.instructions
            .contains(task_worker::langmem::extraction_instructions()),
        "{}",
        role.instructions
    );
    assert!(
        role.instructions
            .contains("artifacts/knowledge-candidates.json")
    );
    assert!(role.instructions.contains("道具は使わない"));
    // `apply_finished` が読む場所に書かれている。
    assert!(
        task_core::artifacts::artifacts_dir_for(&task, dir.path())
            .join("knowledge-candidates.json")
            .exists()
    );
}

/// ADR-0052 D1: 200 が返れば従来どおり `langmem` で走る（フォールバックしない）。
/// 検査は `base_url` ごとに 60 秒キャッシュするので、tick を何度回しても 1 回しか叩かない。
#[tokio::test]
async fn a_reachable_endpoint_keeps_using_langmem_and_the_probe_is_cached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let task = knowledge_task(dir.path());
    store.insert(&task).expect("insert");
    let seen = Arc::new(SyncMutex::new(Vec::new()));
    let mut d = knowledge_dispatcher(store.clone(), seen.clone(), Some(Tier::Cheap), true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    d.set_knowledge_probe(Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reachability::Ok
    }));
    run_until_idle(&mut d, 100).await;

    assert_eq!(started_adapter(&store, task.id).as_deref(), Some("langmem"));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "60 秒キャッシュ");
    let requests = seen.lock().expect("seen");
    assert_eq!(requests[0].task.budget.max_turns, 4, "予算は元のまま");
    assert!(
        requests[0]
            .context
            .role
            .as_ref()
            .is_none_or(|r| !r.instructions.contains("出力の契約 (output contract)"))
    );
    // 進行に「倒す」の 1 行は出ない。
    assert!(
        !store
            .events_for(task.id)
            .expect("events")
            .iter()
            .any(|(_, e)| matches!(e, Event::WorkerProgress { msg, .. } if msg.contains("倒す")))
    );
}

/// Phase 65b: `[knowledge.langmem].api_key_secret` から解決したトークンが probe に渡ること
/// （celeris の `llm-proxy` のように `GET /v1/models` が認証を要求する上流を指したときのため）。
#[tokio::test]
async fn the_resolved_api_key_is_passed_to_the_knowledge_probe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let task = knowledge_task(dir.path());
    store.insert(&task).expect("insert");
    let seen = Arc::new(SyncMutex::new(Vec::new()));
    let mut d = knowledge_dispatcher(store.clone(), seen, Some(Tier::Cheap), true);
    d.config.knowledge.langmem_api_key = Some("secret-proxy-token".to_string());
    let seen_tokens = Arc::new(SyncMutex::new(Vec::new()));
    let capture = seen_tokens.clone();
    d.set_knowledge_probe(Arc::new(move |_, token| {
        capture
            .lock()
            .expect("lock")
            .push(token.map(str::to_string));
        Reachability::Ok
    }));
    run_until_idle(&mut d, 100).await;
    assert_eq!(
        seen_tokens.lock().expect("lock").as_slice(),
        [Some("secret-proxy-token".to_string())]
    );
}

/// ADR-0052 D2: `fallback = false`（＝ `fallback_tier` が無い）なら、届かなくても倒さない
/// （検査もしない。従来どおり `langmem` に出す）。
#[tokio::test]
async fn fallback_false_keeps_the_dedicated_adapter_even_when_the_endpoint_is_down() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let task = knowledge_task(dir.path());
    store.insert(&task).expect("insert");
    let seen = Arc::new(SyncMutex::new(Vec::new()));
    let mut d = knowledge_dispatcher(store.clone(), seen, None, true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    d.set_knowledge_probe(Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reachability::Unreachable {
            reason: "接続できない".into(),
        }
    }));
    run_until_idle(&mut d, 100).await;
    assert_eq!(started_adapter(&store, task.id).as_deref(), Some("langmem"));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "無効なら検査もしない");
}

/// ADR-0052 D2: tier `cheap` の汎用の供給元が 1 つも無ければ、従来どおり dispatch されずに
/// `ready` のまま残る（＝ 供給が戻れば次の tick で拾われる。`retryable = true` と同じ扱い）。
#[tokio::test]
async fn without_any_cheap_generic_provider_the_run_stays_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let task = knowledge_task(dir.path());
    store.insert(&task).expect("insert");
    let seen = Arc::new(SyncMutex::new(Vec::new()));
    let mut d = knowledge_dispatcher(store.clone(), seen.clone(), Some(Tier::Cheap), false);
    d.set_knowledge_probe(Arc::new(|_, _| Reachability::Unreachable {
        reason: "接続できない".into(),
    }));
    for _ in 0..3 {
        assert_eq!(d.tick().expect("tick").dispatched, 0);
    }
    assert_eq!(
        store.get(task.id).expect("get").expect("some").status,
        Status::Ready
    );
    assert!(seen.lock().expect("seen").is_empty());
    assert!(started_adapter(&store, task.id).is_none());
}

/// 知識整理 run 以外（`role` が `knowledge` でない、または adapter が `langmem` でない）は
/// 一切影響を受けない（検査もしない）。
#[tokio::test]
async fn other_tasks_never_trigger_the_probe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let mut task = knowledge_task(dir.path());
    task.role = None;
    task.worker_hint.adapter = Some("fake".into());
    store.insert(&task).expect("insert");
    let seen = Arc::new(SyncMutex::new(Vec::new()));
    let mut d = knowledge_dispatcher(store.clone(), seen, Some(Tier::Cheap), true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    d.set_knowledge_probe(Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Reachability::Unreachable {
            reason: "接続できない".into(),
        }
    }));
    run_until_idle(&mut d, 100).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(started_adapter(&store, task.id).as_deref(), Some("fake"));
}
