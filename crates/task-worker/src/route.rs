//! The worker side of a selected LLM route (ADR 2026-10-11 llm-source-harness-selection).
//!
//! `RunRequest.llm_route` names the harness, source, model and account the dispatcher admitted.
//! The executor is the declared harness's adapter (built from `[harnesses.<id>]`, whose
//! `command`/`args` it already carries), and the model is exactly the route's model. The worker
//! never infers a harness from anything else and never substitutes another model.

use std::collections::HashMap;
use std::sync::Arc;

use task_core::llm_route::HarnessId;

use crate::adapter::WorkerAdapter;
use crate::protocol::{CredentialReference, RunRequest, SelectedLlmRoute};

/// Why a selected route cannot be launched. Every case fails closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    #[error("run request carries no llm_route")]
    Missing,
    #[error("harness {0} is not declared")]
    UnknownHarness(String),
    #[error("harness {harness} cannot take the account environment of account {account}")]
    AccountUnsupported { harness: String, account: String },
    #[error("harness {harness} cannot be pointed at endpoint {base_url}")]
    EndpointUnsupported { harness: String, base_url: String },
    #[error("credential {name} of source {source_id} cannot be resolved")]
    CredentialUnresolved { source_id: String, name: String },
}

/// The adapter that runs `route`: the harness adapter with the route's model, the account's
/// login directory in its environment, and the source endpoint with its resolved credential.
/// `resolve_secret` turns a credential reference name into its value (secrets never travel in
/// the protocol). A harness without a model option (`fake`) keeps its own configuration.
pub fn route_adapter(
    adapters: &HashMap<HarnessId, Arc<dyn WorkerAdapter>>,
    route: &SelectedLlmRoute,
    resolve_secret: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<dyn WorkerAdapter>, RouteError> {
    let harness = route.harness_id.as_str();
    let base = adapters
        .get(&HarnessId::from(harness))
        .ok_or_else(|| RouteError::UnknownHarness(harness.to_string()))?;
    let model_id = if harness == "opencode" && base.id() == crate::AcpAdapter::ID {
        base.model_namespace()
            .map(|namespace| format!("{namespace}/{}", route.model_id))
    } else {
        None
    };
    let model_id = model_id.as_deref().unwrap_or(&route.model_id);
    let mut adapter = base
        .with_model(model_id)
        .unwrap_or_else(|| Arc::clone(base));
    let api_key = match &route.credential {
        Some(CredentialReference::AccountDirectory { path, env }) => {
            adapter = adapter
                .with_env(&[(env.clone(), path.display().to_string())])
                .ok_or_else(|| RouteError::AccountUnsupported {
                    harness: harness.to_string(),
                    account: route.account_id.clone(),
                })?;
            None
        }
        Some(CredentialReference::Environment { name }) => Some(resolve_secret(name).ok_or_else(
            || RouteError::CredentialUnresolved {
                source_id: route.source_id.clone(),
                name: name.clone(),
            },
        )?),
        None => None,
    };
    if let Some(base_url) = route.endpoint.as_ref().and_then(|e| e.base_url.as_deref()) {
        adapter = adapter
            .with_llm_endpoint(base_url, api_key.as_deref())
            .ok_or_else(|| RouteError::EndpointUnsupported {
                harness: harness.to_string(),
                base_url: base_url.to_string(),
            })?;
    }
    Ok(adapter)
}

/// [`route_adapter`] for the route carried by `req`.
pub fn request_adapter(
    adapters: &HashMap<HarnessId, Arc<dyn WorkerAdapter>>,
    req: &RunRequest,
    resolve_secret: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<dyn WorkerAdapter>, RouteError> {
    let route = req.llm_route.as_ref().ok_or(RouteError::Missing)?;
    route_adapter(adapters, route, resolve_secret)
}

#[cfg(test)]
mod tests;
