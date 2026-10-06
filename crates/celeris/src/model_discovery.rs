//! ADR 2026-10-06 D4: 利用可能モデルの発見（取得の側）。
//!
//! 解析（文字列 → `DiscoveredModel`）と保存は `task-core::model_catalog`・`store::model_catalog`。ここは
//! HTTP とコマンド実行だけを行い、LLM は呼ばない。取得に失敗した source は失敗の記録だけを残し、catalog は
//! 変えない（消えたと誤認しない）。資格情報（claude の OAuth token・self-host の API key）は要求にだけ使い、
//! ログ・エラー文・events に値を出さない。
//!
//! 周期の実行は [`DiscoveryTicker`]（tick loop が `tick` を呼ぶ。実際の取得は `tokio::spawn` した先で、
//! hot path を止めない。2 つ重ならない）。手動は `POST /api/v1/llm/models/discover`（`ModelDiscoveryHook`）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use task_core::model_catalog::{
    CatalogDelta, CatalogSource, DiscoveredModel, parse_codex_model_list, parse_openai_models_list,
    parse_opencode_models_stdout,
};
use task_core::{LlmSourceRef, ModelCatalogStore, TaskStore};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::config::Config;

/// HTTP 1 回の上限。
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// CLI 1 回（`opencode models` / `codex app-server` 一式）の上限。
const CLI_TIMEOUT: Duration = Duration::from_secs(40);
/// codex の `model/list` で辿るページ数の上限（無限ループ防止）。
const CODEX_MAX_PAGES: usize = 20;
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// 発見の対象 1 つ。
#[derive(Clone)]
pub enum DiscoveryTarget {
    OpencodeGo {
        models_url: String,
        cli_command: String,
    },
    ClaudeOauth {
        base_url: String,
        /// logged-in な account の dir（無ければ `None`。失敗として記録する）。
        account_dir: Option<PathBuf>,
    },
    CodexOauth {
        command: String,
        account_dir: Option<PathBuf>,
    },
    OpenaiCompatible {
        id: String,
        base_url: String,
        api_key: Option<String>,
    },
}

// api_key を Debug に出さない。
impl std::fmt::Debug for DiscoveryTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpencodeGo {
                models_url,
                cli_command,
            } => f
                .debug_struct("OpencodeGo")
                .field("models_url", models_url)
                .field("cli_command", cli_command)
                .finish(),
            Self::ClaudeOauth {
                base_url,
                account_dir,
            } => f
                .debug_struct("ClaudeOauth")
                .field("base_url", base_url)
                .field("account_dir", account_dir)
                .finish(),
            Self::CodexOauth {
                command,
                account_dir,
            } => f
                .debug_struct("CodexOauth")
                .field("command", command)
                .field("account_dir", account_dir)
                .finish(),
            Self::OpenaiCompatible { id, base_url, .. } => f
                .debug_struct("OpenaiCompatible")
                .field("id", id)
                .field("base_url", base_url)
                .field("api_key", &"<redacted>")
                .finish(),
        }
    }
}

impl DiscoveryTarget {
    pub fn source(&self) -> CatalogSource {
        match self {
            Self::OpencodeGo { .. } => CatalogSource::new(CatalogSource::OPENCODE_GO),
            Self::ClaudeOauth { .. } => CatalogSource::new(CatalogSource::CLAUDE_OAUTH),
            Self::CodexOauth { .. } => CatalogSource::new(CatalogSource::CODEX_OAUTH),
            Self::OpenaiCompatible { id, .. } => CatalogSource::openai_compatible(id),
        }
    }
}

/// 1 source の発見結果の要約。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySummary {
    pub source: String,
    pub ok: bool,
    pub count: u32,
    pub error: Option<String>,
    pub delta: CatalogDelta,
}

/// account 根の下で、`marker` を持つ最初の dir（名前順）。
fn first_logged_in(root: &Path, marker: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.into_iter().find(|d| d.join(marker).is_file())
}

