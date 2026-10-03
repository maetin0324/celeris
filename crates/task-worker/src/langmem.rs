//! `langmem` アダプタ（ADR-0047 D4。Phase 62）— 知識整理 run（`knowledge` harness）を起こす。
//!
//! `paperqa`/`local-deep-research` と同じ「調査・抽出エンジンを包む」形。LangMem
//! （`langmem.create_memory_manager`）は celeris のワーカープロトコルもストリーム型の進捗形式も話さない
//! ただの python ライブラリなので、**アダプタ自身が** ADR-0006 D3 の結果ファイル規約
//! （`artifacts/result.json`）を代わりに書き、`Terminal::Done`/`Terminal::Error` を合成する。
//! 委譲（`delegate.json`）は扱わない（知識整理 run は裏方の支援タスクで、子タスクを作らない）。
//!
//! run ごとに `runs/<run_id>/langmem_run.py` として埋め込みの python ランナーを書き出し、
//! `<command> <その場所> <run_dir>/langmem_input.json` で起動する（`local-deep-research` と同じ配線）。
//! **LLM はこの python プロセスの中だけ**で起きる（CLAUDE.md「ディスパッチャやストアに LLM 呼び出しを
//! 入れない」。dispatch の判断もトリガの決定も celeris 側は決定的なまま）。
//!
//! 入力は `req.task.objective` そのもの — `crates/celeris/src/knowledge_maint.rs` が
//! `task_core::knowledge::maintenance_objective` で、タスクの報告・結果・コメント・関連する既存の
//! KB ページ・担当ノードの手帳・既存の索引の題名を**あらかじめ** 1 本のテキストへ組んである
//! （`local-deep-research` の `build_query` と同じ考え方: アダプタ・ランナーは余計なファイルを
//! 読み書きしない）。出力は `artifacts/knowledge-candidates.json` = `{candidates: [...]}`
//! （`task_core::knowledge::Candidate` の配列。検査・適用は `task_ops::knowledge::apply_candidates`）。

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::progress;
use crate::protocol::RunRequest;
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, kill_now, read_line_limited, read_tail, reap_after_terminal,
    write_result_json,
};

/// run ごとに `runs/<run_id>/langmem_run.py` として書き出すランナー（ADR-0047 D4）。
const RUNNER_SCRIPT: &str = include_str!("langmem_run.py");

/// `artifacts/result.json` の `summary` の上限（他の python アダプタと同じ規則）。
const SUMMARY_MAX_CHARS: usize = 1500;
/// `progress:` 行を `progress` に転送するときの 1 行あたりの上限。
const PROGRESS_LINE_MAX_CHARS: usize = 500;
/// ランナーの最終行の目印。
const RESULT_PREFIX: &str = "CELERIS_RESULT ";
/// `progress:` 行の目印。
const PROGRESS_PREFIX: &str = "progress:";
/// `langmem` が import できないときにランナーが stderr に出す目印（ADR-0047 D4「clear error」）。
/// これが出ていれば `retryable = false`（venv のセットアップが要る。再試行しても直らない）。
pub const LANGMEM_MISSING_MARKER: &str = "CELERIS_LANGMEM_MISSING";

/// `langmem_run.py` の `EXTRACTION_INSTRUCTIONS` の始まりと終わり（ADR-0052 D2: フォールバックの
/// 前置きは**同じ文面**を使う。2 か所に写して食い違わせない）。
const EXTRACTION_BEGIN: &str = "EXTRACTION_INSTRUCTIONS = \"\"\"\\\n";
const EXTRACTION_END: &str = "\n\"\"\"\n";

/// ADR-0047 D4 の抽出の指示（`langmem_run.py` が LangMem に渡しているのと**同じ文面**）。
///
/// 出典は python のランナー 1 つだけ（`include_str!` した [`RUNNER_SCRIPT`] から切り出す）。
/// 切り出せなければ空を返す（前置きは出力契約だけになる。`unwrap` はしない）。
pub fn extraction_instructions() -> &'static str {
    let Some((_, rest)) = RUNNER_SCRIPT.split_once(EXTRACTION_BEGIN) else {
        return "";
    };
    rest.split_once(EXTRACTION_END)
        .map(|(body, _)| body)
        .unwrap_or("")
}

