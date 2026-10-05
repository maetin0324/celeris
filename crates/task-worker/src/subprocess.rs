//! サブプロセス + JSON Lines の実行器（ADR-0003 D1/D3/D4/D5）。

use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal};
use crate::protocol::{ProviderFailure, RunRequest, WorkerMessage};

/// 1 行の上限（ADR-0003 D1）。
pub(crate) const MAX_LINE_BYTES: usize = 1024 * 1024;

/// 起動するコマンド。`program` と `args` は設定からそのまま渡す。cwd は `req.workspace`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubprocessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// ADR-0043 D3（Phase 56）: `Some` ならこのコマンドをコンテナの中で起こす（`container::wrap`）。
    pub container: Option<crate::container::SharedPlan>,
}

/// ADR-0023 D2: ワーカーに渡した指示そのものを `runs/<run_id>/` に残す（後から「何を言われて何をしたか」を追える）。
/// **全アダプタから呼ぶ**（fake は `RunRequest` をそのまま stdin に渡し、claude-code / codex は
/// そこから組み立てたプロンプトを渡すので、後者は `prompt.txt` も書く。ADR-0023 M1）。
/// 秘密は入らない（`RunRequest` に `env` の値やトークンは含まれない。ADR-0013 D11）。書けなくても run は続ける。
pub(crate) async fn write_run_request(run_dir: &Path, req: &RunRequest, run_id: &str) {
    // The opaque routing reference is a bearer capability. Keep it out of persisted
    // request snapshots; context-transport.json records only whether it was delivered.
    let mut snapshot = req.clone();
    snapshot.context.routing_context_ref = None;
    match serde_json::to_string_pretty(&snapshot) {
        Ok(pretty) => {
            if let Err(e) =
                tokio::fs::write(run_dir.join("request.json"), format!("{pretty}\n")).await
            {
                tracing::warn!(%run_id, error = %e, "could not write runs/<run_id>/request.json");
            }
        }
        Err(e) => tracing::warn!(%run_id, error = %e, "could not serialize the run request"),
    }
}

/// ADR-0023 M1: claude-code / codex が実際に渡した文面（`build_prompt` の結果）を残す。
pub(crate) async fn write_run_prompt(run_dir: &Path, prompt: &str, run_id: &str) {
    if let Err(e) = tokio::fs::write(run_dir.join("prompt.txt"), prompt).await {
        tracing::warn!(%run_id, error = %e, "could not write runs/<run_id>/prompt.txt");
    }
}

/// F5-fix10（本番障害: task 01M3MS2JRDJ4GM0D9VN9PJCB6B の planner run 01M3Q21Z9JQWWANGHXJPNH1F8X）:
/// Linux の `MAX_ARG_STRLEN`（1 つの argv 要素・環境変数文字列あたり 32 ページ = 131072 バイト、終端 NUL を
/// 含む）。これ以上の引数は `execve` が `E2BIG`（"Argument list too long (os error 7)"）で拒否する。
/// 135,644 バイトの replan プロンプトを `claude -p <prompt>` で渡してこれを踏んだ。
pub(crate) const MAX_SINGLE_ARG_BYTES: usize = 128 * 1024;

/// F5-fix10: spawn の前に、`command` のどの引数・明示した環境変数も `MAX_SINGLE_ARG_BYTES` 未満であることを
/// 確かめる。超えていれば、アダプタ名・何番目の引数か・大きさを名指しした読める `AdapterError` を返す
/// （将来の回帰が `os error 7` でなくこの文面で落ちるように）。プロンプトは stdin かファイルで渡すこと。
/// コンテナ実行では `container::wrap` の**後**に呼ぶ（`--env K=V` も 1 引数になるため）。
pub(crate) fn check_arg_lengths(adapter: &str, command: &Command) -> Result<(), AdapterError> {
    let std_command = command.as_std();
    for (index, arg) in std_command.get_args().enumerate() {
        let len = arg.len();
        if len >= MAX_SINGLE_ARG_BYTES {
            return Err(AdapterError::Other(format!(
                "{adapter}: argument #{index} is {len} bytes, at or above the \
                 {MAX_SINGLE_ARG_BYTES}-byte per-argument limit (Linux MAX_ARG_STRLEN); long \
                 prompts must be passed via stdin or a file, not argv (F5-fix10)"
            )));
        }
    }
    for (key, value) in std_command.get_envs() {
        let len = key.len() + 1 + value.map_or(0, std::ffi::OsStr::len);
        if len >= MAX_SINGLE_ARG_BYTES {
            return Err(AdapterError::Other(format!(
                "{adapter}: environment variable {} is {len} bytes, at or above the \
                 {MAX_SINGLE_ARG_BYTES}-byte per-string limit (Linux MAX_ARG_STRLEN) (F5-fix10)",
                key.to_string_lossy()
            )));
        }
    }
    Ok(())
}

