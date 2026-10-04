//! `local-deep-research` アダプタ（ADR-0029 D1）。
//!
//! Local Deep Research（LDR）は `paperqa`（ADR-0027 D3）と同じ「調査エンジンを包む」形。LDR には
//! 一発実行の CLI が無く（`ldr-web`/`ldr-mcp` は常駐プロセス）、プログラム的な API
//! （`local_deep_research.api.{quick_summary,detailed_research,generate_report}`）だけがある。
//! そのため celeris 側が実行用の Python スクリプトを持ち（`include_str!`）、run ごとに
//! `runs/<run_id>/ldr_run.py` として書き出して `<command> <その場所> <run_dir>/ldr_input.json` で起動する。
//! celeris の外に置くファイルは venv（`command` が指す python）だけで、スクリプト自体は celeris のバイナリと
//! 一緒に版が進む。
//!
//! ワーカープロトコル（`artifacts/result.json`）は PaperQA2 アダプタと同じく**アダプタが代わりに書く**
//! （ADR-0006 D3 の規約は保つ）。委譲（`delegate.json`）は扱わない（ADR-0029 D1: 「委譲はしない」）。

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use task_core::Task;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::progress;
use crate::protocol::{Answer, RunContext, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, kill_now, read_line_limited, read_tail, reap_after_terminal,
    write_result_json,
};

/// run ごとに `runs/<run_id>/ldr_run.py` として書き出す本体（ADR-0029 D1）。
const RUNNER_SCRIPT: &str = include_str!("local_deep_research_run.py");

/// `artifacts/result.json` の `summary` の上限（`paperqa` と同じ規則。ADR-0029 D1）。
const SUMMARY_MAX_CHARS: usize = 1500;
/// `progress:` 行を `progress` に転送するときの 1 行あたりの上限。
const PROGRESS_LINE_MAX_CHARS: usize = 500;
/// ランナーの最終行の目印（ADR-0029 D1）。
const RESULT_PREFIX: &str = "CELERIS_RESULT ";
/// `progress:` 行の目印。
const PROGRESS_PREFIX: &str = "progress:";

/// `[adapters.local_deep_research].mode`（ADR-0029 D1）。既定 `Quick`。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LdrMode {
    #[default]
    Quick,
    Detailed,
    Report,
}

impl LdrMode {
    fn as_str(self) -> &'static str {
        match self {
            LdrMode::Quick => "quick",
            LdrMode::Detailed => "detailed",
            LdrMode::Report => "report",
        }
    }
}

/// `[adapters.local_deep_research.evidence]`（ADR-0031 D2）: 決定的な証拠ゲートの閾値。ハーネス
/// （このアダプタ）が `CELERIS_RESULT` の `counts` を見て機械的に判定する（LLM に判断させない）。
/// `0` を書けばその項目は見ない。全部 0 なら従来どおり（ゲート無し）の挙動になる（受け入れ条件 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceThresholds {
    /// 検索が返した件数の合計の下限（`counts.search_results`。ランナーは重複排除前の出典件数を使う）。
    #[serde(default = "default_min_search_results")]
    pub min_search_results: u32,
    /// 実際に証拠として集まった出典（URL で重複排除後）の数の下限（`counts.sources`）。
    #[serde(default = "default_min_sources")]
    pub min_sources: u32,
    /// 報告が `[n]` で引用した出典の数の下限（`counts.sources_cited`）。
    #[serde(default = "default_min_cited")]
    pub min_cited: u32,
    /// 出典の異なるドメイン数の下限（`counts.unique_domains`）。
    #[serde(default = "default_min_domains")]
    pub min_domains: u32,
    /// ADR-0063 Phase 109b B4（`task_worker::PaperQaEvidence::insufficient_is_error` と同じ考え方）:
    /// 検索が 0 件（検索経路の問題）は、この設定に関わらず常に hard error。それ以外の閾値未達
    /// （`min_sources`/`min_cited`/`min_domains`）を hard error にするか（既定 `false`）。`false`
    /// （既定）なら証拠不足でも `Terminal::Done` にし、`report.md` の「## 証拠の質」節に内訳を書いて
    /// reviewer / 受け入れ条件の判断に委ねる。`true` にすると Phase 109 までどおり
    /// `Terminal::Error{retryable: true}`。
    #[serde(default)]
    pub insufficient_is_error: bool,
}

