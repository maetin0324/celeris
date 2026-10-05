//! Turn the administrator grant and task policy into agent-browser 0.38.1 inputs (ADR-0080 D1).
//! Everything is decided before any substrate or harness process starts.
use std::path::Path;

use sha2::{Digest, Sha256};
use task_core::{
    BrowserAction, BrowserCapability, BrowserPolicyBinding, BrowserPolicyError, BrowserTaskPolicy,
    EffectiveBrowserPolicy,
};

/// Business actions the Celeris shim implements for agent-browser 0.38.1. `credential_use`
/// runs only in a supervisor segment, never through the harness shim.
pub const SUPPORTED_ACTIONS: [BrowserAction; 8] = BrowserAction::ALL;

#[derive(Debug, Clone)]
pub struct PreparedBrowserPolicy {
    pub effective: EffectiveBrowserPolicy,
    /// Exact bytes of `policy.json`.
    pub action_policy: Vec<u8>,
    /// Hex sha256 of `action_policy`; the shim refuses to run on mismatch.
    pub action_policy_sha256: String,
    pub binding: BrowserPolicyBinding,
}

pub fn prepare(
    grant: &BrowserCapability,
    task: Option<&BrowserTaskPolicy>,
    backend_version: &str,
) -> Result<PreparedBrowserPolicy, BrowserPolicyError> {
    let effective =
        EffectiveBrowserPolicy::derive(grant, task, &SUPPORTED_ACTIONS, backend_version)?;
    let file = effective.harness_action_policy()?;
    if file.default != "deny" || file.allow.is_empty() {
        return Err(BrowserPolicyError::EmptyActions);
    }
    let action_policy = serde_json::to_vec(&file).map_err(|_| BrowserPolicyError::InvalidPolicy)?;
    let action_policy_sha256 = format!("{:x}", Sha256::digest(&action_policy));
    let binding = effective.binding();
    Ok(PreparedBrowserPolicy {
        effective,
        action_policy,
        action_policy_sha256,
        binding,
    })
}

impl PreparedBrowserPolicy {
    pub fn write(&self, runtime: &Path) -> std::io::Result<()> {
        crate::browser::write_private(&runtime.join("policy.json"), &self.action_policy)
    }

    pub fn allowed_domains(&self) -> &[String] {
        &self.effective.allowed_domains
    }
}

/// Whether a navigable URL's origin (scheme, host, port) is covered by an effective allowed
/// origin. Credentials in the URL and wildcard hosts are never navigable.
pub fn url_origin_allowed(url: &str, domains: &[String]) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = url.host_str().filter(|h| !h.starts_with("*.")) else {
        return false;
    };
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let origin = match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    domains
        .iter()
        .any(|d| task_core::browser::origin_covers(d, &origin))
}

/// `(host pattern, port)` of an allowed origin, for host-level filters (egress, agent-browser
/// `--allowed-domains`). The scheme and port are enforced by the origin checks above them.
pub fn origin_host_port(origin: &str) -> Option<(String, u16)> {
    let canonical = task_core::browser::parse_allowed_origin(origin)
        .ok()?
        .canonical();
    let (scheme, authority) = canonical.split_once("://")?;
    let default = if scheme == "https" { 443 } else { 80 };
    match authority.rsplit_once(':') {
        Some((host, port)) if !authority.ends_with(']') => {
            Some((host.to_string(), port.parse().ok()?))
        }
        _ => Some((authority.to_string(), default)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::BrowserDomainMode;

    fn policy(actions: &[BrowserAction]) -> BrowserTaskPolicy {
        BrowserTaskPolicy {
            policy_id: "p".into(),
            revision: 3,
            domain_mode: BrowserDomainMode::CommonHosts,
            navigation_origins: vec![],
            network_domains: vec!["https://example.com".into()],
            allowed_actions: actions.to_vec(),
            approval_actions: vec![],
            credential_policy_ids: vec![],
            artifact_policy_id: None,
        }
    }

    #[test]
    fn prepared_file_is_hash_bound_nonempty_default_deny() {
        let grant = BrowserCapability {
            allowed_domains: vec!["https://example.com".into()],
            ..Default::default()
        };
        let prepared =
            prepare(&grant, Some(&policy(&[BrowserAction::Navigate])), "0.38.1").unwrap();
        assert_eq!(
            prepared.action_policy,
            br#"{"default":"deny","allow":["close","launch","navigate"]}"#
        );
        assert_eq!(
            prepared.action_policy_sha256,
            format!("{:x}", Sha256::digest(&prepared.action_policy))
        );
        assert_eq!(prepared.binding.revision, 3);
        assert_eq!(prepared.allowed_domains(), ["https://example.com"]);
        assert_eq!(
            prepare(&grant, Some(&policy(&[])), "0.38.1").unwrap_err(),
            BrowserPolicyError::EmptyActions
        );
        assert_eq!(
            prepare(&grant, None, "0.38.1").unwrap_err(),
            BrowserPolicyError::BrowserPolicyRequired
        );
    }

    #[test]
    fn url_and_egress_follow_allowed_origin_scheme_host_port() {
        let domains = vec![
            "https://*.example.com".to_string(),
            "http://127.0.0.1:3000".to_string(),
        ];
        for (url, ok) in [
            ("https://a.example.com/x", true),
            ("https://example.com/", false),
            ("http://a.example.com/", false),
            ("https://a.example.com:8443/", false),
            ("http://127.0.0.1:3000/", true),
            ("http://127.0.0.1:3001/", false),
            ("https://user@a.example.com/", false),
        ] {
            assert_eq!(url_origin_allowed(url, &domains), ok, "{url}");
        }
        assert_eq!(
            origin_host_port("https://*.example.com"),
            Some(("*.example.com".into(), 443))
        );
        assert_eq!(
            origin_host_port("http://127.0.0.1:3000"),
            Some(("127.0.0.1".into(), 3000))
        );
        assert_eq!(origin_host_port("http://[::1]"), Some(("[::1]".into(), 80)));
    }
}
