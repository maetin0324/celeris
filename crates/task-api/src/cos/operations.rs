//! `POST /api/v1/cos/operations` and `GET /api/v1/cos/operations/{o}` (ADR 2026-10-05 D2/D3).
//!
//! A CoS run names an existing domain request (`method` + `/api/v1/...` path + JSON body). Only the
//! registered operations in [`allowed`] run; anything else (external URL, scheme/host, `..`, the
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
use crate::state::ApiState;

/// Concatenate the domain-owned allowlists without a second list to maintain.
pub(crate) fn allowed()
-> impl Iterator<Item = (usize, &'static (&'static str, &'static str, &'static str))> {
    super::ops::REGISTRIES
        .iter()
        .enumerate()
        .flat_map(|(index, registry)| registry.allowed.iter().map(move |row| (index, row)))
}

/// Problem code of an `instructed_by` that is not the human message the caller's run answers
/// in the inbox thread (D3: triage runs, earlier or foreign messages and other threads are refused).
pub const INSTRUCTION_INVALID: &str = "cos_instruction_invalid";

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

/// Outcome of an external effect (C): `Ok(Ok(v))` done, `Ok(Err(problem))` refused before any
/// change (settled `rejected`), `Err(problem)` outcome unknown (left pending for remediation).
pub(crate) type Effect<T> = Result<Result<T, ApiProblem>, ApiProblem>;

/// The handler's view of an [`Effect`]: either way a failure is the route's problem.
pub(crate) fn effect_result<T>(effect: Effect<T>) -> Result<T, ApiProblem> {
    effect.and_then(|result| result)
}

/// An [`Effect`] with its value serialized for the operation record.
pub(crate) fn effect_value<T: Serialize>(effect: Effect<T>) -> Effect<Value> {
    effect.and_then(|result| match result {
        Ok(value) => serde_json::to_value(value)
            .map(Ok)
            .map_err(|e| ApiProblem::internal(e.to_string())),
        Err(problem) => Ok(Err(problem)),
    })
}

