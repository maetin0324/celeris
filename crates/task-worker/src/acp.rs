//! 汎用 ACP（Agent Client Protocol）ワーカーアダプタ（DESIGN §5.4 提案 P-63, ADR-0026）。
//!
//! ACP はここでは「運搬・観測・生存管理」だけを担う（ADR-0026 D1）。エージェントの最終回答は成功判定に
//! 使わない。終端は `claude-code`/`codex` と同じく `artifacts/result.json`（ADR-0006 D3）から合成し、
//! 成果物ディレクトリの `delegate.json` も共有ヘルパで転送する。プロンプト組み立ても `claude_code::build_prompt`
//! をそのまま再利用する（kind 別の文面をアダプタごとに複製しない）。
//!
//! # 実装メモ: 公式 SDK ではなく自前の JSON-RPC クライアント
//!
//! `agent-client-protocol` crate（2.1.0）は試したが採用しなかった。理由:
//! - プロセス生成に `async-process` / `blocking`（別スレッドプール経由の同期呼び出し）を使っており、
//!   本クレートが一貫して使っている `tokio::process::Command`（`process_group(0)` / `kill_on_drop` /
//!   `subprocess.rs` の `send_signal_to_group` によるプロセスグループ単位のシグナル送信）と噛み合わない。
//!   celeris 側でプロセスの生死とシグナルを直接握っておく必要がある（ADR-0003 D4 の生存監視）ため、
//!   SDK 側にプロセス起動を委ねる `AcpAgent::from_str` 系の入口は使いにくい。
//! - SDK の `Client::builder()...connect_with(...)` は「コネクション全体を 1 つの async ブロックに
//!   閉じ込める」設計で、`claude_code.rs`/`codex.rs` が使っている「1 行読むたびに wall-clock / idle
//!   タイムアウトを判定してから次を読む」ループ（`subprocess::read_line_limited` + `tokio::time::timeout`）
//!   や、受信した生の行をそのまま `runs/<run_id>/stdout.jsonl` にミラーする既存の作法にはめ込みにくい。
//! - ワーカー → celeris 方向は結局「1 行 1 JSON」を読むだけなので、`serde_json::Value` を手で組み立てる
//!   方が本クレートの他のアダプタ（`claude_code.rs`/`codex.rs` も CLI 独自の JSON Lines を手でパースして
//!   いる）と一貫する。
//!
//! そのため JSON-RPC 2.0 のメッセージは `serde_json::Value` で直接組み立て・パースする。ACP のフィールド名
//! （`protocolVersion`, `sessionId`, `optionId` 等）は `agent-client-protocol-schema` クレートのソースと
//! ADR-0026 の実機確認（opencode 1.18.31）を突き合わせて決めた。

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nix::sys::signal::Signal;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::warn;

use task_core::ProgressKind;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::claude_code::build_prompt;
use crate::delegate_file::{clear_delegate_file, forward_delegate_file};
use crate::progress;
use crate::protocol::{Evidence, ProviderFailure, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, kill_now, read_line_limited, read_tail, reap_after_terminal,
    send_signal_to_group, write_result_json,
};

/// `session/request_permission` への即答（ADR-0026 D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPermission {
    Allow,
    Deny,
}

/// `[adapters.acp]` / `[[providers]]` 行の上書き分（config.toml, ADR-0026 D2）。
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// 起動する ACP エージェントの実行ファイル。既定 `"opencode"`。
    pub command: String,
    /// コマンドへの引数。既定 `["acp"]`。
    pub args: Vec<String>,
    /// 追加の環境変数。
    pub env: Vec<(String, String)>,
    /// `session/request_permission` への即答（既定 `Allow`）。
    pub permission: AcpPermission,
    /// `session/set_config_option` で設定するモデル（空/`None` なら送らない）。
    pub model: Option<String>,
    /// `session/set_config_option` の `configId`（`session/new` の `configOptions[].id`）。既定 `"model"`。
    pub model_option_id: String,
    /// `initialize` の応答を待つ上限（初回はエージェント側のプロバイダ取得で数分かかりうる。既定 300 秒）。
    pub startup_timeout: Duration,
    /// ADR-0043 D3（Phase 56）: `Some` なら ACP エージェントをコンテナの中で起こす（`container::wrap`）。
    pub container: Option<crate::container::SharedPlan>,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            command: "opencode".to_string(),
            args: vec!["acp".to_string()],
            env: Vec::new(),
            permission: AcpPermission::Allow,
            model: None,
            model_option_id: "model".to_string(),
            startup_timeout: Duration::from_secs(300),
            container: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcpAdapter {
    config: AcpConfig,
}

impl AcpAdapter {
    pub const ID: &'static str = "acp";

    pub fn new(config: AcpConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for AcpAdapter {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        run_acp(&self.config, &req, run_id, &limits, sink).await
    }

    /// `claude_code::ClaudeCodeAdapter::with_env` と同じ規則（同名キーは後勝ち）。ADR-0026 D5:
    /// acp はアカウントプールを使わないが、`env` の上書き自体は他アダプタと同じ形で提供しておく。
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(AcpAdapter::new(config)))
    }
    /// ADR-0043 D3（Phase 56）: コンテナの中で ACP エージェントを起こす複製。
    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.container = Some(plan);
        Some(Arc::new(AcpAdapter::new(config)))
    }
}

