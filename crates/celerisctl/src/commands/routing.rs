//! `celerisctl routing show`, versioned routing training export, and the selector stage/evidence
//! views with the human stage change (ADR 2026-10-11-llm-source-model-routing-evaluation D3).
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::error::CliError;
use crate::outln;
use celeris::config::Config;
use clap::Subcommand;
use task_core::{SqliteStore, TaskId};
use task_ops::routing_export::{self, ExportOptions, ExportText};
use task_ops::routing_stage::{self, HumanStageChange, RoutingEvidenceList, RoutingStageList};

#[derive(Subcommand, Debug)]
pub enum RoutingCommand {
    Show(RoutingShowArgs),
    /// Export recorded routing decisions as deterministic training JSONL.
    Export(RoutingExportArgs),
    /// genre×harness ごとの段階（shadow / enforce）を見る・人が変える（ADR 2026-10-11 D3）。
    #[command(subcommand)]
    Stage(StageCommand),
    /// 組ごとの最新版の根拠表（区間と D1 を満たすか）を見る。
    Evidence(StageFilterArgs),
    /// harness 残量切替の guardrail が止めた実行クラスを見る・人が再開する（ADR 2026-10-22 D2）。
    #[command(subcommand)]
    Balance(BalanceCommand),
}

#[derive(Subcommand, Debug)]
pub enum BalanceCommand {
    /// 止まっている実行クラスと理由。
    Trips(BalanceDbArgs),
    /// 止まっているクラスを再開する（人の操作。自動では再開しない）。
    Reset(BalanceResetArgs),
}

#[derive(clap::Args, Debug)]
pub struct BalanceDbArgs {
    /// Explicit database path.
    #[arg(long)]
    pub db: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub struct BalanceResetArgs {
    /// Explicit database path.
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// 実行クラス（coding | planner | review）。
    pub class: String,
}

#[derive(Subcommand, Debug)]
pub enum StageCommand {
    /// 段階の一覧（直近の routing_stage_changed つき）。
    List(StageFilterArgs),
    /// 人の段階変更（actor human）。enforce は D1 の根拠が立たなければ拒否する。
    Set(StageSetArgs),
}

#[derive(clap::Args, Debug)]
pub struct StageFilterArgs {
    /// Explicit read-only database path.
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long)]
    pub genre: Option<String>,
    #[arg(long)]
    pub harness: Option<String>,
    /// JSON（API と同じ形）で出す。
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args, Debug)]
pub struct StageSetArgs {
    /// Explicit database path.
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// routing_stage_changed を記録する task。
    #[arg(long)]
    pub task: String,
    #[arg(long)]
    pub genre: String,
    #[arg(long)]
    pub harness: String,
    #[arg(long, value_enum)]
    pub stage: StageName,
    #[arg(long)]
    pub reason: String,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "snake_case")]
pub enum StageName {
    Shadow,
    Enforce,
}

