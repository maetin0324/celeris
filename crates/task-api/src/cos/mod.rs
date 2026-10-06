//! CoS run credential authentication and the audited CoS API (ADR 2026-10-05 D2/D3).
//!
//! A CoS worker presents `Authorization: Bearer celeris-cos-run.<secret>`. The prefix decides the
//! credential kind deterministically: such a bearer is never compared with the admin token, and the
//! secret after the prefix is verified by `task_core`. The verified `{thread_id, run_id}` becomes the
//! [`CosCaller`] request extension; actor, thread and run claims in headers or JSON are never read.

pub mod inbox;
pub mod operations;
pub mod override_op;
pub mod triage_view;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use task_core::chat::{AuditContext, ChatActor, ChatError, CosRunCredentialError, CosRunIdentity};
use task_core::store::SqliteStore;
use time::{Duration, OffsetDateTime};
use ulid::Ulid;

use crate::handlers::{ApiResult, Params, json_response, no_query};
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// Bearer prefix of a CoS run credential. Admin tokens with this prefix are not supported.
pub const COS_BEARER_PREFIX: &str = "celeris-cos-run.";
/// Only this subtree accepts mutating requests from a CoS run credential.
const COS_API_PREFIX: &str = "/api/v1/cos/";
/// The policy version recorded on rejections made before any CoS policy is involved.
const DIRECT_REJECT_POLICY: &str = "api-guard-1";

/// Identity of a verified CoS run credential. Only the auth middleware constructs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CosCaller {
    pub thread_id: String,
    pub run_id: String,
}

impl From<CosRunIdentity> for CosCaller {
    fn from(identity: CosRunIdentity) -> Self {
        Self {
            thread_id: identity.thread_id,
            run_id: identity.run_id,
        }
    }
}

/// Issue the bearer handed to a CoS worker for one live run. The daemon (chat-run) and tests use
/// this so the plaintext always carries [`COS_BEARER_PREFIX`]; the store keeps only a digest.
pub fn issue_run_bearer(
    store: &SqliteStore,
    thread_id: &str,
    run_id: &str,
    ttl: Duration,
) -> Result<String, ChatError> {
    issue_run_bearer_at(store, thread_id, run_id, ttl, OffsetDateTime::now_utc())
}

/// Clock-injected variant of [`issue_run_bearer`].
pub fn issue_run_bearer_at(
    store: &SqliteStore,
    thread_id: &str,
    run_id: &str,
    ttl: Duration,
    now: OffsetDateTime,
) -> Result<String, ChatError> {
    let secret = store.cos_run_credential_issue_at(thread_id, run_id, ttl, now)?;
    Ok(format!("{COS_BEARER_PREFIX}{secret}"))
}

/// The secret part of a CoS bearer, or `None` when the bearer is not CoS-shaped.
pub(crate) fn cos_bearer_secret(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .and_then(|(_, token)| token.trim().strip_prefix(COS_BEARER_PREFIX))
}

fn credential_problem(error: CosRunCredentialError) -> ApiProblem {
    let detail = match error {
        CosRunCredentialError::Unknown => "unknown CoS run credential",
        CosRunCredentialError::Expired => "expired CoS run credential",
        CosRunCredentialError::Revoked => "revoked CoS run credential",
        CosRunCredentialError::Chat(error) => {
            return ApiProblem::internal(format!("credential lookup failed: {error}"));
        }
    };
    ApiProblem::new(StatusCode::UNAUTHORIZED, "unauthorized", detail)
        .with_extra("credential", "cos_run")
        .with_header(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"celeris\""),
        )
}

/// Called by the guard for CoS-shaped bearers. Verifies the credential, rejects (and records) a
/// direct mutation outside `/api/v1/cos/`, and attaches [`CosCaller`] to the request.
pub(crate) async fn authenticate(state: &ApiState, req: &mut Request) -> Result<(), ApiProblem> {
    let Some(secret) = cos_bearer_secret(req.headers()).map(str::to_owned) else {
        return Ok(());
    };
    let caller: CosCaller = state
        .blocking(move |store| {
            store
                .cos_run_credential_verify(&secret, OffsetDateTime::now_utc())
                .map(CosCaller::from)
                .map_err(credential_problem)
        })
        .await?;
    let mutating = matches!(
        *req.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    let path = req.uri().path().to_string();
    if mutating && !path.starts_with(COS_API_PREFIX) {
        let method = req.method().to_string();
        let reason = "CoS run credential cannot change domain state without /api/v1/cos/operations";
        let recorder = caller.clone();
        let (record_method, record_path) = (method.clone(), path.clone());
        state
            .blocking(move |store| {
                record_direct_rejection(store, &recorder, &record_method, &record_path, reason)
                    .map_err(|error| ApiProblem::internal(format!("audit record failed: {error}")))
            })
            .await?;
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "cos_audit_context_required",
            format!("{method} {path} needs the audit context of POST /api/v1/cos/operations"),
        )
        .with_extra("instead", "/api/v1/cos/operations"));
    }
    req.extensions_mut().insert(caller);
    Ok(())
}

