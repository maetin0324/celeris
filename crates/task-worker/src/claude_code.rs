//! `claude-code` アダプタ（DESIGN §5.4, ADR-0003 D7, ADR-0006）。
//!
//! `claude` CLI は celeris 独自のワーカープロトコルを話さない。`--output-format stream-json` が吐く
//! Claude Code 自身のイベント（`system`/`assistant`/`user`/`result`）を読み、結果ファイル規約
//! （ADR-0006 D3: `artifacts/result.json`）と `result` メッセージ（D4）から `RunOutcome` を合成する。
//! 生存監視（wall-clock・無出力タイムアウト・SIGTERM→SIGKILL）は `subprocess.rs` の低レベル部分を再利用する。

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use task_core::{Check, RateLimitObservation, Task, TaskKind, Usage};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::delegate_file::{clear_delegate_file, forward_delegate_file};
use crate::progress;
use crate::protocol::{Answer, Evidence, ProviderFailure, RunContext, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, adopt_result_json_written_under_work_dir, kill_now,
    read_line_limited, read_tail, reap_after_terminal, write_result_json,
};

/// `[adapters.claude_code]`（config.toml, ADR-0006 D6）。
#[derive(Debug, Clone)]
pub struct ClaudeCodeConfig {
    /// 起動するコマンド名／パス。既定 `"claude"`。
    pub command: String,
    /// 末尾に追加する引数。
    pub extra_args: Vec<String>,
    /// `--permission-mode`。既定 `"bypassPermissions"`（ADR-0006 D6: celeris は許可プロンプトに応答できない）。
    pub permission_mode: String,
    /// `--model`（省略時は claude の既定モデル）。
    pub model: Option<String>,
    /// 追加の環境変数（例: `CLAUDE_CONFIG_DIR`）。
    pub env: Vec<(String, String)>,
    /// ADR-0075 G3-fix1: 子プロセスから外す環境変数（`with_env_removed`。`env` より先に `env_remove` する）。
    pub env_remove: Vec<String>,
    /// ADR-0043 D3（Phase 56）: `Some` なら `claude` をコンテナの中で起こす（`container::wrap`）。
    /// TOML には書かない（ディスパッチャが `with_container` で入れる）。
    pub container: Option<crate::container::SharedPlan>,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            command: "claude".to_string(),
            extra_args: Vec::new(),
            permission_mode: "bypassPermissions".to_string(),
            model: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            container: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter {
    config: ClaudeCodeConfig,
}

impl ClaudeCodeAdapter {
    pub const ID: &'static str = "claude-code";

    pub fn new(config: ClaudeCodeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for ClaudeCodeAdapter {
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
        run_claude_code(&self.config, &req, run_id, &limits, sink).await
    }

    /// ADR-0024 D2: `extra` を `config.env` の末尾に足した複製を返す。同名キーは後勝ち（`envs()` に渡す順で
    /// 最後に指定した値が使われる）ので、末尾に足すだけで `extra` が既存の同名キーに勝つ。
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.model = Some(model.to_owned());
        Some(Arc::new(Self::new(config)))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }
    fn with_env_removed(&self, keys: &[String]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        crate::adapter::remove_env_keys(&mut config.env, &mut config.env_remove, keys);
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }

    /// ADR-0043 D3（Phase 56）: コンテナの中で `claude` を起こす複製。
    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.container = Some(plan);
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }

    /// ADR-0072 D14（Phase E4b 項目3）: `--permission-mode` を上書きした複製。planner run に
    /// `[execution.planner].permission_mode`（既定 `"bypassPermissions"`）を実際の CLI 引数へ反映するために使う
    /// （`with_model`/`with_env` と同じ形。ADR-0072「Phase E3 実装時の逸脱・明確化」で見送っていた
    /// フック）。
    fn with_permission_mode(&self, mode: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.permission_mode = mode.to_owned();
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }
}

// プロンプト文面と子プロセスの実行・結果処理は別の変更境界。
mod prompt;
pub use prompt::build_prompt;
pub use prompt::CLUSTER_JOB_PLANNER_GUIDANCE;
pub(crate) use prompt::work_dir_note;
#[cfg(test)]
use prompt::{
    delegate_workspace_instruction, harness_artifacts_section_for_plan,
    harness_artifacts_section_for_review, workspace_section_for_plan,
};

/// `artifacts/result.json`（ADR-0006 D3）。
#[derive(Debug, Deserialize)]
struct ResultFile {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    question: Option<String>,
    /// 生の JSON で受け、`lenient_evidence` で整形する（ADR-0006 D3: `evidence` の内容の正確さは要求しない。
    /// Phase 5 のドッグフードで、ワーカーが文字列の配列を書いて run 全体が `error` になる事故があった）。
    #[serde(default)]
    evidence: serde_json::Value,
    /// ADR-0072 D9/D10（Phase E1）: graceful yield（`{"yield": {...checkpoint の意味の欄...}}`）。
    /// 中身は寛容に読む（`task_core::WorkerCheckpointInput` と同じ形。生の JSON のまま保持し、
    /// 検証・合成はディスパッチャ側の `task_core::merge_checkpoint` が行う）。
    #[serde(default, rename = "yield")]
    r#yield: Option<serde_json::Value>,
}

