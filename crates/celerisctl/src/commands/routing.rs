//! `celerisctl routing show`（ADR-0069 Phase 118 D3）。
//!
//! 設定ファイルだけを読む読み取り専用コマンド（`config to-harnesses` と同じく DB を開かない）。
//! 2 つの表を出す: (1) `[[providers]] tier_models` の provider ごとの
//! `tier → name / model_id（または unavailable の理由）/ reasoning_effort`、
//! (2) `[llm_proxy.models]` の `tier → claude/gpt/qwen ごとの model`。
//! どちらも設定ファイルに書かれた値をそのまま見せるだけで、到達性・残量は見ない
//! （到達性・cooldown は既存の `GET /llm/sources` の仕事）。
//!
//! `routing export` / `routing evaluate`（ADR 2026-10-04-multi-objective-model-routing §7.2・Phase 4）:
//! export は `--db` で明示した SQLite を task-ops の読み取り専用接続で開き（daemon の DB を既定で
//! 開かない。migration しない）、`dataset.jsonl` と `manifest.json` を書く。evaluate は DB を開かず、
//! dataset と任意の外部 benchmark baseline から report v1 を書く。同じ入力からは同じバイト列になる。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use celeris::config::Config;
use clap::Subcommand;
use task_core::Tier;
use task_ops::routing_replay::{self, BenchmarkBaselineV1, DatasetV1, ExportOptions, ReportV1};

use crate::error::CliError;
use crate::outln;

/// 表示順（頻度の高い順ではなく、上から強い lane の順。GUI の `TIER_OPTIONS` と同じ並び）。
const TIER_ORDER: [Tier; 3] = [Tier::Frontier, Tier::Standard, Tier::Cheap];

#[derive(Subcommand, Debug)]
pub enum RoutingCommand {
    /// provider ごとの tier → 実行モデル/effort と、`[llm_proxy.models]` の tier → 供給元ごとの
    /// モデルを表にして出す。
    Show(RoutingShowArgs),
    /// 過去の events から routing dataset（`dataset.jsonl` + `manifest.json`、schema v1）を書き出す。
    /// DB は全体の `--db` で明示したものだけを読み取り専用で開く（環境変数・既定の DB は使わない）。
    Export(RoutingExportArgs),
    /// export した dataset を候補 policy で再評価し、report v1（JSON、任意で Markdown）を書く。
    /// DB は開かない。
    Evaluate(RoutingEvaluateArgs),
}

#[derive(clap::Args, Debug)]
pub struct RoutingExportArgs {
    /// 書き出し先ディレクトリ（無ければ作る）。`dataset.jsonl` と `manifest.json` を置く。
    #[arg(long)]
    pub out: PathBuf,
    /// 抽出の開始（RFC 3339、含む）。
    #[arg(long)]
    pub since: Option<String>,
    /// 抽出の終わり（RFC 3339、含まない）。
    #[arg(long)]
    pub until: Option<String>,
    /// task 単位の train/calibration/test 分割の seed。
    #[arg(long, default_value_t = 0)]
    pub seed: u64,
    /// manifest に記す policy の識別（hash）。export は設定ファイルを読まないので呼び手が渡す。
    #[arg(long, default_value = "unspecified")]
    pub policy_hash: String,
    /// manifest に記す model catalog の識別（hash）。
    #[arg(long, default_value = "unspecified")]
    pub catalog_hash: String,
    /// manifest に記す quality estimator の識別（hash）。
    #[arg(long, default_value = "unspecified")]
    pub estimator_hash: String,
}

/// report に載せる候補 policy。
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvalPolicy {
    /// 実際の primary（旧経路の決定）。
    Legacy,
    /// 記録された候補のうち eligible で score が最大のもの。
    Heuristic,
    /// 記録された decision shadow の判断。
    Shadow,
}

