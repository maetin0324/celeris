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
//! 日次入力は `celeris::knowledge_curation::extraction` が複数 task の報告を JSON にまとめる。
//! 単体の再試行の入力は `req.task.objective` そのもの — `crates/celeris/src/knowledge_maint.rs` が
//! `task_core::knowledge::maintenance_objective` で、タスクの報告・結果・コメント・関連する既存の
//! KB ページ・担当ノードの手帳・既存の索引の題名を**あらかじめ** 1 本のテキストへ組んである
//! （`local-deep-research` の `build_query` と同じ考え方: アダプタ・ランナーは余計なファイルを
//! 読み書きしない）。出力は `artifacts/knowledge-candidates.json` = `{candidates: [...]}`
//! （`task_core::knowledge::Candidate` の配列。検査・適用は `task_ops::knowledge::apply_candidates`）。

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::adapter::{
    AdapterError, AdapterSetting, AdapterSettings, Configured, EventSink, RunLimits, RunOutcome,
    Terminal, WorkerAdapter,
};
use crate::progress;
use crate::protocol::RunRequest;
use crate::provider::classify_provider_failure;
use crate::research_runner::{
    runner_command, single_line_summary, stream_child, truncate_chars, write_runner_files,
};
use crate::result_report::{ResultFileRead, interpret_result_text, read_result_file};
use crate::subprocess::finish_run;

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
/// （ADR-0047 D4）。celeris 側（`crates/celeris/src/daemon/adapters.rs::build_harness_adapters`）が 2 つの節を合わせて作る。
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

    /// ADR 2026-11-15-research-adapter-deadline D2: 入力は `objective` だけで、前置き（continuation の節）を足さない。予算切れは最初からのやり直し（retry）にする。
    fn continues_from_checkpoint(&self) -> bool {
        false
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

    /// ADR 2026-11-10-worker-capability-ports D3: model と env（同名キーは後勝ち）を受ける。コンテナは受けない。
    fn configure(&self, settings: &AdapterSettings) -> Configured {
        settings.configured(&[AdapterSetting::Model, AdapterSetting::Env], || {
            let mut config = self.config.clone();
            settings.apply_model(&mut config.model);
            settings.apply_env(&mut config.env);
            Arc::new(Self::new(config))
        })
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
    crate::campaign_pin::enforce(&run_dir, req).await?;
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
    let (script_path, input_path) = write_runner_files(
        &run_dir,
        "langmem_run.py",
        RUNNER_SCRIPT,
        "langmem_input.json",
        &input,
    )
    .await?;
    // ADR-0139 D4: 鍵は env にも入れる。provider 行・`[adapters.langmem].env` の古い値より後に
    // 置くので、それらが勝たない。
    let env = config.env.iter().cloned().chain(api_key_env(config));
    let mut command = runner_command(&config.command, &script_path, &input_path, env, req.cwd());

    let child = crate::subprocess::spawn_after_notice(sink, &mut command)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

    // ADR-0047 D4: `[adapters.langmem].idle_timeout_secs` はハーネスの予算（`limits.idle_timeout`）を
    // 上回らない範囲でだけ効く（config が長い秒数を書いても予算を突破できない）。締め切りの規則は
    // `deadline_terminal` が持ち、この値を `idle_timeout` として受ける（ADR 2026-11-15-research-adapter-deadline D1）。
    let idle_timeout = config
        .idle_timeout_secs
        .map(Duration::from_secs)
        .map(|d| d.min(limits.idle_timeout))
        .unwrap_or(limits.idle_timeout);
    let stream_limits = RunLimits {
        idle_timeout,
        ..*limits
    };
    let start = Instant::now();
    let mut task_result: Option<serde_json::Value> = None;
    let streamed = stream_child(
        child,
        stdout_log_path,
        stderr_log_path,
        start,
        &stream_limits,
        run_id,
        sink,
        "langmem runner",
        |line| {
            if let Some(rest) = line.strip_prefix(PROGRESS_PREFIX) {
                progress::emit_status(sink, &truncate_chars(rest.trim(), PROGRESS_LINE_MAX_CHARS));
            } else if let Some(rest) = line.strip_prefix(RESULT_PREFIX) {
                match serde_json::from_str::<serde_json::Value>(rest) {
                    Ok(value) => task_result = Some(value),
                    Err(e) => warn!("run {run_id}: could not parse CELERIS_RESULT line: {e}"),
                }
            }
        },
    )
    .await?;
    let exit_status = streamed.exit;

    if let Some(terminal) = streamed.timeout {
        // タイムアウトは供給側失敗として分類しない（他アダプタと同じ。ADR-0010 D5）。
        return finish_run(&run_dir, terminal, None, exit_status).await;
    }

    let stdout_buf = streamed.stdout;
    let stderr_tail = streamed.stderr_tail;
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
            usage: None,
        };
        return finish_run(&run_dir, terminal, None, exit_status).await;
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
                usage: None,
            },
            pf,
        )
    } else if task_result.is_none() {
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: "langmem runner did not print a CELERIS_RESULT line".to_string(),
                retryable: true,
                usage: None,
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
                usage: None,
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
        (terminal_from_summary(req, start, &summary).await, None)
    };

    finish_run(&run_dir, terminal, provider_failure, exit_status).await
}

/// ADR 2026-11-15-research-adapter-deadline D4: ランナーの要約から `artifacts/result.json` を書き、結果契約の
/// 正本（`result_report::read_result_file`・`interpret_result_text`。ADR 2026-11-10-adapter-result-contract D1）で
/// 読み直して終端を作る。期待の置き場に file があるので誤配置の回収は探索しない。書けなければ
/// `read_result_file` を呼ばずに `Error{retryable: true}`（reviewer が読む file の無い run を `Done` にしない）。
/// `start` は監視 loop の開始（`read_result_file` の `run_started` に直す。aider と同じ）。
async fn terminal_from_summary(req: &RunRequest, start: Instant, summary: &str) -> Terminal {
    let artifacts_rel = req.artifacts_rel();
    let body = serde_json::json!({ "summary": summary, "evidence": [] });
    let written = tokio::fs::write(req.artifact_path("result.json"), format!("{body:#}\n")).await;
    if let Err(e) = written {
        return Terminal::Error {
            message: format!("langmem adapter could not write {artifacts_rel}/result.json: {e}"),
            retryable: true,
            usage: None,
        };
    }
    match read_result_file(req, SystemTime::now() - start.elapsed()).await {
        ResultFileRead::Misplaced(error) => *error,
        ResultFileRead::Missing => Terminal::Error {
            message: format!("langmem adapter could not read back {artifacts_rel}/result.json"),
            retryable: true,
            usage: None,
        },
        ResultFileRead::Text(text) => interpret_result_text(&text, &artifacts_rel, None),
    }
}

#[cfg(test)]
mod tests;