/// `artifacts/result.json`（`claude_code::ResultFile` と同じ規約。ADR-0006 D3, ADR-0026 D3）。
#[derive(Debug, Deserialize)]
struct ResultFile {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    evidence: serde_json::Value,
    /// ADR-0072 D9（Phase E1）: graceful yield（`claude_code::ResultFile::r#yield` と同じ規約）。
    #[serde(default, rename = "yield")]
    r#yield: Option<serde_json::Value>,
}

fn lenient_evidence(value: serde_json::Value) -> Vec<Evidence> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<Evidence>(item).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// --- JSON-RPC 2.0 の組み立て（ADR-0026 の実装メモ参照） ---

fn jsonrpc_request(id: u64, method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn jsonrpc_notification(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn jsonrpc_response(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn jsonrpc_error_response(id: serde_json::Value, code: i64, message: String) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// エラーオブジェクト（`{"code":.., "message":..}`）を分類・ログ用の 1 行にする。
fn jsonrpc_error_text(error: &serde_json::Value, browser: bool) -> String {
    let message = error
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error");
    if browser {
        // Preserve provider routing without retaining reflected RPC message bodies.
        let category = match classify_provider_failure(message) {
            Some(ProviderFailure::Exhausted) => ": quota exhausted",
            Some(ProviderFailure::Throttled { .. }) => ": rate limit",
            Some(ProviderFailure::AuthFailed) => ": authentication failed",
            None => "",
        };
        return format!("browser harness RPC failed{category}");
    }
    match error.get("code").and_then(|c| c.as_i64()) {
        Some(code) => format!("{message} (code {code})"),
        None => message.to_string(),
    }
}

async fn write_line(stdin: &mut ChildStdin, value: &serde_json::Value) -> std::io::Result<()> {
    let mut line = serde_json::to_string(value).map_err(std::io::Error::other)?;
    line.push('\n');
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await
}

/// `pump_one_line` の結果。
enum Pumped {
    Eof,
    /// 通知・エージェントからのリクエスト・空行・非 JSON 行など、呼び出し元が気にする必要のないもの。
    Handled,
    /// 呼び出し元が送った何らかのリクエストへの応答（`id` と `result`/`error` を持つ行）。
    Response(serde_json::Value),
}

/// stdout から 1 行読み、通知（`session/update`）とエージェントからのリクエスト（`session/request_permission`
/// 等）はここで処理してしまい、レスポンス行だけを呼び出し元に返す（ADR-0026 D3/D4）。
async fn pump_one_line(
    reader: &mut BufReader<ChildStdout>,
    stdin: &mut ChildStdin,
    stdout_file: &mut tokio::fs::File,
    sink: &dyn EventSink,
    config: &AcpConfig,
    run_id: &str,
    chunks: &mut ChunkBuffer,
) -> std::io::Result<Pumped> {
    match read_line_limited(reader, MAX_LINE_BYTES).await? {
        LineOutcome::Eof => Ok(Pumped::Eof),
        LineOutcome::TooLong => {
            // ACP エージェント自身のフォーマットは celeris が定義したものではないため寛容に扱う
            // （ADR-0006 D5 と同じ方針）。
            sink.heartbeat();
            warn!("run {run_id}: discarding overlong line from acp agent stdout");
            Ok(Pumped::Handled)
        }
        LineOutcome::Line(bytes) => {
            sink.heartbeat();
            stdout_file.write_all(&bytes).await?;
            stdout_file.write_all(b"\n").await?;
            let text = String::from_utf8_lossy(&bytes);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok(Pumped::Handled);
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                warn!("run {run_id}: discarding non-json line from acp agent stdout");
                return Ok(Pumped::Handled);
            };
            if value.get("method").and_then(|m| m.as_str()).is_some() {
                if value.get("id").is_some() {
                    handle_incoming_request(stdin, &value, config, run_id).await?;
                } else {
                    handle_notification(&value, sink, chunks);
                }
                return Ok(Pumped::Handled);
            }
            if value.get("id").is_some() {
                return Ok(Pumped::Response(value));
            }
            warn!("run {run_id}: discarding unrecognized line from acp agent stdout");
            Ok(Pumped::Handled)
        }
    }
}

/// `session/request_permission`（およびクライアント能力を false にしているため本来来ないはずの
/// `fs/*`/`terminal/*`）に即答する（ADR-0026 D4）。
async fn handle_incoming_request(
    stdin: &mut ChildStdin,
    req: &serde_json::Value,
    config: &AcpConfig,
    run_id: &str,
) -> std::io::Result<()> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    match method {
        "session/request_permission" => {
            let options = req
                .pointer("/params/options")
                .and_then(|o| o.as_array())
                .cloned()
                .unwrap_or_default();
            let outcome = match choose_permission_option(&options, config.permission, run_id) {
                Some(option_id) => {
                    serde_json::json!({"outcome": "selected", "optionId": option_id})
                }
                None => serde_json::json!({"outcome": "cancelled"}),
            };
            write_line(
                stdin,
                &jsonrpc_response(id, serde_json::json!({"outcome": outcome})),
            )
            .await
        }
        other => {
            warn!(
                "run {run_id}: acp agent sent an unsupported request; responding with method not found"
            );
            write_line(
                stdin,
                &jsonrpc_error_response(id, -32601, format!("unsupported method: {other}")),
            )
            .await
        }
    }
}

/// `permission` に沿って選択肢を選ぶ。Allow は "allow always" → "allow once"、Deny は "reject always" →
/// "reject once" の優先順（task 指示: 要求が一致する選択肢を提供しなければ最初の選択肢を選び warn する）。
fn choose_permission_option(
    options: &[serde_json::Value],
    permission: AcpPermission,
    run_id: &str,
) -> Option<String> {
    let preferred_kinds: &[&str] = match permission {
        AcpPermission::Allow => &["allow_always", "allow_once"],
        AcpPermission::Deny => &["reject_always", "reject_once"],
    };
    for kind in preferred_kinds {
        if let Some(option_id) = options
            .iter()
            .find(|o| o.get("kind").and_then(|k| k.as_str()) == Some(*kind))
            .and_then(|o| o.get("optionId").and_then(|v| v.as_str()))
        {
            return Some(option_id.to_string());
        }
    }
    if let Some(option_id) = options
        .first()
        .and_then(|o| o.get("optionId").and_then(|v| v.as_str()))
    {
        warn!(
            "run {run_id}: acp permission request offered no option matching {permission:?}; choosing the first offered option"
        );
        return Some(option_id.to_string());
    }
    warn!("run {run_id}: acp permission request offered no options at all");
    None
}

/// エージェントの本文はトークン単位の細切れで届く（実機の opencode + Qwen3.8-27B では 1 タスクで 259 件・
/// 平均 9 文字だった）。そのまま `progress` にすると `WorkerProgress` イベントが膨れるので、改行が来るか
/// 一定量たまるまで溜めてから出す。`heartbeat()` は溜めずに毎行呼ぶので、無出力タイムアウトの判定は変わらない。
struct ChunkBuffer {
    text: String,
    /// ADR-0048 D2（Phase 60a）: 溜めている本文の種別（`text` = 発話 / `thinking` = 思考）。
    /// 種別が変わったら先に出す（発話と思考を 1 件に混ぜない）。
    kind: ProgressKind,
}

impl Default for ChunkBuffer {
    fn default() -> Self {
        Self {
            text: String::new(),
            kind: ProgressKind::Text,
        }
    }
}

impl ChunkBuffer {
    /// 改行が来なくてもこの文字数でいったん出す。
    const FLUSH_AT: usize = 400;

    fn push(&mut self, chunk: &str, kind: ProgressKind, sink: &dyn EventSink) {
        if kind != self.kind {
            self.flush(sink);
            self.kind = kind;
        }
        self.text.push_str(chunk);
        while let Some(idx) = self.text.find('\n') {
            let line: String = self.text.drain(..=idx).collect();
            Self::emit(line.trim(), kind, sink);
        }
        if self.text.chars().count() >= Self::FLUSH_AT {
            self.flush(sink);
        }
    }

    fn flush(&mut self, sink: &dyn EventSink) {
        let text = std::mem::take(&mut self.text);
        Self::emit(text.trim(), self.kind, sink);
    }

    fn emit(text: &str, kind: ProgressKind, sink: &dyn EventSink) {
        if text.is_empty() {
            return;
        }
        let msg = truncate(text, 500);
        let fields = match kind {
            ProgressKind::Thinking => progress::thinking(&progress::one_line(text)),
            _ => progress::text(&progress::one_line(text)),
        };
        sink.progress_with(&msg, &fields);
    }
}

/// `session/update` 通知を `EventSink` に写す（ADR-0026 D3 の表）。すべての通知で `pump_one_line` が既に
/// `heartbeat()` を呼んでいる。本文は `chunks` にまとめてから出す。
fn handle_notification(value: &serde_json::Value, sink: &dyn EventSink, chunks: &mut ChunkBuffer) {
    let Some(method) = value.get("method").and_then(|m| m.as_str()) else {
        return;
    };
    if method != "session/update" {
        return;
    }
    let Some(update) = value.pointer("/params/update") else {
        return;
    };
    match update
        .get("sessionUpdate")
        .and_then(|k| k.as_str())
        .unwrap_or("")
    {
        // ADR-0048 D2（Phase 60a）: 発話は `text`、思考は `thinking`（要約だけ）。
        kind @ ("agent_message_chunk" | "agent_thought_chunk") => {
            if let Some(text) = update.pointer("/content/text").and_then(|t| t.as_str()) {
                let kind = if kind == "agent_thought_chunk" {
                    ProgressKind::Thinking
                } else {
                    ProgressKind::Text
                };
                chunks.push(text, kind, sink);
            }
        }
        "tool_call" | "tool_call_update" => {
            // 本文の途中でツールが動いたら、溜めていた本文を先に出して順序を保つ。
            chunks.flush(sink);
            let name = update
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("tool");
            let status = update
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("pending");
            // ADR-0048 D2: 終わった道具（`completed` / `failed`）は `tool_result`、それ以外は `tool_use`。
            let fields = match status {
                "completed" | "failed" => {
                    let body = tool_call_output(update);
                    progress::tool_result(Some(name), &body, status == "failed")
                }
                _ => {
                    let input = update.get("rawInput").or_else(|| update.get("locations"));
                    let fields = progress::tool_use(name, input);
                    // `rawInput` が無い実装（opencode など）では題名を要約にする。
                    if fields.summary.as_deref().unwrap_or("").is_empty() {
                        fields.with_summary(name)
                    } else {
                        fields
                    }
                }
            };
            sink.progress_with(&format!("tool: {name} {status}"), &fields);
        }
        // `plan` やそれ以外の通知は heartbeat のみ（ADR-0026 D3 の表）。
        _ => {}
    }
}

/// ADR-0048 D2（Phase 60a）: 終わった `tool_call` の出力（`content[]` の `text`、無ければ `rawOutput`）。
fn tool_call_output(update: &serde_json::Value) -> String {
    if let Some(items) = update.get("content").and_then(|c| c.as_array()) {
        let texts: Vec<&str> = items
            .iter()
            .filter_map(|i| {
                i.pointer("/content/text")
                    .or_else(|| i.get("text"))
                    .and_then(|t| t.as_str())
            })
            .collect();
        if !texts.is_empty() {
            return texts.join("\n");
        }
    }
    match update.get("rawOutput") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// `initialize` の応答を `startup_timeout` まで待つ（ADR-0026 D4）。`None` は EOF またはタイムアウト。
#[allow(clippy::too_many_arguments)]
async fn wait_for_initialize_response(
    reader: &mut BufReader<ChildStdout>,
    stdin: &mut ChildStdin,
    stdout_file: &mut tokio::fs::File,
    sink: &dyn EventSink,
    config: &AcpConfig,
    run_id: &str,
    startup_timeout: Duration,
    chunks: &mut ChunkBuffer,
) -> Result<Option<serde_json::Value>, AdapterError> {
    let deadline = Instant::now() + startup_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let pumped = match tokio::time::timeout(
            remaining,
            pump_one_line(reader, stdin, stdout_file, sink, config, run_id, chunks),
        )
        .await
        {
            Err(_elapsed) => return Ok(None),
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(p)) => p,
        };
        match pumped {
            Pumped::Eof => return Ok(None),
            Pumped::Handled => continue,
            Pumped::Response(value) => {
                if value.get("id").and_then(|v| v.as_u64()) == Some(1) {
                    return Ok(Some(value));
                }
                continue;
            }
        }
    }
}

