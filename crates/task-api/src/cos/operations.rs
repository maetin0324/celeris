//! `POST /api/v1/cos/operations` and `GET /api/v1/cos/operations/{o}` (ADR 2026-10-05 D2/D3).
//!
//! A CoS run names an existing domain request (`method` + `/api/v1/...` path + JSON body). Only the
//! registered operations in [`ALLOWED`] run; anything else (external URL, scheme/host, `..`, the
//! `/api/v1/cos` subtree itself, unregistered paths) is rejected with 422 and recorded as a rejected
//! `cos_operations` row with a reasoned audit event. An allowed request runs the same operation
//! function as the domain handler, with an [`OperationAudit`], so the domain write, the
//! `cos_operations` row, the audit envelope event and the chat card are one transaction.
//! Thread, run and operation ids come from the verified credential, never from the body.

use axum::Extension;
use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::chat::{AuditContext, ChatActor, ChatError, CosOperation};
use task_core::store::SqliteStore;
use ulid::Ulid;

use super::{CosCaller, cos_only, cos_problem, reject_identity_claims};
use crate::handlers::{ApiResult, Params, json_response, no_query};
use crate::problem::ApiProblem;
use crate::query::parse_task_id;
use crate::state::ApiState;

/// Registered CoS operations: `(method, path pattern, action)`. `{id}` matches one id segment.
/// The remaining D3 domains (approval・execution・project・knowledge) are added by ops-domains.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    ("POST", "/api/v1/tasks", "task.create"),
    ("POST", "/api/v1/tasks/{id}/comments", "comment.create"),
    ("POST", "/api/v1/decisions/{id}/answer", "decision.answer"),
];

/// Longest path recorded on a rejection (`target_id`); longer input is truncated.
const MAX_RECORDED_PATH: usize = 512;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/cos/operations", post(create_operation))
        .route("/api/v1/cos/operations/{o}", get(get_operation))
}

/// Result of a shared operation function: the handler's own value, or the audited CoS operation.
pub(crate) enum Applied<T> {
    Direct(T),
    Audited(Box<CosOperation>),
}

impl<T> Applied<T> {
    /// The handler path never passes an audit context, so it always gets `Direct`.
    pub(crate) fn direct(self) -> Result<T, ApiProblem> {
        match self {
            Applied::Direct(value) => Ok(value),
            Applied::Audited(_) => Err(ApiProblem::internal(
                "an operation without audit context returned an audited result",
            )),
        }
    }
}

/// Audit context of one `/cos/operations` request, passed to the shared operation functions.
#[derive(Debug, Clone)]
pub(crate) struct OperationAudit {
    pub(crate) ctx: AuditContext,
    pub(crate) idempotency_key: String,
    pub(crate) request_hash: String,
    pub(crate) expected_revision: Option<String>,
    pub(crate) payload: Value,
}

