//! `celerisctl script` — ADR 2026-11-02-ts-control-layer-boundary 付記 S2-5: TS 制御層の領域の mode を
//! daemon の HTTP API で読む・切り替える（DB は開かない）。
//!
//! - `script domains [--json]` — `GET /script/domains`（既定は表。`--json` で応答そのまま）。
//! - `script mode <domain> rust|compare --expected-version <n> [--generation <bundle_sha256>] [--reason …]` —
//!   `PUT /script/domains/{domain}/mode`。CoS の run では `/cos/operations` の `script_domain.mode_set` に包む。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Map, Value, json};

use super::cron::{ApiConfig, request};
use crate::error::CliError;
use crate::outln;

const DEFAULT_CONFIG: &str = "~/.config/celeris/config.toml";

#[derive(Subcommand, Debug, Clone)]
pub enum ScriptCommand {
    /// List each domain's mode, active generation and version.
    Domains(DomainsArgs),
    /// Switch a domain to `rust` or `compare` (compare-and-set on `--expected-version`).
    Mode(ModeArgs),
}

#[derive(Args, Debug, Clone)]
pub struct DomainsArgs {
    /// Print the API response as JSON.
    #[arg(long)]
    pub json: bool,
}

/// S2 で切り替え先にできる mode（`canary`・`ts` は人の承認が要るので CLI に無い）。
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeArg {
    Rust,
    Compare,
}

impl ModeArg {
    fn as_str(self) -> &'static str {
        match self {
            ModeArg::Rust => "rust",
            ModeArg::Compare => "compare",
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct ModeArgs {
    /// lane_rules | route_priority | cos_digest
    pub domain: String,
    pub mode: ModeArg,
    /// The `version` read from `script domains` (a stale value is refused with 409).
    #[arg(long)]
    pub expected_version: u64,
    /// `bundle_sha256` of the generation to compare against (required for `compare`).
    #[arg(long)]
    pub generation: Option<String>,
    /// Reason recorded in `script_domain_mode_changed`.
    #[arg(long = "mode-reason")]
    pub mode_reason: Option<String>,
}

pub fn run(config_path: Option<PathBuf>, command: ScriptCommand) -> Result<ExitCode, CliError> {
    let config_path = config_path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
    let config = celeris::config::Config::load(&config_path)
        .map_err(|e| CliError::msg(format!("config {}: {e}", config_path.display())))?;
    let listen = config
        .api
        .listen
        .ok_or_else(|| CliError::msg("[api] listen is not configured"))?;
    let api = ApiConfig {
        base_url: format!("http://{listen}/api/v1"),
        token: config
            .api
            .client_token()
            .map_err(|e| CliError::msg(e.to_string()))?,
    };
    match &command {
        ScriptCommand::Domains(args) => {
            let result = request(&api, "GET", "/script/domains", None)?;
            if args.json {
                print_json(&result)?;
            } else {
                for line in render_table(&result) {
                    outln!("{line}");
                }
            }
        }
        ScriptCommand::Mode(_) => {
            if let Some((method, path, body)) = request_of(&command) {
                let result = request(&api, method, &path, Some(body))?;
                print_json(&result)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The domain request (method, path under `/api/v1`, body) of the mutating `script mode`
/// (`None` for `domains`). The direct run and the CoS operation (`cos_mapped`) send the same request.
pub fn request_of(command: &ScriptCommand) -> Option<(&'static str, String, Value)> {
    match command {
        ScriptCommand::Domains(_) => None,
        ScriptCommand::Mode(args) => {
            let mut body = Map::new();
            body.insert("mode".into(), json!(args.mode.as_str()));
            body.insert("expected_version".into(), json!(args.expected_version));
            if let Some(generation) = &args.generation {
                body.insert("generation".into(), json!(generation));
            }
            if let Some(reason) = &args.mode_reason {
                body.insert("reason".into(), json!(reason));
            }
            Some((
                "PUT",
                format!("/script/domains/{}/mode", args.domain),
                Value::Object(body),
            ))
        }
    }
}

fn print_json(value: &Value) -> Result<(), CliError> {
    outln!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| CliError::msg(e.to_string()))?
    );
    Ok(())
}

/// `GET /script/domains` の応答を人が読む表にする（領域・mode・版・bundle の hash の先頭 12 桁・更新者）。
pub(crate) fn render_table(view: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{:<16} {:<8} {:>7} {:<12} {}",
        "DOMAIN", "MODE", "VERSION", "BUNDLE", "UPDATED_BY"
    )];
    for item in view["domains"].as_array().into_iter().flatten() {
        let s = |v: &Value| v.as_str().unwrap_or("-").to_string();
        let bundle: String = item["bundle_sha256"]
            .as_str()
            .map_or_else(|| "-".to_string(), |b| b.chars().take(12).collect());
        lines.push(format!(
            "{:<16} {:<8} {:>7} {:<12} {}",
            s(&item["domain"]),
            s(&item["mode"]),
            item["version"].as_u64().unwrap_or(0),
            bundle,
            s(&item["updated_by"]),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(domain: &str, mode: ModeArg, generation: Option<&str>) -> ScriptCommand {
        ScriptCommand::Mode(ModeArgs {
            domain: domain.into(),
            mode,
            expected_version: 4,
            generation: generation.map(str::to_string),
            mode_reason: None,
        })
    }

    #[test]
    fn script_domain_mode_request_carries_mode_version_and_generation() {
        let (method, path, body) =
            request_of(&mode("lane_rules", ModeArg::Compare, Some("ab12"))).expect("mutating");
        assert_eq!(
            (method, path.as_str()),
            ("PUT", "/script/domains/lane_rules/mode")
        );
        assert_eq!(
            body,
            json!({"mode": "compare", "expected_version": 4, "generation": "ab12"})
        );
        let (_, _, body) =
            request_of(&mode("route_priority", ModeArg::Rust, None)).expect("mutating");
        assert_eq!(body, json!({"mode": "rust", "expected_version": 4}));
        assert!(request_of(&ScriptCommand::Domains(DomainsArgs { json: false })).is_none());
    }

    #[test]
    fn script_domain_table_shows_mode_version_and_short_bundle() {
        let view = json!({"domains": [
            {"domain": "lane_rules", "mode": "compare", "generation_id": "g1",
             "bundle_sha256": "0123456789abcdef0123", "version": 3,
             "updated_at": "t", "updated_by": "human:web"},
            {"domain": "route_priority", "mode": "rust", "generation_id": null,
             "bundle_sha256": null, "version": 0, "updated_at": "t", "updated_by": "migration"}
        ]});
        let lines = render_table(&view);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("lane_rules"));
        assert!(lines[1].contains("compare"));
        assert!(lines[1].contains("0123456789ab "));
        assert!(!lines[1].contains("0123456789abc"));
        assert!(lines[2].contains("route_priority"));
        assert!(lines[2].contains(" - "));
    }
}
