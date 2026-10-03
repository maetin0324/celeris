//! `celerisctl cron` — manage scheduled jobs through the daemon HTTP API.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};

use crate::error::CliError;
use crate::outln;

const DEFAULT_CONFIG: &str = "~/.config/celeris/config.toml";

#[derive(Subcommand, Debug)]
pub enum CronCommand {
    List,
    Show(KeyArgs),
    Create(CreateArgs),
    Update(UpdateArgs),
    Pause(KeyArgs),
    Resume(KeyArgs),
    Run(KeyArgs),
    History(HistoryArgs),
}

#[derive(Args, Debug)]
pub struct KeyArgs {
    pub id: String,
}

#[derive(Args, Debug)]
pub struct HistoryArgs {
    pub id: String,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub schedule: String,
    #[arg(long)]
    pub timezone: String,
    #[arg(long, default_value = "skip")]
    pub overlap: String,
    #[arg(long, default_value = "latest")]
    pub catch_up: String,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub enabled: bool,
    /// JSON object containing the task template.
    #[arg(long)]
    pub template: String,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    pub id: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub schedule: Option<String>,
    #[arg(long)]
    pub timezone: Option<String>,
    #[arg(long)]
    pub overlap: Option<String>,
    #[arg(long)]
    pub catch_up: Option<String>,
    /// JSON object replacing the task template.
    #[arg(long)]
    pub template: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub base_url: String,
    pub token: Option<String>,
}

pub fn run(config_path: Option<PathBuf>, command: CronCommand) -> Result<ExitCode, CliError> {
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
    let (method, path, body) = match command {
        CronCommand::List => ("GET", "/cron-jobs".to_string(), None),
        CronCommand::Show(a) => ("GET", format!("/cron-jobs/{}", a.id), None),
        CronCommand::Create(a) => (
            "POST",
            "/cron-jobs".into(),
            Some(
                json!({"name":a.name,"schedule":a.schedule,"timezone":a.timezone,"overlap":a.overlap,"catch_up":a.catch_up,"enabled":a.enabled,"template":parse_object(&a.template)?}),
            ),
        ),
        CronCommand::Update(a) => {
            let mut map = Map::new();
            for (k, v) in [
                ("name", a.name),
                ("schedule", a.schedule),
                ("timezone", a.timezone),
                ("overlap", a.overlap),
                ("catch_up", a.catch_up),
            ] {
                if let Some(v) = v {
                    map.insert(k.into(), Value::String(v));
                }
            }
            if let Some(v) = a.template {
                map.insert("template".into(), parse_object(&v)?);
            }
            (
                "PATCH",
                format!("/cron-jobs/{}", a.id),
                Some(Value::Object(map)),
            )
        }
        CronCommand::Pause(a) => (
            "POST",
            format!("/cron-jobs/{}/pause", a.id),
            Some(json!({})),
        ),
        CronCommand::Resume(a) => (
            "POST",
            format!("/cron-jobs/{}/resume", a.id),
            Some(json!({})),
        ),
        CronCommand::Run(a) => ("POST", format!("/cron-jobs/{}/run", a.id), Some(json!({}))),
        CronCommand::History(a) => (
            "GET",
            format!("/cron-jobs/{}/runs?limit={}", a.id, a.limit),
            None,
        ),
    };
    let result = request(&api, method, &path, body)?;
    outln!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|e| CliError::msg(e.to_string()))?
    );
    Ok(ExitCode::SUCCESS)
}

fn parse_object(raw: &str) -> Result<Value, CliError> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| CliError::msg(format!("invalid JSON: {e}")))?;
    if !value.is_object() {
        return Err(CliError::msg("expected a JSON object"));
    }
    Ok(value)
}

fn request(
    api: &ApiConfig,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, CliError> {
    let rest = api
        .base_url
        .strip_prefix("http://")
        .ok_or_else(|| CliError::msg("cron API requires an http:// endpoint"))?;
    let (authority, base_path) = rest
        .split_once('/')
        .map_or((rest, ""), |(host, path)| (host, path));
    let mut stream = TcpStream::connect(authority)
        .map_err(|e| CliError::msg(format!("cron API connection failed: {e}")))?;
    let payload = body
        .map(|v| serde_json::to_vec(&v).map_err(|e| CliError::msg(e.to_string())))
        .transpose()?
        .unwrap_or_default();
    let base_path = base_path.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    write!(stream, "{method} /{base_path}{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: application/json\r\n").map_err(|e| CliError::msg(e.to_string()))?;
    if !payload.is_empty() {
        write!(
            stream,
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            payload.len()
        )
        .map_err(|e| CliError::msg(e.to_string()))?;
    }
    if let Some(token) = &api.token {
        write!(stream, "Authorization: Bearer {token}\r\n")
            .map_err(|e| CliError::msg(e.to_string()))?;
    }
    stream
        .write_all(b"\r\n")
        .and_then(|_| stream.write_all(&payload))
        .map_err(|e| CliError::msg(e.to_string()))?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|e| CliError::msg(e.to_string()))?;
    let response = String::from_utf8_lossy(&bytes);
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| CliError::msg("invalid cron API response"))?;
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| CliError::msg("invalid cron API status"))?;
    if !(200..300).contains(&status) {
        return Err(CliError::msg(format!(
            "cron API returned HTTP {status}: {body}"
        )));
    }

    if body.is_empty() {
        Ok(json!({"ok":true}))
    } else {
        serde_json::from_str(body)
            .map_err(|e| CliError::msg(format!("invalid cron API response: {e}")))
    }
}

#[cfg(test)]
#[path = "cron_tests.rs"]
mod tests;
