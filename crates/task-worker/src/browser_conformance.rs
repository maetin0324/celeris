//! Measured capability declarations, ledger validation and backend routing.
use super::SUPPORTED_VERSION;
use crate::AdapterError;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use task_core::browser_backend::{
    self, BackendDescriptor, BackendKind, Capability, ConformanceResult, RoutingRequest,
};

/// Public (non-sensitive) capabilities the existing-harness backends declare. Sensitive
/// capabilities remain undeclared until P4-A/B record real conformance.
pub(crate) fn public_capabilities() -> BTreeSet<Capability> {
    use Capability as C;
    [
        C::Navigate,
        C::Snapshot,
        C::Click,
        C::Screenshot,
        C::Download,
    ]
    .into_iter()
    .collect()
}

fn existing_backends(declared: &BTreeSet<Capability>) -> Vec<BackendDescriptor> {
    BROWSER_BACKEND_IDS
        .into_iter()
        .map(|id| BackendDescriptor {
            id: id.into(),
            kind: if id == "browser-specialist" {
                BackendKind::BrowserSpecialist
            } else {
                BackendKind::ExistingLoop
            },
            version: SUPPORTED_VERSION.into(),
            declared: declared.clone(),
            enabled: true,
        })
        .collect()
}

/// Adapter ids that can carry the browser capability (ADR-0103 D2).
pub const BROWSER_BACKEND_IDS: [&str; 3] = ["acp", "claude-code", "browser-specialist"];

/// The runner ledger: `CELERIS_BROWSER_CONFORMANCE_FILE` (test/dev override) first, then the
/// path the daemon configured (ADR 2026-10-08-browser-prod-enablement D1.4).
pub fn conformance_record_path() -> Option<PathBuf> {
    std::env::var_os("CELERIS_BROWSER_CONFORMANCE_FILE")
        .map(PathBuf::from)
        .or_else(|| crate::browser_ledger::configured_path().map(Path::to_path_buf))
}

/// ADR-0109 D1: adapter ids whose runner-recorded conformance certifies every declared public
/// capability at the supported substrate version. A missing, corrupt or stale ledger fails closed
/// (the caller gets the error and must not offer any fallback).
pub fn conformant_backend_ids(record_path: &Path) -> Result<BTreeSet<String>, AdapterError> {
    let results = load_conformance(record_path)?;
    let declared = public_capabilities();
    Ok(existing_backends(&declared)
        .into_iter()
        .filter(|backend| {
            browser_backend::certify(backend, results.get(&backend.id))
                .is_ok_and(|certified| certified == backend.declared)
        })
        .map(|backend| backend.id)
        .collect())
}

/// Load only measured conformance. The path is supplied by the daemon operator; a missing,
/// corrupt or stale record fails closed. The runner writes the record after invoking the real
/// substrate against its local fixture. Sensitive capabilities remain undeclared until P4-A/B.
pub(super) fn route_existing_backend(
    adapter_id: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    record_path: &Path,
) -> Result<browser_backend::RoutingDecision, AdapterError> {
    use Capability as C;
    let mut supported = public_capabilities();
    // H3 is routed only when the operator ledger attests the sensitive fixture
    // suite. A credential task cannot reach the isolated runtime otherwise.
    if policy
        .effective
        .actions
        .contains(&task_core::BrowserAction::CredentialUse)
    {
        supported.insert(C::CredentialInjection);
    }
    let backends = existing_backends(&supported);
    let results = load_conformance(record_path)?;
    let mut required = BTreeSet::new();
    for action in &policy.effective.actions {
        match action {
            task_core::BrowserAction::Navigate => {
                required.insert(C::Navigate);
            }
            task_core::BrowserAction::Snapshot | task_core::BrowserAction::Extract => {
                required.insert(C::Snapshot);
            }
            task_core::BrowserAction::Click => {
                required.insert(C::Click);
            }
            task_core::BrowserAction::Screenshot => {
                required.insert(C::Screenshot);
            }
            task_core::BrowserAction::Download => {
                required.insert(C::Download);
            }
            task_core::BrowserAction::CredentialUse => {
                required.insert(C::CredentialInjection);
            }
            task_core::BrowserAction::Scroll => {}
        }
    }
    let decision = browser_backend::route(
        &backends,
        &results,
        &RoutingRequest {
            required,
            explicit: Some(adapter_id.into()),
            failed: BTreeSet::new(),
        },
    )
    .map_err(|_| AdapterError::Other("browser backend lacks required conformance".into()))?;
    if decision.primary != adapter_id {
        return Err(AdapterError::Other(
            "browser backend routing mismatch".into(),
        ));
    }
    Ok(decision)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConformanceLedger {
    schema: u32,
    source: String,
    /// ADR 2026-10-08-browser-prod-enablement D1.2: どの release・agent-browser の版で作ったか。
    /// 旧台帳には無い（`stale_release` の判定で古いと扱う）。
    #[serde(default)]
    generated_for: Option<crate::browser_ledger::GeneratedFor>,
    results: Vec<ConformanceResult>,
}

fn load_conformance(path: &Path) -> Result<BTreeMap<String, ConformanceResult>, AdapterError> {
    load_ledger(path).map(|(results, _)| results)
}

/// 台帳の結果と `generated_for`。読めない・壊れた台帳は fail closed。
pub(crate) fn load_ledger(
    path: &Path,
) -> Result<
    (
        BTreeMap<String, ConformanceResult>,
        Option<crate::browser_ledger::GeneratedFor>,
    ),
    AdapterError,
> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| AdapterError::Other("browser conformance record unavailable".into()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(AdapterError::Other(
            "browser conformance record invalid".into(),
        ));
    }
    let bytes = std::fs::read(path)?;
    let ledger: ConformanceLedger = serde_json::from_slice(&bytes)
        .map_err(|_| AdapterError::Other("browser conformance record invalid".into()))?;
    if ledger.schema != 1 || ledger.source != "celeris-browser-conformance" {
        return Err(AdapterError::Other(
            "browser conformance record invalid".into(),
        ));
    }
    let mut results = BTreeMap::new();
    for result in ledger.results {
        if results.insert(result.backend_id.clone(), result).is_some() {
            return Err(AdapterError::Other(
                "browser conformance record invalid".into(),
            ));
        }
    }
    Ok((results, ledger.generated_for))
}