/// Run a route's file effect (`providers.d`, account directories) directly, or with `audit`
/// inside the CoS operation transaction as its last step: a failure rolls the record back to a
/// rejected row, so an applied record always has its file written.
pub(crate) fn file_op<T: Serialize, F>(
    store: &SqliteStore,
    audit: Option<&OperationAudit>,
    target_kind: &str,
    target_id: &str,
    action: &str,
    effect: F,
) -> Result<Applied<T>, ApiProblem>
where
    F: FnOnce() -> Result<T, ApiProblem>,
{
    let Some(audit) = audit else {
        return effect().map(Applied::Direct);
    };
    let operation = audit.apply_checked(store, target_kind, target_id, action, |_| {
        serde_json::to_value(effect()?).map_err(|e| ApiProblem::internal(e.to_string()))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// A C operation whose effect is one store write that either commits or changes nothing (a
/// browser control transition, a run start): any failure of `effect` settles `rejected`.
pub(crate) fn external_store<T: Serialize>(
    store: &SqliteStore,
    audit: &OperationAudit,
    target: (&str, &str),
    action: &str,
    effect: impl FnOnce() -> Result<T, ApiProblem>,
) -> Result<CosOperation, ApiProblem> {
    audit.external(
        store,
        target.0,
        target.1,
        action,
        time::OffsetDateTime::now_utc(),
        || effect_value(Ok(effect())),
    )
}

/// Audit context of one `/cos/operations` request, passed to the shared operation functions.
#[derive(Debug, Clone)]
pub(crate) struct OperationAudit {
    pub(crate) ctx: AuditContext,
    pub(crate) idempotency_key: String,
    pub(crate) request_hash: String,
    pub(crate) expected_revision: Option<String>,
    pub(crate) payload: Value,
    /// Set by `/cos/inbox/{i}/resolve`: the triage item and its outcome, marked in the same
    /// transaction as the domain write and the audit record.
    pub(crate) item: Option<ItemMark>,
}

/// The triage item a resolve request settles (ADR 2026-10-05 D3).
#[derive(Debug, Clone)]
pub(crate) struct ItemMark {
    pub(crate) item_id: String,
    pub(crate) outcome: &'static str,
}

impl OperationAudit {
    /// Persist the first half of a C-class operation before its external effect. `now` is
    /// injected so retries and stale-pending behavior can be tested without wall-clock waits.
    pub(crate) fn begin_external(
        &self,
        store: &SqliteStore,
        target_kind: &str,
        target_id: &str,
        action: &str,
        now: time::OffsetDateTime,
    ) -> Result<(CosOperation, bool), ApiProblem> {
        store
            .cos_operation_begin_external(
                &self.ctx,
                &self.idempotency_key,
                &self.request_hash,
                target_kind,
                target_id,
                self.expected_revision.as_deref(),
                action,
                &self.payload,
                now,
            )
            .map_err(cos_problem)
    }

    /// Record the known result after an external effect has completed.
    pub(crate) fn finish_external(
        &self,
        store: &SqliteStore,
        result: &Value,
        now: time::OffsetDateTime,
    ) -> Result<CosOperation, ApiProblem> {
        store
            .cos_operation_finish_external(&self.ctx, result, now)
            .map_err(cos_problem)
    }

    /// Settle the pending C-class operation as `rejected` with `problem` when the external
    /// system refused the effect before changing anything; returns `problem` for the caller.
    pub(crate) fn fail_external(
        &self,
        store: &SqliteStore,
        problem: ApiProblem,
        now: time::OffsetDateTime,
    ) -> ApiProblem {
        let result = serde_json::json!({"error": problem.code(), "detail": problem.detail()});
        match store.cos_operation_fail_external(&self.ctx, &result, now) {
            Ok(_) => problem,
            Err(error) => cos_problem(error),
        }
    }

    /// Run a C-class operation (ADR 2026-10-09-cos-operations-external-effects): persist the
    /// pending record, run `effect` once, and settle it. A resent request returns the recorded
    /// operation without running `effect`. `Ok(Err(problem))` from `effect` means the external
    /// system refused before any change (settled `rejected`); `Err(problem)` means the outcome is
    /// unknown, so the record stays pending for remediation.
    pub(crate) fn external<F>(
        &self,
        store: &SqliteStore,
        target_kind: &str,
        target_id: &str,
        action: &str,
        now: time::OffsetDateTime,
        effect: F,
    ) -> Result<CosOperation, ApiProblem>
    where
        F: FnOnce() -> Result<Result<Value, ApiProblem>, ApiProblem>,
    {
        let (operation, fresh) = self.begin_external(store, target_kind, target_id, action, now)?;
        if !fresh {
            return Ok(operation);
        }
        match effect()? {
            Ok(result) => self.finish_external(store, &result, time::OffsetDateTime::now_utc()),
            Err(problem) => {
                Err(self.fail_external(store, problem, time::OffsetDateTime::now_utc()))
            }
        }
    }

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
                |tx, ctx| {
                    let result = write(tx)?;
                    if let Some(mark) = &self.item {
                        super::triage_view::mark_tx(
                            tx,
                            &mark.item_id,
                            mark.outcome,
                            &ctx.operation_id,
                            &ctx.reason,
                            time::OffsetDateTime::now_utc(),
                        )?;
                    }
                    Ok(result)
                },
            )
            .map_err(cos_problem)
    }

    /// [`Self::apply`] for domain writes that fail with an [`ApiProblem`]: a failure of `write`
    /// rolls the transaction back, `cos_operation_apply` records the rejected row with its detail,
    /// and the caller gets the same problem (status and code) the direct route would return.
    pub(crate) fn apply_checked<F>(
        &self,
        store: &SqliteStore,
        target_kind: &str,
        target_id: &str,
        action: &str,
        write: F,
    ) -> Result<CosOperation, ApiProblem>
    where
        F: FnOnce(&Transaction<'_>) -> Result<Value, ApiProblem>,
    {
        let mut refused: Option<ApiProblem> = None;
        let outcome = self.apply(store, target_kind, target_id, action, |tx| {
            write(tx).map_err(|problem| {
                let detail = problem.detail().to_string();
                refused = Some(problem);
                ChatError::Invalid(detail)
            })
        });
        match (outcome, refused) {
            (Ok(operation), _) => Ok(operation),
            (Err(_), Some(problem)) | (Err(problem), None) => Err(problem),
        }
    }

    /// Record a value-only operation (a preview that writes nothing) or the validation problem
    /// that refused it.
    pub(crate) fn record(
        &self,
        store: &SqliteStore,
        target_kind: &str,
        target_id: &str,
        action: &str,
        outcome: Result<Value, ApiProblem>,
    ) -> Result<CosOperation, ApiProblem> {
        match outcome {
            Ok(value) => self.apply(store, target_kind, target_id, action, |_| Ok(value)),
            Err(problem) => Err(self.reject(store, target_kind, target_id, problem)),
        }
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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationBody {
    idempotency_key: String,
    expected_revision: Option<String>,
    reason: String,
    policy_version: String,
    request: OperationRequest,
    /// ADR 2026-10-07-cos-inbox-thread-conversation D3: the id of the human message this
    /// operation relays. Accepted only when it is the `role=user` input message of the caller's
    /// own run in the inbox thread (`kind=inbox`); any other message is 422
    /// `cos_instruction_invalid`. Recorded in the payload and the audit reason; waives the
    /// explicit human_required refusal (the person decided, CoS only relays).
    #[serde(default)]
    instructed_by: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationRequest {
    method: String,
    path: String,
    #[serde(default)]
    body: Value,
}

/// `O` of the ADR D2 table.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct OperationView {
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

pub(crate) fn audited<T>(applied: Applied<T>) -> Result<CosOperation, ApiProblem> {
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

pub(crate) fn unprocessable(code: &'static str, detail: impl Into<String>) -> ApiProblem {
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
    pub(crate) registry: usize,
}

/// Match `(method, path)` against [`allowed`]. `Err` carries the rejection reason.
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
    for registry in super::ops::REGISTRIES {
        for (excluded_method, pattern, reason) in registry.excluded {
            if method.eq_ignore_ascii_case(excluded_method) && path_matches(pattern, path) {
                return Err((*reason).into());
            }
        }
    }
    if path == "/api/v1/cos" || path.starts_with("/api/v1/cos/") {
        return Err("recursive_cos: request.path must not call the CoS API recursively".into());
    }
    for (index, (allowed_method, pattern, action)) in allowed() {
        if method.eq_ignore_ascii_case(allowed_method) && path_matches(pattern, path) {
            let id = pattern
                .split('/')
                .zip(path.split('/'))
                .find_map(|(p, s)| (p == "{id}").then(|| s.to_string()));
            return Ok(Matched {
                action,
                id,
                registry: index,
            });
        }
    }
    Err(format!("{method} {path} is not a registered CoS operation"))
}

/// All named placeholders match exactly one safe path segment, regardless of parameter name.
pub(crate) fn path_matches(pattern: &str, path: &str) -> bool {
    let pattern_segments: Vec<_> = pattern.split('/').collect();
    let segments: Vec<_> = path.split('/').collect();
    pattern_segments.len() == segments.len()
        && pattern_segments.iter().zip(&segments).all(|(p, s)| {
            if p.starts_with('{') && p.ends_with('}') {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
            } else {
                p == s
            }
        })
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
    let mut audit = OperationAudit {
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
        item: None,
    };
    if match_operation(&method, &path).is_err() {
        audit.payload["body"] = serde_json::json!({"redacted": true});
    }
    let mut env = DispatchEnv::of(&state);
    let matched = match_operation(&method, &path).ok();
    let mut human_required = match &matched {
        Some(matched) => super::inbox::human_required_for_operation(&state, matched, &path).await?,
        None => None,
    };
    if matched.as_ref().is_some_and(|m| m.action == "inbox.answer") {
        // The delegated domain path decides human_required, exactly as a direct call would.
        let feed = crate::inbox_notifications::human_feed(&state).await?;
        if let Some(found) = matched
            .as_ref()
            .and_then(|m| m.id.as_deref())
            .and_then(|id| feed.items.iter().find(|item| item.id == id))
            && let Ok(input) = serde_json::from_value::<crate::inbox_notifications::InboxAnswerBody>(
                req.request.body.clone(),
            )
            && let Ok((dm, dp, _)) = crate::inbox_notifications::delegated_request(found, &input)
            && let Ok(delegated) = match_operation(dm.as_str(), &dp)
        {
            human_required =
                super::inbox::human_required_for_operation(&state, &delegated, &dp).await?;
        }
        env.inbox_feed = Some(std::sync::Arc::new(feed.items));
    }
    let instructed_by = req.instructed_by.clone();
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
            // Refuse excluded/unknown routes before interpreting a relayed instruction or body.
            // The registry reason is authoritative and no secret body reaches domain validation.
            if matched.is_none() {
                return dispatch(store, &env, &audit, &method, &path, req.request.body);
            }
            if let Some(message_id) = instructed_by.as_deref() {
                // D3: the instruction must be the human message that started this very run, in
                // the inbox thread. Checked from the caller's run (`CosCaller.run_id`), never from
                // the request alone: a triage run (system input), an earlier message of the thread,
                // a message of another thread or a human thread cannot waive human_required.
                let message = match instructed_message(store, &audit.ctx, message_id) {
                    Ok(message) => message,
                    Err(why) => {
                        return Err(audit.reject(
                            store,
                            "api",
                            &path,
                            unprocessable(INSTRUCTION_INVALID, why),
                        ));
                    }
                };
                audit.ctx.reason = format!("人の指示（seq {}）: {}", message.seq, audit.ctx.reason);
                if let Value::Object(payload) = &mut audit.payload {
                    payload.insert(
                        "instructed_by".into(),
                        serde_json::json!({"message_id": message.id, "seq": message.seq}),
                    );
                }
                // The person decided; CoS only relays. The domain validation still applies.
                human_required = None;
            }
            if let Some(why) = human_required {
                return Err(audit.reject(
                    store,
                    "api",
                    &path,
                    super::inbox::human_required_problem(&why),
                ));
            }
            dispatch(store, &env, &audit, &method, &path, req.request.body)
        })
        .await?;
    state.chat.events.notify_waiters();
    Ok(operation_response(operation))
}

/// D3: the message `instructed_by` names, when it is the `role=user` input of the caller's run
/// in the inbox thread. Otherwise the reason it is refused (`cos_instruction_invalid`).
fn instructed_message(
    store: &SqliteStore,
    ctx: &AuditContext,
    message_id: &str,
) -> Result<task_core::chat::ChatMessage, String> {
    let thread_id = &ctx.thread_id;
    let Ok(message) = store.chat_message_get(thread_id, message_id) else {
        return Err(format!(
            "instructed_by {message_id} is not a message of thread {thread_id}"
        ));
    };
    if message.role != task_core::chat::ChatMessageRole::User {
        return Err(format!("instructed_by {message_id} is not a human message"));
    }
    let thread = match store.chat_thread_get(thread_id) {
        Ok(Some(thread)) => thread,
        _ => return Err(format!("thread {thread_id} is not readable")),
    };
    if thread.kind != task_core::chat::ChatThreadKind::Inbox {
        return Err(format!(
            "instructed_by is accepted only in the inbox thread (thread {thread_id} is {:?})",
            thread.kind
        ));
    }
    let Ok(run) = store.chat_run_get(thread_id, &ctx.run_id) else {
        return Err(format!(
            "run {} is not a run of thread {thread_id}",
            ctx.run_id
        ));
    };
    if run.input_message_id != message.id {
        return Err(format!(
            "instructed_by {message_id} is not the human message run {} answers (its input is {})",
            ctx.run_id, run.input_message_id
        ));
    }
    Ok(message)
}

/// What the operation functions need from [`ApiState`], captured before the blocking section.
#[derive(Clone)]
pub(crate) struct DispatchEnv {
    pub(crate) roles: Vec<task_core::RoleSpec>,
    pub(crate) genres: Vec<task_core::GenreSpec>,
    pub(crate) kb_root: Result<std::path::PathBuf, ApiProblem>,
    /// Configured cluster ids (workspace validation of task edit/retry).
    pub(crate) clusters: Vec<String>,
    /// Effective `[execution.tree]` limits (execution-plan replan validation).
    pub(crate) tree_limits: task_core::TreeLimits,
    /// The derived human inbox, when the operation is `inbox.answer` (built before blocking).
    pub(crate) inbox_feed: Option<std::sync::Arc<Vec<task_ops::human_inbox::InboxItem>>>,
    /// The API state, for operations whose effect is API-process state (`report.notified`).
    pub(crate) api: ApiState,
}

impl DispatchEnv {
    pub(crate) fn of(state: &ApiState) -> Self {
        Self {
            roles: state.inner.roles.clone(),
            genres: state.inner.genres.clone(),
            kb_root: crate::knowledge::root_of(state),
            clusters: crate::handlers::cluster_ids(state),
            tree_limits: state.inner.tree_limits,
            inbox_feed: None,
            api: state.clone(),
        }
    }
}

/// Match `(method, path)` against [`allowed`] and run the handler's shared operation function with
/// `audit`. `/cos/operations` and the `answer` outcome of `/cos/inbox/{i}/resolve` both come here, so
/// the domain validation is the same on both routes.
pub(crate) fn dispatch(
    store: &SqliteStore,
    env: &DispatchEnv,
    audit: &OperationAudit,
    method: &str,
    path: &str,
    body: Value,
) -> Result<CosOperation, ApiProblem> {
    let matched = match_operation(method, path).map_err(|why| {
        let mut rejected_audit = audit.clone();
        rejected_audit.payload["body"] = serde_json::json!({"redacted": true});
        rejected_audit.reject(
            store,
            "api",
            path,
            unprocessable("cos_operation_not_allowed", why),
        )
    })?;
    reject_identity_claims(&body).map_err(|problem| audit.reject(store, "api", path, problem))?;
    (super::ops::REGISTRIES[matched.registry].dispatch)(store, env, audit, matched, path, body)
}

/// The value of placeholder `name` of the matched `pattern` in `path` (e.g. `{source}`).
pub(crate) fn path_param(pattern: &str, path: &str, name: &str) -> String {
    pattern
        .split('/')
        .zip(path.split('/'))
        .find_map(|(p, s)| (p == name).then(|| s.to_string()))
        .unwrap_or_default()
}

/// Decode a body the domain route accepts empty (`read_json(body, true)`): `null` reads as `{}`.
pub(crate) fn decode_optional<T: serde::de::DeserializeOwned>(
    body: Value,
) -> Result<T, serde_json::Error> {
    if body.is_null() {
        serde_json::from_value(serde_json::json!({}))
    } else {
        serde_json::from_value(body)
    }
}

pub(crate) fn decode_problem(
    store: &SqliteStore,
    audit: &OperationAudit,
    path: &str,
    error: serde_json::Error,
) -> ApiProblem {
    audit.reject(
        store,
        "api",
        path,
        unprocessable("validation", format!("request.body: {error}")),
    )
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

#[cfg(test)]
mod skill_table_tests {
    use std::collections::BTreeSet;

    /// ADR 2026-10-08-cos-chat-prompt-cache D1.7/T5: the cos-operator skill's table of registered
    /// operations (`config/skills/cos-operator/operations.md`) lists exactly [`super::allowed`].
    #[test]
    fn cos_operator_skill_table_matches_allowed() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/skills/cos-operator/operations.md"
        );
        let text = std::fs::read_to_string(path).expect("operations.md");
        let section = text
            .split("## 登録済みの操作")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("registered operations section");
        let mut listed = BTreeSet::new();
        for line in section.lines().filter(|l| l.starts_with("| ")) {
            let cells: Vec<&str> = line.split(" | ").collect();
            let (Some(route), Some(action)) = (cells.get(1), cells.get(2)) else {
                continue;
            };
            let Some(route) = route.strip_prefix('`').and_then(|r| r.strip_suffix('`')) else {
                continue;
            };
            let Some((method, path)) = route.split_once(' ') else {
                continue;
            };
            let path = path
                .split('/')
                .map(
                    |seg| match seg.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
                        Some(name) => format!("{{{name}}}"),
                        None => seg.to_string(),
                    },
                )
                .collect::<Vec<_>>()
                .join("/");
            listed.insert((method.to_string(), path, action.trim().to_string()));
        }
        let allowed: BTreeSet<_> = super::allowed()
            .map(|(_, (m, p, a))| (m.to_string(), p.to_string(), a.to_string()))
            .collect();
        assert_eq!(listed, allowed);
    }
}