/// `evidence` のうち `Evidence` として読めた要素だけを残す。配列でない／要素が不正でも `done` を失敗にしない。
fn lenient_evidence(value: serde_json::Value) -> Vec<Evidence> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<Evidence>(item).ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// stream-json の最後に観測した `{"type":"result",...}`（ADR-0006 D4）。
#[derive(Debug, Clone)]
struct ResultMeta {
    subtype: String,
    is_error: bool,
    usage: Option<Usage>,
    /// `result` フィールド（文字列。エラー時の文面）。供給側失敗の分類に使う（ADR-0010 D5）。
    result: Option<String>,
    /// F5-fix5: `result` の `stop_reason`（`end_turn` など。無ければ `None`）。
    stop_reason: Option<String>,
    /// F5-fix5: この `result` を観測した時点で、まだ終わっていなかった background task の説明
    /// （`BackgroundTasks::outstanding`）。headless の run は turn を終えると background task を殺す。
    outstanding_background: Vec<String>,
}

/// F5-fix5: stream-json の `system` 行（`task_started` / `task_notification` / `task_updated`）から追う
/// background task（Claude Code CLI 2.1.283 の形。本番の run 01M3KF2HFMHPJR7YEB5HMT38MQ の stdout.jsonl
/// 17・52・53 行目）。未知の形は無視する（ADR-0006 D5）。
#[derive(Debug, Default)]
struct BackgroundTasks {
    /// 開始順の `(task_id, description)`（`is_backgrounded: false` と明示されたものは除く）。
    started: Vec<(String, String)>,
    /// 終わった（通知・状態更新で完了・失敗・停止・kill が観測された）task_id。
    finished: std::collections::BTreeSet<String>,
}

impl BackgroundTasks {
    fn observe(&mut self, value: &serde_json::Value) {
        let subtype = value.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
        let Some(task_id) = value.get("task_id").and_then(|s| s.as_str()) else {
            return;
        };
        match subtype {
            "task_started" => {
                if value.get("is_backgrounded").and_then(|b| b.as_bool()) == Some(false) {
                    return;
                }
                if self.started.iter().any(|(id, _)| id == task_id) {
                    return;
                }
                let description = value
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or(task_id);
                self.started
                    .push((task_id.to_string(), truncate(description, 300)));
            }
            // 終わりの通知（`status`: completed / failed / stopped …）。どの status でも「もう走っていない」。
            "task_notification" => {
                self.finished.insert(task_id.to_string());
            }
            "task_updated" => {
                let status = value
                    .pointer("/patch/status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if matches!(
                    status,
                    "completed" | "failed" | "killed" | "stopped" | "cancelled"
                ) {
                    self.finished.insert(task_id.to_string());
                }
            }
            _ => {}
        }
    }

    /// まだ終わっていない background task の説明（開始順）。
    fn outstanding(&self) -> Vec<String> {
        self.started
            .iter()
            .filter(|(id, _)| !self.finished.contains(id))
            .map(|(_, d)| d.clone())
            .collect()
    }
}

/// F5-fix5: headless の run が background task を残して turn を終えたときの、続きの run への申し送り
/// （`Terminal::Yielded` の checkpoint。ADR-0072 D9 の continuation）。worker 自身の `checkpoint.json`
/// があれば、その欄を土台にして `next_action` と `known_failures` だけを足す。
async fn headless_background_checkpoint(
    artifacts_dir: &Path,
    outstanding: &[String],
) -> serde_json::Value {
    let mut checkpoint = match tokio::fs::read_to_string(artifacts_dir.join("checkpoint.json"))
        .await
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    {
        Some(v @ serde_json::Value::Object(_)) => v,
        _ => serde_json::json!({}),
    };
    let commands = outstanding
        .iter()
        .map(|c| format!("`{c}`"))
        .collect::<Vec<_>>()
        .join("、");
    let previous_next = checkpoint
        .get("next_action")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("（前の run の次の一手: {s}）"))
        .unwrap_or_default();
    let next_action = format!(
        "前の run は headless なのに background で command を走らせたまま turn を終えたため、その command は \
         run の終わりと同時に殺され、結果は残っていない。background を使わず foreground で（Bash の \
         `timeout` を長めに指定して）もう一度実行し、結果を確かめてから続け、最後に result.json を書くこと: \
         {commands}{previous_next}"
    );
    if let Some(obj) = checkpoint.as_object_mut() {
        obj.insert("next_action".into(), serde_json::Value::String(next_action));
        let mut failures = obj
            .get("known_failures")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for c in outstanding {
            failures.push(serde_json::json!({
                "what": format!("headless_background_task: killed when the turn ended: {c}"),
            }));
        }
        obj.insert("known_failures".into(), serde_json::Value::Array(failures));
    }
    checkpoint
}