#[derive(clap::Args, Debug)]
pub struct RoutingExportArgs {
    /// Explicit read-only database path.
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// Output JSONL file.
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub until: Option<String>,
    /// Restrict rows to a recorded stage.
    #[arg(long, value_enum)]
    pub stage: Option<RoutingStage>,
    /// Include or omit the task brief.
    #[arg(long, value_enum, default_value_t = RoutingText::Brief)]
    pub text: RoutingText,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
#[value(rename_all = "snake_case")]
pub enum RoutingStage {
    Shadow,
    Enforce,
    Eval,
}

impl RoutingStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Enforce => "enforce",
            Self::Eval => "eval",
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
#[value(rename_all = "snake_case")]
pub enum RoutingText {
    #[default]
    Brief,
    None,
}

#[derive(clap::Args, Debug)]
pub struct RoutingShowArgs {
    #[arg(long)]
    pub config: PathBuf,
}

pub fn run(_db: Option<&Path>, command: RoutingCommand) -> Result<ExitCode, CliError> {
    match command {
        RoutingCommand::Export(args) => {
            let db = args.db.as_deref().ok_or_else(|| CliError::msg("routing export: --db <path> is required (the daemon DB is never opened by default)"))?;
            if !db.is_file() {
                return Err(CliError::msg(format!(
                    "routing export: no database at {}",
                    db.display()
                )));
            }
            let file = std::fs::File::create(&args.out)
                .map_err(|e| CliError::msg(format!("create {}: {e}", args.out.display())))?;
            let mut writer = BufWriter::new(file);
            let count = routing_export::export(
                db,
                &ExportOptions {
                    since: args.since.as_deref(),
                    until: args.until.as_deref(),
                    stage: args.stage.map(RoutingStage::as_str),
                    text: match args.text {
                        RoutingText::Brief => ExportText::Brief,
                        RoutingText::None => ExportText::None,
                    },
                },
                &mut writer,
            )
            .map_err(|e| CliError::msg(format!("routing export: {e}")))?;
            outln!("exported {count} rows to {}", args.out.display());
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Stage(StageCommand::List(args)) => {
            let db = explicit_db("routing stage list", args.db.as_deref())?;
            let list =
                routing_stage::list_stages(db, args.genre.as_deref(), args.harness.as_deref())
                    .map_err(|e| CliError::msg(format!("routing stage list: {e}")))?;
            if args.json {
                outln!("{}", to_json(&list)?);
            } else {
                outln!("{}", render_stages(&list).trim_end());
            }
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Stage(StageCommand::Set(args)) => {
            let db = explicit_db("routing stage set", args.db.as_deref())?;
            let task_id: TaskId = args.task.parse().map_err(|_| {
                CliError::msg(format!("routing stage set: invalid task id {}", args.task))
            })?;
            let store = SqliteStore::open(db)
                .map_err(|e| CliError::msg(format!("routing stage set: {e}")))?;
            let since = time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|e| CliError::msg(format!("clock format: {e}")))?;
            let view = routing_stage::set_stage_by_human(
                &store,
                db,
                &HumanStageChange {
                    task_id,
                    genre: args.genre,
                    harness_id: args.harness,
                    stage: match args.stage {
                        StageName::Shadow => task_core::route_select::RoutingStage::Shadow,
                        StageName::Enforce => task_core::route_select::RoutingStage::Enforce,
                    },
                    reason: args.reason,
                    actor: "human".into(),
                },
                &since,
            )
            .map_err(|e| CliError::msg(format!("routing stage set: {e}")))?;
            outln!(
                "{}",
                render_stages(&RoutingStageList { stages: vec![view] }).trim_end()
            );
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Balance(BalanceCommand::Trips(args)) => {
            let db = explicit_db("routing balance trips", args.db.as_deref())?;
            let store = SqliteStore::open(db)
                .map_err(|e| CliError::msg(format!("routing balance trips: {e}")))?;
            let trips = task_core::TaskStore::harness_balance_trips(&store)
                .map_err(|e| CliError::msg(format!("routing balance trips: {e}")))?;
            if trips.is_empty() {
                outln!("(no execution class is stopped)");
            }
            for (class, reason) in trips {
                outln!("{class}\t{reason}");
            }
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Balance(BalanceCommand::Reset(args)) => {
            let db = explicit_db("routing balance reset", args.db.as_deref())?;
            let store = SqliteStore::open(db)
                .map_err(|e| CliError::msg(format!("routing balance reset: {e}")))?;
            let was_stopped = task_core::TaskStore::harness_balance_reset(&store, &args.class)
                .map_err(|e| CliError::msg(format!("routing balance reset: {e}")))?;
            outln!(
                "{}",
                if was_stopped {
                    format!("{}: resumed", args.class)
                } else {
                    format!("{}: was not stopped", args.class)
                }
            );
            Ok(ExitCode::SUCCESS)
        }
        RoutingCommand::Evidence(args) => {
            let db = explicit_db("routing evidence", args.db.as_deref())?;
            let store = SqliteStore::open(db)
                .map_err(|e| CliError::msg(format!("routing evidence: {e}")))?;
            let list = routing_stage::list_evidence(
                &store,
                db,
                args.genre.as_deref(),
                args.harness.as_deref(),
            )
            .map_err(|e| CliError::msg(format!("routing evidence: {e}")))?;
            if args.json {
                outln!("{}", to_json(&list)?);
            } else {
                outln!("{}", render_evidence(&list).trim_end());
            }
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

fn explicit_db<'a>(what: &str, db: Option<&'a Path>) -> Result<&'a Path, CliError> {
    let db = db.ok_or_else(|| {
        CliError::msg(format!(
            "{what}: --db <path> is required (the daemon DB is never opened by default)"
        ))
    })?;
    if !db.is_file() {
        return Err(CliError::msg(format!(
            "{what}: no database at {}",
            db.display()
        )));
    }
    Ok(db)
}

fn to_json(value: &impl serde::Serialize) -> Result<String, CliError> {
    serde_json::to_string_pretty(value).map_err(|e| CliError::msg(format!("json: {e}")))
}

/// `routing stage list|set` の表（1 組 1 行）。
pub fn render_stages(list: &RoutingStageList) -> String {
    if list.stages.is_empty() {
        return "(no routing stages)\n".into();
    }
    let mut out = String::new();
    for s in &list.stages {
        out.push_str(&format!(
            "{}/{}: {} since={} evidence={} latest={}",
            s.genre,
            s.harness_id,
            s.stage,
            s.since.as_deref().unwrap_or("-"),
            s.evidence_version.as_deref().unwrap_or("-"),
            s.latest_evidence_version.as_deref().unwrap_or("-"),
        ));
        if let Some(c) = &s.last_change {
            out.push_str(&format!(
                " last={}->{} by {} at {} ({})",
                c.from.as_deref().unwrap_or("-"),
                c.to,
                c.actor,
                c.ts,
                c.reason
            ));
        }
        out.push('\n');
    }
    out
}

/// `routing evidence` の表（組ごとに見出し、行ごとに区間）。
pub fn render_evidence(list: &RoutingEvidenceList) -> String {
    if list.groups.is_empty() {
        return "(no routing evidence)\n".into();
    }
    let mut out = String::new();
    for g in &list.groups {
        out.push_str(&format!(
            "=== {}/{} ({}) ===\n",
            g.genre,
            g.harness_id,
            g.evidence_version.as_deref().unwrap_or("-")
        ));
        for r in &g.rows {
            out.push_str(&format!(
                "{}/{} harness={} n={} pass={} [{:.3}, {:.3}] wall={} usd={} timeout={:.3}{}\n",
                r.source_id,
                r.model_id,
                r.harness_version,
                r.n,
                r.pass,
                r.pass_lower,
                r.pass_upper,
                r.wall_p90_s
                    .map_or_else(|| "-".to_string(), |v| format!("{v:.1}s")),
                r.usd_per_pass
                    .map_or_else(|| "-".to_string(), |v| format!("{v:.3}")),
                r.timeout_rate,
                if r.qualifies { " qualifies" } else { "" }
            ));
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
