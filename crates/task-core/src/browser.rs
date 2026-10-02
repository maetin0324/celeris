//! Browser execution is a capability granted by an administrator's profile, never an agent genre.
//! No credentials, browser storage, page content, or process I/O belong in these contracts.
use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::TaskId;

pub const BROWSER_SKILL: &str = "browser-enabled";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserCapability {
    /// Exact hosts (or `*.example.com`) passed to agent-browser's built-in domain policy.
    pub allowed_domains: Vec<String>,
    /// Business actions the administrator grants (ADR-0080 D1). Absent means the Phase 1
    /// set; `credential_use` is never implied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_actions: Option<Vec<BrowserAction>>,
    /// Credential policies a task may reference. Absent/empty means no credential use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_policy_ids: Vec<String>,
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
        Some(adapter @ ("acp" | "claude-code" | "browser-specialist")) => Ok(adapter),
        Some(_) => Err("browser capability requires acp, claude-code or browser-specialist".into()),
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
    /// The effective policy this run was launched with (ADR-0080 D1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<BrowserPolicyBinding>,
}

/// Task-level browser operation vocabulary (ADR-0080 D1). Unknown names are schema errors:
/// no aliases, categories or pass-through of upstream names.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    Navigate,
    Click,
    Snapshot,
    Extract,
    Screenshot,
    Download,
    Scroll,
    CredentialUse,
}

impl BrowserAction {
    pub const ALL: [BrowserAction; 8] = [
        Self::Navigate,
        Self::Click,
        Self::Snapshot,
        Self::Extract,
        Self::Screenshot,
        Self::Download,
        Self::Scroll,
        Self::CredentialUse,
    ];
    /// The grant implied by a Phase 1 capability without `allowed_actions`.
    pub const PHASE1: [BrowserAction; 7] = [
        Self::Navigate,
        Self::Click,
        Self::Snapshot,
        Self::Extract,
        Self::Screenshot,
        Self::Download,
        Self::Scroll,
    ];
    /// Phase 2 treats every click/download and credential use as high risk (ADR-0080 D5).
    pub const ALWAYS_APPROVED: [BrowserAction; 3] =
        [Self::Click, Self::Download, Self::CredentialUse];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::Click => "click",
            Self::Snapshot => "snapshot",
            Self::Extract => "extract",
            Self::Screenshot => "screenshot",
            Self::Download => "download",
            Self::Scroll => "scroll",
            Self::CredentialUse => "credential_use",
        }
    }

    pub fn parse(name: &str) -> Result<Self, BrowserPolicyError> {
        Self::ALL
            .into_iter()
            .find(|a| a.as_str() == name)
            .ok_or(BrowserPolicyError::UnknownAction)
    }

    /// agent-browser 0.38.1 internal action names (policy.rs compares these, not categories).
    pub fn upstream_actions(self) -> &'static [&'static str] {
        match self {
            Self::Navigate => &["navigate"],
            Self::Click => &["click"],
            Self::Snapshot => &["snapshot"],
            Self::Extract => &["gettext"],
            Self::Screenshot => &["screenshot"],
            Self::Download => &["download"],
            Self::Scroll => &["scroll"],
            Self::CredentialUse => &["auth_login", CREDENTIAL_PLUGIN_ACTION],
        }
    }
}

pub const CREDENTIAL_PLUGIN_ACTION: &str = "plugin:celeris-credential:credential.read";
/// Lifecycle actions added only when a business action exists; not task permissions.
pub const LIFECYCLE_UPSTREAM_ACTIONS: [&str; 2] = ["close", "launch"];
/// Every upstream name a general harness policy may contain. The shim's allowlist must
/// equal this (checked by scripts/tests/test_browser_cli.py).
// harness-upstream-actions:begin
pub const HARNESS_UPSTREAM_ACTIONS: [&str; 9] = [
    "click",
    "close",
    "download",
    "gettext",
    "launch",
    "navigate",
    "screenshot",
    "scroll",
    "snapshot",
];
// harness-upstream-actions:end

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDomainMode {
    CommonHosts,
    SeparateOrigins,
}

/// Per-task browser restriction (ADR-0080 D1). It can only narrow the administrator grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserTaskPolicy {
    pub policy_id: String,
    pub revision: u64,
    pub domain_mode: BrowserDomainMode,
    #[serde(default)]
    pub navigation_origins: Vec<String>,
    pub network_domains: Vec<String>,
    pub allowed_actions: Vec<BrowserAction>,
    #[serde(default)]
    pub approval_actions: Vec<BrowserAction>,
    #[serde(default)]
    pub credential_policy_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_policy_id: Option<String>,
}

