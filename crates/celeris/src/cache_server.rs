//! `celeris cache-server`（ADR-0075 D5 (b)、Phase G3）: sccache の webdav backend に対する Celeris の階層 cache
//! server を `127.0.0.1:<[scratch.cache_server] port>` で動かす。常駐は専用の user unit `celeris-scratch-cache.service`
//! （`celeris@` の cgroup の外。昇格で自動的には再起動しない）。

use std::time::Duration;

use scratch_cache::store::MB;
use scratch_cache::{StoreConfig, TieredStore};
use task_worker::scratch::ScratchSettings;

use crate::Config;

/// `[scratch]` / `[scratch.l2]` から `TieredStore` の設定を組む（純粋）。
pub fn store_config(s: &ScratchSettings) -> StoreConfig {
    let l2 = &s.l2;
    let mut c = StoreConfig::new(s.pool().cache_l1_dir(), l2.enabled.then(|| l2.dir.clone()));
    c.l1_max_bytes = s.l1_max_bytes;
    c.l2_max_bytes = l2.max_bytes;
    c.flush_bytes_per_sec = l2.flush_mbps.saturating_mul(MB);
    // 1 秒分を貯められる（大きな entry 1 つでも待ちは rate で決まる。`TokenBucket` は負債を許す）。
    c.flush_burst_bytes = c.flush_bytes_per_sec;
    c.flush_queue_max_bytes = l2.flush_queue_max_mb.saturating_mul(MB);
    c.l2_get_timeout = Duration::from_millis(l2.get_timeout_ms.max(1));
    c.l2_threads = l2.io_threads.max(1);
    c.l2_gc_interval = Duration::from_secs(l2.gc_interval_secs);
    c
}

/// cache server を動かす（SIGTERM / SIGINT で止まり、未 flush の key は `.pending` に残る）。
pub async fn run(config: &Config) -> Result<(), String> {
    let settings = config.scratch_settings();
    if !settings.enabled {
        return Err(format!(
            "scratch is disabled{}; the cache server keeps its L1 in the scratch pool",
            settings
                .disabled_reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        ));
    }
    let cs = settings.cache_server.clone();
    if !cs.enabled {
        return Err("[scratch.cache_server] enabled = false".to_string());
    }
    let token = task_worker::scratch::ensure_token(&cs.token_file)
        .map_err(|e| format!("token {}: {e}", cs.token_file.display()))?;
    let cfg = store_config(&settings);
    tracing::info!(
        l1 = %cfg.l1_dir.display(),
        l1_max_bytes = cfg.l1_max_bytes,
        l2 = ?cfg.l2_dir,
        l2_max_bytes = cfg.l2_max_bytes,
        flush_mbps = settings.l2.flush_mbps,
        port = cs.port,
        "scratch cache server starting (ADR-0075 D5)"
    );
    let store = tokio::task::spawn_blocking(move || TieredStore::open(cfg))
        .await
        .map_err(|e| format!("open the store: {e}"))?
        .map_err(|e| format!("open the store: {e}"))?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", cs.port))
        .await
        .map_err(|e| format!("bind 127.0.0.1:{}: {e}", cs.port))?;
    let router = scratch_cache::server::router(
        store.clone(),
        scratch_cache::server::ServerConfig {
            token: Some(token),
            ..Default::default()
        },
    );
    let shutdown = async {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    };
    tracing::info!(
        port = cs.port,
        "scratch cache server listening on 127.0.0.1"
    );
    let served = scratch_cache::server::serve(listener, router, shutdown).await;
    let stats = store.stats();
    let _ = tokio::task::spawn_blocking(move || store.shutdown()).await;
    tracing::info!(
        pending = stats.flush_queue_len,
        l1_hits = stats.l1_hits,
        l2_hits = stats.l2_hits,
        misses = stats.misses,
        "scratch cache server stopped"
    );
    served.map_err(|e| format!("serve: {e}"))
}