impl OperationAudit {
    /// Run `write` inside the `cos_operation_apply` transaction. All domain writes go through `tx`.
    pub(crate) fn apply<F>(
        &self,
        store: &SqliteStore,
        target_kind: &str,
        target_id: &str,
        action: &str,
        write: F,
    ) -> Result<CosOperation, ApiProblem>
    where
        F: FnOnce(&Transaction<'_>) -> Result<Value, ChatError>,
    {
        store
            .cos_operation_apply(
                &self.ctx,
                &self.idempotency_key,
                &self.request_hash,
                target_kind,
                target_id,
                self.expected_revision.as_deref(),
                action,
                &self.payload,
                |tx, _| write(tx),
            )
            .map_err(cos_problem)
    }

    /// Record `problem` as a rejected operation (row + reasoned audit event) and give it back.
    /// When recording itself fails, that failure is returned instead.
    pub(crate) fn reject(
        &self,
        store: &SqliteStore,
        target_kind: &str,
        target_id: &str,
        problem: ApiProblem,
    ) -> ApiProblem {
        let mut ctx = self.ctx.clone();
        ctx.reason = format!(
            "rejected: {}; requested reason: {}",
            problem.detail(),
            self.ctx.reason
        );
        if ctx.policy_version.trim().is_empty() {
            ctx.policy_version = "unspecified".into();
        }
        let target_id: String = target_id.chars().take(MAX_RECORDED_PATH).collect();
        let target_id = if target_id.trim().is_empty() {
            "-".to_string()
        } else {
            target_id
        };
        let detail = problem.detail().to_string();
        let outcome = store.cos_operation_apply(
            &ctx,
            &self.idempotency_key,
            &self.request_hash,
            target_kind,
            &target_id,
            self.expected_revision.as_deref(),
            "rejected",
            &self.payload,
            |_, _| Err(ChatError::Invalid(detail)),
        );
        match outcome {
            Err(ChatError::Invalid(_)) => problem,
            Err(error) => cos_problem(error),
            Ok(_) => ApiProblem::internal("a CoS rejection was not recorded as rejected"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationBody {
    idempotency_key: String,
    expected_revision: Option<String>,
    reason: String,
    policy_version: String,
    request: OperationRequest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationRequest {
    method: String,
    path: String,
    #[serde(default)]
    body: Value,
}

/// `O` of the ADR D2 table.
#[derive(Debug, Serialize)]
pub(crate) struct OperationView {
    id: String,
    actor: &'static str,
    thread_id: String,
    run_id: String,
    idempotency_key: String,
    request_hash: String,
    target_kind: String,
    target_id: String,
    expected_revision: Option<String>,
    action: String,
    payload: Value,
    reason: String,
    policy_version: String,
    state: String,
    result: Option<Value>,
    event_id: Option<String>,
}

impl From<CosOperation> for OperationView {
    fn from(op: CosOperation) -> Self {
        Self {
            id: op.id,
            actor: "cos",
            thread_id: op.thread_id,
            run_id: op.run_id,
            idempotency_key: op.idempotency_key,
            request_hash: op.request_hash,
            target_kind: op.target_kind,
            target_id: op.target_id,
            expected_revision: op.expected_revision,
            action: op.action,
            payload: op.payload,
            reason: op.reason,
            policy_version: op.policy_version,
            state: op.state,
            result: op.result,
            event_id: op.event_id.map(|id| id.to_string()),
        }
    }
}

fn audited<T>(applied: Applied<T>) -> Result<CosOperation, ApiProblem> {
    match applied {
        Applied::Audited(op) => Ok(*op),
        Applied::Direct(_) => Err(ApiProblem::internal(
            "a CoS operation returned without its audit record",
        )),
    }
}

fn operation_response(op: CosOperation) -> axum::response::Response {
    json_response(
        StatusCode::OK,
        &serde_json::json!({"operation": OperationView::from(op)}),
    )
}

fn unprocessable(code: &'static str, detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, code, detail)
}

/// JSON with object keys sorted at every level, so the hash ignores key order.
fn canonical_json(value: &Value) -> String {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: std::collections::BTreeMap<&String, Value> =
                    map.iter().map(|(k, v)| (k, sort(v))).collect();
                serde_json::to_value(sorted).unwrap_or(Value::Null)
            }
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(value).to_string()
}

/// `request_hash` = sha256 of `METHOD \n path \n canonical body`.
pub(crate) fn request_hash(method: &str, path: &str, body: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(method.as_bytes());
    hasher.update(b"\n");
    hasher.update(path.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical_json(body).as_bytes());
    format!("{:x}", hasher.finalize())
}

/// A registered operation matched from `(method, path)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Matched {
    pub(crate) action: &'static str,
    pub(crate) id: Option<String>,
}

/// Match `(method, path)` against [`ALLOWED`]. `Err` carries the rejection reason.
pub(crate) fn match_operation(method: &str, path: &str) -> Result<Matched, String> {
    if path.contains("://") || !path.starts_with('/') || path.starts_with("//") {
        return Err("request.path must be a local /api/v1/ path, not a URL or host".into());
    }
    if path
        .chars()
        .any(|c| matches!(c, '?' | '#' | '%' | '\\') || c.is_whitespace() || c.is_control())
    {
        return Err("request.path must not carry a query, fragment, escape or whitespace".into());
    }
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err("request.path must be normalized (no empty, `.` or `..` segments)".into());
    }
    if !path.starts_with("/api/v1/") {
        return Err("request.path must be under /api/v1/".into());
    }
    if path == "/api/v1/cos" || path.starts_with("/api/v1/cos/") {
        return Err("request.path must not call the CoS API recursively".into());
    }
    for (allowed_method, pattern, action) in ALLOWED {
        let pattern_segments: Vec<&str> = pattern.split('/').skip(1).collect();
        if pattern_segments.len() != segments.len() {
            continue;
        }
        let mut id = None;
        let matched = pattern_segments
            .iter()
            .zip(&segments)
            .all(|(p, s)| match *p {
                "{id}" => {
                    let ok = s
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                    id = Some((*s).to_string());
                    ok
                }
                literal => literal == *s,
            });
        if matched {
            if !method.eq_ignore_ascii_case(allowed_method) {
                return Err(format!(
                    "{method} {pattern} is not a registered CoS operation"
                ));
            }
            return Ok(Matched { action, id });
        }
    }
    Err(format!("{method} {path} is not a registered CoS operation"))
}

fn body_problem(error: serde_json::Error) -> ApiProblem {
    let detail = error.to_string();
    if detail.starts_with("unknown field") {
        ApiProblem::bad_request(detail)
    } else {
        unprocessable("validation", detail)
    }
}

async fn create_operation(
    State(state): State<ApiState>,
    caller: Option<Extension<CosCaller>>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let Some(Extension(caller)) = caller else {
        return Err(cos_only(
            "/api/v1/cos/operations accepts only a CoS run credential",
        ));
    };
    let value: Value = crate::handlers::read_json(body, false).await?;
    reject_identity_claims(&value)?;
    if value.get("expected_revision").is_none() && value.is_object() {
        return Err(unprocessable(
            "validation",
            "missing field `expected_revision` (send null when there is none)",
        ));
    }
    let req: OperationBody = serde_json::from_value(value).map_err(body_problem)?;
    if req.reason.trim().is_empty() {
        return Err(unprocessable("validation", "reason must not be empty"));
    }
    if req.policy_version.trim().is_empty() {
        return Err(unprocessable(
            "validation",
            "policy_version must not be empty",
        ));
    }
    if req.idempotency_key.trim().is_empty() {
        return Err(unprocessable(
            "validation",
            "idempotency_key must not be empty",
        ));
    }
    let method = req.request.method.to_ascii_uppercase();
    let path = req.request.path.clone();
    let audit = OperationAudit {
        ctx: AuditContext {
            actor: ChatActor::Cos,
            thread_id: caller.thread_id.clone(),
            run_id: caller.run_id.clone(),
            operation_id: Ulid::new().to_string(),
            reason: req.reason.clone(),
            policy_version: req.policy_version.clone(),
        },
        idempotency_key: req.idempotency_key.clone(),
        request_hash: request_hash(&method, &path, &req.request.body),
        expected_revision: req.expected_revision.clone(),
        payload: serde_json::json!({
            "method": method,
            "path": path,
            "body": req.request.body,
        }),
    };
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let operation = state
        .blocking(move |store| {
            if let Some(existing) = store
                .cos_operation_find(&audit.ctx.thread_id, &audit.idempotency_key)
                .map_err(cos_problem)?
            {
                if existing.request_hash != audit.request_hash {
                    return Err(ApiProblem::new(
                        StatusCode::CONFLICT,
                        "chat_conflict",
                        "idempotency_key was used with a different request",
                    ));
                }
                return Ok(existing);
            }
            let matched = match_operation(&method, &path).map_err(|why| {
                audit.reject(
                    store,
                    "api",
                    &path,
                    unprocessable("cos_operation_not_allowed", why),
                )
            })?;
            reject_identity_claims(&req.request.body)
                .map_err(|problem| audit.reject(store, "api", &path, problem))?;
            let body = req.request.body;
            let decode = |error: serde_json::Error| {
                audit.reject(
                    store,
                    "api",
                    &path,
                    unprocessable("validation", format!("request.body: {error}")),
                )
            };
            match matched.action {
                "task.create" => {
                    let input = serde_json::from_value(body).map_err(decode)?;
                    audited(crate::handlers::tasks::create_task_op(
                        store,
                        &roles,
                        &genres,
                        input,
                        Some(&audit),
                    )?)
                }
                "comment.create" => {
                    let raw_id = matched.id.unwrap_or_default();
                    let id = parse_task_id(&raw_id)
                        .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
                    let input = serde_json::from_value(body).map_err(decode)?;
                    audited(crate::handlers::task_actions::create_comment_op(
                        store,
                        id,
                        input,
                        Some(&audit),
                    )?)
                }
                "decision.answer" => {
                    let decision_id = matched.id.unwrap_or_default();
                    let input = serde_json::from_value(body).map_err(decode)?;
                    audited(crate::decisions::answer_op(
                        store,
                        &decision_id,
                        input,
                        Some(&audit),
                    )?)
                }
                other => Err(ApiProblem::internal(format!(
                    "registered CoS operation {other} has no implementation"
                ))),
            }
        })
        .await?;
    state.chat.events.notify_waiters();
    Ok(operation_response(operation))
}

async fn get_operation(
    State(state): State<ApiState>,
    caller: Option<Extension<CosCaller>>,
    Params(o): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let lookup = o.clone();
    let operation = state
        .blocking(move |store| store.cos_operation_get(&lookup).map_err(cos_problem))
        .await?;
    // A CoS run sees only its own thread's operations; others look absent (no id probing).
    let visible = match (&operation, &caller) {
        (Some(op), Some(Extension(caller))) => op.thread_id == caller.thread_id,
        (Some(_), None) => true,
        (None, _) => false,
    };
    match operation {
        Some(op) if visible => Ok(operation_response(op)),
        _ => Err(cos_problem(ChatError::NotFound {
            kind: "operation",
            id: o,
        })),
    }
}