/// Fixed failure codes. Never carry page text, URLs or input values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPolicyError {
    BrowserPolicyRequired,
    InvalidPolicy,
    UnknownAction,
    InvalidDomain,
    InvalidGrant,
    ApprovalNotAllowed,
    CredentialPolicyNotGranted,
    UnsupportedOriginSeparation,
    EmptyActions,
    EmptyDomains,
    PolicyExpansion,
}

impl BrowserPolicyError {
    pub fn code(self) -> &'static str {
        match self {
            Self::BrowserPolicyRequired => "browser_policy_required",
            Self::InvalidPolicy => "invalid_browser_policy",
            Self::UnknownAction => "unknown_browser_action",
            Self::InvalidDomain => "invalid_browser_domain",
            Self::InvalidGrant => "invalid_browser_grant",
            Self::ApprovalNotAllowed => "approval_action_not_allowed",
            Self::CredentialPolicyNotGranted => "credential_policy_not_granted",
            Self::UnsupportedOriginSeparation => "unsupported_origin_separation",
            Self::EmptyActions => "empty_browser_actions",
            Self::EmptyDomains => "empty_browser_domains",
            Self::PolicyExpansion => "browser_policy_expansion",
        }
    }
}

impl std::fmt::Display for BrowserPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for BrowserPolicyError {}

impl BrowserTaskPolicy {
    /// Strict parse for create/edit APIs and run contexts: unknown fields/actions are errors.
    pub fn from_json(text: &str) -> Result<Self, BrowserPolicyError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|_| BrowserPolicyError::InvalidPolicy)?;
        if let Some(actions) = value.get("allowed_actions").and_then(|v| v.as_array()) {
            for name in actions.iter().chain(
                value
                    .get("approval_actions")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten(),
            ) {
                BrowserAction::parse(name.as_str().ok_or(BrowserPolicyError::InvalidPolicy)?)?;
            }
        }
        let policy: Self =
            serde_json::from_value(value).map_err(|_| BrowserPolicyError::InvalidPolicy)?;
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), BrowserPolicyError> {
        if !valid_identifier(&self.policy_id)
            || self
                .artifact_policy_id
                .as_deref()
                .is_some_and(|id| !valid_identifier(id))
            || self
                .credential_policy_ids
                .iter()
                .any(|id| !valid_identifier(id))
        {
            return Err(BrowserPolicyError::InvalidPolicy);
        }
        for domain in &self.network_domains {
            normalize_host_pattern(domain)?;
        }
        for origin in &self.navigation_origins {
            normalize_https_origin(origin).ok_or(BrowserPolicyError::InvalidDomain)?;
        }
        if self.domain_mode == BrowserDomainMode::SeparateOrigins
            || !self.navigation_origins.is_empty()
        {
            return Err(BrowserPolicyError::UnsupportedOriginSeparation);
        }
        if self
            .approval_actions
            .iter()
            .any(|a| !self.allowed_actions.contains(a))
        {
            return Err(BrowserPolicyError::ApprovalNotAllowed);
        }
        Ok(())
    }

    /// Model/page-originated edits may only narrow the policy in force. Grant, credential
    /// binding and approval removal stay on administrator paths (ADR-0080 D1).
    pub fn check_narrowing(
        &self,
        current: &EffectiveBrowserPolicy,
    ) -> Result<(), BrowserPolicyError> {
        self.validate()?;
        let domains = self
            .network_domains
            .iter()
            .map(|d| normalize_host_pattern(d))
            .collect::<Result<Vec<_>, _>>()?;
        let within_domains = domains
            .iter()
            .all(|d| current.allowed_domains.iter().any(|c| host_covers(c, d)));
        let actions: BTreeSet<_> = self.allowed_actions.iter().copied().collect();
        let within_actions = actions.is_subset(&current.actions);
        let credentials: BTreeSet<_> = self.credential_policy_ids.iter().cloned().collect();
        let within_credentials = credentials.is_subset(&current.credential_policy_ids);
        let keeps_approvals = current
            .approval_actions
            .iter()
            .filter(|a| actions.contains(a))
            .all(|a| {
                self.approval_actions.contains(a) || BrowserAction::ALWAYS_APPROVED.contains(a)
            });
        if self.policy_id != current.policy_id
            || self.revision <= current.task_revision
            || !within_domains
            || !within_actions
            || !within_credentials
            || !keeps_approvals
            || self.artifact_policy_id != current.artifact_policy_id
        {
            return Err(BrowserPolicyError::PolicyExpansion);
        }
        Ok(())
    }
}