/// `initialize` 以外のリクエストへの応答を、通常の壁時計・アイドルタイムアウトの下で待つ（ADR-0026 D4）。
enum WaitOutcome {
    Response(serde_json::Value),
    Eof,
    TimedOut(Terminal),
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_response(
    reader: &mut BufReader<ChildStdout>,
    stdin: &mut ChildStdin,
    stdout_file: &mut tokio::fs::File,
    sink: &dyn EventSink,
    config: &AcpConfig,
    run_id: &str,
    expected_id: u64,
    start: Instant,
    last_activity: &mut Instant,
    limits: &RunLimits,
    chunks: &mut ChunkBuffer,
) -> Result<WaitOutcome, AdapterError> {
    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            // ADR-0072 D7/§6 (i)（Phase E1）: acp には turn の上限が無いので、wall-clock の打ち切り
            // が continuation の唯一の入口になる（`Terminal::BudgetExhausted{kind: WallClock}`）。
            return Ok(WaitOutcome::TimedOut(Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::WallClock,
                message: "wall clock exceeded".into(),
                usage: None,
            }));
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= limits.idle_timeout {
            // ADR-0072 D7: idle timeout は E1 では harness_error に分類変更しない（§7 U7）。
            return Ok(WaitOutcome::TimedOut(Terminal::Error {
                message: "idle timeout".into(),
                retryable: true,
            }));
        }
        let wait = (limits.wall_clock - wall_elapsed).min(limits.idle_timeout - idle_elapsed);
        let pumped = match tokio::time::timeout(
            wait,
            pump_one_line(reader, stdin, stdout_file, sink, config, run_id, chunks),
        )
        .await
        {
            Err(_elapsed) => continue, // タイムアウト。ループ先頭で上限超過を検知する。
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(p)) => p,
        };
        match pumped {
            Pumped::Eof => return Ok(WaitOutcome::Eof),
            Pumped::Handled => {
                *last_activity = Instant::now();
                continue;
            }
            Pumped::Response(value) => {
                *last_activity = Instant::now();
                if value.get("id").and_then(|v| v.as_u64()) == Some(expected_id) {
                    return Ok(WaitOutcome::Response(value));
                }
                warn!("run {run_id}: discarding response with unexpected id");
                continue;
            }
        }
    }
}

