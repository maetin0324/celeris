//! 主 API 以外の付属サーバ（llm-proxy、MCP）の組み立てと起動・停止。

use std::sync::Arc;
use std::time::Duration;

use llm_proxy::routing_context::{ProxyEventSink, ProxyRoutingEvent};
use task_api::ApiError;
use task_core::model_router::context_registry::RoutingContextRegistry;
use task_core::{Event, SharedRole, TaskId, TaskStore};
use task_dispatch::Dispatcher;

use super::api::bind_reuseport;
use crate::{Config, DaemonError};

/// 動いている LLM source プロキシ（ADR-0053 D1。Phase 65）。`stop` で graceful に止める。
pub(crate) struct RunningLlmProxy {
    stop: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl RunningLlmProxy {
    pub(crate) async fn stop(self) {
        let _ = self.stop.send(());
        match tokio::time::timeout(Duration::from_secs(5), self.handle).await {
            Ok(Ok(Ok(()))) => tracing::info!("llm-proxy stopped"),
            Ok(Ok(Err(e))) => tracing::error!(error = %e, "llm-proxy server failed"),
            Ok(Err(e)) => tracing::error!(error = %e, "llm-proxy task panicked"),
            Err(_) => tracing::warn!("llm-proxy did not stop within 5s"),
        }
    }
}

/// 動いている MCP サーバー（ADR-0056 D1/D5。Phase 78）。口ごとに別の listener を持つので、
/// 止めるときは全部の handle をまとめて待つ。
pub(crate) struct RunningMcp {
    listeners: Vec<(
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<std::io::Result<()>>,
    )>,
}

impl RunningMcp {
    pub(crate) async fn stop(self) {
        for (stop, handle) in self.listeners {
            let _ = stop.send(());
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(Ok(()))) => tracing::info!("mcp listener stopped"),
                Ok(Ok(Err(e))) => tracing::error!(error = %e, "mcp listener failed"),
                Ok(Err(e)) => tracing::error!(error = %e, "mcp listener task panicked"),
                Err(_) => tracing::warn!("mcp listener did not stop within 5s"),
            }
        }
    }
}

/// ADR-0056 D1/D5（Phase 78）: `[mcp]` が有効なら `celeris_mcp::McpState` を組み立てる（bind はまだ
/// しない）。`[mcp]` は独立の DB 接続を持つ（`task-api` の `ApiState` と同じ多重接続の流儀）。
pub(crate) fn build_mcp_state(
    config: &Config,
) -> Result<Option<Arc<celeris_mcp::McpState>>, DaemonError> {
    if !config.mcp.effective_enabled() {
        return Ok(None);
    }
    let state = celeris_mcp::McpState::open(
        &config.db.path,
        config.db.busy_timeout(),
        config.mcp.rate_limit_per_min,
        config.role_specs(),
        config.genre_specs(),
        config.conversation_genre_id().to_string(),
        Some(config.knowledge.root.clone()),
        // ADR-0064 D5: MCP もデーモン内の接続なので背景チェックポイント側に回す。
        true,
    )
    .map_err(|e| ApiError::Startup(format!("mcp: could not open the store: {e}")))?;
    Ok(Some(state))
}

/// `state` を `[mcp]`/`[[mcp.listeners]]` の全ての口に bind して動かす（`config.mcp.resolve_listeners()`
/// は `Config::validate` が既に検査済み。ここで失敗するのは bind そのものだけ）。
pub(crate) async fn start_mcp(
    config: &Config,
    state: Arc<celeris_mcp::McpState>,
) -> Result<RunningMcp, DaemonError> {
    let resolved = config
        .mcp
        .resolve_listeners()
        .map_err(|e| ApiError::Startup(format!("mcp: {e}")))?;
    let mut listeners = Vec::with_capacity(resolved.len());
    for listener in resolved {
        let bound = bind_reuseport(listener.listen).map_err(|source| ApiError::Bind {
            addr: listener.listen,
            source,
        })?;
        let addr = bound.local_addr().unwrap_or(listener.listen);
        tracing::info!(%addr, auth = ?listener.auth, "mcp listening");
        let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(celeris_mcp::serve(
            bound,
            Arc::clone(&state),
            listener.auth,
            async move {
                let _ = stop_rx.await;
            },
        ));
        listeners.push((stop, handle));
    }
    Ok(RunningMcp { listeners })
}

