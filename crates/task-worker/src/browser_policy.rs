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

/// ADR 2026-10-05-browser-department-web-live-view D2.0: the stored task policy is narrowed to
/// the task's `requirements.browser.allowed_domains`, then intersected with the grant in force.
/// Dispatch and the worker both call this with the same inputs; narrowing is idempotent.
pub fn prepare_for_task(
    grant: &BrowserCapability,
    task: &task_core::Task,
    stored: Option<&BrowserTaskPolicy>,
    backend_version: &str,
) -> Result<PreparedBrowserPolicy, BrowserPolicyError> {
    let policy = task_core::browser::task_run_policy(&task.requirements, stored)?;
    prepare(grant, policy.as_ref(), backend_version)
}

/// Dispatch-time admission (D2.0): refuse a browser run whose effective origins (task ∩ the
/// grant read for this run) are empty before any process starts. Runs the worker would not treat
/// as browser runs, and runs without a grant (the worker reports that itself), pass through.
pub fn admit(req: &crate::protocol::RunRequest) -> Result<(), BrowserPolicyError> {
    if !task_core::browser::requests_browser(&req.task.skills)
        || req.context.execution_planner.is_some()
        || req.context.review.is_some()
        || req.task.kind != task_core::TaskKind::Execute
    {
        return Ok(());
    }
    let Some(grant) = req
        .context
        .profile
        .as_ref()
        .and_then(|p| p.browser.as_ref())
    else {
        return Ok(());
    };
    prepare_for_task(
        grant,
        &req.task,
        req.context.browser_policy.as_ref(),
        crate::browser::SUPPORTED_VERSION,
    )
    .map(|_| ())
}

impl PreparedBrowserPolicy {
    /// `host:port` entries for the egress proxy. CONNECT carries no scheme, so the scheme is
    /// enforced through its port (HTTPS 443, HTTP 80); anything else is outside the set.
    pub fn egress_allow(&self) -> std::collections::BTreeSet<String> {
        self.allowed_domains()
            .iter()
            .filter_map(|d| origin_host_port(d))
            .map(|(host, port)| format!("{host}:{port}"))
            .collect()
    }

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
            approval_actions: vec![],
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

    fn browser_task(origins: &[&str]) -> task_core::Task {
        let mut task = crate::protocol::tests::sample_task();
        task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
        task.requirements = task_core::TaskRequirements {
            browser: Some(task_core::BrowserRequirements {
                allowed_domains: origins.iter().map(|o| o.to_string()).collect(),
            }),
        };
        task
    }

    fn wide_policy() -> BrowserTaskPolicy {
        BrowserTaskPolicy {
            network_domains: vec![
                "https://*.example.com".into(),
                "http://127.0.0.1:3000".into(),
            ],
            ..policy(&[BrowserAction::Navigate])
        }
    }

    fn grant(origins: &[&str]) -> BrowserCapability {
        BrowserCapability {
            approval_actions: vec![],
            allowed_domains: origins.iter().map(|o| o.to_string()).collect(),
            ..Default::default()
        }
    }

    /// D2.0: the broker's navigation check (action server open / shared CDP navigate) only
    /// admits task ∩ grant, compared by scheme, host and port.
    #[test]
    fn browser_allowed_domains_broker_denies_outside_task_and_grant() {
        let grant = grant(&["https://*.example.com", "http://127.0.0.1:3000"]);
        let task = browser_task(&["https://billing.example.com", "https://evil.test"]);
        let prepared = prepare_for_task(&grant, &task, Some(&wide_policy()), "0.38.1").unwrap();
        assert_eq!(prepared.allowed_domains(), ["https://billing.example.com"]);
        for (url, ok) in [
            ("https://billing.example.com/invoices", true),
            // scheme and port differences
            ("http://billing.example.com/", false),
            ("https://billing.example.com:8443/", false),
            // in the grant, not in the task
            ("https://app.example.com/", false),
            ("http://127.0.0.1:3000/", false),
            // in the task, not in the grant
            ("https://evil.test/", false),
        ] {
            assert_eq!(
                url_origin_allowed(url, prepared.allowed_domains()),
                ok,
                "{url}"
            );
        }
    }