/// 設定から発見の対象を組む。設定にない source は対象にしない。
pub fn targets_from_config(config: &Config) -> Vec<DiscoveryTarget> {
    let mut out = Vec::new();
    let accounts = config.accounts.as_ref();
    let opencode_in_use = accounts.is_some_and(|a| a.opencode_dir.is_some())
        || config.providers.iter().any(|p| {
            config
                .provider_llm_source(&p.id)
                .is_some_and(|r| r.source == LlmSourceRef::OpencodeGo)
        });
    if opencode_in_use {
        // `[model_catalog] opencode_cli` を明示していなければ、acp adapter の command（既定 `opencode`）を使う。
        let cli = if config.model_catalog.opencode_cli == "opencode" {
            config.adapters.acp.command.clone()
        } else {
            config.model_catalog.opencode_cli.clone()
        };
        out.push(DiscoveryTarget::OpencodeGo {
            models_url: config.model_catalog.opencode_go_models_url.clone(),
            cli_command: cli,
        });
    }
    let claude = config.llm_proxy.sources.claude_oauth.as_ref();
    if claude.is_some() || accounts.is_some_and(|a| a.claude_dir.is_some()) {
        let root = claude
            .map(|c| c.accounts_dir.clone())
            .filter(|d| !d.as_os_str().is_empty())
            .or_else(|| accounts.and_then(|a| a.claude_dir.clone()));
        out.push(DiscoveryTarget::ClaudeOauth {
            base_url: claude
                .map(|c| c.base_url.clone())
                .unwrap_or_else(|| DEFAULT_ANTHROPIC_BASE_URL.to_string()),
            account_dir: root.and_then(|r| first_logged_in(&r, ".credentials.json")),
        });
    }
    let codex = config.llm_proxy.sources.codex_oauth.as_ref();
    if codex.is_some() || accounts.is_some_and(|a| a.codex_dir.is_some()) {
        let root = codex
            .map(|c| c.accounts_dir.clone())
            .filter(|d| !d.as_os_str().is_empty())
            .or_else(|| accounts.and_then(|a| a.codex_dir.clone()));
        out.push(DiscoveryTarget::CodexOauth {
            command: config.adapters.codex.command.clone(),
            account_dir: root.and_then(|r| first_logged_in(&r, "auth.json")),
        });
    }
    for src in &config.llm_proxy.sources.openai_compatible {
        if src.enabled {
            out.push(DiscoveryTarget::OpenaiCompatible {
                id: src.id.clone(),
                base_url: src.base_url.clone(),
                api_key: src.api_key.clone(),
            });
        }
    }
    out
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// `GET url` の本文（2xx のみ）。エラー文に URL のクエリ・資格情報を入れない。
async fn http_get_text(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
) -> Result<String, String> {
    let mut req = client.get(url);
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    resp.text()
        .await
        .map_err(|e| format!("cannot read the response: {}", describe_reqwest(&e)))
}

fn describe_reqwest(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connection failed"
    } else {
        "network error"
    }
}

fn non_empty(models: Vec<DiscoveredModel>, what: &str) -> Result<Vec<DiscoveredModel>, String> {
    if models.is_empty() {
        // 空の一覧で全モデルを「消えた」にしない。
        Err(format!("{what} returned an empty model list"))
    } else {
        Ok(models)
    }
}