/// What the run is bound to: approvals, waits and leases compare this hash (ADR-0080 D1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserPolicyBinding {
    pub policy_id: String,
    pub revision: u64,
    /// `sha256:<hex>` of the canonical effective policy.
    pub hash: String,
}

/// admin grant ∩ task policy ∩ backend-supported actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveBrowserPolicy {
    pub policy_id: String,
    pub task_revision: u64,
    pub actions: BTreeSet<BrowserAction>,
    pub approval_actions: BTreeSet<BrowserAction>,
    /// Sorted, normalized, redundancy-free host patterns for `--allowed-domains`.
    pub allowed_domains: Vec<String>,
    pub credential_policy_ids: BTreeSet<String>,
    pub artifact_policy_id: Option<String>,
    pub backend_version: String,
}

/// agent-browser action policy file. Always non-empty `allow` with `default: deny`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBrowserActionPolicy {
    pub default: String,
    pub allow: Vec<String>,
}

impl EffectiveBrowserPolicy {
    pub fn derive(
        grant: &BrowserCapability,
        task: Option<&BrowserTaskPolicy>,
        supported: &[BrowserAction],
        backend_version: &str,
    ) -> Result<Self, BrowserPolicyError> {
        let task = task.ok_or(BrowserPolicyError::BrowserPolicyRequired)?;
        grant
            .validate()
            .map_err(|_| BrowserPolicyError::InvalidGrant)?;
        if grant
            .credential_policy_ids
            .iter()
            .any(|id| !valid_identifier(id))
        {
            return Err(BrowserPolicyError::InvalidGrant);
        }
        task.validate()?;
        if task
            .credential_policy_ids
            .iter()
            .any(|id| !grant.credential_policy_ids.contains(id))
        {
            return Err(BrowserPolicyError::CredentialPolicyNotGranted);
        }
        let granted: BTreeSet<BrowserAction> = match &grant.allowed_actions {
            Some(actions) => actions.iter().copied().collect(),
            None => BrowserAction::PHASE1.into_iter().collect(),
        };
        let actions: BTreeSet<BrowserAction> = task
            .allowed_actions
            .iter()
            .copied()
            .filter(|a| granted.contains(a) && supported.contains(a))
            .collect();
        if actions.iter().all(|a| *a == BrowserAction::CredentialUse) {
            return Err(BrowserPolicyError::EmptyActions);
        }
        let approval_actions = task
            .approval_actions
            .iter()
            .copied()
            .chain(BrowserAction::ALWAYS_APPROVED)
            .filter(|a| actions.contains(a))
            .collect();
        let grant_hosts = grant
            .allowed_domains
            .iter()
            .map(|d| normalize_host_pattern(d))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| BrowserPolicyError::InvalidGrant)?;
        let mut domains = Vec::new();
        for requested in &task.network_domains {
            let requested = normalize_host_pattern(requested)?;
            domains.extend(
                grant_hosts
                    .iter()
                    .filter_map(|g| intersect_hosts(g, &requested)),
            );
        }
        let allowed_domains = minimize_hosts(domains);
        if allowed_domains.is_empty() {
            return Err(BrowserPolicyError::EmptyDomains);
        }
        let credential_policy_ids = if actions.contains(&BrowserAction::CredentialUse) {
            task.credential_policy_ids.iter().cloned().collect()
        } else {
            BTreeSet::new()
        };
        Ok(Self {
            policy_id: task.policy_id.clone(),
            task_revision: task.revision,
            actions,
            approval_actions,
            allowed_domains,
            credential_policy_ids,
            artifact_policy_id: task.artifact_policy_id.clone(),
            backend_version: backend_version.into(),
        })
    }

    /// Policy for the general harness: credential actions are never included here; the
    /// supervisor builds a dedicated policy for an approved credential segment.
    pub fn harness_action_policy(&self) -> Result<AgentBrowserActionPolicy, BrowserPolicyError> {
        let business: BTreeSet<&str> = self
            .actions
            .iter()
            .filter(|a| **a != BrowserAction::CredentialUse)
            .flat_map(|a| a.upstream_actions().iter().copied())
            .collect();
        if business.is_empty() {
            return Err(BrowserPolicyError::EmptyActions);
        }
        let allow: BTreeSet<&str> = business
            .into_iter()
            .chain(LIFECYCLE_UPSTREAM_ACTIONS)
            .collect();
        Ok(AgentBrowserActionPolicy {
            default: "deny".into(),
            allow: allow.into_iter().map(String::from).collect(),
        })
    }

    /// Value of `--allowed-domains` (comma-separated, sorted).
    pub fn allowed_domains_arg(&self) -> String {
        self.allowed_domains.join(",")
    }

    pub fn requires_approval(&self, action: BrowserAction) -> bool {
        self.approval_actions.contains(&action)
    }

    /// Hash over the canonical effective policy, including the grant-derived intersection.
    pub fn hash(&self) -> String {
        let canonical = serde_json::json!({
            "format": 1,
            "policy_id": self.policy_id,
            "task_revision": self.task_revision,
            "actions": self.actions,
            "approval_actions": self.approval_actions,
            "allowed_domains": self.allowed_domains,
            "credential_policy_ids": self.credential_policy_ids,
            "artifact_policy_id": self.artifact_policy_id,
            "backend_version": self.backend_version,
        });
        format!(
            "sha256:{:x}",
            Sha256::digest(canonical.to_string().as_bytes())
        )
    }

    pub fn binding(&self) -> BrowserPolicyBinding {
        BrowserPolicyBinding {
            policy_id: self.policy_id.clone(),
            revision: self.task_revision,
            hash: self.hash(),
        }
    }
}