/// ADR-0053 D1（Phase 65）: `[llm_proxy]` が有効なら `llm_proxy::ProxyState` を組み立てる（bind はまだ
/// しない。`GET /llm/sources`（主 API）とプロキシ自身の両方がこの同じ `Arc` を使うため、`run()` が
/// 主 API の起動より前に 1 度だけ呼ぶ）。アカウントプール（cooldown・観測値）はディスパッチャの帳簿を
/// **そのまま共有する**（`crates/task-dispatch/src/dispatcher.rs` の `account_book`。別の写しを作らない）。
/// Bearer は `[api] token_file` と同じ（`docs/guides/llm-source.md`）。
pub(crate) fn build_llm_proxy_state(
    config: &Config,
    dispatcher: &Dispatcher,
    role: SharedRole,
    registry: Arc<dyn RoutingContextRegistry>,
) -> Result<Option<Arc<llm_proxy::ProxyState>>, DaemonError> {
    if !config.llm_proxy.effective_enabled() {
        return Ok(None);
    }
    let token = config.api.read_token()?;
    let client = reqwest::Client::builder().build().map_err(|e| {
        ApiError::Startup(format!("llm-proxy: could not build the HTTP client: {e}"))
    })?;
    let claude_book = dispatcher.account_book(task_core::AccountAdapter::ClaudeCode);
    let codex_book = dispatcher.account_book(task_core::AccountAdapter::Codex);
    let state = llm_proxy::ProxyState::new(
        config.llm_proxy.clone(),
        client,
        claude_book,
        codex_book,
        token,
        role,
        Some(config.db.path.clone()),
        config.db.busy_timeout(),
    );
    // ADR 2026-10-04 Phase 2: `[model_routing.retry]` の分類別上限と breaker（未設定は proxy の既定）。
    // proxy は起動時に 1 度だけ組むので、retry の変更は再起動で効く（reload では変えない）。
    let state = match &config.model_routing.runtime {
        Some(runtime) => state.with_fallback(
            runtime.fallback.clone(),
            Arc::new(llm_proxy::reservation::SystemClock),
        ),
        None => state,
    };
    let sink: Arc<dyn ProxyEventSink> = Arc::new(TaskProxyEventSink::new(dispatcher.store()));
    Ok(Some(state.with_routing_context(Some(registry), Some(sink))))
}

/// proxy 要求の task event 追記。DB 操作は blocking pool へ送り、HTTP 応答を待たせない。
struct TaskProxyEventSink {
    store: Arc<dyn TaskStore>,
    append_lock: Arc<std::sync::Mutex<()>>,
}

impl TaskProxyEventSink {
    fn new(store: Arc<dyn TaskStore>) -> Self {
        Self {
            store,
            append_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }
}

impl ProxyEventSink for TaskProxyEventSink {
    fn parent_decision(&self, run_id: &str) -> Option<String> {
        let run = self.store.run_index_get(run_id).ok()??;
        let task_id: TaskId = run.task_id.parse().ok()?;
        self.store
            .events_for(task_id)
            .ok()?
            .into_iter()
            .find_map(|(_, event)| match event {
                Event::RoutingDecided { run_id: id, record } if id == run_id => {
                    record.optimizer.map(|trace| trace.decision_id)
                }
                _ => None,
            })
    }

