//! Owner-attested metadata-only management of saved credentials.
use crate::{
    browser::{BrokerFailure, HumanAttestation},
    browser_live::verify_signature,
    middleware::require_admin,
    problem::ApiProblem,
    state::ApiState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SavedCredentialItem {
    pub credential_id: String,
    pub policy_id: String,
    pub created_at: u64,
    pub expires_at: u64,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct SavedCredentialList {
    pub items: Vec<SavedCredentialItem>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    purpose: String,
    owner_session_id: String,
    owner_session: bool,
    actor_id: String,
    #[serde(default)]
    credential_id: Option<String>,
    expires_at: i64,
}
fn denied() -> ApiProblem {
    ApiProblem::new(
        StatusCode::FORBIDDEN,
        "credential_access_denied",
        "credential access denied",
    )
}
fn verify(
    state: &ApiState,
    headers: &HeaderMap,
    purpose: &str,
    id: Option<&str>,
) -> Result<String, ApiProblem> {
    require_admin(state, headers)?;
    let value = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(denied)
    };
    let assertion = HumanAttestation {
        payload: value("x-celeris-assertion-payload")?,
        signature: value("x-celeris-assertion-signature")?,
    };
    verify_signature(state, &assertion)?;
    let claims: Claims = serde_json::from_str(&assertion.payload).map_err(|_| denied())?;
    if claims.purpose != purpose
        || claims.credential_id.as_deref() != id
        || !claims.owner_session
        || claims.owner_session_id.is_empty()
        || claims.actor_id.is_empty()
        || claims.expires_at < state.now_secs()
        || claims.expires_at > state.now_secs() + 30
    {
        return Err(denied());
    }
    Ok(claims.actor_id)
}
fn unavailable(_: BrokerFailure) -> ApiProblem {
    ApiProblem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "credential_unavailable",
        "credential broker unavailable",
    )
}
async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<SavedCredentialList>, ApiProblem> {
    let owner = verify(&state, &headers, "credential_list", None)?;
    let broker = state
        .browser
        .broker
        .clone()
        .ok_or_else(|| unavailable(BrokerFailure::Unavailable))?;
    let items = tokio::task::spawn_blocking(move || broker.list_saved(&owner))
        .await
        .map_err(|_| unavailable(BrokerFailure::Unavailable))?
        .map_err(unavailable)?;
    Ok(Json(SavedCredentialList { items }))
}
async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiProblem> {
    let owner = verify(&state, &headers, "credential_delete", Some(&id))?;
    let broker = state
        .browser
        .broker
        .clone()
        .ok_or_else(|| unavailable(BrokerFailure::Unavailable))?;
    tokio::task::spawn_blocking(move || broker.delete_saved(&owner, &id))
        .await
        .map_err(|_| unavailable(BrokerFailure::Unavailable))?
        .map_err(unavailable)?;
    Ok(StatusCode::NO_CONTENT)
}
pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/browser/credentials", axum::routing::get(list))
        .route(
            "/api/v1/browser/credentials/{id}",
            axum::routing::delete(delete),
        )
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}