/// Phase A（`initialize`・`session/new`。まだセッションが無い）の失敗をまとめて処理する: プロセスを
/// 止め、JSON-RPC のエラー本文と stderr の末尾を分類する（ADR-0026 D5）。分類できなければ
/// `AdapterError::Spawn`（ADR-0026 D3: 版が V1 でなければ「spawn_failed として終える」の実装）。
async fn kill_and_classify(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<()>,
    stderr_log_path: &Path,
    grace: Duration,
    jsonrpc_error_text: Option<String>,
    fallback_message: String,
) -> AdapterError {
    let _ = kill_now(child, grace).await;
    if let Err(e) = stderr_task.await {
        warn!("stderr capture task failed while classifying an acp spawn failure: {e}");
    }
    let tail = read_tail(stderr_log_path, 4096).await;
    let combined = match &jsonrpc_error_text {
        Some(t) => format!("{t}\n{tail}"),
        None => tail,
    };
    match classify_provider_failure(&combined) {
        Some(pf) => AdapterError::from_provider_failure(
            pf,
            jsonrpc_error_text.as_deref().unwrap_or(&fallback_message),
        ),
        None => AdapterError::Spawn(std::io::Error::other(fallback_message)),
    }
}

/// Phase B（セッションが存在する状態）で run を終えるときの生データ。
enum RawOutcome {
    TimedOut(Terminal),
    Eof {
        context: &'static str,
    },
    RpcError {
        context: &'static str,
        error_text: String,
    },
    Success {
        stop_reason: String,
    },
}

