//! Read the daemon's D5 report. Never open or migrate its database from the CLI.
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use serde::Deserialize;

use super::cron::{ApiConfig, request};
use crate::error::CliError;

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// API 接続設定。socket・runtime の点検は daemon 自身の設定で行う。
    #[arg(
        long,
        env = "CELERIS_CONFIG",
        default_value = "~/.config/celeris/config.toml"
    )]
    pub config: PathBuf,
    /// 明示 URL 用の bearer token file（秘密の値をコマンドラインに渡さない）。
    #[arg(long, env = "CELERIS_API_TOKEN_FILE")]
    pub token_file: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    status: String,
    check: String,
    detail: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    items: Vec<Item>,
}

pub fn run(args: DoctorArgs, api_url: Option<&str>) -> Result<ExitCode, CliError> {
    let result = fetch(&args, api_url);
    let (report, exit) = match result {
        Ok(report) => {
            let exit = if report.items.iter().any(|i| i.status == "NG") { 1 } else { 0 };
            (report, exit)
        }
        // Do not echo an HTTP response, token, or raw untrusted server error.
        Err(_) => (Report { items: vec![Item { status: "NG".into(), check: "daemon".into(), detail: "readiness API に接続できない; 修正: --config/--api-url・token file・daemon の稼働と版を確認".into() }] }, 2),
    };
    if args.json {
        let items: Vec<_> = report
            .items
            .iter()
            .map(|i| serde_json::json!({"status": i.status, "check": i.check, "detail": i.detail}))
            .collect();
        println!("{}", serde_json::json!({"items": items}));
    } else {
        for i in report.items {
            // Preserve one physical line per item even with a malformed API reply.
            let clean = |s: &str| {
                s.chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect::<String>()
            };
            println!("{:<4} {} {}", i.status, clean(&i.check), clean(&i.detail));
        }
    }
    Ok(ExitCode::from(exit))
}

fn fetch(args: &DoctorArgs, api_url: Option<&str>) -> Result<Report, CliError> {
    let url = api_url
        .map(str::to_owned)
        .or_else(|| std::env::var("CELERIS_API_URL").ok());
    let mut api = if let Some(url) = url {
        if !url.trim_end_matches('/').ends_with("/api/v1") {
            return Err(CliError::msg("API URL must end in /api/v1"));
        }
        ApiConfig {
            base_url: url.trim_end_matches('/').into(),
            token: None,
        }
    } else {
        let config =
            celeris::Config::load(&args.config).map_err(|_| CliError::msg("cannot load config"))?;
        ApiConfig {
            base_url: format!(
                "http://{}/api/v1",
                config
                    .api
                    .listen
                    .ok_or_else(|| CliError::msg("API listen missing"))?
            ),
            token: config
                .api
                .read_token()
                .map_err(|_| CliError::msg("cannot read API token"))?,
        }
    };
    if let Some(path) = &args.token_file {
        api.token = Some(
            std::fs::read_to_string(path)
                .map_err(|_| CliError::msg("cannot read API token"))?
                .trim()
                .to_owned(),
        );
    }
    if api
        .token
        .as_ref()
        .is_some_and(|t| t.is_empty() || t.chars().any(char::is_control))
    {
        return Err(CliError::msg("invalid token"));
    }
    let report: Report = serde_json::from_value(request(&api, "GET", "/browser/readiness", None)?)
        .map_err(|_| CliError::msg("invalid readiness report"))?;
    if report.items.is_empty()
        || report
            .items
            .iter()
            .any(|i| !matches!(i.status.as_str(), "OK" | "NG" | "WARN" | "SKIP"))
    {
        return Err(CliError::msg("invalid readiness status"));
    }
    Ok(report)
}