impl Default for EvidenceThresholds {
    fn default() -> Self {
        Self {
            min_search_results: default_min_search_results(),
            min_sources: default_min_sources(),
            min_cited: default_min_cited(),
            min_domains: default_min_domains(),
            insufficient_is_error: false,
        }
    }
}

fn default_min_search_results() -> u32 {
    5
}
fn default_min_sources() -> u32 {
    3
}
fn default_min_cited() -> u32 {
    2
}
fn default_min_domains() -> u32 {
    2
}

/// `[adapters.local_deep_research]`（config.toml, ADR-0029 D1）。`[[providers]] adapter =
/// "local-deep-research"` の行ごとに `model`（= `settings` の `llm.model` を上書き）と `env` を上書きできる
/// （`paperqa`/`acp` と同じ作り）。行の `settings` の上書きは無い（`ProviderConfig.settings` は `paperqa` 専用
/// のフィールドで、LDR では再利用しない。celeris 側の実装判断）。
#[derive(Debug, Clone)]
pub struct LdrConfig {
    /// 起動するコマンド（LDR を入れた venv の python）。
    pub command: String,
    pub mode: LdrMode,
    /// `quick_summary`/`detailed_research` の `iterations`。未指定なら渡さない。
    pub iterations: Option<u32>,
    /// `quick_summary`/`detailed_research` の `questions_per_iteration`。未指定なら渡さない。
    pub questions_per_iteration: Option<u32>,
    /// `settings_override` に渡すキー。値は文字列で持ち、数値・真偽値・JSON 配列/オブジェクトに見えるものは
    /// ランナー（Python）側で変換する（ADR-0029 D1/D3: TOML の型を混ぜない）。
    pub settings: Vec<(String, String)>,
    /// 設定されていれば `settings` の `llm.model` を上書きする（`paperqa` の `--llm` と同じ考え方）。
    pub model: Option<String>,
    /// 追加の環境変数。
    pub env: Vec<(String, String)>,
    /// ADR-0031 D2: 決定的な証拠ゲートの閾値。
    pub evidence: EvidenceThresholds,
    /// ADR-0063 D2（Phase 109）: `attempts >= 1`（前回が reviewer 不合格）の run で使う `mode`
    /// （既定 `Detailed`）。
    pub retry_mode: LdrMode,
    /// ADR-0063 D2: 同じく再挑戦の run で使う `iterations`（既定 `Some(5)`）。`mode`/`iterations` の
    /// 通常値より優先する。
    pub retry_iterations: Option<u32>,
    /// ADR-0063 Phase 109c B3: 目的文から対象（`research_targets`）が取れたとき、LDR の答えと必読の
    /// 一次情報の抜粋を材料に、プロキシの LLM（LDR と同じ `settings` の `llm.model` /
    /// `llm.openai_endpoint`）で対象×観点の表と対象ごとの節を合成し、`report.md` の先頭に置く
    /// （既定 `true`）。`false` にすると Phase 109b までどおり LDR の生の findings だけ。
    pub structured_synthesis: bool,
}