/// プロセスを止め（タイムアウトなら `session/cancel` → `kill_grace` 後 SIGKILL、そうでなければ穏やかな
/// 刈り取り。ADR-0026 D4）、結果ファイル（`<artifacts_dir>/result.json`）から終端を合成し、`runs/<run_id>/result.json` に書く
/// （ADR-0006 D3, ADR-0010 D10）。
#[allow(clippy::too_many_arguments)]
async fn finish_run(
    child: &mut Child,
    mut stdin: ChildStdin,
    session_id: &str,
    limits: &RunLimits,
    stderr_task: tokio::task::JoinHandle<()>,
    mut stdout_file: tokio::fs::File,
    run_dir: &Path,
    // ADR-0036 D1/D2: 成果物と結果ファイルの置き場（`RunRequest.artifacts_dir`）と、その workspace 相対表記。
    artifacts_dir: &Path,
    artifacts_rel: &str,
    stderr_log_path: &Path,
    sink: &dyn EventSink,
    outcome: RawOutcome,
    run_id: &str,
) -> Result<RunOutcome, AdapterError> {
    let force_kill = matches!(outcome, RawOutcome::TimedOut(_));
    let exit_status = if force_kill {
        // ADR-0026 D4: まず `session/cancel` を送り、`kill_grace` 待ってからプロセスグループへ SIGKILL。
        let _ = write_line(
            &mut stdin,
            &jsonrpc_notification(
                "session/cancel",
                serde_json::json!({"sessionId": session_id}),
            ),
        )
        .await;
        match tokio::time::timeout(limits.kill_grace, child.wait()).await {
            Ok(status) => status?,
            Err(_elapsed) => {
                send_signal_to_group(child, Signal::SIGKILL);
                child.wait().await?
            }
        }
    } else {
        reap_after_terminal(child, limits.kill_grace).await?
    };
    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;

    let (terminal, provider_failure): (Terminal, Option<ProviderFailure>) = match outcome {
        // タイムアウトは分類しない（ADR-0010 D5 と同じ方針）。
        RawOutcome::TimedOut(t) => (t, None),
        RawOutcome::Eof { context } => {
            let exit_repr = match exit_status.code() {
                Some(code) => code.to_string(),
                None => "signal".to_string(),
            };
            let tail = read_tail(stderr_log_path, 4096).await;
            let pf = classify_provider_failure(&tail);
            (
                Terminal::Error {
                    message: format!(
                        "acp agent exited before responding to {context} (exit={exit_repr})"
                    ),
                    retryable: true,
                },
                pf,
            )
        }
        RawOutcome::RpcError {
            context,
            error_text,
        } => {
            let tail = read_tail(stderr_log_path, 4096).await;
            let combined = format!("{error_text}\n{tail}");
            let pf = classify_provider_failure(&combined);
            (
                Terminal::Error {
                    message: format!("acp agent returned an error for {context}: {error_text}"),
                    retryable: true,
                },
                pf,
            )
        }
        RawOutcome::Success { stop_reason } => (
            terminal_from_result_file(artifacts_dir, artifacts_rel, &stop_reason).await,
            None,
        ),
    };

    forward_delegate_file(artifacts_dir, sink).await;
    write_result_json(run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

/// 結果ファイル（`<artifacts_dir>/result.json`）から終端を合成する（ADR-0026 D3 手順 7、ADR-0036 D2）。
/// ACP 自体には成功/失敗の強い意味論が無いので（D1: 運搬・観測・生存管理だけ）、`stopReason` に関わらず
/// 常にこのファイルを見る。ファイルが無ければ `stopReason` を理由として `Error{retryable:true}` にする。
async fn terminal_from_result_file(
    artifacts_dir: &Path,
    artifacts_rel: &str,
    stop_reason: &str,
) -> Terminal {
    // ADR-0072 D7/§6 (i)（Phase E1）: `stopReason == "max_turn_requests"` は turn の上限に当たったと
    // 読める（実機の文言は未確認。§7 U1）。result.json より優先する（claude-code の `error_max_turns`
    // と同じ扱い）。
    if stop_reason == "max_turn_requests" {
        return Terminal::BudgetExhausted {
            kind: task_core::BudgetKind::Turns,
            message: format!("acp stopReason: {stop_reason}"),
            usage: None,
        };
    }
    let result_path = artifacts_dir.join("result.json");
    let text = match tokio::fs::read_to_string(&result_path).await {
        Ok(t) => t,
        Err(_) => {
            return Terminal::Error {
                message: format!(
                    "acp agent stopped ({stop_reason}) without {artifacts_rel}/result.json"
                ),
                retryable: true,
            };
        }
    };
    // ADR-0090 D1: クラスタ job の終了待ち（`question` が無ければ `summary` より優先）。
    if let Some(terminal) = crate::adapter::result_file_wait(&text, None) {
        return terminal;
    }
    match serde_json::from_str::<ResultFile>(&text) {
        Ok(rf) => {
            // ADR-0072 D9: 優先順位は `question` > `summary` > `yield`。
            if let Some(question) = rf.question {
                Terminal::Question { text: question }
            } else if let Some(summary) = rf.summary {
                // ADR-0026: PromptResponse の token 使用量は `unstable_end_turn_token_usage`
                // （安定版 ACP には無い）でしか運ばれず、opencode の実機確認でも観測されていないため
                // 常に `None` にする（`usage` を埋める標準フィールドが無い）。
                Terminal::Done {
                    summary,
                    evidence: lenient_evidence(rf.evidence),
                    usage: None,
                }
            } else if let Some(checkpoint) = rf.r#yield {
                Terminal::Yielded {
                    checkpoint,
                    usage: None,
                }
            } else {
                Terminal::Error {
                    message: format!(
                        "{artifacts_rel}/result.json has neither 'summary', 'question' nor 'yield'"
                    ),
                    retryable: true,
                }
            }
        }
        Err(e) => Terminal::Error {
            message: format!("{artifacts_rel}/result.json is not valid JSON: {e}"),
            retryable: true,
        },
    }
}

async fn run_acp(
    config: &AcpConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    // ADR-0054 D2（Phase 68）: CoS の対話 run だけ、道具の許可要求を常に拒否する（fail-closed）。
    // ACP には claude-code の `--allowedTools` / codex の `sandbox_mode` に相当する「道具単位の読み取り
    // 許可」が無く、`session/request_permission` にはこのコードベースが解釈できる形で道具の識別子が
    // 乗らない（実機で確認していない）。曖昧な照合で書き込みを誤って許すより、対話 run の間は
    // 一律で拒否する方が安全という判断（`choose_permission_option` の `Deny` 経路をそのまま使う。
    // `reject_always` → `reject_once` → 選択肢の最初、の優先順は変えない）。読み取りだけの道具
    // （`celerisctl knowledge search|get` 等）は、モデルが許可要求を経ない組み込みの読み取りで
    // 済ませられる範囲でしか使えない（ACP エージェント実装依存。`docs/adr/0054-*.md` の「Phase 68
    // 追記」に明記）。
    let config = &if req.context.conversation_addressee
        == Some(crate::protocol::ConversationAddressee::Secretary)
    {
        AcpConfig {
            permission: AcpPermission::Deny,
            ..config.clone()
        }
    } else {
        config.clone()
    };
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;
    // Browser commands and page content must not be persisted through the harness's raw
    // stream capture. Parsing still uses the pipes; the browser supervisor emits safe audit events.
    let (stdout_log_path, stderr_log_path) = if req.context.browser.is_some() {
        (
            Path::new("/dev/null").to_path_buf(),
            Path::new("/dev/null").to_path_buf(),
        )
    } else {
        (run_dir.join("stdout.jsonl"), run_dir.join("stderr.log"))
    };
    let stderr_log_path_for_task = stderr_log_path.clone();

    // 前回の run（リトライ）が残した結果ファイルを、今回の run の結果と誤読しないよう先に消す
    // （claude_code / codex と同じ。ADR-0006 D3）。
    // ADR-0036 D1/D2: 置き場はディスパッチャが決めた `artifacts_dir`（共有 workspace ではタスクごと）。
    let artifacts_rel = req.artifacts_rel();
    let result_path = req.artifact_path("result.json");
    let _ = tokio::fs::remove_file(&result_path).await;
    clear_delegate_file(&req.artifacts_dir).await;

    let mut prompt = build_prompt(&req.task, &req.context, run_id, &artifacts_rel);
    // ADR-0056 D3（Phase 79）: mount された skills を前置きに直接埋め込む（acp にはファイルを自動で
    // 読む契約が無いため。`skills` が空なら 1 バイトも変わらない）。
    prompt.push_str(&crate::skills::preamble_section(&req.context.skills));
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &prompt, run_id).await;

    let mut command = Command::new(&config.command);
    command
        .args(&config.args)
        .envs(config.env.iter().cloned())
        .current_dir(req.cwd());
    // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
    let mut command = crate::db_guard::launch(command, config.container.as_deref());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdin was not piped".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdout was not piped".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AdapterError::Other("worker stderr was not piped".into()))?;

    let stderr_task = tokio::spawn(async move {
        let mut reader = stderr;
        match tokio::fs::File::create(&stderr_log_path_for_task).await {
            Ok(mut file) => {
                if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
                    warn!("failed to write worker stderr.log: {e}");
                }
            }
            Err(e) => warn!("failed to create worker stderr.log: {e}"),
        }
    });

    let mut stdout_file = tokio::fs::File::create(&stdout_log_path).await?;
    let mut reader = BufReader::new(stdout);
    // 本文のチャンクをまとめる入れ物（run 全体で 1 つ。initialize の前から使う）。
    let mut chunks = ChunkBuffer::default();

    // --- Phase A: initialize（ADR-0026 D3 手順 2, D4: startup_timeout） ---
    let init_params = serde_json::json!({
        "protocolVersion": 1,
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
    });
    if write_line(&mut stdin, &jsonrpc_request(1, "initialize", init_params))
        .await
        .is_err()
    {
        return Err(kill_and_classify(
            &mut child,
            stderr_task,
            &stderr_log_path,
            limits.kill_grace,
            None,
            "failed to write the initialize request to the acp agent's stdin".into(),
        )
        .await);
    }

    let init_response = match wait_for_initialize_response(
        &mut reader,
        &mut stdin,
        &mut stdout_file,
        sink,
        config,
        run_id,
        config.startup_timeout,
        &mut chunks,
    )
    .await
    {
        Ok(Some(v)) => v,
        Ok(None) => {
            return Err(kill_and_classify(
                &mut child,
                stderr_task,
                &stderr_log_path,
                limits.kill_grace,
                None,
                "acp agent did not respond to initialize within the startup timeout".into(),
            )
            .await);
        }
        Err(e) => {
            let _ = kill_now(&mut child, limits.kill_grace).await;
            let _ = stderr_task.await;
            return Err(e);
        }
    };

    if let Some(error) = init_response.get("error") {
        let msg = jsonrpc_error_text(error, req.context.browser.is_some());
        return Err(kill_and_classify(
            &mut child,
            stderr_task,
            &stderr_log_path,
            limits.kill_grace,
            Some(msg.clone()),
            format!("acp agent rejected initialize: {msg}"),
        )
        .await);
    }
    let negotiated = init_response
        .pointer("/result/protocolVersion")
        .and_then(|v| v.as_u64());
    if negotiated != Some(1) {
        // ADR-0026 D3: 版の交渉はアダプタの中に閉じる。V1 でなければ spawn_failed として終える。
        return Err(kill_and_classify(
            &mut child,
            stderr_task,
            &stderr_log_path,
            limits.kill_grace,
            None,
            format!("acp agent negotiated protocol version {negotiated:?}, expected 1"),
        )
        .await);
    }

    // ここから先は通常の壁時計・アイドルタイムアウトを使う（ADR-0026 D4。`startup_timeout` は
    // initialize の応答待ちだけに使う別枠）。
    let start = Instant::now();
    let mut last_activity = Instant::now();

    // --- Phase A 続き: session/new、または継続セッションなら session/load（ADR-0026 D3 手順 3、
    // ADR-0054 D1 Phase 67）。`context.session` がこのアダプタ宛て（`adapter == "acp"`）で
    // `resume: true` のときだけ `session/load` に切り替える（ACP のセッション継続手段）。 ---
    let acp_session = req
        .context
        .session
        .as_ref()
        .filter(|s| s.adapter == AcpAdapter::ID);
    let resuming = acp_session.is_some_and(|s| s.resume);
    let (new_session_method, new_session_params) =
        if let Some(session) = acp_session.filter(|_| resuming) {
            (
                "session/load",
                serde_json::json!({
                    "sessionId": session.session_id,
                    "cwd": req.cwd().to_string_lossy(),
                    "mcpServers": [],
                }),
            )
        } else {
            (
                "session/new",
                serde_json::json!({
                    "cwd": req.cwd().to_string_lossy(),
                    "mcpServers": [],
                }),
            )
        };
    if write_line(
        &mut stdin,
        &jsonrpc_request(2, new_session_method, new_session_params),
    )
    .await
    .is_err()
    {
        return Err(kill_and_classify(
            &mut child,
            stderr_task,
            &stderr_log_path,
            limits.kill_grace,
            None,
            format!("failed to write the {new_session_method} request to the acp agent's stdin"),
        )
        .await);
    }
    let new_session_response = match wait_for_response(
        &mut reader,
        &mut stdin,
        &mut stdout_file,
        sink,
        config,
        run_id,
        2,
        start,
        &mut last_activity,
        limits,
        &mut chunks,
    )
    .await
    {
        Ok(WaitOutcome::Response(v)) => v,
        Ok(WaitOutcome::Eof) => {
            if resuming {
                sink.session_resume_failed("acp agent exited before responding to session/load");
            }
            return Err(kill_and_classify(
                &mut child,
                stderr_task,
                &stderr_log_path,
                limits.kill_grace,
                None,
                format!("acp agent exited before responding to {new_session_method}"),
            )
            .await);
        }
        Ok(WaitOutcome::TimedOut(terminal)) => {
            let message = match &terminal {
                Terminal::Error { message, .. } => message.clone(),
                _ => format!("timeout waiting for {new_session_method}"),
            };
            if resuming {
                sink.session_resume_failed(&message);
            }
            return Err(kill_and_classify(
                &mut child,
                stderr_task,
                &stderr_log_path,
                limits.kill_grace,
                None,
                message,
            )
            .await);
        }
        Err(e) => {
            let _ = kill_now(&mut child, limits.kill_grace).await;
            let _ = stderr_task.await;
            return Err(e);
        }
    };
    if let Some(error) = new_session_response.get("error") {
        let msg = jsonrpc_error_text(error, req.context.browser.is_some());
        // ADR-0054 D1（Phase 67）: `session/load` が拒否された（セッションが無い・失効した）ことを
        // 報告する。ディスパッチャはこれを見て `node_sessions` の該当行を retire し、次の run は
        // 新規セッションになる。`session/new`（継続でない run）の拒否はこれまでどおりただのエラー。
        if resuming {
            sink.session_resume_failed(&msg);
        }
        return Err(kill_and_classify(
            &mut child,
            stderr_task,
            &stderr_log_path,
            limits.kill_grace,
            Some(msg.clone()),
            format!("acp agent rejected {new_session_method}: {msg}"),
        )
        .await);
    }
    // ADR-0054 D1（Phase 67）: `session/load` は ACP の仕様上 `sessionId` を返さないことがある
    // （渡した id をそのまま使い続けるだけでよい）。`session/new` は必ず返す（これまでどおり必須）。
    let response_session_id = new_session_response
        .pointer("/result/sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let Some(session_id) = response_session_id.or_else(|| {
        resuming
            .then(|| acp_session.map(|s| s.session_id.clone()))
            .flatten()
    }) else {
        if resuming {
            sink.session_resume_failed("acp agent session/load response had no sessionId");
        }
        return Err(kill_and_classify(
            &mut child,
            stderr_task,
            &stderr_log_path,
            limits.kill_grace,
            None,
            format!("acp agent {new_session_method} response had no sessionId"),
        )
        .await);
    };
    // ADR-0054 D1（Phase 67）: 継続セッションの対象なら、実際に使った id を報告する
    // （新規は agent が割り当てた id、継続は渡した id——`session/load` が確認以上の応答をしなくても、
    // その id が引き続き現役であることに変わりはない）。
    if acp_session.is_some() {
        sink.session_established(&session_id);
    }

    // --- Phase B: セッションが存在する。以降の失敗は artifacts/result.json 経由の終端として扱う。 ---
    let mut next_id = 3u64;

    // 手順 4: モデル指定があれば session/set_config_option（ADR-0026 D3）。
    if let Some(model) = config.model.as_deref().filter(|m| !m.is_empty()) {
        let has_option = new_session_response
            .pointer("/result/configOptions")
            .and_then(|v| v.as_array())
            .map(|opts| {
                opts.iter().any(|o| {
                    o.get("id").and_then(|i| i.as_str()) == Some(config.model_option_id.as_str())
                })
            })
            .unwrap_or(false);
        if has_option {
            let id = next_id;
            next_id += 1;
            // 実機（opencode 1.18.31）は `configId` を要求する（`optionId` は -32602 Invalid params）。
            // `session/new` の `configOptions[].id` と対になる名前で、権限要求の `optionId` とは別物。
            let params = serde_json::json!({
                "sessionId": session_id,
                "configId": config.model_option_id,
                "value": model,
            });
            if write_line(
                &mut stdin,
                &jsonrpc_request(id, "session/set_config_option", params),
            )
            .await
            .is_ok()
            {
                match wait_for_response(
                    &mut reader,
                    &mut stdin,
                    &mut stdout_file,
                    sink,
                    config,
                    run_id,
                    id,
                    start,
                    &mut last_activity,
                    limits,
                    &mut chunks,
                )
                .await
                {
                    Ok(WaitOutcome::Response(v)) => {
                        if let Some(error) = v.get("error") {
                            warn!(
                                "run {run_id}: acp agent rejected session/set_config_option: {}",
                                jsonrpc_error_text(error, req.context.browser.is_some())
                            );
                        }
                    }
                    Ok(WaitOutcome::Eof) => {
                        return finish_run(
                            &mut child,
                            stdin,
                            &session_id,
                            limits,
                            stderr_task,
                            stdout_file,
                            &run_dir,
                            &req.artifacts_dir,
                            &artifacts_rel,
                            &stderr_log_path,
                            sink,
                            RawOutcome::Eof {
                                context: "session/set_config_option",
                            },
                            run_id,
                        )
                        .await;
                    }
                    Ok(WaitOutcome::TimedOut(terminal)) => {
                        return finish_run(
                            &mut child,
                            stdin,
                            &session_id,
                            limits,
                            stderr_task,
                            stdout_file,
                            &run_dir,
                            &req.artifacts_dir,
                            &artifacts_rel,
                            &stderr_log_path,
                            sink,
                            RawOutcome::TimedOut(terminal),
                            run_id,
                        )
                        .await;
                    }
                    Err(e) => {
                        let _ = kill_now(&mut child, limits.kill_grace).await;
                        let _ = stderr_task.await;
                        return Err(e);
                    }
                }
            } else {
                warn!(
                    "run {run_id}: failed to write session/set_config_option to the acp agent's stdin"
                );
            }
        } else {
            warn!(
                "run {run_id}: acp model_option_id {:?} was not found in session/new's configOptions; skipping session/set_config_option",
                config.model_option_id
            );
        }
    }

    // 手順 5: session/prompt（ADR-0026 D3）。
    let prompt_id = next_id;
    let prompt_params = serde_json::json!({
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": prompt}],
    });
    if let Err(e) = write_line(
        &mut stdin,
        &jsonrpc_request(prompt_id, "session/prompt", prompt_params),
    )
    .await
    {
        let _ = kill_now(&mut child, limits.kill_grace).await;
        let _ = stderr_task.await;
        return Err(AdapterError::Io(e));
    }

    let prompt_wait = match wait_for_response(
        &mut reader,
        &mut stdin,
        &mut stdout_file,
        sink,
        config,
        run_id,
        prompt_id,
        start,
        &mut last_activity,
        limits,
        &mut chunks,
    )
    .await
    {
        Ok(w) => w,
        Err(e) => {
            let _ = kill_now(&mut child, limits.kill_grace).await;
            let _ = stderr_task.await;
            return Err(e);
        }
    };

    // 溜めたままの本文を最後に出し切る（改行で終わらない返答を落とさない）。
    chunks.flush(sink);

    let outcome = match prompt_wait {
        WaitOutcome::TimedOut(terminal) => RawOutcome::TimedOut(terminal),
        WaitOutcome::Eof => RawOutcome::Eof {
            context: "session/prompt",
        },
        WaitOutcome::Response(value) => match value.get("error") {
            Some(error) => RawOutcome::RpcError {
                context: "session/prompt",
                error_text: jsonrpc_error_text(error, req.context.browser.is_some()),
            },
            None => {
                let stop_reason = value
                    .pointer("/result/stopReason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let stop_reason = if req.context.browser.is_some()
                    && !matches!(
                        stop_reason.as_str(),
                        "end_turn" | "max_turn_requests" | "cancelled" | "refusal"
                    ) {
                    "unknown".to_string()
                } else {
                    stop_reason
                };
                RawOutcome::Success { stop_reason }
            }
        },
    };

    finish_run(
        &mut child,
        stdin,
        &session_id,
        limits,
        stderr_task,
        stdout_file,
        &run_dir,
        &req.artifacts_dir,
        &artifacts_rel,
        &stderr_log_path,
        sink,
        outcome,
        run_id,
    )
    .await
}

#[cfg(test)]
mod tests;