/// F5-fix10: spawn 済みの `child`（stdin を `Stdio::piped()` で起こしたもの）の stdin に `input` を書き、
/// 書き終えたら閉じる（EOF。CLI がそれ以上の入力を待たないように）。書き込みは別タスクで行うので、
/// 呼び出し側はすぐ stdout の読み取りに入ってよい（数百 KB の入力でも、子が stdout を書き詰まって stdin を
/// 読まなくなるデッドロックにならない）。子が先に終わった・読まずに閉じた場合の `BrokenPipe` は警告に留める
/// （run の成否は stdout と終了コードで決まる）。返した `JoinHandle` は子を刈り取った後に `abort` してよい。
pub(crate) fn feed_stdin(
    child: &mut Child,
    input: String,
    adapter: &'static str,
    run_id: &str,
) -> Result<tokio::task::JoinHandle<()>, AdapterError> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AdapterError::Other(format!("{adapter}: worker stdin was not piped")))?;
    let run_id = run_id.to_string();
    Ok(tokio::spawn(async move {
        let result = async {
            stdin.write_all(input.as_bytes()).await?;
            stdin.flush().await?;
            stdin.shutdown().await
        }
        .await;
        if let Err(e) = result {
            warn!(%run_id, error = %e, "{adapter}: failed to write the prompt to the worker's stdin");
        }
        // `stdin` はここで drop され、パイプの書き込み側が閉じる（子は EOF を読む）。
    }))
}

