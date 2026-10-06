//! HTTP transport for audited CoS operations. The API owns the allowlist and audit transaction.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use serde_json::{Value, json};
use task_core::TaskId;

use super::cron::{self, ApiConfig};
use crate::error::CliError;
use crate::outln;

pub(crate) const CREDENTIAL_ENV: &str = "CELERIS_COS_RUN_CREDENTIAL";

#[derive(Debug, Default)]
pub(crate) struct Options {
    pub reason: Option<String>,
    pub idempotency_key: Option<String>,
    pub expected_revision: Option<String>,
    pub api_url: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct ApiRequestArgs {
    /// Domain method, for example POST.
    pub method: String,
    /// Absolute domain API path, for example /api/v1/tasks/{id}/comments.
    pub path: String,
    /// JSON body to send to the domain operation.
    #[arg(long, default_value = "{}")]
    pub body: String,
}

pub(crate) fn run(args: ApiRequestArgs, options: &Options) -> Result<ExitCode, CliError> {
    let run_credential = credential()
        .ok_or_else(|| CliError::msg("api-request requires CELERIS_COS_RUN_CREDENTIAL"))?;
    let body: Value = serde_json::from_str(&args.body)
        .map_err(|e| CliError::msg(format!("invalid JSON body: {e}")))?;
    let api = api_config(options.api_url.as_deref())?;
    let result = send(
        &api,
        &args.method,
        &args.path,
        body,
        options,
        Some(&run_credential),
    )?;
    outln!("{result}");
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn credential() -> Option<String> {
    std::env::var(CREDENTIAL_ENV).ok().filter(|s| !s.is_empty())
}

pub(crate) fn api_config(api_url: Option<&str>) -> Result<ApiConfig, CliError> {
    if let Some(url) = api_url {
        if !url.trim_end_matches('/').ends_with("/api/v1") {
            return Err(CliError::msg("--api-url must end in /api/v1"));
        }
        return Ok(ApiConfig {
            base_url: url.trim_end_matches('/').to_string(),
            token: None,
        });
    }
    let path = std::env::var_os("CELERIS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~/.config/celeris/config.toml"));
    let config = celeris::Config::load(&path)
        .map_err(|e| CliError::msg(format!("config {}: {e}", path.display())))?;
    let listen = config
        .api
        .listen
        .ok_or_else(|| CliError::msg("[api] listen is not configured"))?;
    Ok(ApiConfig {
        base_url: format!("http://{listen}/api/v1"),
        token: if credential().is_some() {
            None
        } else {
            config
                .api
                .read_token()
                .map_err(|e| CliError::msg(e.to_string()))?
        },
    })
}

fn safe_header(value: &str, name: &str) -> Result<(), CliError> {
    if value.is_empty() || value.chars().any(|c| c.is_control()) {
        return Err(CliError::msg(format!("invalid {name}")));
    }
    Ok(())
}

/// Send a domain request. A run credential forces every mutation through `/cos/operations`.
pub(crate) fn send(
    api: &ApiConfig,
    method: &str,
    path: &str,
    body: Value,
    options: &Options,
    run_credential: Option<&str>,
) -> Result<Value, CliError> {
    let method = method.to_ascii_uppercase();
    if !matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return Err(CliError::msg(
            "CoS operations require a mutating HTTP method",
        ));
    }
    if !path.starts_with("/api/v1/") || path.starts_with("/api/v1/cos/") {
        return Err(CliError::msg(
            "request path must be a domain path under /api/v1/",
        ));
    }
    let relative = path.strip_prefix("/api/v1").expect("checked prefix");
    let Some(run_credential) = run_credential else {
        return cron::request(api, &method, relative, Some(body));
    };
    safe_header(run_credential, "CoS run credential")?;
    let reason = options
        .reason
        .clone()
        .or_else(|| std::env::var("CELERIS_COS_REASON").ok())
        .unwrap_or_default();
    if reason.trim().is_empty() {
        return Err(CliError::msg(
            "CoS operation needs --reason or CELERIS_COS_REASON",
        ));
    }
    let policy_version =
        std::env::var("CELERIS_COS_POLICY_VERSION").unwrap_or_else(|_| "1".to_string());
    if policy_version.trim().is_empty() {
        return Err(CliError::msg(
            "CELERIS_COS_POLICY_VERSION must not be empty",
        ));
    }
    let key = options
        .idempotency_key
        .clone()
        .unwrap_or_else(|| TaskId::new().to_string());
    if key.trim().is_empty() {
        return Err(CliError::msg("idempotency key must not be empty"));
    }
    let envelope = json!({
        "idempotency_key": key,
        "expected_revision": options.expected_revision,
        "reason": reason,
        "policy_version": policy_version,
        "request": {"method": method, "path": path, "body": body},
    });
    let mut cos_api = api.clone();
    cos_api.token = Some(run_credential.to_string());
    cron::request(&cos_api, "POST", "/cos/operations", Some(envelope))
}

#[cfg(test)]
#[path = "cos_ops_tests.rs"]
mod tests;
