//! `celerisctl routing show` and versioned routing training export.
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::error::CliError;
use crate::outln;
use celeris::config::Config;
use clap::Subcommand;
use task_ops::routing_export::{self, ExportOptions, ExportText};

#[derive(Subcommand, Debug)]
pub enum RoutingCommand {
    Show(RoutingShowArgs),
    /// Export recorded routing decisions as deterministic training JSONL.
    Export(RoutingExportArgs),
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
        RoutingCommand::Show(args) => {
            let config = Config::load(&args.config)
                .map_err(|e| CliError::msg(format!("config {}: {e}", args.config.display())))?;
            outln!("{}", render_routing_table(&config).trim_end());
            Ok(ExitCode::SUCCESS)
        }
    }
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