impl Default for LdrConfig {
    fn default() -> Self {
        Self {
            command: "python3".to_string(),
            mode: LdrMode::Quick,
            iterations: None,
            questions_per_iteration: None,
            settings: Vec::new(),
            model: None,
            env: Vec::new(),
            evidence: EvidenceThresholds::default(),
            retry_mode: LdrMode::Detailed,
            retry_iterations: Some(5),
            structured_synthesis: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LdrAdapter {
    config: LdrConfig,
}

impl LdrAdapter {
    pub const ID: &'static str = "local-deep-research";

    pub fn new(config: LdrConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for LdrAdapter {
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
        run_ldr(&self.config, &req, run_id, &limits, sink).await
    }

    /// 他のアダプタ（`paperqa`/`claude_code`/`codex`）と同じ規則: `extra` は `config.env` の末尾に足すので、
    /// 同名キーは `extra` が勝つ（celeris の環境 < アダプタの環境 < `with_env` の追加分）。
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(LdrAdapter::new(config)))
    }
}

/// ADR-0063 D2（Phase 109）: 前回 reviewer に不合格にされた条件の理由（`context.prior_review` で
/// `pass = false` のもの）。空欄・空白だけの理由は落とす。
fn must_cover_items(prior_review: &[crate::protocol::PriorReview]) -> Vec<String> {
    prior_review
        .iter()
        .filter(|p| !p.pass)
        .map(|p| p.reason.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

/// ADR-0063 Phase 109c B1: 「前回からの改善点」節。空なら空文字（従来どおりの問いのまま）。
/// Phase 109 まではこの節を問いの**先頭**に置いていたが、本番観測（2026-09-23）で LDR が
/// この節だけに検索・合成を引きずられ、目的文の他の対象を落とすことが分かった。以後は
/// `build_query` が目的文の**後ろ**に置く（置き換えない）。
fn build_must_cover_section(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let mut out = String::from("## 前回からの改善点（必ず埋める）\n");
    for item in items {
        out.push_str(&format!("- {item}\n"));
    }
    out.trim_end().to_string()
}

/// テキストの中の `http(s)://` で始まるトークンを全て拾う（`paperqa.rs::extract_urls` と同じ決定的な
/// トークナイズ。正規表現は使わない）。
fn extract_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '「' | '」' | '（' | '）' | '(' | ')' | '<' | '>' | '"' | '\'' | '　'
            )
    }) {
        let trimmed = token.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'));
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// ADR-0063 D2: 必読の一次情報の URL。目的文中の URL に加え、知識ベースの索引
/// （`context.knowledge.index`）のうち `primary-sources` / `一次情報` タグを持つページの
/// `sources`（前置きの出典。`celerisctl knowledge record --source` で人が付けたもの）を拾う。
/// 重複は落とし、出現順を保つ（決定的。LLM は使わない）。
pub fn must_read_urls(
    objective: &str,
    knowledge: Option<&crate::protocol::KnowledgeContext>,
) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for url in extract_urls(objective) {
        if seen.insert(url.clone()) {
            out.push(url);
        }
    }
    if let Some(k) = knowledge {
        for item in &k.index {
            let is_primary = item.tags.iter().any(|t| {
                let t = t.to_ascii_lowercase();
                t.contains("primary-source") || t.contains("一次情報")
            });
            if !is_primary {
                continue;
            }
            for url in &item.sources {
                // ADR-0063 Phase 109b B1: 知識ベースの `sources` は前置きの出典（人が
                // `celerisctl knowledge record --source` で付けたもの）で、`human` のような
                // URL でない値が混じることがある（本番の観測、2026-09-23）。`http(s)://` で
                // 始まるものだけを拾う。
                let is_url = url.starts_with("http://") || url.starts_with("https://");
                if is_url && seen.insert(url.clone()) {
                    out.push(url.clone());
                }
            }
        }
    }
    out
}

/// LDR に渡す**問い**を組み立てる。
///
/// 実機で分かったこと（2026-09-17）: ここにタスクのタイトルの見出し（`# ...`）や役割の指示文まで入れると、
/// 検索エンジンがその文字列ごと検索して**何も返さない**（同じ問いを素で投げれば出典が取れる）。
/// LDR は受け取った問いをそのまま検索にも使うので、**素の目的だけ**を渡す。
/// 人間の回答履歴は短い補足として後ろに付ける（検索語としての邪魔が少ない）。
/// 役割の指示文とタイトルは `runs/<run_id>/request.json` に残るので記録は失われない。
///
/// ADR-0063 D2（Phase 109）: 前回 reviewer に不合格にされた run（`context.prior_review` に
/// `pass = false` の条件がある）では、その理由を「前回からの改善点」として問いに足す。
///
/// ADR-0063 Phase 109c B1（本番観測 2026-09-23）: この節を目的文の**先頭**に置くと、LDR がそこだけに
/// 検索・合成を引きずられ、目的文が挙げる他の対象を落とすことが分かった（BeeOND のみの報告になった
/// 事故）。以後は**目的文をそのまま先に置き、置き換えない**。前回の `report.md`（`is_retry` のときだけ、
/// 先頭 20 KB）があれば、それを改善する材料として続けて足す。
pub fn build_query(task: &Task, context: &RunContext, prior_report: Option<&str>) -> String {
    // ADR-0029 / Phase 19 / ADR-0033 D6（Phase 27 の監査 M-2）: 検索ハーネスに渡すのは**素の目的だけ**。
    // 役職・記憶・直近のやり取り・記憶の書式指示は載せない（問いを濁すと検索が何も返さない）。
    let mut out = String::new();
    out.push_str(task.objective.trim());
    let must_cover = build_must_cover_section(&must_cover_items(&context.prior_review));
    if !must_cover.is_empty() {
        out.push_str("\n\n");
        out.push_str(&must_cover);
    }
    if let Some(report) = prior_report {
        let trimmed = report.trim();
        if !trimmed.is_empty() {
            out.push_str("\n\n## 前回の報告（これを改善する。削らない）\n");
            out.push_str(trimmed);
        }
    }
    if !context.answers.is_empty() {
        out.push_str("\n\n補足（人間の回答）:");
        for Answer { question, answer } in &context.answers {
            out.push_str(&format!("\n- {question} → {answer}"));
        }
    }
    out
}

/// ADR-0063 D4（Phase 109）: `settings` の値のうち秘密らしいもの（キーが `api_key` / `token` /
/// `password` / `secret` で終わる）は `ldr_input.json` に平文で書かない。実際の値は、LDR 自身が
/// `LDR_` + 設定キーの大文字化（`.` は `_`）で環境変数から読む規則（ADR-0031 の実測）に沿った名前の
/// 環境変数として子プロセスに渡し、JSON にはその環境変数名へのプレースホルダ（`"<env:NAME>"`）だけを
/// 書く。戻り値は `(JSON に書く settings, 子プロセスへ追加する env)`。
fn redact_secret_settings(
    settings: &std::collections::BTreeMap<String, String>,
) -> (
    std::collections::BTreeMap<String, String>,
    Vec<(String, String)>,
) {
    fn is_secret_key(key: &str) -> bool {
        let lower = key.to_ascii_lowercase();
        ["api_key", "token", "password", "secret"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
    }
    fn env_name(key: &str) -> String {
        format!("LDR_{}", key.to_ascii_uppercase().replace('.', "_"))
    }
    let mut redacted = std::collections::BTreeMap::new();
    let mut extra_env = Vec::new();
    for (key, value) in settings {
        if is_secret_key(key) && !value.is_empty() {
            let name = env_name(key);
            redacted.insert(key.clone(), format!("<env:{name}>"));
            extra_env.push((name, value.clone()));
        } else {
            redacted.insert(key.clone(), value.clone());
        }
    }
    (redacted, extra_env)
}

/// ADR-0063 Phase 109c B1: 前回の run の `report.md`（先頭 20 KB。文字境界で安全に切る）を読む。
/// 無い・空・読めない場合は `None`（run は止めない。あくまで再挑戦の材料）。
const PRIOR_REPORT_MAX_BYTES: usize = 20 * 1024;

async fn read_prior_report(report_path: &Path) -> Option<String> {
    let text = tokio::fs::read_to_string(report_path).await.ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= PRIOR_REPORT_MAX_BYTES {
        return Some(trimmed.to_string());
    }
    let mut end = PRIOR_REPORT_MAX_BYTES;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}\n\n…（以下省略）", &trimmed[..end]))
}

async fn run_ldr(
    config: &LdrConfig,
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
    let report_path = artifacts_dir.join("report.md");
    // ADR-0063 D2（Phase 109）: 前回 reviewer に不合格にされた run（`attempts >= 1`）は、mode /
    // iterations を強く（既定 `detailed` / 5 周）する。
    let is_retry = req.task.attempts >= 1;
    // ADR-0063 Phase 109c B1: 前回の run の `report.md` は、消す前に読んでおく（再挑戦のときだけ。
    // 「削らない」ための材料として問いに足す）。
    let prior_report = if is_retry {
        read_prior_report(&report_path).await
    } else {
        None
    };
    // 前回の run（リトライ）の名残を今回の結果と誤読しない（paperqa/claude_code/codex と同じ理由。ADR-0006 D3）。
    let _ = tokio::fs::remove_file(&report_path).await;
    let _ = tokio::fs::remove_file(artifacts_dir.join("result.json")).await;

    let query = build_query(&req.task, &req.context, prior_report.as_deref());
    // ADR-0023 D2 / M1: この run で何を渡したかを残す。
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &query, run_id).await;
    let mode = if is_retry {
        config.retry_mode
    } else {
        config.mode
    };
    let iterations = if is_retry {
        config.retry_iterations.or(config.iterations)
    } else {
        config.iterations
    };
    if is_retry {
        progress::emit_status(
            sink,
            &format!(
                "retrying (attempt {}); escalating to mode={} iterations={:?}",
                req.task.attempts + 1,
                mode.as_str(),
                iterations
            ),
        );
    }

