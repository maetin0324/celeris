//! 研究系 adapter（`langmem`・`local-deep-research`・`paperqa`）が共有する Python ランナーの起動と読み取り。
//!
//! どの adapter も、埋め込みのランナーと入力の JSON を `runs/<run_id>/` に書き、`<command> <script> <input>`
//! を本番 DB を読み取り専用にした namespace で起動し（ADR-0095 D2。コンテナ非対応）、標準出力を 1 行ずつ
//! 読みながら生存監視（壁時計・無出力）をする。行の意味（`progress:`・結果の目印）は adapter ごとに違うので
//! `stream_child` の `on_line` に任せる。締め切りの規則は `adapter::deadline_terminal`
//! （ADR 2026-11-15-research-adapter-deadline D1）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Instant, SystemTime};

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, Terminal};
use crate::child_stream::{next_line, open_child_streams};
use crate::protocol::RunRequest;
use crate::result_report::{ResultFileRead, interpret_result_text, read_result_file};
use crate::subprocess::{LineOutcome, kill_now, read_tail, reap_after_terminal};

/// `artifacts/result.json` の `summary` の上限（研究系 adapter で同じ規則。ADR-0027 D3・ADR-0029 D1）。
pub(crate) const SUMMARY_MAX_CHARS: usize = 1500;
/// ランナーの `progress:` 行を `progress` に転送するときの 1 行あたりの上限。
pub(crate) const PROGRESS_LINE_MAX_CHARS: usize = 500;
/// ランナーの進捗行の目印。
pub(crate) const PROGRESS_PREFIX: &str = "progress:";
/// ランナーの最終行の目印（ADR-0029 D1。langmem・local-deep-research）。
pub(crate) const RESULT_PREFIX: &str = "CELERIS_RESULT ";

/// langmem・local-deep-research のランナーの 1 行: `progress: <text>`（ADR-0029 D1 手順 3）は進捗に写し、
/// `CELERIS_RESULT <json>` は `result` に入れる。他の行は読み捨てる（`stdout.log` には残る）。
pub(crate) fn progress_or_result(
    line: &str,
    sink: &dyn EventSink,
    run_id: &str,
    result: &mut Option<serde_json::Value>,
) {
    if let Some(rest) = line.strip_prefix(PROGRESS_PREFIX) {
        crate::progress::emit_status(sink, &truncate_chars(rest.trim(), PROGRESS_LINE_MAX_CHARS));
    } else if let Some(rest) = line.strip_prefix(RESULT_PREFIX) {
        match serde_json::from_str::<serde_json::Value>(rest) {
            Ok(value) => *result = Some(value),
            Err(e) => warn!("run {run_id}: could not parse CELERIS_RESULT line: {e}"),
        }
    }
}

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

/// ADR 2026-11-15-research-adapter-deadline D4: ランナー（回答）の要約から `artifacts/result.json` を書き、
/// 結果契約の正本（`result_report::read_result_file`・`interpret_result_text`。ADR 2026-11-10-adapter-result-contract
/// D1）で読み直して終端を作る。期待の置き場に file があるので誤配置の回収は探索しない。書けなければ
/// `read_result_file` を呼ばずに `Error{retryable: true}`（reviewer が読む file の無い run を `Done` にしない）。
/// `start` は監視 loop の開始（`read_result_file` の `run_started` に直す）。`adapter` は誤りの文に出す名前。
pub(crate) async fn terminal_from_summary(
    adapter: &str,
    req: &RunRequest,
    start: Instant,
    summary: &str,
) -> Terminal {
    let artifacts_rel = req.artifacts_rel();
    let body = serde_json::json!({ "summary": summary, "evidence": [] });
    let written = tokio::fs::write(req.artifact_path("result.json"), format!("{body:#}\n")).await;
    if let Err(e) = written {
        return Terminal::Error {
            message: format!("{adapter} adapter could not write {artifacts_rel}/result.json: {e}"),
            retryable: true,
            usage: None,
        };
    }
    match read_result_file(req, SystemTime::now() - start.elapsed()).await {
        ResultFileRead::Misplaced(error) => *error,
        ResultFileRead::Missing => Terminal::Error {
            message: format!("{adapter} adapter could not read back {artifacts_rel}/result.json"),
            retryable: true,
            usage: None,
        },
        ResultFileRead::Text(text) => interpret_result_text(&text, &artifacts_rel, None),
    }
}

/// `artifacts/result.json` の `summary`: 改行・連続空白を単一の空白にたたみ（single-line-safe）、
/// 文字数で上限まで切り詰める（ランナー側でも行うが、二重に安全側へ倒す）。
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
