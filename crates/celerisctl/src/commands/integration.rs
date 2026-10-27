//! `celerisctl integration` — stranded task branches and their durable integration state, through
//! the daemon HTTP API (`/api/v1/integrations`, `/api/v1/tasks/{id}/integrations/{repo}/…`).
//!
//! These routes are human decisions: they take the admin token from the config and refuse CoS run
//! credentials on the server side, so they are not registered CoS operations (`cos_mapped` in
//! `main.rs` leaves them unmapped and a run is refused).

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use task_core::TaskRepoIntegrationStatus;

use crate::commands::cron::{ApiConfig, request};
use crate::error::{self, CliError};
use crate::outln;

const DEFAULT_CONFIG: &str = "~/.config/celeris/config.toml";

#[derive(Subcommand, Debug, Clone)]
pub enum IntegrationCommand {
    /// Unintegrated / integrated / discarded / held branches of done tasks.
    List(ListArgs),
    /// Merge the task branch into the repository's default branch (without pushing).
    Merge(TargetArgs),
    /// Discard the stranded branch. `--reason` is required and recorded.
    Discard(ReasonTargetArgs),
    /// Hold the stranded branch for later. `--reason` is required and recorded.
    Hold(ReasonTargetArgs),
    /// Reopen a discarded or held branch so it can be integrated again.
    Reopen(TargetArgs),
}

#[derive(Args, Debug, Clone)]
pub struct ListArgs {
    /// Only this state: unintegrated | integrated | discarded | held.
    #[arg(long, value_parser = parse_status)]
    pub state: Option<TaskRepoIntegrationStatus>,
    /// Only the branches of this task.
    #[arg(long)]
    pub task: Option<String>,
    /// Print the API response as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug, Clone)]
pub struct TargetArgs {
    /// Task id.
    pub task: String,
    /// Repository name in the task's project.
    pub repo: String,
}

#[derive(Args, Debug, Clone)]
pub struct ReasonTargetArgs {
    /// Task id.
    pub task: String,
    /// Repository name in the task's project.
    pub repo: String,
    /// Why this branch is discarded or held (recorded in the event log).
    #[arg(long)]
    pub reason: String,
}

fn parse_status(raw: &str) -> Result<TaskRepoIntegrationStatus, String> {
    TaskRepoIntegrationStatus::parse(raw).ok_or_else(|| {
        format!("unknown integration state '{raw}' (unintegrated | integrated | discarded | held | not_applicable)")
    })
}

/// The daemon request (method, path under `/api/v1`, body) of an integration subcommand. Kept
/// separate from the HTTP call so tests can check it.
pub(crate) fn request_of(
    command: &IntegrationCommand,
) -> Result<(&'static str, String, Option<Value>), CliError> {
    Ok(match command {
        IntegrationCommand::List(args) => {
            let path = match (&args.task, args.state) {
                (Some(task), _) => format!("/tasks/{}/integrations", parse_id(task)?),
                // The task route has no `state` filter, so `--state` is applied to its items below.
                (None, Some(state)) => format!("/integrations?state={}", state.as_str()),
                (None, None) => "/integrations".to_string(),
            };
            ("GET", path, None)
        }
        IntegrationCommand::Merge(a) => (
            "POST",
            target_path(&a.task, &a.repo, "merge")?,
            Some(json!({})),
        ),
        IntegrationCommand::Discard(a) => (
            "POST",
            target_path(&a.task, &a.repo, "discard")?,
            Some(json!({"reason": a.reason})),
        ),
        IntegrationCommand::Hold(a) => (
            "POST",
            target_path(&a.task, &a.repo, "hold")?,
            Some(json!({"reason": a.reason})),
        ),
        IntegrationCommand::Reopen(a) => (
            "POST",
            target_path(&a.task, &a.repo, "reopen")?,
            Some(json!({})),
        ),
    })
}

fn parse_id(raw: &str) -> Result<String, CliError> {
    Ok(error::parse_task_id(raw)?.to_string())
}

fn target_path(task: &str, repo: &str, action: &str) -> Result<String, CliError> {
    Ok(format!(
        "/tasks/{}/integrations/{repo}/{action}",
        parse_id(task)?
    ))
}

pub fn run(
    config_path: Option<PathBuf>,
    command: IntegrationCommand,
) -> Result<ExitCode, CliError> {
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
    let (method, path, body) = request_of(&command)?;
    let result = request(&api, method, &path, body)?;
    match command {
        IntegrationCommand::List(args) => {
            let items = filter_items(&result, args.state);
            if args.json {
                outln!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"items": items}))
                        .map_err(|e| CliError::msg(e.to_string()))?
                );
            } else {
                outln!("{}", render_table(&items));
            }
        }
        _ => outln!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| CliError::msg(e.to_string()))?
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// The `items` of a list response whose `state` matches `state` (all items when `None`). The
/// daemon already filters `/integrations?state=`; this also covers the task route, which has no filter.
pub(crate) fn filter_items(
    response: &Value,
    state: Option<TaskRepoIntegrationStatus>,
) -> Vec<Value> {
    let wanted = state.map(TaskRepoIntegrationStatus::as_str);
    response
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| wanted.is_none_or(|w| item.get("state").and_then(Value::as_str) == Some(w)))
        .cloned()
        .collect()
}

/// One line per branch: `task repo-id state branch head reason`. The reason is `-` when absent.
pub(crate) fn render_table(items: &[Value]) -> String {
    if items.is_empty() {
        return "no integration rows".to_string();
    }
    let mut out = String::new();
    for item in items {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("-");
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            text("task_id"),
            text("repo_id"),
            text("state"),
            text("branch"),
            text("head_sha"),
            text("reason"),
        );
    }
    out.trim_end().to_string()
}

#[cfg(test)]
#[path = "integration_tests.rs"]
mod tests;