pub async fn run_subprocess(
    spec: &SubprocessSpec,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;
    let stdout_log_path = run_dir.join("stdout.jsonl");
    let stderr_log_path = run_dir.join("stderr.log");
    let result_log_path = run_dir.join("result.json");
    write_run_request(&run_dir, req, run_id).await;

    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .envs(spec.env.iter().cloned())
        .current_dir(req.cwd());
    // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
    let mut command = crate::db_guard::launch(command, spec.container.as_deref());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

    let payload = serde_json::to_string(req)?;
    if let Some(mut stdin) = child.stdin.take() {
        // 子が先に stdin を閉じて死んだ場合の書き込みエラーは無視してよい（仕様どおり）。
        let _ = stdin.write_all(payload.as_bytes()).await;
        let _ = stdin.write_all(b"\n").await;
        drop(stdin);
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
        match tokio::fs::File::create(&stderr_log_path).await {
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
    let mut terminal: Option<Terminal> = None;
    let mut terminal_raw: Option<String> = None;
    // `error.provider_failure`（ADR-0010 D5）。終端が `error` でこれが付いていれば、result.json を
    // 書いた後に `AdapterError` として返す（ディスパッチャに requeue 判断をさせるため）。
    let mut provider_failure: Option<ProviderFailure> = None;
    // true なら終端後すぐに SIGTERM→SIGKILL（タイムアウト・プロトコル違反）。
    // false なら「終端メッセージを受け取った後」の穏やかな刈り取り（仕様 6）。
    let mut force_kill = false;

    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            terminal = Some(Terminal::Error {
                message: "wall clock exceeded".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= limits.idle_timeout {
            terminal = Some(Terminal::Error {
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
                // ADR-0010 D7: 1 行読むたび（読めなかった超過行も含む）に生存通知する。
                sink.heartbeat();
                terminal = Some(Terminal::Error {
                    message: "protocol violation: stdout line exceeds 1 MiB".into(),
                    retryable: false,
                });
                force_kill = true;
                break;
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;

                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    warn!("run {run_id}: discarding blank line from worker stdout");
                    continue;
                }
                match serde_json::from_str::<WorkerMessage>(trimmed) {
                    Ok(msg) => match msg {
                        // ADR-0048 D2（Phase 60a）: 構造化フィールドがあればそのまま渡す。
                        // 無ければ従来どおり `progress`（この口を実装していないシンクも動く）。
                        WorkerMessage::Progress {
                            msg,
                            kind,
                            tool,
                            summary,
                            detail,
                            truncated,
                            error,
                        } => {
                            let fields = task_core::ProgressFields {
                                kind,
                                tool,
                                summary,
                                detail,
                                truncated,
                                error,
                            };
                            if fields.is_plain() {
                                sink.progress(&msg);
                            } else {
                                sink.progress_with(&msg, &fields);
                            }
                        }
                        // ADR-0044 D2（Phase 53）: 残る記録。run は止まらない。
                        WorkerMessage::Comment { body } => sink.comment(&body),
                        WorkerMessage::Delegate { tasks } => sink.delegate(&tasks),
                        WorkerMessage::Artifact { name, path, kind } => {
                            match crate::artifact::resolve(
                                &req.workspace,
                                &name,
                                &path,
                                kind.as_deref(),
                            ) {
                                Ok(aref) => sink.artifact(&aref),
                                Err(e) => {
                                    warn!("run {run_id}: discarding invalid artifact {name:?}: {e}")
                                }
                            }
                        }
                        WorkerMessage::Question { text } => {
                            terminal_raw = Some(trimmed.to_string());
                            terminal = Some(Terminal::Question { text });
                        }
                        WorkerMessage::Done {
                            summary,
                            evidence,
                            usage,
                        } => {
                            terminal_raw = Some(trimmed.to_string());
                            terminal = Some(Terminal::Done {
                                summary,
                                evidence,
                                usage,
                            });
                        }
                        WorkerMessage::Error {
                            message,
                            retryable,
                            provider_failure: pf,
                        } => {
                            terminal_raw = Some(trimmed.to_string());
                            provider_failure = pf;
                            terminal = Some(Terminal::Error { message, retryable });
                        }
                        // ADR-0072 D7/D9（Phase E1）: 外部ハーネスが直接プロトコルで yield /
                        // 予算切れを申告する経路（claude-code / codex / acp / aider は自前で終端を
                        // 合成するのでこの経路は通らない）。
                        WorkerMessage::Yielded { checkpoint, usage } => {
                            terminal_raw = Some(trimmed.to_string());
                            terminal = Some(Terminal::Yielded { checkpoint, usage });
                        }
                        WorkerMessage::BudgetExhausted {
                            kind,
                            message,
                            usage,
                        } => {
                            terminal_raw = Some(trimmed.to_string());
                            terminal = Some(Terminal::BudgetExhausted {
                                kind,
                                message,
                                usage,
                            });
                        }
                        // ADR-0090 D1: 外部ハーネスが直接プロトコルでクラスタ job の wait を申告する経路。
                        // `result.json` と同じ検証（`parse_wait_request`）を通す。
                        WorkerMessage::Wait { usage, .. } => {
                            terminal_raw = Some(trimmed.to_string());
                            terminal = serde_json::from_str::<serde_json::Value>(trimmed)
                                .ok()
                                .and_then(|v| crate::adapter::wait_terminal(&v, usage));
                        }
                    },
                    Err(parse_err) => {
                        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                            // JSON としては妥当だが WorkerMessage に解析できない: プロトコル違反。
                            terminal = Some(Terminal::Error {
                                message: format!("protocol violation: {parse_err}"),
                                retryable: false,
                            });
                            force_kill = true;
                        } else {
                            warn!(
                                "run {run_id}: discarding non-json line from worker stdout: {trimmed}"
                            );
                        }
                    }
                }
                if terminal.is_some() {
                    break;
                }
            }
        }
    }

    let exit_status = if force_kill {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };
    // ADR-0043 D3: `--rm` と signal の転送でふつうは消えるが、クライアントだけを殺した場合に
    // 取り残さないようラベルでも消す（ADR-0044 B2 のプロセスグループ kill からも同じ口を呼べる）。
    if let Some(plan) = &spec.container {
        let plan = Arc::clone(plan);
        let _ = tokio::task::spawn_blocking(move || {
            crate::container::ContainerStopper::stop_blocking(&*plan)
        })
        .await;
    }

    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;

    if let Some(raw) = &terminal_raw {
        tokio::fs::write(&result_log_path, format!("{raw}\n")).await?;
    }

    // ADR-0010 D5: 供給側失敗は result.json を書いた後（上記）に `AdapterError` として返す。
    // 遷移の判断はここでは行わない（ディスパッチャの責務）。
    if let (Some(Terminal::Error { message, .. }), Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }

    let terminal = terminal.unwrap_or_else(|| {
        let exit_repr = match exit_status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        Terminal::Error {
            message: format!("worker exited without terminal message (exit={exit_repr})"),
            retryable: true,
        }
    });

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

/// `Terminal` を `WorkerMessage` に正規化して `runs/<run_id>/result.json` に 1 行 JSON として書く
/// （ADR-0010 D10 P-26）。`run_subprocess` はワーカーから受信した生の行をそのまま書くので使わないが、
/// `claude-code`/`codex` は自前で終端を合成するのでこの共通ヘルパを使う。
pub(crate) async fn write_result_json(
    run_dir: &Path,
    terminal: &Terminal,
    provider_failure: Option<ProviderFailure>,
) -> std::io::Result<()> {
    let msg = match terminal {
        Terminal::Done {
            summary,
            evidence,
            usage,
        } => WorkerMessage::Done {
            summary: summary.clone(),
            evidence: evidence.clone(),
            usage: *usage,
        },
        Terminal::Question { text } => WorkerMessage::Question { text: text.clone() },
        Terminal::Error { message, retryable } => WorkerMessage::Error {
            message: message.clone(),
            retryable: *retryable,
            provider_failure,
        },
        Terminal::Yielded { checkpoint, usage } => WorkerMessage::Yielded {
            checkpoint: checkpoint.clone(),
            usage: *usage,
        },
        Terminal::BudgetExhausted {
            kind,
            message,
            usage,
        } => WorkerMessage::BudgetExhausted {
            kind: *kind,
            message: message.clone(),
            usage: *usage,
        },
        Terminal::Waiting {
            request,
            checkpoint,
            usage,
        } => WorkerMessage::Wait {
            kind: task_core::cluster_job::WAIT_KIND_CLUSTER_JOB.to_string(),
            cluster: request.cluster.clone(),
            scheduler: Some(request.scheduler),
            jobs: request.jobs.clone(),
            poll_secs: request.poll_secs,
            timeout_secs: request.timeout_secs,
            checkpoint: checkpoint.clone(),
            summary: request.summary.clone(),
            usage: *usage,
        },
    };
    let text = serde_json::to_string(&msg).map_err(std::io::Error::other)?;
    tokio::fs::write(run_dir.join("result.json"), format!("{text}\n")).await
}

/// ADR-0006 Phase 115 D2（本番障害 01M3915FARENW8M0JM11XVF6W0 / 01M38T8N17MEWPTJQXGX1TNYJD）:
/// `work_dir != workspace`（部署のリポジトリの git worktree で走るタスク）の run が、正しい置き場
/// `<artifacts_dir>/result.json` の代わりに cwd 相対の `artifacts/result.json`（= `<work_dir>/artifacts/
/// result.json`）に書いてしまっていたら、それを正しい置き場へ移して採用する。worktree の中には残さない
/// （移動後に空になった `artifacts/` も消す）。呼び出し元は結果ファイルを読む**前**（`claude-code`/`codex`
/// 両アダプタとも、Phase 112 D3 の「最終メッセージから回収」より前）に呼ぶこと。
///
/// 何もしないケース: `<artifacts_dir>/result.json` が既にある／`work_dir` が無い・`workspace` と同じ／
/// `<work_dir>/artifacts/result.json` も無い。
pub(crate) async fn adopt_result_json_written_under_work_dir(
    artifacts_dir: &Path,
    work_dir: Option<&Path>,
    workspace: &Path,
    run_id: &str,
) {
    if tokio::fs::metadata(artifacts_dir.join("result.json"))
        .await
        .is_ok()
    {
        return;
    }
    let Some(work_dir) = work_dir else { return };
    if work_dir == workspace {
        return;
    }
    let stray_dir = work_dir.join("artifacts");
    let stray = stray_dir.join("result.json");
    if tokio::fs::metadata(&stray).await.is_err() {
        return;
    }
    let target = artifacts_dir.join("result.json");
    if let Err(e) = tokio::fs::create_dir_all(artifacts_dir).await {
        warn!(
            "run {run_id}: could not create {} to adopt a stray result.json: {e}",
            artifacts_dir.display()
        );
        return;
    }
    if let Err(rename_err) = tokio::fs::rename(&stray, &target).await {
        // 別ファイルシステム等で rename できない場合はコピーしてから消す。
        if let Err(copy_err) = tokio::fs::copy(&stray, &target).await {
            warn!(
                "run {run_id}: found {} but could not move it to {} (rename: {rename_err}, copy: {copy_err})",
                stray.display(),
                target.display()
            );
            return;
        }
        let _ = tokio::fs::remove_file(&stray).await;
    }
    warn!(
        "run {run_id}: result.json was written under work_dir ({}); moved to {}",
        stray.display(),
        target.display()
    );
    // worktree の中に残さない。移動後に空になっていれば `artifacts/` も消す（空でなければ何もしない）。
    let _ = tokio::fs::remove_dir(&stray_dir).await;
}

/// ファイル末尾 `max` バイトを文字列として読む（供給側失敗の分類用。ADR-0010 D5）。
/// ファイルが無い・読めない場合は空文字列を返す（分類対象が無ければ `None` になるだけで、run の
/// 結果全体には影響させない）。
pub(crate) async fn read_tail(path: &Path, max: usize) -> String {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let start = bytes.len().saturating_sub(max);
            String::from_utf8_lossy(&bytes[start..]).into_owned()
        }
        Err(_) => String::new(),
    }
}

pub(crate) enum LineOutcome {
    Eof,
    Line(Vec<u8>),
    TooLong,
}

/// `max` バイトを超える行は `TooLong` を返す（末尾の `\n` は含まない基準）。
pub(crate) async fn read_line_limited<R>(reader: &mut R, max: usize) -> std::io::Result<LineOutcome>
where
    R: AsyncBufRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    let n = reader
        .take(max as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Ok(LineOutcome::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        return Ok(LineOutcome::Line(buf));
    }
    if buf.len() as u64 > max as u64 {
        return Ok(LineOutcome::TooLong);
    }
    // 上限内で改行の無いまま EOF に達した最終行。
    Ok(LineOutcome::Line(buf))
}

/// プロセスグループへ signal を送る。既に居なければ (`ESRCH`) 無視する。
pub(crate) fn send_signal_to_group(child: &Child, sig: Signal) {
    if let Some(pid) = child.id() {
        let pgid = Pid::from_raw(pid as i32);
        if let Err(e) = signal::killpg(pgid, sig)
            && e != Errno::ESRCH
        {
            warn!("failed to send {sig:?} to worker process group {pid}: {e}");
        }
    }
}

/// SIGTERM を直ちに送り、`grace` 待って生きていれば SIGKILL する（ADR-0003 D4）。
pub(crate) async fn kill_now(child: &mut Child, grace: Duration) -> std::io::Result<ExitStatus> {
    send_signal_to_group(child, Signal::SIGTERM);
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(status) => status,
        Err(_elapsed) => {
            send_signal_to_group(child, Signal::SIGKILL);
            child.wait().await
        }
    }
}

/// 終端メッセージ受信後の後始末: 最大 `grace` 待ち、まだ生きていれば kill する（仕様 6）。
///
/// Phase 119 D2: リーダーが**自分から**終わったとき（下の `Ok(status)` の枝。タイムアウトで
/// `kill_now` に落ちる方は `send_signal_to_group` が既にグループ全体へ SIGTERM/SIGKILL を送っている
/// ので対象外）も、そのプロセスグループに孤児（ワーカーが `&` で起こしたバックグラウンドジョブ）が
/// 残っていないか掃除する。`child.id()` は `child.wait()` を呼ぶ**前**に取る（`await` を挟まず、
/// 最も新鮮な pid で `process_group::sweep` を呼ぶため — 詳細はそのモジュール先頭のコメント）。
pub(crate) async fn reap_after_terminal(
    child: &mut Child,
    grace: Duration,
) -> std::io::Result<ExitStatus> {
    let pid = child.id();
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(status) => {
            if let Some(pid) = pid {
                crate::process_group::sweep(pid, grace);
            }
            status
        }
        Err(_elapsed) => kill_now(child, grace).await,
    }
}

#[cfg(test)]
mod tests;
