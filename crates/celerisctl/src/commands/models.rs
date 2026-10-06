//! `celerisctl models` — ADR 2026-10-06 D5: the model catalog through the daemon HTTP API.
//!
//! - `models list [--json]` — `GET /llm/models`（既定は表。`--json` で応答そのまま）。
//! - `models discover [--source S]` — `POST /llm/models/discover`（202 の要約を JSON で出す）。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::cron::{ApiConfig, request};
use crate::error::CliError;
use crate::outln;

const DEFAULT_CONFIG: &str = "~/.config/celeris/config.toml";

#[derive(Subcommand, Debug)]
pub enum ModelsCommand {
    /// List the discovered model catalog.
    List(ListArgs),
    /// Run model discovery now (all sources, or one with `--source`).
    Discover(DiscoverArgs),
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Print the API response as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct DiscoverArgs {
    /// claude-oauth | codex-oauth | opencode-go | openai-compatible:<id>
    #[arg(long)]
    pub source: Option<String>,
}

pub fn run(config_path: Option<PathBuf>, command: ModelsCommand) -> Result<ExitCode, CliError> {
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
            .read_token()
            .map_err(|e| CliError::msg(e.to_string()))?,
    };
    match command {
        ModelsCommand::List(args) => {
            let result = request(&api, "GET", "/llm/models", None)?;
            if args.json {
                print_json(&result)?;
            } else {
                for line in render_table(&result) {
                    outln!("{line}");
                }
            }
        }
        ModelsCommand::Discover(args) => {
            let body = match args.source {
                Some(source) => json!({ "source": source }),
                None => json!({}),
            };
            let result = request(&api, "POST", "/llm/models/discover", Some(body))?;
            print_json(&result)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_json(value: &Value) -> Result<(), CliError> {
    outln!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| CliError::msg(e.to_string()))?
    );
    Ok(())
}

/// `GET /llm/models` の応答を人が読む表にする（source・model・状態・上書き・routing）。
pub(crate) fn render_table(view: &Value) -> Vec<String> {
    let mut lines = vec![format!(
        "{:<28} {:<36} {:<12} {:<20} {}",
        "SOURCE", "MODEL", "STATE", "OVERRIDE", "ROUTING"
    )];
    let items = view["items"].as_array().cloned().unwrap_or_default();
    for item in &items {
        let s = |v: &Value| v.as_str().unwrap_or("-").to_string();
        let state = if item["available"].as_bool().unwrap_or(false) {
            "available"
        } else {
            "missing"
        };
        let ov = match item.get("override").filter(|o| o.is_object()) {
            Some(o) => {
                let mut parts = Vec::new();
                if o["disabled"].as_bool().unwrap_or(false) {
                    parts.push("disabled".to_string());
                }
                if let Some(t) = o["tier"].as_str() {
                    parts.push(format!("tier={t}"));
                }
                if let Some(a) = o["alias"].as_str() {
                    parts.push(format!("alias={a}"));
                }
                if parts.is_empty() {
                    "-".to_string()
                } else {
                    parts.join(",")
                }
            }
            None => "-".to_string(),
        };
        let routing = item["routing"]["deployments"]
            .as_array()
            .map(|d| {
                d.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "-".to_string());
        lines.push(format!(
            "{:<28} {:<36} {:<12} {:<20} {}",
            s(&item["source"]),
            s(&item["model_id"]),
            state,
            ov,
            routing
        ));
    }
    if items.is_empty() {
        lines.push("(no models discovered yet; run `celerisctl models discover`)".to_string());
    }
    for rec in view["last_discovery"].as_array().into_iter().flatten() {
        let ok = rec["ok"].as_bool().unwrap_or(false);
        lines.push(format!(
            "discovery {} at {}: {}",
            rec["source"].as_str().unwrap_or("-"),
            rec["at"].as_str().unwrap_or("-"),
            if ok {
                format!("ok ({} models)", rec["count"])
            } else {
                format!("FAILED: {}", rec["error"].as_str().unwrap_or("unknown"))
            }
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_shows_state_override_routing_and_discovery() {
        let view = json!({
            "items": [
                {"source": "opencode-go", "model_id": "glm-5", "available": true,
                 "override": {"disabled": true, "tier": "cheap", "alias": null, "note": null},
                 "routing": {"tiers": ["cheap"], "deployments": ["d1", "d2"]}},
                {"source": "opencode-go", "model_id": "old", "available": false,
                 "override": null, "routing": {"tiers": [], "deployments": []}}
            ],
            "last_discovery": [
                {"source": "opencode-go", "at": "t", "ok": false, "error": "HTTP 500", "count": 0}
            ]
        });
        let lines = render_table(&view);
        assert!(lines[0].starts_with("SOURCE"));
        assert!(lines[1].contains("glm-5") && lines[1].contains("available"));
        assert!(lines[1].contains("disabled,tier=cheap") && lines[1].ends_with("d1,d2"));
        assert!(lines[2].contains("missing") && lines[2].ends_with(" -"));
        assert!(lines[3].contains("FAILED: HTTP 500"));
    }

    #[test]
    fn empty_catalog_points_at_discover() {
        let lines = render_table(&json!({"items": [], "last_discovery": []}));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("celerisctl models discover"))
        );
    }
}
