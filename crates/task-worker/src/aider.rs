//! `aider` アダプタ（ADR-0061, Phase 104）。
//!
//! ADR-0061 のハーネス分類表の「明確で局所的な少数ファイル修正 → aider 系」の実装。`aider` CLI は
//! celeris 独自のワーカープロトコルもストリーム JSON も話さない、プレーンテキストの対話ツールなので、
//! `claude-code`/`codex`（ADR-0006/ADR-0008）と同じ「結果ファイル規約」（`artifacts/result.json`）で
//! `RunOutcome` を合成する。プロンプト組み立ては `claude_code::build_prompt` をそのまま再利用する
//! （kind 別の文面をアダプタごとに複製しない、という既存の方針を踏襲）。
//!
//! `codex.rs`/`claude_code.rs` と違い、`aider` の標準出力は 1 行 1 JSON ではなく人間向けの文章なので、
//! この実装はそれらより単純: JSON Lines の逐次解釈をせず、各行をそのまま `sink.progress` に流し、
//! プロセスの終了後に `artifacts/result.json` を読むだけである（`fake` アダプタの「起動して待つだけ」
//! に近い。生存監視〈wall-clock・無出力タイムアウト・SIGTERM→SIGKILL〉は `subprocess.rs` を再利用）。
//!
//! トークン使用量は aider 自身が終了直前に stdout へ書く `Tokens: N sent, M received.` /
//! `Cost: $X message, $Y session.` を最良努力で拾う（無ければ `None`。ADR-0061「取得可能な範囲」）。
//! 実機（`aider-chat` 0.86.2、モック OpenAI 互換エンドポイント）で書式を確認済み（`agent-docs/PROGRESS.md`
//! Phase 104 参照）。`Cost:` 行があればそれをそのまま使い、無ければ `model` が分かるときだけ
//! `adapter::with_estimated_cost`（静的単価表）で推定する（aider 自身の実測値を優先する）。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use task_core::Usage;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{
    AdapterError, AdapterSetting, AdapterSettings, Configured, EventSink, RunLimits, RunOutcome,
    Terminal, WorkerAdapter, deadline_terminal, spawn_capture_stderr, with_estimated_cost,
};
use crate::claude_code::build_prompt;
use crate::delegate_file::forward_delegate_file;
use crate::protocol::RunRequest;
use crate::provider::classify_provider_failure;
use crate::result_report::{ResultFileRead, interpret_result_text, read_result_file};
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, finish_run, kill_now, prepare_run, read_line_limited, read_tail,
    reap_after_terminal,
};

/// `[adapters.aider]`（config.toml, ADR-0061）。
#[derive(Debug, Clone)]
pub struct AiderConfig {
    /// 起動するコマンド。既定 `"aider"`。
    pub command: String,
    /// `--message` の前に追加する引数（例: `["--architect"]`）。
    pub extra_args: Vec<String>,
    /// モデル指定（`--model`。省略時は aider 自身の既定モデル）。
    pub model: Option<String>,
    /// 追加の環境変数（`OPENAI_API_KEY` 等）。
    pub env: Vec<(String, String)>,
    /// ADR-0043 D3（Phase 56）: `Some` なら `aider` をコンテナの中で起こす（`container::wrap`）。
    pub container: Option<crate::container::SharedPlan>,
}