async fn discover_opencode_go(
    models_url: &str,
    cli_command: &str,
) -> Result<Vec<DiscoveredModel>, String> {
    let http = async {
        let client = http_client()?;
        let body = http_get_text(&client, models_url, &[]).await?;
        non_empty(parse_openai_models_list(&body)?, "gateway")
    }
    .await;
    let http_err = match http {
        Ok(models) => return Ok(models),
        Err(e) => e,
    };
    let cli = async {
        let output = tokio::time::timeout(
            CLI_TIMEOUT,
            Command::new(cli_command)
                .args(["models", CatalogSource::OPENCODE_GO])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "timeout".to_string())?
        .map_err(|e| format!("cannot run {cli_command}: {}", e.kind()))?;
        if !output.status.success() {
            return Err(format!(
                "{cli_command} models exited with {}",
                output.status
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        non_empty(
            parse_opencode_models_stdout(&stdout, CatalogSource::OPENCODE_GO),
            "opencode models",
        )
    }
    .await;
    cli.map_err(|cli_err| format!("gateway: {http_err}; cli: {cli_err}"))
}

async fn discover_claude(
    base_url: &str,
    account_dir: Option<&Path>,
) -> Result<Vec<DiscoveredModel>, String> {
    let dir = account_dir.ok_or_else(|| "no logged-in claude account".to_string())?;
    let value = llm_proxy::credentials::read_json(
        &dir.join(llm_proxy::credentials::CLAUDE_CREDENTIALS_FILE),
    )
    .map_err(|e| e.to_string())?;
    let tokens = llm_proxy::credentials::parse_claude_tokens(&value).map_err(|e| e.to_string())?;
    let now_ms = time::OffsetDateTime::now_utc().unix_timestamp() * 1000;
    if tokens.expires_at_ms <= now_ms {
        // トークンの更新は proxy の使用時に行う。ここでは資格情報ファイルに触れない。
        return Err("claude OAuth token is expired (refreshed on next use)".to_string());
    }
    let client = http_client()?;
    let url = format!("{}/v1/models?limit=1000", base_url.trim_end_matches('/'));
    let body = http_get_text(
        &client,
        &url,
        &[
            ("anthropic-version", ANTHROPIC_VERSION.to_string()),
            ("anthropic-beta", ANTHROPIC_BETA.to_string()),
            ("authorization", format!("Bearer {}", tokens.access_token)),
        ],
    )
    .await?;
    non_empty(parse_openai_models_list(&body)?, "anthropic")
}

async fn discover_openai_compatible(
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<DiscoveredModel>, String> {
    let client = http_client()?;
    // `base_url` は `/v1` までを含む（llm-proxy の到達性 probe と同じ）。
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let headers: Vec<(&str, String)> = api_key
        .map(|k| vec![("authorization", format!("Bearer {k}"))])
        .unwrap_or_default();
    let body = http_get_text(&client, &url, &headers).await?;
    non_empty(parse_openai_models_list(&body)?, "server")
}

/// 1 行 1 JSON の応答を待つ（`id` が合う行だけ。通知は読み飛ばす）。
async fn codex_rpc(
    input: &mut tokio::process::ChildStdin,
    output: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let msg = serde_json::json!({"id": id, "method": method, "params": params});
    input
        .write_all(format!("{msg}\n").as_bytes())
        .await
        .map_err(|_| "app-server input closed".to_string())?;
    loop {
        let line = output
            .next_line()
            .await
            .map_err(|_| "app-server output unreadable".to_string())?
            .ok_or_else(|| "app-server closed before responding".to_string())?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
            continue;
        }
        if value.get("error").is_some() {
            // サーバの本文には account 情報が入りうるので転送しない。
            return Err(format!("codex {method} failed"));
        }
        return value
            .get("result")
            .cloned()
            .ok_or_else(|| "app-server result missing".to_string());
    }
}

async fn discover_codex(
    command: &str,
    account_dir: Option<&Path>,
) -> Result<Vec<DiscoveredModel>, String> {
    let dir = account_dir.ok_or_else(|| "no logged-in codex account".to_string())?;
    let work = async {
        let mut child = Command::new(command)
            .arg("app-server")
            .env("CODEX_HOME", dir)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start codex app-server: {}", e.kind()))?;
        let mut input = child.stdin.take().ok_or("app-server stdin missing")?;
        let mut output =
            BufReader::new(child.stdout.take().ok_or("app-server stdout missing")?).lines();
        codex_rpc(
            &mut input,
            &mut output,
            1,
            "initialize",
            serde_json::json!({
                "clientInfo": {"name": "celeris", "version": env!("CARGO_PKG_VERSION")}
            }),
        )
        .await?;
        input
            .write_all(b"{\"method\":\"initialized\",\"params\":{}}\n")
            .await
            .map_err(|_| "app-server input closed".to_string())?;
        let mut models: Vec<DiscoveredModel> = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..CODEX_MAX_PAGES {
            let params = match &cursor {
                Some(c) => serde_json::json!({"includeHidden": false, "cursor": c}),
                None => serde_json::json!({"includeHidden": false}),
            };
            let result = codex_rpc(
                &mut input,
                &mut output,
                2 + page as u64,
                "model/list",
                params,
            )
            .await?;
            let (found, next) = parse_codex_model_list(&result)?;
            for m in found {
                if !models.iter().any(|x| x.model_id == m.model_id) {
                    models.push(m);
                }
            }
            match next {
                Some(n) => cursor = Some(n),
                None => break,
            }
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
        Ok::<_, String>(models)
    };
    let models = tokio::time::timeout(CLI_TIMEOUT, work)
        .await
        .map_err(|_| "timeout".to_string())??;
    non_empty(models, "codex")
}

/// 1 source を取得する。失敗は `Err`（理由に資格情報の値を含めない）。
pub async fn discover_one(target: &DiscoveryTarget) -> Result<Vec<DiscoveredModel>, String> {
    match target {
        DiscoveryTarget::OpencodeGo {
            models_url,
            cli_command,
        } => discover_opencode_go(models_url, cli_command).await,
        DiscoveryTarget::ClaudeOauth {
            base_url,
            account_dir,
        } => discover_claude(base_url, account_dir.as_deref()).await,
        DiscoveryTarget::CodexOauth {
            command,
            account_dir,
        } => discover_codex(command, account_dir.as_deref()).await,
        DiscoveryTarget::OpenaiCompatible {
            base_url, api_key, ..
        } => discover_openai_compatible(base_url, api_key.as_deref()).await,
    }
}

/// `targets` を順に発見して catalog に反映する。失敗は記録だけして catalog を変えない。
pub async fn run_targets<S: ModelCatalogStore + ?Sized>(
    store: &S,
    targets: &[DiscoveryTarget],
    only_source: Option<&str>,
    now: i64,
) -> Vec<DiscoverySummary> {
    let mut out = Vec::new();
    for target in targets {
        let source = target.source();
        if only_source.is_some_and(|s| s != source.as_str()) {
            continue;
        }
        let result = discover_one(target).await;
        let summary = match result {
            Ok(models) => match store.model_catalog_apply(&source, &models, now) {
                Ok(delta) => DiscoverySummary {
                    source: source.0.clone(),
                    ok: true,
                    count: u32::try_from(models.len()).unwrap_or(u32::MAX),
                    error: None,
                    delta,
                },
                Err(e) => failure_summary(store, &source, format!("store: {e}"), now),
            },
            Err(error) => failure_summary(store, &source, error, now),
        };
        out.push(summary);
    }
    out
}

fn failure_summary<S: ModelCatalogStore + ?Sized>(
    store: &S,
    source: &CatalogSource,
    error: String,
    now: i64,
) -> DiscoverySummary {
    if let Err(e) = store.model_catalog_record_failure(source, &error, now) {
        tracing::warn!(source = %source, error = %e, "model discovery: could not record the failure");
    }
    DiscoverySummary {
        source: source.0.clone(),
        ok: false,
        count: 0,
        error: Some(error),
        delta: CatalogDelta {
            source: source.0.clone(),
            ..CatalogDelta::default()
        },
    }
}

/// 設定の全 source（または `only_source` だけ）を発見して反映する。
pub async fn run_discovery<S: ModelCatalogStore + ?Sized>(
    store: &S,
    config: &Config,
    only_source: Option<&str>,
    now: i64,
) -> Vec<DiscoverySummary> {
    run_targets(store, &targets_from_config(config), only_source, now).await
}

/// 発見の周期実行の状態。`last_run` は試験から注入できる。
#[derive(Debug, Default)]
pub struct DiscoveryTicker {
    pub last_run: Option<i64>,
    in_flight: Arc<AtomicBool>,
}

struct InFlightGuard(Arc<AtomicBool>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl DiscoveryTicker {
    /// 周期が来ていて、前の取得が終わっていれば `true`（`refresh_interval_seconds = 0` は常に `false`）。
    pub fn due(&self, interval_secs: u64, now: i64) -> bool {
        if interval_secs == 0 || self.in_flight.load(Ordering::SeqCst) {
            return false;
        }
        match self.last_run {
            None => true,
            Some(last) => {
                now.saturating_sub(last) >= i64::try_from(interval_secs).unwrap_or(i64::MAX)
            }
        }
    }

    /// 周期が来ていれば取得を `tokio::spawn` する（tick を止めない）。始めたら `JoinHandle` を返す。
    /// 取得が終わるまで次は始めない。`after` は反映後に同期で呼ばれる（routing catalog の更新など）。
    pub fn tick(
        &mut self,
        store: Arc<dyn TaskStore>,
        config: &Config,
        now: i64,
        after: impl FnOnce(&[DiscoverySummary]) + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<Vec<DiscoverySummary>>> {
        if !self.due(config.model_catalog.refresh_interval_seconds, now) {
            return None;
        }
        let targets = targets_from_config(config);
        if targets.is_empty() {
            self.last_run = Some(now);
            return None;
        }
        self.last_run = Some(now);
        self.in_flight.store(true, Ordering::SeqCst);
        let guard = InFlightGuard(Arc::clone(&self.in_flight));
        Some(tokio::spawn(async move {
            let _guard = guard;
            let summaries = run_targets(store.as_ref(), &targets, None, now).await;
            for s in &summaries {
                match &s.error {
                    None => tracing::info!(
                        source = %s.source, count = s.count,
                        added = s.delta.added.len(), removed = s.delta.removed.len(),
                        restored = s.delta.restored.len(), "model discovery: ok"
                    ),
                    Some(e) => {
                        tracing::warn!(source = %s.source, error = %e, "model discovery: failed")
                    }
                }
            }
            after(&summaries);
            summaries
        }))
    }
}

/// config から routing catalog を組み直し、catalog の `available` と上書きを当てて共有 snapshot
/// （`routing_catalog_state`）を差し替える。共有 snapshot が無ければ何もしない。
/// 発見の後・reload の後に呼ぶ（catalog は config を足さない・絞るだけ）。
pub fn refresh_routing_catalog<S: ModelCatalogStore + ?Sized>(store: &S, config: &Config) {
    let Some(shared) = config.routing_catalog_state.as_ref() else {
        return;
    };
    let mut catalog = match config.routing_catalog() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "model catalog: routing catalog could not be rebuilt");
            return;
        }
    };
    let (entries, overrides) = match (store.model_catalog_list(), store.model_catalog_overrides()) {
        (Ok(e), Ok(o)) => (e, o),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(error = %e, "model catalog: could not read the catalog");
            return;
        }
    };
    let dropped = crate::config::apply_model_catalog(&mut catalog, &entries, &overrides);
    if !dropped.is_empty() {
        tracing::info!(
            count = dropped.len(),
            "model catalog: deployments excluded from routing"
        );
    }
    *shared
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(catalog);
}

/// reload で組み直した `routing_catalog_snapshot` に catalog を当てる。外す deployment があるときだけ、
/// 当てた写しで snapshot と共有 snapshot の両方を差し替える（無ければ何も変えない）。
pub fn apply_catalog_to_snapshot<S: ModelCatalogStore + ?Sized>(store: &S, config: &mut Config) {
    let Some(snapshot) = config.routing_catalog_snapshot.as_ref() else {
        return;
    };
    let (Ok(entries), Ok(overrides)) =
        (store.model_catalog_list(), store.model_catalog_overrides())
    else {
        return;
    };
    let mut applied = (**snapshot).clone();
    if crate::config::apply_model_catalog(&mut applied, &entries, &overrides).is_empty() {
        return;
    }
    let applied = Arc::new(applied);
    if let Some(shared) = config.routing_catalog_state.as_ref() {
        *shared
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::clone(&applied);
    }
    config.routing_catalog_snapshot = Some(applied);
}

/// `POST /api/v1/llm/models/discover` の実体（daemon が API の設定へ差す）。起動時の設定の写しで発見し、
/// 反映後に routing catalog を更新する。
pub struct DaemonDiscoveryHook {
    pub store: Arc<dyn TaskStore>,
    pub config: Arc<Config>,
}

impl task_api::ModelDiscoveryHook for DaemonDiscoveryHook {
    fn discover<'a>(
        &'a self,
        source: Option<String>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<task_api::DiscoverySummaryView>> + Send + 'a>,
    > {
        Box::pin(async move {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            let summaries =
                run_discovery(self.store.as_ref(), &self.config, source.as_deref(), now).await;
            refresh_routing_catalog(self.store.as_ref(), &self.config);
            summaries
                .into_iter()
                .map(|s| task_api::DiscoverySummaryView {
                    source: s.source,
                    ok: s.ok,
                    count: s.count,
                    error: s.error,
                    delta: s.delta,
                })
                .collect()
        })
    }
}

#[cfg(test)]
#[path = "model_discovery/tests.rs"]
mod tests;
