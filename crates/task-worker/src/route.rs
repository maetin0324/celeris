//! The worker side of a selected LLM route (ADR 2026-10-11 llm-source-harness-selection).
//!
//! `RunRequest.llm_route` names the harness, source, model and account the dispatcher admitted.
//! The executor is the declared harness's adapter (built from `[harnesses.<id>]`, whose
//! `command`/`args` it already carries), and the model is exactly the route's model. The worker
//! never infers a harness from anything else and never substitutes another model.

use std::collections::HashMap;
use std::sync::Arc;

use task_core::llm_route::HarnessId;

use crate::adapter::{AdapterSetting, AdapterSettings, LlmEndpoint, WorkerAdapter};
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
    let mut settings = AdapterSettings {
        model: Some(model_id.unwrap_or_else(|| route.model_id.clone())),
        ..AdapterSettings::default()
    };
    let api_key = match &route.credential {
        Some(CredentialReference::AccountDirectory { path, env }) => {
            settings.env.push((env.clone(), path.display().to_string()));
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
    let base_url = route.endpoint.as_ref().and_then(|e| e.base_url.clone());
    settings.llm_endpoint = base_url
        .clone()
        .map(|base_url| LlmEndpoint { base_url, api_key });
    // ADR 2026-11-10-worker-capability-ports D3 3b: a harness without a model option keeps its
    // own configuration; the account environment and the endpoint fail closed (account first).
    let (adapter, unsupported) = base.configure(&settings).resolve(base);
    if unsupported.contains(&AdapterSetting::Env) {
        return Err(RouteError::AccountUnsupported {
            harness: harness.to_string(),
            account: route.account_id.clone(),
        });
    }
    if let Some(base_url) = base_url.filter(|_| unsupported.contains(&AdapterSetting::LlmEndpoint))
    {
        return Err(RouteError::EndpointUnsupported {
            harness: harness.to_string(),
            base_url,
        });
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