/// F5-fix5: 続き（continuation）に回してよい run か。coding 系の execute run だけ（ADR-0072 D10 の
/// 予算の予告と同じ範囲。対話・計画・レビューの run は continuation の仕組みを持たない）。
fn is_continuable_execute_run(task: &Task, context: &RunContext) -> bool {
    matches!(task.kind, TaskKind::Execute | TaskKind::Approval)
        && context.execution_planner.is_none()
        && context.conversation_addressee.is_none()
}

async fn run_claude_code(
    config: &ClaudeCodeConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
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
    // `stderr_task` (below) moves a copy into its `async move` block; this one stays available for
    // the crash-classification read after the loop (ADR-0010 D5).
    let stderr_log_path_for_task = stderr_log_path.clone();

    // 前回の run（リトライ）が残した結果ファイルを、今回の run の結果と誤読しないよう先に消す
    // （監査で指摘。ADR-0006 D3 は「この run が書いたファイル」を前提にしている）。
    // ADR-0036 D1/D2: 置き場はディスパッチャが決めた `artifacts_dir`（共有 workspace ではタスクごと）。
    let artifacts_rel = req.artifacts_rel();
    let result_path = req.artifact_path("result.json");
    let _ = tokio::fs::remove_file(&result_path).await;
    clear_delegate_file(&req.artifacts_dir).await;
    // ADR-0006 Phase 115 D2: 同じ理由（前回の run の名残と誤読しない）で、work_dir 側の名残候補も消す
    // （`work_dir != workspace` のときだけ意味がある。無ければ何もしない）。
    if let Some(work_dir) = req.work_dir.as_deref()
        && work_dir != req.workspace
    {
        let _ = tokio::fs::remove_file(work_dir.join("artifacts").join("result.json")).await;
    }

    let prompt = format!(
        "{}{}",
        work_dir_note(req.work_dir.as_deref(), &req.workspace, &req.artifacts_dir),
        build_prompt(&req.task, &req.context, run_id, &artifacts_rel)
    );
    // ADR-0023 D2 / M1: この run で何を渡したかを残す（`request.json` は構造、`prompt.txt` は実際の文面）。
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &prompt, run_id).await;
    // ADR-0056 D3（Phase 79）: mount された skills を `.claude/skills/<name>/` に写す（claude-code が
    // 自動で読む形式。run が失敗しても打ち切らない。読み取れる限りは失敗しない見込み — 失敗すれば
    // ワーカー起動前の警告としてログに残す）。
    if let Err(e) = crate::skills::deliver_claude_code(req.cwd(), &req.context.skills).await {
        warn!("run {run_id}: failed to deliver skills to .claude/skills: {e}");
    }

    let mut command = Command::new(&config.command);
    // F5-fix10（本番障害 run 01M3Q21Z9JQWWANGHXJPNH1F8X）: プロンプトは argv に載せず stdin で渡す
    // （`-p/--print` は値を取らない真偽フラグで、位置引数の prompt が無ければ stdin を読む。Claude Code
    // CLI 2.1.284 の `--help`: `Usage: claude [options] [command] [prompt]`・`--input-format` 既定 "text"）。
    // 135,644 バイトの replan プロンプトが Linux の MAX_ARG_STRLEN（131072）を超えて spawn が E2BIG で
    // 落ちたため。書き込みは spawn 直後に `feed_stdin`（別タスク）で行う。
    command
        .arg("-p")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--permission-mode")
        .arg(&config.permission_mode)
        .arg("--max-turns")
        .arg(req.task.budget.max_turns.to_string())
        // F5-fix5: どの run（worker / planner / reviewer / 対話）にも、headless であること・turn を
        // 終えると run が終わること・長い command も foreground で走らせることを system prompt に足す。
        .arg("--append-system-prompt")
        .arg(crate::preamble::HEADLESS_RUN_NOTE);
    // ADR-0054 D1（Phase 67）: `context.session`（この run が継続セッションの一部）が無ければ、
    // Phase 66 までと同じ `--no-session-persistence`（session を残さない）。ある場合は、そのアダプタが
    // `claude-code` のときだけ、初回は `--session-id <id>`（これから使う id を固定）、2 回目以降は
    // `--resume <id>`（続ける）に切り替える。他アダプタ向けの `session` は無視する（渡り歩きは無い）。
    // ADR-0054 D1（Phase 67）: `resume` を頼んだ run かどうかは、後段の crash 分類（resume 失敗の
    // 検出）でも使う。
    let is_resuming = req
        .context
        .session
        .as_ref()
        .is_some_and(|s| s.adapter == ClaudeCodeAdapter::ID && s.resume);
    // ADR-0054 Phase 67b 追記: Claude Code CLI 2.1.278 は `--session-id`/`--resume` に渡す id が UUID
    // でなければ拒否する（本番で ULID を渡してすべての CoS 対話・部門長レビュー run が失敗した事故。
    // 2026-09-21）。celeris 側の発行（`crate::sessions` 相当。呼び出し元は `resolve_node_session` の
    // 自己修復）は Phase 67b で直したが、ここでも**境界で** spawn 前に拒否する（将来の回帰がテストで
    // 静かに ULID を通してしまわないよう、falsely-loud に落とす）。
    match req.context.session.as_ref() {
        Some(session) if session.adapter == ClaudeCodeAdapter::ID && session.resume => {
            if !crate::provider::is_valid_uuid(&session.session_id) {
                return Err(AdapterError::Other(format!(
                    "refusing to --resume claude-code session id {:?}: not a valid UUID \
                     (Claude Code CLI 2.1.278+ requires one; ADR-0054 Phase 67b)",
                    session.session_id
                )));
            }
            command.arg("--resume").arg(&session.session_id);
        }
        Some(session) if session.adapter == ClaudeCodeAdapter::ID => {
            if !crate::provider::is_valid_uuid(&session.session_id) {
                return Err(AdapterError::Other(format!(
                    "refusing to --session-id claude-code session id {:?}: not a valid UUID \
                     (Claude Code CLI 2.1.278+ requires one; ADR-0054 Phase 67b)",
                    session.session_id
                )));
            }
            command.arg("--session-id").arg(&session.session_id);
        }
        _ => {
            command.arg("--no-session-persistence");
        }
    }
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    // ADR-0054 D2（Phase 68）: CoS の対話 run だけ、読み取りだけの `celerisctl` を許す
    // （`Bash(celerisctl <サブコマンド>:*)` の形。claude-code の `--allowedTools` はこの許可リストに
    // 無い道具を拒否する＝それ以外は禁止のまま。ADR-0033 D4 の「対話 run は道具を使わない」の例外）。
    if req.context.conversation_addressee == Some(crate::protocol::ConversationAddressee::Secretary)
    {
        let allowed = crate::protocol::CONVERSATION_READONLY_CELERISCTL
            .iter()
            .map(|sub| format!("Bash(celerisctl {sub}:*)"))
            .collect::<Vec<_>>()
            .join(",");
        command.arg("--allowedTools").arg(allowed);
    }
    command.args(&config.extra_args);
    // ADR-0075 G3-fix1: 継いだ値を外してから重ねる（コンテナ実行では `container::wrap` が無視する）。
    crate::adapter::apply_env_removal(&mut command, &config.env_remove);
    // F5-fix5: headless の run では background task を無効にする（Claude Code CLI 2.1.283 は
    // `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` が立っていると Bash / Agent の `run_in_background` を道具の
    // schema から外す）。background が無いぶん、foreground の Bash が run の壁時計まで待てるよう
    // `BASH_MAX_TIMEOUT_MS`（既定 600000）を壁時計に合わせる。どちらも `config.env` が同名を持てば
    // そちらが勝つ（後に `envs` で重ねる）。
    command.env(DISABLE_BACKGROUND_TASKS_ENV, "1").env(
        BASH_MAX_TIMEOUT_ENV,
        limits
            .wall_clock
            .as_millis()
            .clamp(600_000, u128::from(u32::MAX))
            .to_string(),
    );
    command
        .envs(config.env.iter().cloned())
        .current_dir(req.cwd());
    // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
    let mut command = crate::container::wrap(command, config.container.as_deref());
    // F5-fix10: stdin はプロンプトを渡すためだけに開く（書き終えたら閉じるので、対話の入力待ちにはならない。
    // コンテナ実行は `container::argv` が `-i` を付けているので stdin がそのまま中に届く）。
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::subprocess::check_arg_lengths(ClaudeCodeAdapter::ID, &command)?;

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());
    // F5-fix10: プロンプトを stdin に流して閉じる（別タスク。下の stdout 読み取りと並行に進む）。
    let stdin_writer =
        crate::subprocess::feed_stdin(&mut child, prompt, ClaudeCodeAdapter::ID, run_id)?;
    // ADR-0054 D1（Phase 67）: 起動できたら、このアダプタ宛ての継続セッションの id をそのまま報告する
    // （claude-code は id を`自分で`固定するので、成功した spawn の直後に確定する）。
    if let Some(session) = req
        .context
        .session
        .as_ref()
        .filter(|s| s.adapter == ClaudeCodeAdapter::ID)
    {
        sink.session_established(&session.session_id);
    }

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

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_result: Option<ResultMeta> = None;
    let mut background = BackgroundTasks::default();
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;

    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            // ADR-0072 D7（Phase E1）: wall-clock の打ち切りは予算切れ（continuation の対象）。
            // usage はここでは取れない（`result` メッセージを観測する前に打ち切っている）。
            timeout_terminal = Some(Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::WallClock,
                message: "wall clock exceeded".into(),
                usage: None,
            });
            force_kill = true;
            break;
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= limits.idle_timeout {
            // ADR-0072 D7（Phase E1）: idle timeout は E1 では harness_error に分類変更しない
            // （§7 U7。従来どおり retryable な `Error` のまま attempts を消費する）。
            timeout_terminal = Some(Terminal::Error {
                message: "idle timeout".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let wait = (limits.wall_clock - wall_elapsed).min(limits.idle_timeout - idle_elapsed);

        let outcome = match tokio::time::timeout(
            wait,
            read_line_limited(&mut reader, MAX_LINE_BYTES),
        )
        .await
        {
            Err(_elapsed) => continue, // タイムアウト。ループ先頭で上限超過を検知する。
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => outcome,
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                // claude 自身のフォーマットは celeris が定義したものではないため、寛容に無視する（ADR-0006 D5）。
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from claude stdout");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    handle_line(trimmed, sink, &mut last_result, &mut background);
                }
            }
        }
    }

    let exit_status = if force_kill {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };
    // F5-fix10: 子は刈り取った。まだ書き込み中なら（読まずに終わった子）打ち切る。
    stdin_writer.abort();

    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;

    let (mut terminal, provider_failure): (Terminal, Option<ProviderFailure>) = match (
        timeout_terminal,
        &last_result,
    ) {
        // タイムアウト（wall-clock / idle）は分類しない（ADR-0010 D5）。
        (Some(t), _) => (t, None),
        // `result` メッセージを一度も観測できずに exit した場合はクラッシュとして扱い、
        // artifacts/result.json（前回の run の名残や書きかけの内容）を一切信用しない（ADR-0006 D4）。
        // stderr.log の末尾を供給側失敗として分類する（ADR-0010 D5）。
        (None, None) => {
            let exit_repr = match exit_status.code() {
                Some(code) => code.to_string(),
                None => "signal".to_string(),
            };
            let tail = read_tail(&stderr_log_path, 4096).await;
            let pf = classify_provider_failure(&tail);
            // ADR-0054 D1（Phase 67）: resume を頼んだ run が、セッションを拒否されたように見える
            // crash なら報告する（ディスパッチャが `node_sessions` を retire し、次の run は新規
            // セッションになる）。文言は実機で確認していない（`provider::looks_like_resume_rejection`
            // のコメント参照）。
            if is_resuming && crate::provider::looks_like_resume_rejection(&tail) {
                sink.session_resume_failed(&tail);
            }
            (
                Terminal::Error {
                    message: format!("worker exited without a result message (exit={exit_repr})"),
                    retryable: true,
                },
                pf,
            )
        }
        (None, Some(meta)) => {
            // ADR-0006 Phase 115 D2: `result.json` を読む前に、work_dir 側の名残を採用する
            // （Phase 112 D3 相当の「回収」はこのアダプタには無いが、同じ原則で最優先に判定する）。
            adopt_result_json_written_under_work_dir(
                &req.artifacts_dir,
                req.work_dir.as_deref(),
                &req.workspace,
                run_id,
            )
            .await;
            let mut outcome = terminal_from_result(
                &req.artifacts_dir,
                &artifacts_rel,
                meta,
                config.model.as_deref(),
            )
            .await;
            // F5-fix5: `result` は success（`stop_reason: end_turn`）なのに `result.json` が無く、
            // その `result` の時点で background task がまだ走っていた＝headless の run が「通知を
            // 待つ」と言って turn を終え、CLI がその task を殺した（本番 run 01M3KF2HFMHPJR7YEB5HMT38MQ）。
            // coding 系の execute run は ADR-0072 D9 の continuation（新しい session + checkpoint）に
            // 回す（`Terminal::Yielded`。continuation の上限・進捗なしの判定はディスパッチャの既存の
            // 規則に任せる）。それ以外の run は従来どおり `result.json` 不在の失敗のまま、文言に分類名を足す。
            let missing_result_json = match &outcome {
                (Terminal::Error { message, .. }, None)
                    if message.starts_with(RESULT_JSON_MISSING_MARKER) =>
                {
                    Some(message.clone())
                }
                _ => None,
            };
            if let Some(message) = missing_result_json
                && req.context.browser.is_none()
                && !meta.is_error
                && meta.subtype == "success"
                && meta.stop_reason.as_deref().is_none_or(|r| r == "end_turn")
                && !meta.outstanding_background.is_empty()
            {
                let commands = meta.outstanding_background.join(" | ");
                if is_continuable_execute_run(&req.task, &req.context) {
                    sink.progress(&format!(
                            "{HEADLESS_BACKGROUND_TASK_CLASS}: the run ended its turn while background \
                             task(s) were still running; they were killed and no result.json was written. \
                             Handing over to a continuation run (re-run in the foreground): {commands}"
                        ));
                    outcome = (
                        Terminal::Yielded {
                            checkpoint: headless_background_checkpoint(
                                &req.artifacts_dir,
                                &meta.outstanding_background,
                            )
                            .await,
                            usage: usage_with_cost(meta.usage, config.model.as_deref()),
                        },
                        None,
                    );
                } else {
                    sink.progress(&format!(
                        "{HEADLESS_BACKGROUND_TASK_CLASS}: the run ended its turn while background \
                             task(s) were still running; they were killed: {commands}"
                    ));
                    outcome = (
                        Terminal::Error {
                            message: format!(
                                "{message} ({HEADLESS_BACKGROUND_TASK_CLASS}: background task \
                                     killed when the headless turn ended: {commands})"
                            ),
                            retryable: true,
                        },
                        None,
                    );
                }
            }
            // ADR-0054 D1（Phase 113 追記）: `result` メッセージは一度観測できたが、供給側の失敗
            // （`subtype: error_during_execution` かつ `is_error`）として終わった run は、上の
            // `(None, None)` のクラッシュ分類（resume 拒否の検出）を一切通らなかった。本番の
            // タスク 01M35X86XTK84F97QW0CN5PGMR / reviewer run 01M388BENASH3JEBWFS03KEQYT はこの
            // 穴に落ちた（stderr は `No conversation found with session ID: …` の 1 行だったが、
            // stdout の `result` の `result` フィールドにはその文言が無かったため、`result` を
            // 観測できてしまった＝ここに来て、resume 拒否として扱われなかった）。resume を頼んだ
            // run が `error_during_execution`/`is_error` で終わったときは、stderr の末尾と
            // `result` メッセージ自身の文面の両方を resume 拒否の文言と照らす。
            if is_resuming && meta.is_error && meta.subtype == "error_during_execution" {
                let tail = read_tail(&stderr_log_path, 4096).await;
                let result_text = meta.result.as_deref().unwrap_or("");
                if crate::provider::looks_like_resume_rejection(&tail)
                    || crate::provider::looks_like_resume_rejection(result_text)
                {
                    sink.session_resume_failed(&tail);
                }
            }
            outcome
        }
    };

    forward_delegate_file(&req.artifacts_dir, sink).await;

    // CLI result errors can reflect command arguments or page text even when raw capture is off.
    // Keep the failure classification and normal public summaries, but omit reflected error bodies.
    if req.context.browser.is_some() {
        match &mut terminal {
            Terminal::Error { message, .. } | Terminal::BudgetExhausted { message, .. } => {
                if message.starts_with(RESULT_JSON_MISSING_MARKER) {
                    *message = format!("{RESULT_JSON_MISSING_MARKER}browser result.json");
                } else {
                    *message = "browser harness failed".into();
                }
            }
            _ => {}
        }
    }
    write_result_json(&run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }
    // ADR-0072 E2（P-E0-2 の修正）: `result.json` そのものが無かった run（`RESULT_JSON_MISSING_MARKER`）
    // は `ProviderFailure` を持たない `Err(AdapterError)` にする。`provider_failure_outcome` は
    // これを分類できない失敗として扱い、ディスパッチャは ADR-0070 D3 の `InfraRequeue`
    // （attempts を消費しない。上限に達したときだけ `WorkerError{retryable:false}`）に倒す。
    if let Terminal::Error { message, .. } = &terminal
        && message.starts_with(RESULT_JSON_MISSING_MARKER)
    {
        return Err(AdapterError::Other(message.clone()));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

/// 壁時計の Unix 秒（ADR-0024 D4: 観測時刻は celeris の壁時計）。`claude_account` の確認・ログイン中継からも使う。
pub(crate) fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// stream-json の 1 行を解釈する。既知でない `type` や JSON として不正な行は無視する（ADR-0006 D5）。
fn handle_line(
    line: &str,
    sink: &dyn EventSink,
    last_result: &mut Option<ResultMeta>,
    background: &mut BackgroundTasks,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let Some(ty) = value.get("type").and_then(|t| t.as_str()) else {
        return;
    };
    if ty == "rate_limit_event"
        && let Some(obs) = RateLimitObservation::from_stream_json(&value, now_unix_secs())
    {
        sink.rate_limit(obs);
    }
    match ty {
        // ADR-0048 D2（Phase 60a）: stream-json → 正規化した進行。`msg` の文面は Phase 59 までと同じ
        // （`tool_use: <名前> <入力>` / 本文そのまま）で、構造化フィールドを**足すだけ**。
        // 本文（`text`）は 1 つの assistant メッセージ分をまとめて 1 件にする。
        "assistant" => {
            if let Some(content) = value.pointer("/message/content").and_then(|c| c.as_array()) {
                let mut texts: Vec<&str> = Vec::new();
                let flush = |texts: &mut Vec<&str>, sink: &dyn EventSink| {
                    if texts.is_empty() {
                        return;
                    }
                    let joined = texts.join("\n");
                    texts.clear();
                    let msg = truncate(&joined, 500);
                    sink.progress_with(&msg, &progress::text(&joined));
                };
                for item in content {
                    match item.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                texts.push(text);
                            }
                        }
                        Some("thinking") => {
                            flush(&mut texts, sink);
                            // 思考は**要約だけ**（本文は流さない）。要約が取れなければ何も出さない。
                            let summary = item
                                .get("thinking")
                                .or_else(|| item.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            if !summary.trim().is_empty() {
                                let fields = progress::thinking(&progress::one_line(summary));
                                let msg = format!(
                                    "thinking: {}",
                                    fields.summary.clone().unwrap_or_default()
                                );
                                sink.progress_with(&msg, &fields);
                            }
                        }
                        Some("tool_use") => {
                            flush(&mut texts, sink);
                            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                            let input = item.get("input");
                            let shown = input
                                .map(|v| truncate(&v.to_string(), 200))
                                .unwrap_or_default();
                            sink.progress_with(
                                &format!("tool_use: {name} {shown}"),
                                &progress::tool_use(name, input),
                            );
                        }
                        _ => {}
                    }
                }
                flush(&mut texts, sink);
            }
        }
        // 道具の結果は `user` メッセージに `tool_result` として返る（`is_error` が失敗の印）。
        "user" => {
            if let Some(content) = value.pointer("/message/content").and_then(|c| c.as_array()) {
                for item in content {
                    if item.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let body = tool_result_text(item);
                    let error = item
                        .get("is_error")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let fields = progress::tool_result(None, &body, error);
                    let head = fields.summary.clone().unwrap_or_default();
                    let msg = if error {
                        format!("tool_result (error): {head}")
                    } else {
                        format!("tool_result: {head}")
                    };
                    sink.progress_with(&msg, &fields);
                }
            }
        }
        "result" => {
            let subtype = value
                .get("subtype")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string();
            let is_error = value
                .get("is_error")
                .and_then(|b| b.as_bool())
                .unwrap_or(subtype != "success");
            // ADR-0061（Phase 104）: claude-code CLI の `result.usage` は Anthropic API と同じ形
            // （`cache_creation_input_tokens` / `cache_read_input_tokens` を含む）。`cost_usd` はここでは
            // 計算しない（model 文字列は呼び出し元でしか分からない。`terminal_from_result` が埋める）。
            let usage = value.get("usage").map(|u| Usage {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()),
                cache_read_tokens: u.get("cache_read_input_tokens").and_then(|v| v.as_u64()),
                cache_creation_tokens: u
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64()),
                cost_usd: None,
            });
            let result = value
                .get("result")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let stop_reason = value
                .get("stop_reason")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            *last_result = Some(ResultMeta {
                subtype,
                is_error,
                usage,
                result,
                stop_reason,
                outstanding_background: background.outstanding(),
            });
        }
        // F5-fix5: background task の開始・終わり（`result` の後に届く kill の通知も含めて追う）。
        "system" => background.observe(&value),
        _ => {}
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

