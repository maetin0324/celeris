//! `celerisctl routing show`（ADR-0069 Phase 118 D3）。
//!
//! 設定ファイルだけを読む読み取り専用コマンド（`config to-harnesses` と同じく DB を開かない）。
//! `[llm_sources.<id>]`・`[harnesses.<id>]`・`[[llm_routes]]`・`[route_defaults.<harness>]`・
//! `[cos] route_candidates` の宣言を表にする。
//! 設定ファイルに書かれた値をそのまま見せるだけで、到達性・残量は見ない
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
use task_ops::routing_replay::{
    self, BenchmarkBaselineV1, DatasetV1, EstimatorComparisonInputV1, EstimatorComparisonV1,
    ExportOptions, ReportV1,
};

use crate::error::CliError;
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum RoutingCommand {
    /// LLM source・harness・route の宣言を表にして出す。
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
    /// estimator shadow の比較（`estimator_comparison`。候補 policy ではなく、pin した
    /// descriptor/pair と dataset の estimator shadow の比較欄を載せる）。
    Estimator,
}

impl EvalPolicy {
    /// report v1 の `policies` の key。`estimator` は `policies` に入らない。
    fn report_key(self) -> Option<&'static str> {
        match self {
            EvalPolicy::Legacy => Some("legacy"),
            EvalPolicy::Heuristic => Some("heuristic"),
            EvalPolicy::Shadow => Some("shadow_recorded"),
            EvalPolicy::Estimator => None,
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
    /// pin した estimator の記述 JSON（`--policy estimator` が必要）。descriptor と任意の
    /// RouteLLM pair（strong/weak・calibration_version）。dataset の estimator shadow と照合する。
    #[arg(long)]
    pub estimator: Option<PathBuf>,
    /// pin した RouteLLM pair の記述 JSON（`--estimator` と併用。`--policy estimator` が必要）。
    #[arg(long)]
    pub pair: Option<PathBuf>,
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

/// `--estimator` / `--pair` の pin 読み込み（`--policy estimator` が無いと拒否）。
fn estimator_input(
    args: &RoutingEvaluateArgs,
) -> Result<Option<EstimatorComparisonInputV1>, CliError> {
    if args.estimator.is_none() && args.pair.is_none() {
        return Ok(None);
    }
    if !args.policy.contains(&EvalPolicy::Estimator) {
        return Err(CliError::msg(
            "routing evaluate: --estimator/--pair require --policy estimator",
        ));
    }
    let descriptor = match &args.estimator {
        Some(path) => {
            let json = read_file(path)?;
            serde_json::from_str(&json)
                .map_err(|e| CliError::msg(format!("estimator {}: {e}", path.display())))?
        }
        None => {
            return Err(CliError::msg(
                "routing evaluate: --pair requires --estimator (a pinned descriptor)",
            ));
        }
    };
    let pair = match &args.pair {
        Some(path) => {
            let json = read_file(path)?;
            Some(
                serde_json::from_str(&json)
                    .map_err(|e| CliError::msg(format!("pair {}: {e}", path.display())))?,
            )
        }
        None => None,
    };
    Ok(Some(EstimatorComparisonInputV1 { descriptor, pair }))
}

/// evaluate の本体（DB は開かない、HTTP も呼ばない）。
fn run_evaluate(args: &RoutingEvaluateArgs) -> Result<ReportV1, CliError> {
    let estimator = estimator_input(args)?;
    let dataset = load_dataset(&args.dataset)?;
    let baseline: Option<BenchmarkBaselineV1> = args
        .baseline
        .as_deref()
        .map(|path| {
            serde_json::from_str(&read_file(path)?)
                .map_err(|e| CliError::msg(format!("baseline {}: {e}", path.display())))
        })
        .transpose()?;
    let mut report =
        routing_replay::evaluate_with_estimator(&dataset, baseline, None, estimator.as_ref())
            .map_err(|e| CliError::msg(format!("routing evaluate: {e}")))?;
    if args.policy.contains(&EvalPolicy::Estimator) && report.estimator_comparison.is_none() {
        return Err(CliError::msg(
            "routing evaluate: --policy estimator found no estimator shadow in the dataset \
             and no pinned estimator was supplied (--estimator)",
        ));
    }
    if !args.policy.is_empty() {
        let keep: Vec<&str> = args.policy.iter().filter_map(|p| p.report_key()).collect();
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
    if let Some(est) = &report.estimator_comparison {
        out.push_str(&render_estimator_comparison_markdown(est));
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

/// estimator shadow 比較の人向け Markdown（JSON の `estimator_comparison` と同じ値だけを並べる）。
pub fn render_estimator_comparison_markdown(est: &EstimatorComparisonV1) -> String {
    let mut out = String::new();
    out.push_str("## Estimator shadow comparison\n\n");
    let descriptor = est
        .descriptor
        .as_ref()
        .map(|d| {
            format!(
                "{} @ {} (protocol v{})",
                d.estimator_id, d.version, d.protocol_version
            )
        })
        .unwrap_or_else(|| "(not pinned)".to_string());
    let pair = match &est.pair {
        Some(p) => format!(
            "{}: strong={} weak={}{}",
            p.router,
            p.strong_model,
            p.weak_model,
            p.calibration_version
                .as_ref()
                .map(|v| format!(" calibration={v}"))
                .unwrap_or_default()
        ),
        None => "(not pinned)".to_string(),
    };
    out.push_str(&format!("- descriptor: {descriptor}\n"));
    out.push_str(&format!("- pair: {pair}\n"));
    out.push_str(&format!("- calibrated: {}\n", est.calibrated));
    out.push_str(&format!(
        "- observed estimator versions: {}\n",
        if est.observed_estimator_versions.is_empty() {
            "(none)".to_string()
        } else {
            est.observed_estimator_versions.join(", ")
        }
    ));
    out.push_str(&format!(
        "- target decisions: {}\n- evaluated: {}\n- coverage: {}\n",
        est.target_decisions, est.evaluated, est.coverage
    ));
    out.push_str(&format!(
        "- completed: {}\n- failed: {}\n- timeout: {}\n- dropped: {}\n- prompt_required: {}\n",
        est.completed, est.failed, est.timeout, est.dropped, est.prompt_required
    ));
    out.push_str(&format!(
        "- overhead ms: mean={} p50={} p95={}\n",
        opt_f64(est.overhead_ms_mean),
        opt_u64(est.overhead_ms_p50),
        opt_u64(est.overhead_ms_p95)
    ));
    out.push_str(&format!(
        "- vs heuristic primary: same={} differs={} heuristic unavailable={}\n",
        est.same_as_heuristic, est.differs_from_heuristic, est.heuristic_unavailable
    ));
    out.push_str(&format!(
        "- same as observed primary: {}\n",
        est.same_as_primary
    ));
    out.push_str(&format!(
        "- quality observed: {} unknown: {} acceptance success: {}\n",
        est.quality_observed,
        est.quality_unknown,
        opt_f64(est.acceptance_success_rate)
    ));
    out.push_str(&format!("\nUnknown: {}\n", est.unknown_reason));
    if est.incomparable_reasons.is_empty() {
        out.push_str("\nIncomparable: (none)\n");
    } else {
        out.push_str("\nIncomparable reasons\n\n| reason | count |\n| --- | --- |\n");
        for (reason, count) in &est.incomparable_reasons {
            out.push_str(&format!("| {reason} | {count} |\n"));
        }
    }
    out
}

/// `celerisctl routing show` の本体（純粋関数。テストしやすいよう `run` から分離）。
/// ADR 2026-10-11-llm-source-harness-selection: LLM source（kind・model・account）、harness、
/// `[[llm_routes]]` の結合、harness ごとの既定候補、CoS の候補を宣言の順に見せる。
pub fn render_routing_table(config: &Config) -> String {
    let mut out = String::new();
    out.push_str("=== llm_sources ===\n");
    if config.llm_sources.is_empty() {
        out.push_str("(no llm_sources configured)\n");
    }
    for (id, s) in &config.llm_sources {
        out.push_str(&format!(
            "{id} ({}, api_format={}): models=[{}] accounts=[{}]\n",
            s.kind,
            s.api_format.as_deref().unwrap_or(&s.kind),
            s.models.join(", "),
            s.accounts.join(", ")
        ));
    }
    out.push_str("=== harnesses ===\n");
    if config.harness_declarations.is_empty() {
        out.push_str("(no harnesses declared)\n");
    }
    for (id, h) in &config.harness_declarations {
        let formats: Vec<&str> = h.api_formats.iter().map(String::as_str).collect();
        out.push_str(&format!(
            "{id} ({}): api_formats=[{}]\n",
            h.kind,
            formats.join(", ")
        ));
    }
    out.push_str("=== llm_routes ===\n");
    if config.llm_routes.is_empty() {
        out.push_str("(no llm_routes declared)\n");
    }
    for r in &config.llm_routes {
        out.push_str(&format!("{} -> {}/{}", r.harness, r.source, r.model));
        if let Some(via) = &r.via {
            out.push_str(&format!(" via {via}"));
        }
        out.push('\n');
    }
    out.push_str("=== route_defaults ===\n");
    for (harness, d) in &config.route_defaults {
        out.push_str(&format!("{harness}: {}\n", candidates_cell(&d.candidates)));
        for (class, c) in &d.classes {
            out.push_str(&format!(
                "{harness} class {class}: {}\n",
                candidates_cell(&c.candidates)
            ));
        }
    }
    out.push_str(&format!(
        "cos: {}\n",
        candidates_cell(&config.cos.route_candidates)
    ));
    out
}

fn candidates_cell(candidates: &[celeris::config::RouteCandidateConfig]) -> String {
    if candidates.is_empty() {
        return "(none)".to_string();
    }
    candidates
        .iter()
        .map(|c| format!("{}/{}", c.source, c.model))
        .collect::<Vec<_>>()
        .join(", ")
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
    fn shows_llm_sources_harnesses_and_routes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
[llm_sources.claude]
kind = "claude_oauth"
api_format = "anthropic_messages"
accounts = ["a1"]
models = ["claude-sonnet-5"]

[harnesses.claude_code]
kind = "claude_code"
api_formats = ["anthropic_messages"]
source_kinds = ["claude_oauth"]

[[llm_routes]]
harness = "claude_code"
source = "claude"
model = "claude-sonnet-5"

[route_defaults.claude_code]
candidates = [{ source = "claude", model = "claude-sonnet-5" }]
"#,
        );
        let config = Config::load(&path).unwrap();
        let table = render_routing_table(&config);
        assert!(table.contains(
            "claude (claude_oauth, api_format=anthropic_messages): models=[claude-sonnet-5] accounts=[a1]"
        ));
        assert!(table.contains("claude_code (claude_code): api_formats=[anthropic_messages]"));
        assert!(table.contains("claude_code -> claude/claude-sonnet-5\n"));
        assert!(table.contains("claude_code: claude/claude-sonnet-5\n"));
        assert!(table.contains("cos: (none)"));
    }

    #[test]
    fn show_prints_class_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            r#"
[llm_sources.claude]
kind = "claude_oauth"
api_format = "anthropic_messages"
accounts = ["a1"]
models = ["claude-sonnet-5-5", "claude-opus-5-5"]

[harnesses.claude_code]
kind = "claude_code"
api_formats = ["anthropic_messages"]
source_kinds = ["claude_oauth"]

[route_defaults.claude_code]
candidates = [{ source = "claude", model = "claude-sonnet-5-5" }]

[route_defaults.claude_code.classes.coding]
candidates = [{ source = "claude", model = "claude-opus-5-5" }]
"#,
        );
        let config = Config::load(&path).unwrap();
        let table = render_routing_table(&config);
        assert!(table.contains("claude_code: claude/claude-sonnet-5-5\n"));
        assert!(table.contains("claude_code class coding: claude/claude-opus-5-5\n"));
    }

    #[test]
    fn a_source_without_harnesses_or_routes_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            "[llm_sources.relay]\nkind = \"openai_compatible\"\nmodels = [\"m1\"]\n",
        );
        let config = Config::load(&path).unwrap();
        let table = render_routing_table(&config);
        assert!(table.contains(
            "relay (openai_compatible, api_format=openai_compatible): models=[m1] accounts=[]"
        ));
        assert!(table.contains("(no harnesses declared)"));
        assert!(table.contains("(no llm_routes declared)"));
    }
}
