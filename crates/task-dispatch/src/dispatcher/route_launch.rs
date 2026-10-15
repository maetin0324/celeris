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
    let account_adapter = account_adapter_for_source_kind(source.kind.as_ref());
    let oauth = account_adapter.is_some();
    let credential = if let Some(account_adapter) = account_adapter {
        account_root.map(|root| CredentialReference::AccountDirectory {
            path: root.join(pick.account.as_ref()),
            env: account_adapter.env_var().to_string(),
        })
    } else {
        source
            .credential_ref
            .as_ref()
            .map(|name| CredentialReference::Environment { name: name.clone() })
    };
    let proxy = task_core::llm_route::route_via(registry, &pick.harness, &pick.candidate.key())
        == Some("llm_proxy");
    let endpoint =
        (!proxy && (source.endpoint.is_some() || !oauth)).then(|| LlmEndpointReference {
            base_url: source.endpoint.clone(),
            api_format: pick.candidate.api_format.to_string(),
        });
    Ok(SelectedLlmRoute {
        harness_id: pick.harness.to_string(),
        source_id: pick.candidate.source.to_string(),
        model_id: pick.wire_model_id.to_string(),
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
            .with_model(pick.wire_model_id.as_ref())
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

// ---- ADR 2026-10-15-mlsys-min-routing-policy D6: 利用可否による起動の可否 ----

use task_core::route_select::{Availability, split_by_availability};

/// 候補（既存の選定順）を利用可否で絞った結果。`Unknown`（quota/cost/予算が未確認）は 0 扱いで
/// 可にせず、`Available` が 1 つも無ければ起動せず待機（保守側）にする。
// 起動経路への接続は後続の統合葉が行う（dispatcher.rs は本 unit の範囲外）。それまで試験のみが使う。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAdmission<T> {
    Launch(T),
    /// 起動しない。`unconfirmed` が非空なら未確認のため、空なら全て不可（待機・再試行）。
    Hold {
        unconfirmed: usize,
    },
}

/// 先頭の `Available` を選ぶ。`Unavailable` は落とし、`Unknown` だけが残るなら `Hold`。
#[allow(dead_code)]
pub fn admit_by_availability<T>(candidates: Vec<(T, Availability)>) -> LaunchAdmission<T> {
    let split = split_by_availability(candidates);
    let unconfirmed = split.unconfirmed.len();
    match split.available.into_iter().next() {
        Some(t) => LaunchAdmission::Launch(t),
        None => LaunchAdmission::Hold { unconfirmed },
    }
}