    /// Legacy host-form grant and stored policy (`example.com`, `*.example.org`) are read as
    /// HTTPS:443 origins; the wildcard contains a subdomain but not the apex, and HTTP or other
    /// ports are never added. Mirrors the userns-only launch test so it runs on every host.
    #[test]
    fn legacy_host_form_grant_and_policy_narrow_to_requested_https_origins() {
        let grant = grant(&["example.com", "*.example.org"]);
        let policy = BrowserTaskPolicy {
            network_domains: vec!["example.com".into(), "docs.example.org".into()],
            ..policy(&[BrowserAction::Navigate])
        };
        let task = browser_task(&["https://example.com", "https://docs.example.org"]);
        let prepared = prepare_for_task(&grant, &task, Some(&policy), "0.38.1").unwrap();
        assert_eq!(
            prepared.allowed_domains(),
            ["https://docs.example.org", "https://example.com"]
        );
        for (url, ok) in [
            ("https://docs.example.org/x", true),
            ("https://example.org/", false),
            ("http://docs.example.org/", false),
            ("https://docs.example.org:8443/", false),
        ] {
            assert_eq!(
                url_origin_allowed(url, prepared.allowed_domains()),
                ok,
                "{url}"
            );
        }
        let narrowed = browser_task(&["https://example.com"]);
        let prepared = prepare_for_task(&grant, &narrowed, Some(&policy), "0.38.1").unwrap();
        assert_eq!(prepared.allowed_domains(), ["https://example.com"]);
        let outside = browser_task(&["https://fixture.example.com"]);
        assert_eq!(
            prepare_for_task(&grant, &outside, Some(&policy), "0.38.1").unwrap_err(),
            BrowserPolicyError::EmptyDomains
        );
    }

    /// D2.0: the egress allow list is task ∩ grant as `host:port`; the scheme shows as the port.
    #[test]
    fn browser_allowed_domains_egress_allow_is_task_and_grant() {
        let grant = grant(&["https://*.example.com", "http://127.0.0.1:3000"]);
        let task = browser_task(&["https://billing.example.com", "https://evil.test"]);
        let prepared = prepare_for_task(&grant, &task, Some(&wide_policy()), "0.38.1").unwrap();
        assert_eq!(
            prepared.egress_allow(),
            ["billing.example.com:443".to_string()].into()
        );
    }

    /// D2.0: a grant shrink after the task was created applies to the same task's next run, and
    /// the binding hash changes so approvals and waits bound to the old policy are refused.
    #[test]
    fn browser_allowed_domains_grant_shrink_applies_to_existing_task() {
        let task = browser_task(&["https://billing.example.com", "https://app.example.com"]);
        let before = prepare_for_task(
            &grant(&["https://*.example.com"]),
            &task,
            Some(&wide_policy()),
            "0.38.1",
        )
        .unwrap();
        assert_eq!(
            before.allowed_domains(),
            ["https://app.example.com", "https://billing.example.com"]
        );
        let after = prepare_for_task(
            &grant(&["https://app.example.com"]),
            &task,
            Some(&wide_policy()),
            "0.38.1",
        )
        .unwrap();
        assert_eq!(after.allowed_domains(), ["https://app.example.com"]);
        assert!(!url_origin_allowed(
            "https://billing.example.com/",
            after.allowed_domains()
        ));
        assert_ne!(after.binding.hash, before.binding.hash);
        // Shrunk to nothing the task shares: no run.
        assert_eq!(
            prepare_for_task(
                &grant(&["https://other.example.com"]),
                &task,
                Some(&wide_policy()),
                "0.38.1"
            )
            .unwrap_err(),
            BrowserPolicyError::EmptyDomains
        );
    }

    /// D2.0: dispatch refuses a browser run with an empty effective set before it starts.
    #[test]
    fn browser_allowed_domains_empty_intersection_is_refused_before_run() {
        let mut req = crate::protocol::RunRequest {
            cargo_target_dir: None,
            protocol: crate::protocol::PROTOCOL_VERSION,
            task: browser_task(&["https://billing.example.com"]),
            artifacts_dir: "/nonexistent/artifacts".into(),
            workspace: "/nonexistent".into(),
            work_dir: None,
            context: crate::protocol::RunContext::default(),
        };
        req.context.browser_policy = Some(wide_policy());
        req.context.profile = Some(task_core::EffectiveProfile {
            browser: Some(grant(&["https://*.example.com"])),
            ..Default::default()
        });
        assert_eq!(admit(&req), Ok(()));
        req.context.profile = Some(task_core::EffectiveProfile {
            browser: Some(grant(&["https://billing.example.com:8443"])),
            ..Default::default()
        });
        assert_eq!(admit(&req), Err(BrowserPolicyError::EmptyDomains));
        // Not a browser run: nothing to admit.
        req.task.skills.clear();
        assert_eq!(admit(&req), Ok(()));
    }
}
