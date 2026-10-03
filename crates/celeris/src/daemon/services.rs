//! 主 API 以外の付属サーバ（llm-proxy、MCP）の組み立てと起動・停止。

use std::sync::Arc;
use std::time::Duration;

use task_api::ApiError;
use task_core::SharedRole;
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
    Ok(Some(llm_proxy::ProxyState::new(
        config.llm_proxy.clone(),
        client,
        claude_book,
        codex_book,
        token,
        role,
        Some(config.db.path.clone()),
        config.db.busy_timeout(),
    )))
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