    // ADR-0063 D2: 必読の一次情報（目的文中の URL + 知識ベースの primary-sources / 一次情報 タグ）。
    // `ldr_run.py` が LDR の検索の前に直接 fetch して sources に含める。
    let must_read = must_read_urls(&req.task.objective, req.context.knowledge.as_ref());
    if !must_read.is_empty() {
        progress::emit_status(
            sink,
            &format!("{} must-read primary source(s)", must_read.len()),
        );
    }

    // `model` は `settings` の `llm.model` より優先する（ADR-0029 D1）。`BTreeMap` で決定的な順序にする。
    let mut settings: std::collections::BTreeMap<String, String> =
        config.settings.iter().cloned().collect();
    if let Some(model) = &config.model {
        settings.insert("llm.model".to_string(), model.clone());
    }
    // ADR-0063 D4（Phase 109）: 秘密らしい値（`api_key`/`token`/`password`/`secret` で終わるキー）は
    // `ldr_input.json` に平文で書かない。実際の値は子プロセスの環境変数として渡し、JSON にはその
    // 環境変数名へのプレースホルダだけを書く（`ldr_run.py::convert_setting_value` が解決する）。
    let (json_settings, secret_env) = redact_secret_settings(&settings);

    // ADR-0063 Phase 109c A/B3: 目的文から取れた対象・観点。`targets` が空なら `ldr_run.py` は
    // 構造化合成をせず、従来どおり LDR の生の findings だけを report.md に書く。
    let targets = crate::research_targets::research_targets(&req.task.objective);
    let aspects = crate::research_targets::research_aspects(&req.task.objective);
    if !targets.is_empty() {
        progress::emit_status(
            sink,
            &format!(
                "{} target(s), {} aspect(s) for structured synthesis: {}",
                targets.len(),
                aspects.len(),
                targets.join(" / ")
            ),
        );
    }

