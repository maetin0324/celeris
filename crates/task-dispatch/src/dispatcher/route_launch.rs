//! Launch inputs for an admitted route (ADR 2026-10-11 llm-source-harness-selection).
//!
//! A [`RoutePick`] names a harness, a source×model and an account. The worker adapter is the
//! declared harness's base adapter with the picked model applied and, for an OAuth source, the
//! picked account's login directory in its environment. The [`SelectedLlmRoute`] the worker
//! receives carries the endpoint and a credential reference only; secret values never travel in it.

use super::route_select::{RoutePick, account_adapter_for_source_kind};
use super::*;
use task_core::llm_route::{HarnessId, Registry, SourceId};
use task_worker::protocol::{CredentialReference, LlmEndpointReference, SelectedLlmRoute};

/// Why an admitted route cannot be turned into a worker launch. Every case fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteLaunchError {
    /// No adapter was built for the picked harness.
    UnknownHarness(HarnessId),
    /// The picked source is not declared in the registry.
    UnknownSource(SourceId),
    /// The OAuth source's account cannot be applied (no `[accounts]` root, or the harness
    /// adapter does not take an account environment).
    AccountUnavailable { source: SourceId, account: String },
}

impl std::fmt::Display for RouteLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteLaunchError::UnknownHarness(h) => write!(f, "no adapter for harness {h}"),
            RouteLaunchError::UnknownSource(s) => write!(f, "source {s} is not declared"),
            RouteLaunchError::AccountUnavailable { source, account } => write!(
                f,
                "account {account} of source {source} cannot be applied to the harness adapter"
            ),
        }
    }
}

impl std::error::Error for RouteLaunchError {}

/// The worker-facing description of `pick`: ids, the source endpoint and its credential
/// reference. An OAuth source's credential is the account's login directory under
/// `account_root` (the `[accounts]` root of its pool), when one is configured.
pub fn selected_llm_route(
    registry: &Registry,
    pick: &RoutePick,
    account_root: Option<&std::path::Path>,
) -> Result<SelectedLlmRoute, RouteLaunchError> {
    let source = registry
        .sources
        .get(&pick.candidate.source)
        .ok_or_else(|| RouteLaunchError::UnknownSource(pick.candidate.source.clone()))?;
    let oauth = account_adapter_for_source_kind(source.kind.as_ref()).is_some();
    let credential = if oauth {
        account_root.map(|root| CredentialReference::AccountDirectory {
            path: root.join(pick.account.as_ref()),
        })
    } else {
        source
            .credential_ref
            .as_ref()
            .map(|name| CredentialReference::Environment { name: name.clone() })
    };
    let endpoint = (source.endpoint.is_some() || !oauth).then(|| LlmEndpointReference {
        base_url: source.endpoint.clone(),
        api_format: pick.candidate.api_format.to_string(),
    });
    Ok(SelectedLlmRoute {
        harness_id: pick.harness.to_string(),
        source_id: pick.candidate.source.to_string(),
        model_id: pick.candidate.model.to_string(),
        account_id: pick.account.to_string(),
        endpoint,
        credential,
    })
}

impl Dispatcher {
    /// Daemon wiring (bootstrap and reload): the base adapter of every declared harness.
    pub fn set_harness_adapters(&mut self, adapters: HashMap<HarnessId, Arc<dyn WorkerAdapter>>) {
        self.harness_adapters = adapters;
    }

    pub fn harness_adapter(&self, harness: &HarnessId) -> Option<&Arc<dyn WorkerAdapter>> {
        self.harness_adapters.get(harness)
    }

    /// The adapter that runs `pick`: the harness adapter with the picked model, plus the picked
    /// account's environment when the source is an OAuth pool. An adapter that has no model
    /// option keeps its own configuration.
    pub fn route_adapter(
        &self,
        pick: &RoutePick,
    ) -> Result<Arc<dyn WorkerAdapter>, RouteLaunchError> {
        let base = self
            .harness_adapters
            .get(&pick.harness)
            .ok_or_else(|| RouteLaunchError::UnknownHarness(pick.harness.clone()))?;
        let source = self
            .llm_route_registry
            .sources
            .get(&pick.candidate.source)
            .ok_or_else(|| RouteLaunchError::UnknownSource(pick.candidate.source.clone()))?;
        let adapter = base
            .with_model(pick.candidate.model.as_ref())
            .unwrap_or_else(|| Arc::clone(base));
        match account_adapter_for_source_kind(source.kind.as_ref()) {
            Some(account_adapter) => self
                .adapter_for_account(&adapter, account_adapter, pick.account.as_ref())
                .ok_or_else(|| RouteLaunchError::AccountUnavailable {
                    source: pick.candidate.source.clone(),
                    account: pick.account.to_string(),
                }),
            None => Ok(adapter),
        }
    }

    /// [`selected_llm_route`] with this dispatcher's registry and `[accounts]` roots.
    pub fn route_launch_target(
        &self,
        pick: &RoutePick,
    ) -> Result<SelectedLlmRoute, RouteLaunchError> {
        let root = self
            .llm_route_registry
            .sources
            .get(&pick.candidate.source)
            .and_then(|s| account_adapter_for_source_kind(s.kind.as_ref()))
            .and_then(|a| {
                self.config
                    .accounts
                    .as_ref()?
                    .root_for(a)
                    .map(|r| r.to_path_buf())
            });
        selected_llm_route(&self.llm_route_registry, pick, root.as_deref())
    }
}