/// ADR-0052 D2: フォールバック run（tier `cheap` の汎用ハーネス）に渡す前置き。
///
/// 中身は「[`extraction_instructions`]（= LangMem に渡しているのと同じ指示）＋ 出力契約」。
/// 依頼文（`task_core::knowledge::maintenance_objective` が組んだ `maintenance_objective`）は
/// タスクの `objective` としてそのまま渡るので、ここでは繰り返さない。
///
/// 決定的（LLM も I/O も無い）。`candidates_rel` はワーカーから見た候補ファイルの位置
/// （通常 `artifacts/knowledge-candidates.json`）。
pub fn knowledge_fallback_instructions(candidates_rel: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "この run は**知識整理**（ADR-0047 D4）です。設定された `langmem` の接続先に届かなかったので、\
         あなたのハーネスで同じ抽出をします（ADR-0052 D2）。下の依頼文には、終わった仕事 1 件の\
         題名・目的・報告・コメント・関連する既存の知識ベースのページ・担当の手帳・既存の索引の題名が\
         すでに全部入っています。\n\n### 抽出の規則 (extraction rules)\n",
    );
    out.push_str(extraction_instructions());
    out.push_str("\n### 出力の契約 (output contract)\n");
    out.push_str(&format!(
        "- `{candidates_rel}` に **JSON を 1 つだけ**書く: \
         `{{\"candidates\": [{{\"op\", \"path\", \"title\", \"tags\", \"scope\", \"body\", \"sources\", \"confidence\"}}]}}`。\n"
    ));
    out.push_str(
        "- 残す価値のあるものが 1 つも無ければ `{\"candidates\": []}`（空の配列）を書く。無理に作らない。\n\
         - **他のファイルは作らない**（知識ベースに直接書かない。検査と適用は celeris が決定的に行う）。\n\
         - **道具は使わない**。読む必要のあるものは全部この前置きと下の依頼文にある（検索も取得も要らない）。\n\
         - 終わったら結果ファイルの `summary` に、候補を何件書いたかを 1 行で書く。\n",
    );
    out
}

/// `[knowledge.langmem].provider`。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LangMemProvider {
    #[default]
    OpenaiCompatible,
    Anthropic,
}

impl LangMemProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            LangMemProvider::OpenaiCompatible => "openai-compatible",
            LangMemProvider::Anthropic => "anthropic",
        }
    }
}