impl Default for AiderConfig {
    fn default() -> Self {
        Self {
            command: "aider".to_string(),
            extra_args: Vec::new(),
            model: None,
            env: Vec::new(),
            container: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AiderAdapter {
    config: AiderConfig,
}

impl AiderAdapter {
    pub const ID: &'static str = "aider";

    pub fn new(config: AiderConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for AiderAdapter {
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
        run_aider(&self.config, &req, run_id, &limits, sink).await
    }

    /// ADR 2026-11-10-worker-capability-ports D3: model・env（同名キーは後勝ち）・コンテナを受ける。
    fn configure(&self, settings: &AdapterSettings) -> Configured {
        settings.configured(
            &[
                AdapterSetting::Model,
                AdapterSetting::Env,
                AdapterSetting::Container,
            ],
            || {
                let mut config = self.config.clone();
                settings.apply_model(&mut config.model);
                settings.apply_env(&mut config.env);
                settings.apply_container(&mut config.container);
                Arc::new(Self::new(config))
            },
        )
    }
}

async fn run_aider(
    config: &AiderConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let paths = prepare_run(req, run_id, "stdout.log").await?;
    let run_dir = &paths.run_dir;
    tokio::fs::create_dir_all(&req.artifacts_dir).await?;

    let prompt = build_prompt(&req.task, &req.context, run_id, &paths.artifacts_rel);
    crate::subprocess::write_run_prompt(run_dir, &prompt, run_id).await;

    let mut command = Command::new(&config.command);
    command
        // 対話プロンプトを一切出さない（一度限りの `--message` 実行。ADR-0061）。
        .arg("--yes-always")
        .arg("--no-check-update")
        .arg("--no-show-model-warnings")
        // celeris のタスクライフサイクルが commit を管理する（Phase 27 の「提案 vs 結果」規約と同じ理由で、
        // アダプタが黙って git へコミットしない）。
        .arg("--no-auto-commits")
        .arg("--no-gitignore");
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    command.args(&config.extra_args);
    // F5-fix10（claude-code / codex と同じ MAX_ARG_STRLEN 対策）: aider には stdin からメッセージを読む
    // 形が無いので、run dir の中のファイルに書いて `--message-file` で渡す（`-f/--message-file`: そのファイルの
    // 中身を 1 回送って終わる。`--message` と同じ非対話実行）。run dir はタスクのディレクトリの下なので、
    // コンテナ実行でも同じパスでマウントされている（`container::argv`）。`prompt.txt`（記録）とは別に、
    // 書けなければ run を始めない（best-effort ではない）。
    let message_path = run_dir.join("aider-message.txt");
    tokio::fs::write(&message_path, &prompt).await?;
    command.arg("--message-file").arg(&message_path);
    command
        .envs(config.env.iter().cloned())
        .current_dir(req.cwd());
    let context_settings = crate::routing_context_transport::configure_aider(
        &mut command,
        &config.env,
        config.model.as_deref(),
        &config.extra_args,
        run_dir,
        req,
    );
    crate::routing_context_transport::record(run_dir, req, context_settings.is_some()).await;
    // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
    let mut command = crate::db_guard::launch(command, config.container.as_deref());
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::subprocess::check_arg_lengths(AiderAdapter::ID, &command)?;

    let mut child = crate::subprocess::spawn_after_notice(sink, &mut command)?;
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdout was not piped".into()))?;
    let stderr_task = spawn_capture_stderr(&mut child, &paths.stderr_log, run_id)?;

    let mut stdout_file = tokio::fs::File::create(&paths.stdout_log).await?;
    let mut reader = BufReader::new(stdout);

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;
    let mut stdout_text = String::new();

    loop {
        // aider には turn の上限が無いので、wall-clock の打ち切りが continuation の唯一の入口になる。
        let wait =
            match deadline_terminal(start, last_activity, limits.wall_clock, limits.idle_timeout) {
                Ok(wait) => wait,
                Err(terminal) => {
                    timeout_terminal = Some(*terminal);
                    force_kill = true;
                    break;
                }
            };

        let outcome = match tokio::time::timeout(
            wait,
            read_line_limited(&mut reader, MAX_LINE_BYTES),
        )
        .await
        {
            Err(_elapsed) => continue,
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => outcome,
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from aider stdout");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    stdout_text.push_str(trimmed);
                    stdout_text.push('\n');
                    // aider は 1 行 1 JSON を話さない（ADR-0061）ので、行をそのまま節目として流す。
                    sink.progress(trimmed);
                }
            }
        }
    }

    let exit_status = if force_kill {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };

    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;

    forward_delegate_file(&req.artifacts_dir, sink).await;

    let (terminal, provider_failure) = if let Some(t) = timeout_terminal {
        (t, None)
    } else {
        let usage = with_estimated_cost(parse_aider_usage(&stdout_text), config.model.as_deref());
        let terminal = terminal_from_result(req, start, usage).await;
        let provider_failure = if matches!(terminal, Terminal::Error { .. }) {
            let tail = read_tail(&paths.stderr_log, 4096).await;
            classify_provider_failure(&tail).or_else(|| classify_provider_failure(&stdout_text))
        } else {
            None
        };
        (terminal, provider_failure)
    };

    finish_run(run_dir, terminal, provider_failure, exit_status).await
}

/// 結果ファイルの読み取り結果から終端を合成する。本文の解釈は結果契約の正本
/// （`result_report::interpret_result_text`。ADR 2026-11-10-adapter-result-contract D1）に任せ、ここは
/// 結果ファイルが無いときの aider の文言だけを持つ。
/// `start` は監視 loop の開始（spawn の直後）。誤配置の候補は、それを壁時計に直した時刻より後に書かれた file に絞る
/// （`result_location::misplaced_report` は 2 秒の余裕を取る）。
async fn terminal_from_result(req: &RunRequest, start: Instant, usage: Option<Usage>) -> Terminal {
    let artifacts_rel = req.artifacts_rel();
    match read_result_file(req, std::time::SystemTime::now() - start.elapsed()).await {
        ResultFileRead::Misplaced(error) => *error,
        ResultFileRead::Missing => Terminal::Error {
            message: format!("aider exited without {artifacts_rel}/result.json"),
            retryable: true,
            usage: None,
        },
        ResultFileRead::Text(text) => interpret_result_text(&text, &artifacts_rel, usage),
    }
}

/// aider が stdout の最後に書く `Tokens: N sent, M received.`（`--no-stream` でも `--stream` でも
/// 出す。実機確認: aider-chat 0.86.2）を拾う。桁が大きいと `1.2k` のような省略形になることがあり、
/// 誤ったトークン数を記録しないためそのときは `None` のまま無視する（取れる範囲だけ埋める、
/// という ADR-0061 の方針）。同じ行の直後に出ることがある
/// `Cost: $0.0132 message, $0.0132 session.` の `session` 側の金額も拾う。
fn parse_aider_usage(stdout: &str) -> Option<Usage> {
    let mut input_tokens = None;
    let mut output_tokens = None;
    let mut session_cost = None;
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Tokens:") {
            let mut parts = rest.trim_end_matches('.').split(',');
            input_tokens = parts.next().and_then(|p| exact_token_count(p, "sent"));
            output_tokens = parts.next().and_then(|p| exact_token_count(p, "received"));
        } else if let Some(rest) = line.strip_prefix("Cost:") {
            session_cost = dollar_amount_before(rest, "session");
        }
    }
    if input_tokens.is_none() && output_tokens.is_none() && session_cost.is_none() {
        return None;
    }
    Some(Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        cost_usd: session_cost,
        duplicate_reads: None,
        session_resumed: None,
        context_tokens: None,
    })
}

/// `" 123 sent"` のような区間から、`label` を含み、省略形（`k`/`M`/小数点）でない整数だけを拾う。
fn exact_token_count(segment: &str, label: &str) -> Option<u64> {
    let segment = segment.trim();
    if !segment.contains(label) {
        return None;
    }
    let digits: String = segment.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest_after_digits = &segment[digits.len()..];
    // 数字の直後が空白のみ（そのあと label が続く）なら省略形でない整数とみなす。
    if digits.is_empty() || !rest_after_digits.trim_start().starts_with(label) {
        return None;
    }
    digits.parse().ok()
}

/// `"$0.0132 message, $0.0132 session"` のような文字列から、`label` の直前にある `$` 金額を拾う。
fn dollar_amount_before(text: &str, label: &str) -> Option<f64> {
    let idx = text.find(label)?;
    let before = &text[..idx];
    let dollar = before.rfind('$')?;
    let amount: String = before[dollar + 1..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    amount.parse().ok()
}

#[cfg(test)]
mod tests;