/// ADR-0048 D2: `tool_result` の本文。Claude Code は文字列の `content` と、`[{"type":"text","text":…}]`
/// の配列の両方を出す（どちらも読む）。
fn tool_result_text(item: &serde_json::Value) -> String {
    let Some(content) = item.get("content") else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(parts) = content.as_array() {
        let joined: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect();
        if !joined.is_empty() {
            return joined.join("\n");
        }
    }
    content.to_string()
}

/// `result` メッセージと結果ファイルから終端を合成する（ADR-0006 D3/D4, ADR-0010 D5）。呼び出し元は
/// `result` メッセージを一度でも観測できた場合にのみこれを呼ぶ（観測できなかった場合は
/// クラッシュとして扱い、この関数を呼ばずに `Error` にする。ADR-0006 D4）。`is_error`/`subtype != "success"`
/// のときは `result` のテキスト（無ければ `subtype`）を供給側失敗として分類する。
/// ADR-0061（Phase 104）: model が分かる時だけ静的単価表から USD を推定して埋める（不明なモデル・
/// token 欠落は `None` のまま。ここでは断定しない）。
fn usage_with_cost(usage: Option<Usage>, model: Option<&str>) -> Option<Usage> {
    usage.map(|mut u| {
        if let Some(model) = model {
            u.cost_usd = task_core::estimate_cost_usd(model, &u);
        }
        u
    })
}