    fn record(&self, event: ProxyRoutingEvent) {
        let store = Arc::clone(&self.store);
        let lock = Arc::clone(&self.append_lock);
        tokio::task::spawn_blocking(move || {
            if let Err(error) = append_proxy_event(store.as_ref(), &lock, event) {
                tracing::warn!(%error, "failed to append proxy routing event");
            }
        });
    }
}

fn append_proxy_event(
    store: &dyn TaskStore,
    lock: &std::sync::Mutex<()>,
    event: ProxyRoutingEvent,
) -> Result<(), String> {
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let task_id: TaskId = event
        .task_id
        .parse()
        .map_err(|e| format!("invalid task id: {e}"))?;
    let run_id = event
        .request
        .run_id
        .as_deref()
        .ok_or("proxy event has no run id")?;
    // 登録済み context に含まれる task id も、DB の run 所属と照合する。推定で task に結ばない。
    if !store
        .run_index_get(run_id)
        .map_err(|e| e.to_string())?
        .is_some_and(|r| r.task_id == event.task_id)
    {
        return Err("proxy event run does not belong to task".into());
    }
    if event.features.request_id.as_deref() != Some(event.request.request_id.as_str())
        || event.features.decision_id != event.request.decision_id
    {
        return Err("proxy event feature/request identity differs".into());
    }
    let prior = store.events_for(task_id).map_err(|e| e.to_string())?;
    if prior.iter().any(|(_, prior)| matches!(prior, Event::RoutingRequestDecided { record }
        if record.request_id == event.request.request_id && record.decision_id == event.request.decision_id)) {
        return Ok(());
    }
    store
        .append_event(
            task_id,
            &Event::RoutingFeaturesRecorded {
                record: Box::new(event.features),
            },
        )
        .map_err(|e| e.to_string())?;
    store
        .append_event(
            task_id,
            &Event::RoutingRequestDecided {
                record: Box::new(event.request),
            },
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// `state` を `[llm_proxy] listen` に bind して動かす。`standby`/`draining` の間はプロキシ自身の
/// `guard` が他の管理 API と同じく 503 を返す（bind/unbind は主 API と同じ `SO_REUSEPORT` のライフサイクル）。
pub(crate) async fn start_llm_proxy(
    config: &Config,
    state: Arc<llm_proxy::ProxyState>,
) -> Result<RunningLlmProxy, DaemonError> {
    let listen = config.llm_proxy.listen;
    let listener = bind_reuseport(listen).map_err(|source| ApiError::Bind {
        addr: listen,
        source,
    })?;
    let addr = listener.local_addr().unwrap_or(listen);
    tracing::info!(%addr, "llm-proxy listening");
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(llm_proxy::serve(listener, state, async move {
        let _ = stop_rx.await;
    }));
    Ok(RunningLlmProxy { stop, handle })
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use task_core::execution_plan::{RunIndexRole, RunIndexStatus, RunRow};
    use task_core::model::{Budget, Status, Task, TaskKind, Tier, WorkerHint, WorkspaceSpec};
    use task_core::model_router::feedback::{
        FeatureStage, RoutingFeaturesRecord, RoutingRequestRecord,
    };

    /// daemon は 1 つの registry を dispatcher と proxy の両方に渡す（proxy 側が黙って捨てない）。
    #[test]
    fn one_registry_is_shared_by_the_dispatcher_and_the_proxy() {
        use task_core::model_router::context_registry::InMemoryRoutingContextRegistry;
        let dir = tempfile::tempdir().unwrap();
        let token = dir.path().join("token");
        std::fs::write(&token, "t\n").unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!(
                "db = {:?}\nworkspace_root = {:?}\n[api]\ntoken_file = {token:?}\n\
                 [llm_proxy]\nenabled = true\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
                dir.path().join("db.sqlite3"),
                dir.path().join("ws"),
            ),
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        let mut dispatcher =
            crate::daemon::bootstrap::build_dispatcher(&config, Default::default()).unwrap();
        let registry: Arc<dyn RoutingContextRegistry> =
            Arc::new(InMemoryRoutingContextRegistry::new());
        dispatcher.set_routing_context_registry(Arc::clone(&registry));
        assert_eq!(Arc::strong_count(&registry), 2);
        let role = SharedRole::new(task_core::InstanceRole::Active);
        let state = build_llm_proxy_state(&config, &dispatcher, role, Arc::clone(&registry))
            .unwrap()
            .expect("proxy enabled");
        // test・dispatcher・proxy の 3 者が同じ Arc を持つ（with_routing_context が効いた）。
        assert_eq!(Arc::strong_count(&registry), 3);
        drop(state);
        assert_eq!(Arc::strong_count(&registry), 2);
    }

    #[test]
    fn proxy_sink_appends_only_correlated_requests_once() {
        let store = task_core::SqliteStore::open_in_memory().unwrap();
        let now = time::OffsetDateTime::now_utc();
        let task = Task {
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "routing".into(),
            objective: "routing".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Draft,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::local("/tmp/routing-test"),
            repos: vec![],
            budget: Budget {
                max_turns: 1,
                max_wall_secs: 10,
                max_retries: 0,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            labels: vec![],
            category: Default::default(),
            skills: vec![],
            mode: Default::default(),
            conversation: None,
            routing: None,
            paused_at: None,
            tree: None,
        };
        store.insert(&task).unwrap();
        store
            .run_index_start(RunRow {
                run_id: "run-1".into(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: RunIndexRole::Worker,
                seq: 1,
                status: RunIndexStatus::Running,
                adapter: None,
                model: None,
                account: None,
                session_id: None,
                checkpoint: None,
                usage: None,
                metrics: None,
                started_at: "2026-10-05T00:00:00Z".into(),
                finished_at: None,
            })
            .unwrap();
        let event = ProxyRoutingEvent {
            task_id: task.id.to_string(),
            features: RoutingFeaturesRecord {
                decision_id: "d1".into(),
                context_version: "1".into(),
                features: serde_json::json!({}),
                provenance: Default::default(),
                missing_fields: vec![],
                run_id: Some("run-1".into()),
                request_id: Some("req-1".into()),
                stage: Some(FeatureStage::Proxy),
            },
            request: RoutingRequestRecord {
                request_id: "req-1".into(),
                decision_id: "d1".into(),
                parent_decision_id: None,
                run_id: Some("run-1".into()),
                trace: None,
                attempts: vec![],
                fallback_reason: None,
            },
        };
        let lock = std::sync::Mutex::new(());
        append_proxy_event(&store, &lock, event.clone()).unwrap();
        append_proxy_event(&store, &lock, event.clone()).unwrap();
        assert_eq!(store.events_for(task.id).unwrap().len(), 2);
        let mut spoofed = event;
        spoofed.request.run_id = Some("other-run".into());
        assert!(append_proxy_event(&store, &lock, spoofed).is_err());
        assert_eq!(store.events_for(task.id).unwrap().len(), 2);
    }
}
