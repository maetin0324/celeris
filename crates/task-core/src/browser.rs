//! Browser execution is a capability granted by an administrator's profile, never an agent genre.
//! No credentials, browser storage, page content, or process I/O belong in these contracts.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::TaskId;

pub const BROWSER_SKILL: &str = "browser-enabled";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserCapability {
    /// Exact hosts (or `*.example.com`) passed to agent-browser's built-in domain policy.
    pub allowed_domains: Vec<String>,
    /// Administrator-operated authenticated HTTPS reverse proxy to the substrate dashboard.
    /// This is not a CDP endpoint or a bearer-token URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_view_url: Option<String>,
}

impl BrowserCapability {
    pub fn validate(&self) -> Result<(), String> {
        if self.allowed_domains.is_empty()
            || self
                .allowed_domains
                .iter()
                .any(|host| !valid_host(host.strip_prefix("*.").unwrap_or(host)))
        {
            return Err(
                "browser.allowed_domains must contain explicit hosts (optional *. prefix)".into(),
            );
        }
        if self
            .live_view_url
            .as_deref()
            .is_some_and(|url| !valid_live_view_url(url))
        {
            return Err("browser.live_view_url must be an HTTPS dashboard URL without credentials, query, or fragment".into());
        }
        Ok(())
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

/// Deliberately narrow URL grammar: DNS/IPv4 HTTPS origins and simple proxy paths.
/// Reject encodings and userinfo rather than accepting alternate interpretations by browsers.
pub fn valid_live_view_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = if let Some((host, port)) = authority.split_once(':') {
        if !port.bytes().all(|c| c.is_ascii_digit())
            || !matches!(port.parse::<u16>(), Ok(1..=65535))
        {
            return false;
        }
        host
    } else {
        authority
    };
    valid_host(host)
        && path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'_' | b'.' | b'~'))
}

pub fn requests_browser(skills: &[String]) -> bool {
    skills.iter().any(|skill| skill == BROWSER_SKILL)
}

/// Pin capability-bearing tasks to a supported adapter: never fall back to a backend that loses it.
pub fn browser_adapter(explicit: Option<&str>) -> Result<&str, String> {
    match explicit {
        None => Ok("acp"),
        Some(adapter @ ("acp" | "claude-code")) => Ok(adapter),
        Some(_) => Err("browser capability requires acp (OpenCode) or claude-code".into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BrowserRunState {
    Running,
    WaitingForAuth,
    WaitingForApproval,
    WaitingForHuman,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRun {
    pub task_id: TaskId,
    pub run_id: String,
    pub session_id: String,
    pub state: BrowserRunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_view_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_credential_urls_and_domain_policy_bypasses() {
        for url in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/?token=secret",
            "https://example.com/#secret",
            "https://example.com\\@evil.com",
            "https://example.com/%2f%2fevil.com",
            "javascript:alert(1)",
            "https://example.com:0",
            "https://example.com:",
        ] {
            assert!(!valid_live_view_url(url), "{url}");
        }
        assert!(valid_live_view_url(
            "https://browser.example.com:8443/live/session-123"
        ));
        for host in [
            "*",
            "",
            "example.com/path",
            "example.com,evil.com",
            "example.com:443",
            "*.com.*",
            "-bad.com",
        ] {
            assert!(
                BrowserCapability {
                    allowed_domains: vec![host.into()],
                    live_view_url: None
                }
                .validate()
                .is_err(),
                "{host}"
            );
        }
        assert!(
            BrowserCapability {
                allowed_domains: vec!["example.com".into(), "*.example.org".into()],
                live_view_url: None
            }
            .validate()
            .is_ok()
        );
        assert!(BrowserCapability::default().validate().is_err());
    }

    #[test]
    fn adapter_selection_is_explicit_and_does_not_drop_browser_capability() {
        assert_eq!(browser_adapter(None).unwrap(), "acp");
        assert_eq!(browser_adapter(Some("claude-code")).unwrap(), "claude-code");
        for adapter in ["codex", "aider", "mini-swe-agent"] {
            assert!(browser_adapter(Some(adapter)).is_err());
        }
    }

    #[test]
    fn state_schema_is_explicit_and_credentials_are_not_accepted() {
        assert_eq!(
            serde_json::to_string(&BrowserRunState::WaitingForAuth).unwrap(),
            "\"WAITING_FOR_AUTH\""
        );
        assert!(
            serde_json::from_str::<BrowserCapability>(
                r#"{"allowed_domains":["example.com"],"password":"secret"}"#
            )
            .is_err()
        );
    }
}
