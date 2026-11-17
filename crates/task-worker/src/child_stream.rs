//! harness（claude_code・codex・pi・acp）と研究ランナーの監視 loop が共有する、子プロセスの stdout・stderr
//! の用意と締め切り付きの 1 行読み。`adapter`（終端・締め切りの規則）と `subprocess`（行の読み取り）を使う側に置き、
//! どちらからも参照しない（module の循環を作らない）。

use std::path::Path;
use std::time::Instant;

use tokio::process::Child;
use tokio::task::JoinHandle;

use crate::adapter::{AdapterError, RunLimits, Terminal, deadline_terminal, spawn_capture_stderr};
use crate::subprocess::{LineOutcome, MAX_LINE_BYTES, read_line_limited};

/// 締め切り（`deadline_terminal`）まで待って stdout の次の 1 行を読む（harness・研究ランナーの監視 loop が共有）。
/// 上限を超えたら `Ok(Err(terminal))`。待つ間に時間切れになっても締め切りを測り直して読み続ける。
pub(crate) async fn next_line<R>(
    reader: &mut R,
    start: Instant,
    last_activity: Instant,
    limits: &RunLimits,
) -> Result<Result<LineOutcome, Terminal>, AdapterError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    loop {
        let wait =
            match deadline_terminal(start, last_activity, limits.wall_clock, limits.idle_timeout) {
                Ok(wait) => wait,
                Err(terminal) => return Ok(Err(*terminal)),
            };
        let read = read_line_limited(reader, MAX_LINE_BYTES);
        match tokio::time::timeout(wait, read).await {
            Err(_elapsed) => continue,
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => return Ok(Ok(outcome)),
        }
    }
}

/// 子の stdout を読む reader と、その写し先の `stdout_log`、stderr を `stderr_log` に写す task を用意する
/// （harness・研究ランナーの監視 loop が共有する入口）。
pub(crate) async fn open_child_streams(
    child: &mut Child,
    stdout_log: &Path,
    stderr_log: &Path,
    run_id: &str,
) -> Result<ChildStreams, AdapterError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdout was not piped".into()))?;
    let stderr_task = spawn_capture_stderr(child, stderr_log, run_id)?;
    let stdout_file = tokio::fs::File::create(stdout_log).await?;
    Ok((tokio::io::BufReader::new(stdout), stdout_file, stderr_task))
}

/// `open_child_streams` の戻り値（stdout の reader・`stdout_log` の file・stderr の写しの task）。
pub(crate) type ChildStreams = (
    tokio::io::BufReader<tokio::process::ChildStdout>,
    tokio::fs::File,
    JoinHandle<()>,
);