/// ADR-0072 E2（P-E0-2 の修正）: `result` メッセージは観測できたのに `result.json` 自体が無いときの
/// `Terminal::Error.message` の接頭辞。`run` がこの接頭辞を見て `Err(AdapterError)`（InfraRequeue）に
/// 倒す（他の `result.json` の失敗〈壊れた JSON・必須欄の欠落〉は worker 自身の誤りのまま）。
const RESULT_JSON_MISSING_MARKER: &str = "claude exited without ";

/// F5-fix5: headless の run が background task を残して turn を終えた失敗の分類名（`Terminal` の文言・
/// 進行のメッセージに載せる。GUI の run の終わり方・進行の欄にそのまま出る）。
pub const HEADLESS_BACKGROUND_TASK_CLASS: &str = "headless_background_task";

/// F5-fix5: Claude Code CLI の background task を無効にする環境変数（2.1.283 で確認）。
const DISABLE_BACKGROUND_TASKS_ENV: &str = "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS";

/// F5-fix5: Claude Code CLI の Bash の `timeout` の上限（ミリ秒。既定 600000）。
const BASH_MAX_TIMEOUT_ENV: &str = "BASH_MAX_TIMEOUT_MS";

async fn terminal_from_result(
    artifacts_dir: &Path,
    artifacts_rel: &str,
    last_result: &ResultMeta,
    model: Option<&str>,
) -> (Terminal, Option<ProviderFailure>) {
    if last_result.is_error || last_result.subtype != "success" {
        // ADR-0072 D7（Phase E1）: `error_max_turns` は `--max-turns` の上限に当たったという
        // claude-code 自身の分類なので、字句判定なしで構造化できる（wall-clock は別経路。上の呼び出し元
        // の loop の timeout で検出する）。それ以外の `is_error`/`subtype != success` は、`result` の
        // 文言が context 超過の語彙に当たれば `BudgetExhausted{kind: Context}`（§7 U1: 実機の文言は
        // 未確認。分類できなければ従来どおり `Error`）。usage は予算切れでも運ぶ（従来は捨てていた）。
        let usage = usage_with_cost(last_result.usage, model);
        if last_result.subtype == "error_max_turns" {
            return (
                Terminal::BudgetExhausted {
                    kind: task_core::BudgetKind::Turns,
                    message: "claude result: error_max_turns".to_string(),
                    usage,
                },
                None,
            );
        }
        let text_for_classification = last_result
            .result
            .clone()
            .unwrap_or_else(|| last_result.subtype.clone());
        if task_core::looks_like_context_exceeded(&text_for_classification) {
            return (
                Terminal::BudgetExhausted {
                    kind: task_core::BudgetKind::Context,
                    message: format!(
                        "claude result: {}: {text_for_classification}",
                        last_result.subtype
                    ),
                    usage,
                },
                None,
            );
        }
        let pf = classify_provider_failure(&text_for_classification);
        let message = match &last_result.result {
            Some(result_text) => format!("claude result: {}: {result_text}", last_result.subtype),
            None => format!("claude result: {}", last_result.subtype),
        };
        return (
            Terminal::Error {
                message,
                retryable: true,
            },
            pf,
        );
    }

    let result_path = artifacts_dir.join("result.json");
    let text = match tokio::fs::read_to_string(&result_path).await {
        Ok(t) => t,
        Err(_) => {
            // ADR-0072 E2（P-E0-2 の修正）: `result` メッセージは観測できた（= claude 自身は正常に
            // 終わったと申告した）のに `result.json` そのものが無い（Phase 112 D3 / 115 D2 の
            // work_dir 側の回収を試みた後もなお無い）。これは worker 自身の判断の誤りというより、
            // 書き込みが間に合わなかった・消えたという供給側/インフラ側の事情に近い。呼び出し元
            // （`run`）はこの文言（`RESULT_JSON_MISSING_MARKER`）を見て `Err(AdapterError)`
            // （`ProviderFailure` は無し）に倒し、ADR-0070 D3 どおり attempts を消費しない
            // `InfraRequeue` の経路に乗せる（従来は `Ok(Terminal::Error{retryable:true})` になり、
            // `WorkerError{true}` として attempts を消費していた）。
            return (
                Terminal::Error {
                    message: format!("{RESULT_JSON_MISSING_MARKER}{artifacts_rel}/result.json"),
                    retryable: true,
                },
                None,
            );
        }
    };

    // ADR-0090 D1: クラスタ job の終了待ち（`question` が無ければ `summary` より優先）。
    if let Some(terminal) =
        crate::adapter::result_file_wait(&text, usage_with_cost(last_result.usage, model))
    {
        return (terminal, None);
    }
    let terminal = match serde_json::from_str::<ResultFile>(&text) {
        Ok(rf) => {
            // ADR-0072 D9: 優先順位は `question` > `summary` > `yield`（ADR-0090: `wait` は `summary` の前）。
            if let Some(question) = rf.question {
                Terminal::Question { text: question }
            } else if let Some(summary) = rf.summary {
                Terminal::Done {
                    summary,
                    evidence: lenient_evidence(rf.evidence),
                    usage: usage_with_cost(last_result.usage, model),
                }
            } else if let Some(checkpoint) = rf.r#yield {
                Terminal::Yielded {
                    checkpoint,
                    usage: usage_with_cost(last_result.usage, model),
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
    };
    (terminal, None)
}

#[cfg(test)]
mod tests;