/// `[adapters.langmem]`（config.toml）＋ `[knowledge.langmem]` の LLM 接続先をまとめた設定
/// （ADR-0047 D4）。celeris 側（`crates/celeris/src/lib.rs::build_adapters`）が 2 つの節を合わせて作る。
#[derive(Debug, Clone)]
pub struct LangMemConfig {
    /// 起動する python（`tools/langmem/.venv/bin/python` のような venv の python）。
    pub command: String,
    /// 無出力タイムアウト（`RunLimits.idle_timeout` を上回らない範囲で使う。既定はハーネスの予算のまま）。
    pub idle_timeout_secs: Option<u64>,
    pub provider: LangMemProvider,
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// `[knowledge.langmem].api_key_secret` を celeris が解決した値（無ければ渡さない）。
    pub api_key: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Default for LangMemConfig {
    fn default() -> Self {
        Self {
            command: "python3".to_string(),
            idle_timeout_secs: None,
            provider: LangMemProvider::OpenaiCompatible,
            base_url: None,
            model: None,
            api_key: None,
            env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LangMemAdapter {
    config: LangMemConfig,
}

impl LangMemAdapter {
    pub const ID: &'static str = "langmem";

    pub fn new(config: LangMemConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for LangMemAdapter {
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
        run_langmem(&self.config, &req, run_id, &limits, sink).await
    }

    /// 他のアダプタと同じ規則: `extra` は `config.env` の末尾に足す（同名キーは `extra` が勝つ）。
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(LangMemAdapter::new(config)))
    }
}

/// ADR-0139 D4: ランナーの起動 env に入れる鍵（`provider` に応じた 1 つの変数）。鍵が無ければ空。
pub(crate) fn api_key_env(config: &LangMemConfig) -> Vec<(String, String)> {
    let Some(key) = config.api_key.as_ref().filter(|k| !k.is_empty()) else {
        return Vec::new();
    };
    let var = match config.provider {
        LangMemProvider::OpenaiCompatible => "OPENAI_API_KEY",
        LangMemProvider::Anthropic => "ANTHROPIC_API_KEY",
    };
    vec![(var.to_string(), key.clone())]
}

async fn run_langmem(
    config: &LangMemConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;
    let stdout_log_path = run_dir.join("stdout.log");
    let stderr_log_path = run_dir.join("stderr.log");
    let stderr_log_path_for_task = stderr_log_path.clone();

    // ADR-0036 D1/D2: 成果物の置き場はディスパッチャが決めた `artifacts_dir`（共有 workspace ではタスクごと）。
    let artifacts_dir = req.artifacts_dir.clone();
    let artifacts_rel = req.artifacts_rel();
    tokio::fs::create_dir_all(&artifacts_dir).await?;
    let candidates_path = artifacts_dir.join("knowledge-candidates.json");
    // 前回の run（リトライ）の名残を今回の結果と誤読しない（他アダプタと同じ理由。ADR-0006 D3）。
    let _ = tokio::fs::remove_file(&candidates_path).await;
    let _ = tokio::fs::remove_file(artifacts_dir.join("result.json")).await;

    // ADR-0047 D4: 依頼文（`objective`）は `crates/celeris/src/knowledge_maint.rs` が
    // `task_core::knowledge::maintenance_objective` で組み立て済み。アダプタは前置きを足さず
    // そのまま渡す（`local-deep-research` の `build_query` と同じ考え方）。
    let objective = req.task.objective.clone();
    // ADR-0023 D2 / M1: この run で何を渡したかを残す。
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &objective, run_id).await;

    let input = serde_json::json!({
        "task_id": req.task.id.to_string(),
        "objective": objective,
        "llm": {
            "provider": config.provider.as_str(),
            "base_url": config.base_url,
            "model": config.model,
            "api_key": config.api_key,
        },
        "candidates_path": candidates_path.to_string_lossy(),
    });
    let script_path = run_dir.join("langmem_run.py");
    let input_path = run_dir.join("langmem_input.json");
    tokio::fs::write(&script_path, RUNNER_SCRIPT).await?;
    let input_text = serde_json::to_string_pretty(&input)?;
    tokio::fs::write(&input_path, format!("{input_text}\n")).await?;

    let mut command = Command::new(&config.command);
    command
        .arg(&script_path)
        .arg(&input_path)
        .envs(config.env.iter().cloned())
        // ADR-0139 D4: 鍵は env にも入れる。provider 行・`[adapters.langmem].env` の古い値より後に
        // 置くので、それらが勝たない。
        .envs(api_key_env(config))
        .current_dir(req.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    // ADR-0095 D2: 本番 DB を読み取り専用にした namespace で起動する（コンテナ非対応の adapter）。
    let mut command = crate::db_guard::launch(command, None);

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

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
    // ADR-0047 D4: `[adapters.langmem].idle_timeout_secs` はハーネスの予算（`limits.idle_timeout`）を
    // 上回らない範囲でだけ効く（config が長い秒数を書いても予算を突破できない）。
    let idle_timeout = config
        .idle_timeout_secs
        .map(Duration::from_secs)
        .map(|d| d.min(limits.idle_timeout))
        .unwrap_or(limits.idle_timeout);
    let mut last_activity = Instant::now();
    let mut stdout_buf = String::new();
    let mut task_result: Option<serde_json::Value> = None;
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;

    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            timeout_terminal = Some(Terminal::Error {
                message: "wall clock exceeded".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= idle_timeout {
            timeout_terminal = Some(Terminal::Error {
                message: "idle timeout".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let wait = (limits.wall_clock - wall_elapsed).min(idle_timeout - idle_elapsed);

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
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from the langmem runner");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim();
                stdout_buf.push_str(&text);
                stdout_buf.push('\n');
                if let Some(rest) = trimmed.strip_prefix(PROGRESS_PREFIX) {
                    progress::emit_status(
                        sink,
                        &truncate_chars(rest.trim(), PROGRESS_LINE_MAX_CHARS),
                    );
                } else if let Some(rest) = trimmed.strip_prefix(RESULT_PREFIX) {
                    match serde_json::from_str::<serde_json::Value>(rest) {
                        Ok(value) => task_result = Some(value),
                        Err(e) => warn!("run {run_id}: could not parse CELERIS_RESULT line: {e}"),
                    }
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

    if let Some(terminal) = timeout_terminal {
        // タイムアウトは供給側失敗として分類しない（他アダプタと同じ。ADR-0010 D5）。
        write_result_json(&run_dir, &terminal, None).await?;
        return Ok(RunOutcome {
            terminal,
            exit_code: exit_status.code(),
        });
    }

    let stderr_tail = read_tail(&stderr_log_path, 4096).await;
    let classify_text = format!("{stdout_buf}\n{stderr_tail}");

    // ADR-0047 D4: `langmem` が import できないのは venv のセットアップが要る設定の誤りで、
    // 再試行しても直らない（`retryable = false`）。
    if classify_text.contains(LANGMEM_MISSING_MARKER) {
        let terminal = Terminal::Error {
            message: format!(
                "langmem is not importable (set up the venv: scripts/knowledge/setup-langmem.sh): {}",
                stderr_tail.lines().next_back().unwrap_or("").trim()
            ),
            retryable: false,
        };
        write_result_json(&run_dir, &terminal, None).await?;
        return Ok(RunOutcome {
            terminal,
            exit_code: exit_status.code(),
        });
    }

    let candidates_len = tokio::fs::read_to_string(&candidates_path)
        .await
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| {
            v.get("candidates")
                .and_then(|c| c.as_array().map(|a| a.len()))
        });

    let (terminal, provider_failure) = if !exit_status.success() {
        let exit_repr = match exit_status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: format!("langmem runner exited with a non-zero status (exit={exit_repr})"),
                retryable: true,
            },
            pf,
        )
    } else if task_result.is_none() {
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: "langmem runner did not print a CELERIS_RESULT line".to_string(),
                retryable: true,
            },
            pf,
        )
    } else if candidates_len.is_none() {
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: "langmem runner did not write artifacts/knowledge-candidates.json"
                    .to_string(),
                retryable: true,
            },
            pf,
        )
    } else {
        let value = task_result.unwrap_or(serde_json::Value::Null);
        let raw_summary = value
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let count = candidates_len.unwrap_or(0);
        let summary = if raw_summary.trim().is_empty() {
            format!("知識の候補 {count} 件を抽出した")
        } else {
            single_line_summary(raw_summary, SUMMARY_MAX_CHARS)
        };
        // ADR-0047 D4: 成果物として申告する（celeris が終端の run 一覧・`Check::ArtifactExists` に使う）。
        match crate::artifact::resolve(
            &req.workspace,
            "knowledge-candidates.json",
            &format!("{artifacts_rel}/knowledge-candidates.json"),
            Some("json"),
        ) {
            Ok(artifact) => sink.artifact(&artifact),
            Err(e) => warn!("run {run_id}: could not register knowledge-candidates.json: {e}"),
        }
        let result_file = serde_json::json!({ "summary": summary, "evidence": [] });
        match serde_json::to_string_pretty(&result_file) {
            Ok(text) => {
                if let Err(e) =
                    tokio::fs::write(artifacts_dir.join("result.json"), format!("{text}\n")).await
                {
                    warn!("run {run_id}: could not write artifacts/result.json: {e}");
                }
            }
            Err(e) => warn!("run {run_id}: could not serialize artifacts/result.json: {e}"),
        }
        (
            Terminal::Done {
                summary,
                evidence: Vec::new(),
                usage: None,
            },
            None,
        )
    };

    write_result_json(&run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

fn single_line_summary(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, max_chars)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests;