impl EvalPolicy {
    /// report v1 の `policies` の key。
    fn report_key(self) -> &'static str {
        match self {
            EvalPolicy::Legacy => "legacy",
            EvalPolicy::Heuristic => "heuristic",
            EvalPolicy::Shadow => "shadow_recorded",
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct RoutingEvaluateArgs {
    /// `routing export` の書き出し先ディレクトリ。
    #[arg(long)]
    pub dataset: PathBuf,
    /// 載せる policy（複数可）。省略時は全部。
    #[arg(long, value_enum)]
    pub policy: Vec<EvalPolicy>,
    /// 外部 benchmark（RouterBench 型）の baseline JSON（`{"name":…,"models":{…}}`）。別欄で載せる。
    #[arg(long)]
    pub baseline: Option<PathBuf>,
    /// report v1（JSON）の書き先。
    #[arg(long)]
    pub out: PathBuf,
    /// 人が読む Markdown の書き先（任意）。
    #[arg(long)]
    pub markdown: Option<PathBuf>,
}

const DATASET_FILE: &str = "dataset.jsonl";
const MANIFEST_FILE: &str = "manifest.json";

#[derive(clap::Args, Debug)]
pub struct RoutingShowArgs {
    /// 読み込む設定ファイル（`~/.config/celeris/config.toml` など）。
    #[arg(long)]
    pub config: PathBuf,
}

/// `db` は全体の `--db` の値そのもの（`CELERIS_DB` などの既定は解決しない）。`export` だけが使う。
pub fn run(db: Option<&Path>, command: RoutingCommand) -> Result<ExitCode, CliError> {
    match command {
        RoutingCommand::Export(args) => {
            let db = db.ok_or_else(|| {
                CliError::msg("routing export: --db <path> is required (the daemon DB is never opened by default)")
            })?;
            let rows = run_export(db, &args)?;
            outln!("exported {rows} rows to {}", args.out.display());
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Evaluate(args) => {
            run_evaluate(&args)?;
            outln!("wrote {}", args.out.display());
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Show(args) => {
            let config = Config::load(&args.config)
                .map_err(|e| CliError::msg(format!("config {}: {e}", args.config.display())))?;
            outln!("{}", render_routing_table(&config).trim_end());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), CliError> {
    std::fs::write(path, text).map_err(|e| CliError::msg(format!("write {}: {e}", path.display())))
}

fn read_file(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path)
        .map_err(|e| CliError::msg(format!("read {}: {e}", path.display())))
}

/// export の本体。書いた行数を返す。
fn run_export(db: &Path, args: &RoutingExportArgs) -> Result<usize, CliError> {
    if !db.is_file() {
        return Err(CliError::msg(format!(
            "routing export: no database at {}",
            db.display()
        )));
    }
    let options = ExportOptions {
        policy_hash: args.policy_hash.clone(),
        catalog_hash: args.catalog_hash.clone(),
        estimator_hash: args.estimator_hash.clone(),
        from_utc: args.since.clone(),
        until_utc: args.until.clone(),
        seed: args.seed,
    };
    let dataset = routing_replay::export(db, &options)
        .map_err(|e| CliError::msg(format!("routing export: {e}")))?;
    std::fs::create_dir_all(&args.out)
        .map_err(|e| CliError::msg(format!("create {}: {e}", args.out.display())))?;
    let jsonl = dataset
        .jsonl()
        .map_err(|e| CliError::msg(format!("routing export: {e}")))?;
    let manifest = serde_json::to_string_pretty(&dataset.manifest)
        .map_err(|e| CliError::msg(format!("routing export: {e}")))?;
    write_file(&args.out.join(DATASET_FILE), &jsonl)?;
    write_file(&args.out.join(MANIFEST_FILE), &format!("{manifest}\n"))?;
    Ok(dataset.rows.len())
}

/// `routing export` の書き出しを読み戻す。
fn load_dataset(dir: &Path) -> Result<DatasetV1, CliError> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest = serde_json::from_str(&read_file(&manifest_path)?)
        .map_err(|e| CliError::msg(format!("{}: {e}", manifest_path.display())))?;
    let dataset_path = dir.join(DATASET_FILE);
    let mut rows = Vec::new();
    for (i, line) in read_file(&dataset_path)?.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(
            serde_json::from_str(line)
                .map_err(|e| CliError::msg(format!("{}:{}: {e}", dataset_path.display(), i + 1)))?,
        );
    }
    Ok(DatasetV1 { manifest, rows })
}

/// evaluate の本体（DB は開かない）。
fn run_evaluate(args: &RoutingEvaluateArgs) -> Result<ReportV1, CliError> {
    let dataset = load_dataset(&args.dataset)?;
    let baseline: Option<BenchmarkBaselineV1> = args
        .baseline
        .as_deref()
        .map(|path| {
            serde_json::from_str(&read_file(path)?)
                .map_err(|e| CliError::msg(format!("baseline {}: {e}", path.display())))
        })
        .transpose()?;
    let mut report = routing_replay::evaluate(&dataset, baseline)
        .map_err(|e| CliError::msg(format!("routing evaluate: {e}")))?;
    if !args.policy.is_empty() {
        let keep: Vec<&str> = args.policy.iter().map(|p| p.report_key()).collect();
        report.policies.retain(|k, _| keep.contains(&k.as_str()));
    }
    let json = report
        .json()
        .map_err(|e| CliError::msg(format!("routing evaluate: {e}")))?;
    write_file(&args.out, &format!("{json}\n"))?;
    if let Some(md) = &args.markdown {
        write_file(md, &render_report_markdown(&report))?;
    }
    Ok(report)
}

fn opt_f64(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_string(), |v| format!("{v}"))
}

fn opt_u64(v: Option<u64>) -> String {
    v.map_or_else(|| "n/a".to_string(), |v| v.to_string())
}

/// report v1 の人向け Markdown（JSON と同じ値だけを並べる。単一の改善率に混ぜない）。
pub fn render_report_markdown(report: &ReportV1) -> String {
    let mut out = String::new();
    out.push_str("# Routing offline evaluation\n\n");
    out.push_str(&format!(
        "- schema: {}\n- dataset: {}\n- policy_hash: {}\n- catalog_hash: {}\n- estimator_hash: {}\n- seed: {}\n\n",
        report.schema,
        report.dataset_schema,
        report.policy_hash,
        report.catalog_hash,
        report.estimator_hash,
        report.seed
    ));
    out.push_str("## Policies\n\n");
    out.push_str("| policy | decisions | observed | acceptance success | cash mean (observed) | effective mean (observed) | cash mean (estimated) | api p50/p95 ms | task wall p50/p95 ms | retry | escalation | quota exhaustion | violations | unknown | timeout | drop | coverage |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for (name, p) in &report.policies {
        out.push_str(&format!(
            "| {name} | {} | {} | {} | {} | {} | {} | {}/{} | {}/{} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            p.decisions,
            p.observed_outcomes,
            opt_f64(p.acceptance_success_rate),
            opt_f64(p.cash_usd_mean),
            opt_f64(p.effective_usd_mean),
            opt_f64(p.estimated_cash_usd_mean),
            opt_u64(p.api_latency_p50_ms),
            opt_u64(p.api_latency_p95_ms),
            opt_u64(p.task_wall_p50_ms),
            opt_u64(p.task_wall_p95_ms),
            opt_f64(p.retry_rate),
            opt_f64(p.escalation_rate),
            opt_f64(p.quota_exhaustion_rate),
            p.constraint_violations,
            p.unknown_rate,
            p.timeout_rate,
            p.drop_rate,
            p.counterfactual_coverage,
        ));
    }
    if let Some(p) = report.policies.values().next() {
        out.push_str(&format!("\nUnknown: {}\n", p.unknown_reason));
    }
    out.push_str("\n## Paired metrics\n\n");
    for (k, v) in &report.paired_metrics {
        out.push_str(&format!("- {k}: {v}\n"));
    }
    out.push_str("\n## External benchmark baseline\n\n");
    match &report.external_benchmark_baseline {
        None => out.push_str("(none)\n"),
        Some(b) => {
            out.push_str(&format!("{} (external; not task outcomes)\n\n", b.name));
            out.push_str("| model | quality | cost usd |\n| --- | --- | --- |\n");
            for (model, m) in &b.models {
                out.push_str(&format!(
                    "| {model} | {} | {} |\n",
                    opt_f64(m.quality),
                    opt_f64(m.cost_usd)
                ));
            }
        }
    }
    out
}

fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

/// provider の 1 tier の表示（`ModelBinding` が無ければ「未設定」、`unavailable_reason` があれば
/// それを優先して見せる。`model_routing::resolve` と同じ優先順）。
fn binding_cell(binding: Option<&task_core::model_routing::ModelBinding>) -> String {
    match binding {
        None => "(未設定)".to_string(),
        Some(b) => {
            let mut cell = b.name.clone();
            if let Some(reason) = &b.unavailable_reason {
                cell.push_str(&format!(" -> unavailable: {reason}"));
            } else if let Some(id) = &b.model_id {
                cell.push_str(&format!(" -> {id}"));
            } else {
                cell.push_str(" -> (model_id 未設定)");
            }
            if let Some(effort) = &b.reasoning_effort {
                cell.push_str(&format!(" [effort={effort}]"));
            }
            cell
        }
    }
}

/// `celerisctl routing show` の本体（純粋関数。テストしやすいよう `run` から分離）。
pub fn render_routing_table(config: &Config) -> String {
    let mut out = String::new();
    out.push_str("=== providers (tier_models) ===\n");
    if config.providers.is_empty() {
        out.push_str("(no providers configured)\n");
    }
    for p in &config.providers {
        if p.tier_models.is_empty() {
            out.push_str(&format!(
                "{} ({}): tier_models not configured (legacy single model = {:?})\n",
                p.id,
                p.adapter,
                if p.model.is_empty() {
                    "adapter default".to_string()
                } else {
                    p.model.clone()
                }
            ));
            continue;
        }
        out.push_str(&format!("{} ({}):\n", p.id, p.adapter));
        for tier in TIER_ORDER {
            out.push_str(&format!(
                "  {:<8} {}\n",
                tier_label(tier),
                binding_cell(p.tier_models.get(&tier))
            ));
        }
    }
    out.push('\n');
    out.push_str("=== [llm_proxy.models] (tier -> source model) ===\n");
    out.push_str(&format!(
        "  {:<8} {:<24} {:<24} {}\n",
        "tier", "claude", "gpt", "qwen"
    ));
    for tier in TIER_ORDER {
        let claude = config
            .llm_proxy
            .models
            .claude
            .get(&tier)
            .map(String::as_str)
            .unwrap_or("(未設定)");
        let gpt = config
            .llm_proxy
            .models
            .gpt
            .get(&tier)
            .map(String::as_str)
            .unwrap_or("(未設定)");
        let qwen = config
            .llm_proxy
            .models
            .qwen
            .get(&tier)
            .map(String::as_str)
            .unwrap_or("(未設定)");
        out.push_str(&format!(
            "  {:<8} {:<24} {:<24} {}\n",
            tier_label(tier),
            claude,
            gpt,
            qwen
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &std::path::Path, text: &str) -> PathBuf {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn shows_provider_tier_models_and_llm_proxy_models_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
[[providers]]
id = "claude"
adapter = "claude-code"
[providers.tier_models.frontier]
name = "fable"
model_id = "claude-fable-5-1"
[providers.tier_models.standard]
name = "opus"
unavailable_reason = "not verified yet"
[providers.tier_models.cheap]
name = "sonnet"
model_id = "claude-sonnet-5"

[[providers]]
id = "legacy"
adapter = "fake"
"#,
        );
        let config = Config::load(&path).unwrap();
        let table = render_routing_table(&config);
        assert!(table.contains("claude (claude-code):"));
        assert!(table.contains("frontier fable -> claude-fable-5-1"));
        assert!(table.contains("standard opus -> unavailable: not verified yet"));
        assert!(table.contains("cheap    sonnet -> claude-sonnet-5"));
        assert!(table.contains("legacy (fake): tier_models not configured"));
        // llm_proxy.models の既定表（ADR-0069 Phase 118 D2）。
        assert!(table.contains("[llm_proxy.models]"));
        assert!(table.contains("claude-fable-5-1"));
        assert!(table.contains("gpt-6-astra"));
        assert!(table.contains("qwen3.8-27b"));
    }

    #[test]
    fn a_provider_without_a_tier_binding_is_marked_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
[[providers]]
id = "gpt"
adapter = "codex"
[providers.tier_models.frontier]
name = "astra"
model_id = "gpt-6-astra"
reasoning_effort = "high"
"#,
        );
        let config = Config::load(&path).unwrap();
        let table = render_routing_table(&config);
        assert!(table.contains("frontier astra -> gpt-6-astra [effort=high]"));
        assert!(table.contains("standard (未設定)"));
        assert!(table.contains("cheap    (未設定)"));
    }
}
