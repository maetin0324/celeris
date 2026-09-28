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
            network_domains: vec!["example.com".into()],
            allowed_actions: actions.to_vec(),
            approval_actions: vec![],
            credential_policy_ids: vec![],
            artifact_policy_id: None,
        }
    }

    #[test]
    fn prepared_file_is_hash_bound_nonempty_default_deny() {
        let grant = BrowserCapability {
            allowed_domains: vec!["example.com".into()],
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
        assert_eq!(prepared.allowed_domains(), ["example.com"]);
        assert_eq!(
            prepare(&grant, Some(&policy(&[])), "0.38.1").unwrap_err(),
            BrowserPolicyError::EmptyActions
        );
        assert_eq!(
            prepare(&grant, None, "0.38.1").unwrap_err(),
            BrowserPolicyError::BrowserPolicyRequired
        );
    }
}
