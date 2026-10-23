//! `McpState`: ハンドラが共有するもの一式（ADR-0056 D5）。`task-api::ApiState` と同じ規律
//! （専用の `SqliteStore` 接続、`spawn_blocking` で同期の store 呼び出しを逃がす）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use task_core::{RoleSpec, SqliteStore, StoreOptions};

use crate::ratelimit::RateLimiter;
use crate::tools::api::ApiTarget;
use crate::tools::db::MAX_CONCURRENT_QUERIES;

/// `McpState::new` の失敗。
#[derive(Debug, thiserror::Error)]
pub enum McpStateError {
    #[error("could not open the store: {0}")]
    Store(#[from] task_core::StoreError),
}

pub struct McpState {
    pub(crate) store: Arc<SqliteStore>,
    pub(crate) rate_limit_per_min: u32,
    pub(crate) limiter: Mutex<RateLimiter>,
    pub(crate) knowledge_root: Option<PathBuf>,
    /// MCP セッション（`initialize` で発行）。`session_id -> client_id`。
    pub(crate) sessions: Mutex<HashMap<String, String>>,
    /// ADR 2026-10-23-mcp-read-only-db-and-api D2: `db_query` / `db_schema` が読み取り専用で開く DB。
    /// `open` は store と同じ path を入れる。
    pub(crate) db_read_path: OnceLock<PathBuf>,
    /// 同時に走らせる `db_query` の数。
    pub(crate) db_query_slots: Arc<tokio::sync::Semaphore>,
    /// ADR 2026-10-23-mcp-read-only-db-and-api D5: `api_get` の宛先（daemon が `[api]` から入れる）。
    pub(crate) api_target: OnceLock<ApiTarget>,
}

impl McpState {
    // Routing arguments remain accepted for constructor compatibility; the chat dispatcher resolves them.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        db_path: &std::path::Path,
        busy_timeout: std::time::Duration,
        rate_limit_per_min: u32,
        _roles: Vec<RoleSpec>,
        _genres: Vec<task_core::GenreSpec>,
        _conversation_genre: String,
        knowledge_root: Option<PathBuf>,
        // ADR-0064 D5: `true` なら `wal_autocheckpoint=0`（celeris の背景チェックポイント tick が
        // 別に打つ前提。デーモンの本番経路だけ `true`）。
        background_checkpoint: bool,
    ) -> Result<Arc<Self>, McpStateError> {
        let store = SqliteStore::open_with(
            db_path,
            StoreOptions {
                busy_timeout,
                background_checkpoint,
                ..StoreOptions::default()
            },
        )?;
        let state = Self::new(Arc::new(store), rate_limit_per_min, knowledge_root);
        let _ = state.db_read_path.set(db_path.to_path_buf());
        Ok(state)
    }

    /// テスト用: 開いてある `SqliteStore` をそのまま共有する（in-memory の店をテストごとに開き直さない
    /// ため）。
    #[allow(clippy::too_many_arguments)]
    pub fn from_store(
        store: Arc<SqliteStore>,
        rate_limit_per_min: u32,
        _roles: Vec<RoleSpec>,
        _genres: Vec<task_core::GenreSpec>,
        _conversation_genre: String,
        knowledge_root: Option<PathBuf>,
    ) -> Arc<Self> {
        Self::new(store, rate_limit_per_min, knowledge_root)
    }

    fn new(
        store: Arc<SqliteStore>,
        rate_limit_per_min: u32,
        knowledge_root: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            rate_limit_per_min,
            limiter: Mutex::new(RateLimiter::default()),
            knowledge_root,
            sessions: Mutex::new(HashMap::new()),
            db_read_path: OnceLock::new(),
            db_query_slots: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_QUERIES)),
            api_target: OnceLock::new(),
        })
    }

    /// `db_query` / `db_schema` が読む DB を決める（`open` は store と同じ path を既に入れている。
    /// `from_store` で作った試験はファイルの DB をここで渡す）。2 回目以降は無視する。
    pub fn set_db_read_path(&self, path: PathBuf) {
        let _ = self.db_read_path.set(path);
    }

    /// `api_get` の宛先を決める（daemon が `[api] listen` と token から作る）。2 回目以降は無視する。
    pub fn set_api_target(&self, target: ApiTarget) {
        let _ = self.api_target.set(target);
    }

    /// 同期の store 呼び出しをブロッキングスレッドへ逃がす（`task_api::ApiState::blocking` と同じ形）。
    pub(crate) async fn blocking<T, F>(&self, f: F) -> T
    where
        T: Send + 'static,
        F: FnOnce(&SqliteStore) -> T + Send + 'static,
    {
        let store = Arc::clone(&self.store);
        match tokio::task::spawn_blocking(move || f(&store)).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "celeris-mcp: blocking task panicked");
                // 呼び出し側は `T` を返せる必要があるため、panic はここでは起こさず伝播させる。
                std::panic::resume_unwind(e.into_panic())
            }
        }
    }

    /// `initialize` で新しいセッションを発行する。
    pub(crate) fn create_session(&self, client_id: &str) -> String {
        let id = ulid::Ulid::new().to_string();
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), client_id.to_string());
        id
    }

    /// そのセッションがこのクライアントのものか（`initialize` 後の全メソッドで要る）。
    pub(crate) fn session_client(&self, session_id: &str) -> Option<String> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
    }
}