    let input = serde_json::json!({
        "query": query,
        "mode": mode.as_str(),
        "settings": json_settings,
        "iterations": iterations,
        "questions_per_iteration": config.questions_per_iteration,
        "report_path": report_path.to_string_lossy(),
        "must_read_urls": must_read,
        "targets": targets,
        "aspects": aspects,
        "structured_synthesis": config.structured_synthesis,
    });
    let script_path = run_dir.join("ldr_run.py");
    let input_path = run_dir.join("ldr_input.json");
    tokio::fs::write(&script_path, RUNNER_SCRIPT).await?;
    let input_text = serde_json::to_string_pretty(&input)?;
    tokio::fs::write(&input_path, format!("{input_text}\n")).await?;

    let mut command = Command::new(&config.command);
    command
        .arg(&script_path)
        .arg(&input_path)
        .envs(config.env.iter().cloned())
        .envs(secret_env)
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
        if idle_elapsed >= limits.idle_timeout {
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
                // ランナーの出力形式は celeris が定義したものではないので寛容に無視する（paperqa と同じ考え方）。
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from the local-deep-research runner");
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
                    // ADR-0029 D1 手順 3: ランナーは検索・要約の進捗を `progress: <text>` の形で出す。
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

    let report_len = tokio::fs::metadata(&report_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let (terminal, provider_failure) = if !exit_status.success() {
        let exit_repr = match exit_status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: format!(
                    "local-deep-research runner exited with a non-zero status (exit={exit_repr})"
                ),
                retryable: true,
            },
            pf,
        )
    } else if task_result.is_none() {
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: "local-deep-research runner did not print a CELERIS_RESULT line"
                    .to_string(),
                retryable: true,
            },
            pf,
        )
    } else if report_len == 0 {
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: "local-deep-research runner produced an empty report".to_string(),
                retryable: true,
            },
            pf,
        )
    } else {
        // ADR-0029 D1: アダプタが `artifacts/report.md`（ランナーが直接書いた）を成果物として申告し、
        // `artifacts/result.json` を書く。exit=0 かつ `report_len > 0` の枝で `task_result` は必ず
        // `Some`（上の 2 つの分岐で `None`/空報告は既に処理済み）なので `unwrap_or_default` で十分。
        let value = task_result.unwrap_or(serde_json::Value::Null);
        let raw_summary = value
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let summary = single_line_summary(raw_summary, SUMMARY_MAX_CHARS);

        // ADR-0031 D1: `report.md` に加えて、ランナーが機械的に作った証拠の記録
        // （`sources.json` / `research.json`）も成果物として申告する。ゲート（下）に落ちても
        // **消さずに残す**（D2: 人が読めるように）ので、この申告はゲートの判定より前に行う。
        // ADR-0036 D4: 申告する `path` は workspace 相対のまま（`artifacts_dir` 基準で組む）。
        for (name, rel_path, kind) in [
            (
                "report.md",
                format!("{artifacts_rel}/report.md"),
                "markdown",
            ),
            (
                "sources.json",
                format!("{artifacts_rel}/sources.json"),
                "json",
            ),
            (
                "research.json",
                format!("{artifacts_rel}/research.json"),
                "json",
            ),
        ] {
            match crate::artifact::resolve(&req.workspace, name, &rel_path, Some(kind)) {
                Ok(artifact) => sink.artifact(&artifact),
                Err(e) => warn!("run {run_id}: could not register {rel_path}: {e}"),
            }
        }

        // ADR-0031 D2: 決定的な証拠ゲート。`CELERIS_RESULT` の `counts` を見る（古いランナー/スタブで
        // 無ければ全 0 扱い＝閾値を全部 0 にしないと落ちる）。LLM には判断させない。
        let counts = value.get("counts");
        let count_of = |key: &str| -> u32 {
            counts
                .and_then(|c| c.get(key))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32
        };
        let search_results = count_of("search_results");
        let sources = count_of("sources");
        let sources_cited = count_of("sources_cited");
        let unique_domains = count_of("unique_domains");
        let ev = &config.evidence;

        // どれか 1 つでも閾値が立っているか（全部 0 なら ADR-0031 D2 どおりゲートを見ない）。
        let gate_enabled = ev.min_search_results > 0
            || ev.min_sources > 0
            || ev.min_cited > 0
            || ev.min_domains > 0;
        // 検索経路そのものの問題（鍵切れ・CAPTCHA・ネットワーク遮断）を、調べた結果情報が無かった
        // ケースと区別できるように、別メッセージにする（ADR-0031 D2）。`min_search_results = 0` に
        // していてもこの区別は要る（他の項目で落ちるので、運用者が原因を知りたいのは同じ）。これは
        // `insufficient_is_error` に関わらず常に hard error（ADR-0063 Phase 109b B4。PaperQA の
        // 「取得 0 件」と同じ考え方）。
        let zero_search_results = gate_enabled && search_results == 0;
        let mut problems = Vec::new();
        if !zero_search_results {
            if ev.min_search_results > 0 && search_results < ev.min_search_results {
                problems.push(format!(
                    "search_results={search_results} (min {})",
                    ev.min_search_results
                ));
            }
            if ev.min_sources > 0 && sources < ev.min_sources {
                problems.push(format!("sources={sources} (min {})", ev.min_sources));
            }
            if ev.min_cited > 0 && sources_cited < ev.min_cited {
                problems.push(format!("cited={sources_cited} (min {})", ev.min_cited));
            }
            if ev.min_domains > 0 && unique_domains < ev.min_domains {
                problems.push(format!("domains={unique_domains} (min {})", ev.min_domains));
            }
        }
        let insufficient = zero_search_results || !problems.is_empty();

        // ADR-0063 Phase 109b B4: 閾値未達（`search_results == 0` を除く）は、`insufficient_is_error`
        // （既定 false）が true のときだけ hard error。既定では `Terminal::Done` にし、`report.md` の
        // 「## 証拠の質」節に内訳を書いて reviewer / 受け入れ条件の判断に委ねる。
        let hard_error_message = if zero_search_results {
            Some(
                "web search returned nothing (possible search path failure: expired key, CAPTCHA, or network block)"
                    .to_string(),
            )
        } else if !problems.is_empty() && ev.insufficient_is_error {
            Some(format!(
                "insufficient web evidence: {}",
                problems.join(", ")
            ))
        } else {
            None
        };

        // ADR-0063 Phase 109b B4: ゲートを見た run では常に「## 証拠の質」節を足す（合否に関わらず。
        // hard error でも report.md 自体は残るので、人が読めるようにしておく）。
        if gate_enabled {
            append_evidence_quality_section(
                &report_path,
                sources,
                sources_cited,
                unique_domains,
                &problems,
                insufficient,
                run_id,
            )
            .await;
        }

        if let Some(message) = hard_error_message {
            // ADR-0031 D2: retryable な `Terminal::Error`。供給側の失敗（`AdapterError`）にはしない
            // （プロバイダを cooldown にする話ではない）ので `provider_failure` は `None` のまま。
            (
                Terminal::Error {
                    message,
                    retryable: true,
                },
                None,
            )
        } else {
            let result_file = serde_json::json!({ "summary": summary, "evidence": [] });
            match serde_json::to_string_pretty(&result_file) {
                Ok(text) => {
                    if let Err(e) =
                        tokio::fs::write(artifacts_dir.join("result.json"), format!("{text}\n"))
                            .await
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
        }
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

/// `artifacts/result.json` の `summary`: 改行・連続空白を単一の空白にたたみ（single-line-safe）、
/// 文字数で上限まで切り詰める（ランナー側でも行うが、二重に安全側へ倒す）。
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

/// `sources.json`（`report_path` の隣。ランナーが書く）から `primary: true` の件数と、そのうち
/// `cited: true` の件数を数える（ADR-0063 Phase 109b B1/B4: 必読の一次情報が実際に report に
/// 反映されたか）。読めない・壊れていれば `(0, 0)`（この節は補足情報であって、失敗しても run は
/// 止めない）。
async fn read_primary_source_counts(report_path: &Path) -> (u32, u32) {
    let Some(dir) = report_path.parent() else {
        return (0, 0);
    };
    let Ok(text) = tokio::fs::read_to_string(dir.join("sources.json")).await else {
        return (0, 0);
    };
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return (0, 0);
    };
    let mut total = 0u32;
    let mut cited = 0u32;
    for entry in &entries {
        if entry.get("primary").and_then(serde_json::Value::as_bool) == Some(true) {
            total += 1;
            if entry.get("cited").and_then(serde_json::Value::as_bool) == Some(true) {
                cited += 1;
            }
        }
    }
    (total, cited)
}

/// `report.md` の末尾に「## 証拠の質」節を足す（ADR-0063 Phase 109b B4。決定的の証拠ゲートを見た
/// run では常に。PaperQA の `render_evidence_section` と同じ考え方）。出典/引用/ドメイン数と、
/// 必読の一次情報のうち report に反映された件数、証拠不足ならその理由を書く。合否は reviewer に
/// 委ねる（`insufficient` でも run 自体は Done になり得る）。読み書きに失敗しても run は止めない。
#[allow(clippy::too_many_arguments)]
async fn append_evidence_quality_section(
    report_path: &Path,
    sources: u32,
    sources_cited: u32,
    unique_domains: u32,
    problems: &[String],
    insufficient: bool,
    run_id: &str,
) {
    let (primary_total, primary_cited) = read_primary_source_counts(report_path).await;
    let mut section = String::from("\n\n## 証拠の質\n\n");
    section.push_str(&format!(
        "- 出典: {sources} 件（引用: {sources_cited} 件、異なるドメイン: {unique_domains} 件）\n"
    ));
    if primary_total > 0 {
        section.push_str(&format!(
            "- 必読の一次情報: {primary_total} 件中 {primary_cited} 件が report に反映（引用）された\n"
        ));
    }
    if insufficient {
        let reason = if problems.is_empty() {
            "web search returned nothing".to_string()
        } else {
            problems.join(", ")
        };
        section.push_str(&format!(
            "- 証拠不足（{reason}）。合否は reviewer の判断に委ねる。\n"
        ));
    }
    match tokio::fs::read_to_string(report_path).await {
        Ok(mut body) => {
            body.push_str(&section);
            if let Err(e) = tokio::fs::write(report_path, body).await {
                warn!(
                    "run {run_id}: could not append the evidence-quality section to report.md: {e}"
                );
            }
        }
        Err(e) => warn!(
            "run {run_id}: could not read report.md to append the evidence-quality section: {e}"
        ),
    }
}

#[cfg(test)]
mod tests;