/// Write a rejected `cos_operations` row and its reasoned audit event through the ops store.
fn record_direct_rejection(
    store: &SqliteStore,
    caller: &CosCaller,
    method: &str,
    path: &str,
    reason: &str,
) -> Result<(), ChatError> {
    let operation_id = Ulid::new().to_string();
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: caller.thread_id.clone(),
        run_id: caller.run_id.clone(),
        operation_id: operation_id.clone(),
        reason: reason.into(),
        policy_version: DIRECT_REJECT_POLICY.into(),
    };
    let hash = format!(
        "{:x}",
        Sha256::digest(format!("{method} {path}").as_bytes())
    );
    let payload = serde_json::json!({"method": method, "path": path});
    let outcome = store.cos_operation_apply(
        &ctx,
        &format!("direct-api:{operation_id}"),
        &hash,
        "api",
        path,
        None,
        method,
        &payload,
        |_, _| {
            Err(ChatError::Invalid(
                "direct domain mutation without audit context".into(),
            ))
        },
    );
    match outcome {
        Err(ChatError::Invalid(_)) => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(ChatError::Invalid(
            "direct mutation was not rejected".into(),
        )),
    }
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/cos/threads/{t}/checkpoint", post(checkpoint))
        .merge(operations::routes())
        .merge(inbox::routes())
        .merge(override_op::routes())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRequest {
    run_id: String,
    summary: String,
    through_seq: u64,
    expected_summary_through_seq: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CheckpointResponse {
    thread_id: String,
    summary_through_seq: u64,
}

pub(crate) fn cos_problem(error: ChatError) -> ApiProblem {
    let status =
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let code = match &error {
        ChatError::Store(_) => "chat_store_error",
        ChatError::BadRequest(_) => "bad_request",
        ChatError::Invalid(_) => "validation",
        ChatError::NotFound { .. } => "chat_not_found",
        ChatError::Conflict(_) => "chat_conflict",
        ChatError::TooLarge(_) => "payload_too_large",
        ChatError::QueueFull { .. } => "chat_queue_full",
        ChatError::CursorExpired(_) => "chat_cursor_expired",
    };
    ApiProblem::new(status, code, error.to_string())
}

fn cos_only(detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::FORBIDDEN, "cos_credential_required", detail)
}

/// Reject identity claims in a CoS request body: the credential alone decides the actor.
pub(crate) fn reject_identity_claims(value: &serde_json::Value) -> Result<(), ApiProblem> {
    if let Some(object) = value.as_object() {
        for key in ["actor", "thread_id"] {
            if object.contains_key(key) {
                return Err(ApiProblem::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "cos_identity_claim",
                    format!("`{key}` is decided by the CoS run credential and cannot be sent"),
                ));
            }
        }
    }
    Ok(())
}

async fn checkpoint(
    State(state): State<ApiState>,
    caller: Option<Extension<CosCaller>>,
    Params(t): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let Some(Extension(caller)) = caller else {
        return Err(cos_only(
            "checkpoint accepts only the CoS run credential of the current run",
        ));
    };
    let value: serde_json::Value = crate::handlers::read_json(body, false).await?;
    reject_identity_claims(&value)?;
    let req: CheckpointRequest = serde_json::from_value(value).map_err(|error| {
        let detail = error.to_string();
        if detail.starts_with("unknown field") {
            ApiProblem::bad_request(detail)
        } else {
            ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", detail)
        }
    })?;
    if caller.thread_id != t || caller.run_id != req.run_id {
        return Err(cos_only(
            "the CoS run credential belongs to another thread or run",
        ));
    }
    let saved = state
        .blocking(move |store| {
            store
                .chat_thread_checkpoint(
                    &t,
                    &caller.run_id,
                    &req.summary,
                    req.through_seq,
                    req.expected_summary_through_seq,
                )
                .map_err(cos_problem)
        })
        .await?;
    state.chat.events.notify_waiters();
    Ok(json_response(
        StatusCode::OK,
        &CheckpointResponse {
            thread_id: saved.thread_id,
            summary_through_seq: saved.summary_through_seq,
        },
    ))
}