fn valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':'))
}

/// Lowercase ASCII DNS name, optionally with a leading `*.` (subdomains only, not apex).
pub fn normalize_host_pattern(pattern: &str) -> Result<String, BrowserPolicyError> {
    if !pattern.is_ascii() {
        return Err(BrowserPolicyError::InvalidDomain);
    }
    let lower = pattern.to_ascii_lowercase();
    let host = lower.strip_prefix("*.").unwrap_or(&lower);
    // A single-label wildcard base (`*.com`) would cover a public suffix.
    if !valid_host(host) || (host.len() != lower.len() && !host.contains('.')) {
        return Err(BrowserPolicyError::InvalidDomain);
    }
    Ok(lower)
}

/// Does pattern `outer` admit every host admitted by `inner`?
fn host_covers(outer: &str, inner: &str) -> bool {
    match (outer.strip_prefix("*."), inner.strip_prefix("*.")) {
        (None, None) => outer == inner,
        (None, Some(_)) => false,
        (Some(base), Some(inner_base)) => {
            inner_base == base || inner_base.ends_with(&format!(".{base}"))
        }
        (Some(base), None) => inner.ends_with(&format!(".{base}")),
    }
}

fn intersect_hosts(a: &str, b: &str) -> Option<String> {
    if host_covers(a, b) {
        Some(b.into())
    } else if host_covers(b, a) {
        Some(a.into())
    } else {
        None
    }
}

fn minimize_hosts(hosts: Vec<String>) -> Vec<String> {
    let unique: BTreeSet<String> = hosts.into_iter().collect();
    unique
        .iter()
        .filter(|h| !unique.iter().any(|o| o != *h && host_covers(o, h)))
        .cloned()
        .collect()
}

/// Canonical `https://host[:port]` (default port elided), or None.
pub fn normalize_https_origin(origin: &str) -> Option<String> {
    let rest = origin.strip_prefix("https://")?;
    if rest.contains(['/', '?', '#', '@', '\\']) || !rest.is_ascii() {
        return None;
    }
    let lower = rest.to_ascii_lowercase();
    let (host, port) = match lower.split_once(':') {
        Some((host, port)) => {
            if port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            (host, Some(port.parse::<u16>().ok().filter(|p| *p != 0)?))
        }
        None => (lower.as_str(), None),
    };
    if !valid_host(host) {
        return None;
    }
    Some(match port {
        Some(p) if p != 443 => format!("https://{host}:{p}"),
        _ => format!("https://{host}"),
    })
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
                    live_view_url: None,
                    ..Default::default()
                }
                .validate()
                .is_err(),
                "{host}"
            );
        }
        assert!(
            BrowserCapability {
                allowed_domains: vec!["example.com".into(), "*.example.org".into()],
                live_view_url: None,
                ..Default::default()
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

#[cfg(test)]
#[path = "browser/policy_tests.rs"]
mod policy_tests;
