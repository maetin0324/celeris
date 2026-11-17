//! 研究系 adapter（`langmem`・`local-deep-research`・`paperqa`）が共有する Python ランナーの起動と読み取り。
//!
//! どの adapter も、埋め込みのランナーと入力の JSON を `runs/<run_id>/` に書き、`<command> <script> <input>`
//! を本番 DB を読み取り専用にした namespace で起動し（ADR-0095 D2。コンテナ非対応）、標準出力を 1 行ずつ
//! 読みながら生存監視（壁時計・無出力）をする。行の意味（`progress:`・結果の目印）は adapter ごとに違うので
//! `stream_child` の `on_line` に任せる。締め切りの規則は `adapter::deadline_terminal`
//! （ADR 2026-11-15-research-adapter-deadline D1）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, Terminal, next_line, open_child_streams};
use crate::subprocess::{LineOutcome, kill_now, read_tail, reap_after_terminal};

/// ランナー（`script`）と入力（`input`、整形した JSON と末尾の改行）を `run_dir` に書き、両方の path を返す。
pub(crate) async fn write_runner_files(
    run_dir: &Path,
    script_name: &str,
    script: &str,
    input_name: &str,
    input: &serde_json::Value,
) -> Result<(PathBuf, PathBuf), AdapterError> {
    let script_path = run_dir.join(script_name);
    let input_path = run_dir.join(input_name);
    tokio::fs::write(&script_path, script).await?;
    let input_text = serde_json::to_string_pretty(input)?;
    tokio::fs::write(&input_path, format!("{input_text}\n")).await?;
    Ok((script_path, input_path))
}

/// `<program> <script> <input>` を組む。env は `env` の順に入れる（同名の鍵は後勝ち）。stdin は閉じ、
/// stdout・stderr は pipe、process group を分け、本番 DB を読み取り専用にした namespace で起動する。
pub(crate) fn runner_command(
    program: impl AsRef<std::ffi::OsStr>,
    script_path: &Path,
    input_path: &Path,
    env: impl IntoIterator<Item = (String, String)>,
    cwd: &Path,
) -> Command {
    let mut command = Command::new(program);
    command
        .arg(script_path)
        .arg(input_path)
        .envs(env)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    // ADR-0095 D2: 本番 DB を読み取り専用にした namespace で起動する（コンテナ非対応の adapter）。
    crate::db_guard::launch(command, None)
}

/// 子プロセスの標準出力を読み切った結果。
pub(crate) struct StreamedRun {
    pub(crate) stdout: String,
    pub(crate) stderr_tail: String,
    pub(crate) exit: std::process::ExitStatus,
    /// 上限を超えて強制終了したときの終端（超えていなければ `None`）。
    pub(crate) timeout: Option<Terminal>,
}

/// 子プロセスの標準出力を 1 行ずつ読み、生存監視（壁時計は `start` から、無出力は `limits.idle_timeout`）を
/// 行う。空でない行を trim して `on_line` に渡し、全行を `stdout_log_path` と戻り値の `stdout` に残す。
/// 長すぎる行は捨てる（ランナーの出力形式は celeris が定義したものではないので寛容に無視する）。
/// `label` は捨てたときの warn に出す名前。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_child(
    mut child: Child,
    stdout_log_path: PathBuf,
    stderr_log_path: PathBuf,
    start: Instant,
    limits: &RunLimits,
    run_id: &str,
    sink: &dyn EventSink,
    label: &str,
    mut on_line: impl FnMut(&str),
) -> Result<StreamedRun, AdapterError> {
    let (mut reader, mut stdout_file, stderr_task) =
        open_child_streams(&mut child, &stdout_log_path, &stderr_log_path, run_id).await?;

    let mut last_activity = Instant::now();
    let mut stdout_buf = String::new();
    let mut timeout_terminal: Option<Terminal> = None;

    loop {
        // wall clock の超過は `BudgetExhausted{WallClock}`、idle の超過は retryable な `Error`。
        let outcome = match next_line(&mut reader, start, last_activity, limits).await? {
            Ok(outcome) => outcome,
            Err(terminal) => {
                timeout_terminal = Some(terminal);
                break;
            }
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from the {label}");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim().to_string();
                stdout_buf.push_str(&text);
                stdout_buf.push('\n');
                if !trimmed.is_empty() {
                    on_line(&trimmed);
                }
            }
        }
    }

    let exit = if timeout_terminal.is_some() {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };
    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;
    let stderr_tail = read_tail(&stderr_log_path, 4096).await;

    Ok(StreamedRun {
        stdout: stdout_buf,
        stderr_tail,
        exit,
        timeout: timeout_terminal,
    })
}

/// 空白を 1 つに畳んでから文字数で切る（`artifacts/result.json` の `summary` に使う）。
pub(crate) fn single_line_summary(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, max_chars)
}

/// 文字数で切る（末尾に印を付けない。`progress::truncate_chars` は `…` を付ける）。
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}
