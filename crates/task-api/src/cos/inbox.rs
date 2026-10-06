//! `GET /api/v1/cos/inbox` and `POST /api/v1/cos/inbox/{i}/resolve` (ADR 2026-10-05 D3).
//!
//! A CoS run settles one triage item with `outcome=answer|observe|escalate`. The API decides only
//! structure, credential, the current revision and explicit `human_required`; the judgment itself
//! is the worker's. `answer` turns the existing [`InboxAnswerBody`] into the source's domain request
//! and runs it through [`super::operations::dispatch`], the same operation layer as
//! `/cos/operations`, so the domain validation is never bypassed. The item outcome, the
//! `cos_operations` row, its audit envelope event and its chat card are one transaction.
//! `escalate` validates the escalation packet and claims the durable outbox row; the notifier sends.

use axum::Extension;
use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use task_core::chat::{AuditContext, ChatActor, ChatError};
use task_core::store::SqliteStore;
use task_core::{TaskId, TaskStore};
use task_ops::human_inbox::{InboxItem, InboxKind};
use time::OffsetDateTime;
use ulid::Ulid;

use super::operations::{
    DispatchEnv, ItemMark, Matched, OperationAudit, OperationView, dispatch, request_hash,
};
use super::triage_view::{self, CosInboxItem};
use super::{CosCaller, cos_only, cos_problem, reject_identity_claims};
use crate::handlers::{ApiResult, Params, json_response};
use crate::inbox_notifications::{InboxAnswerBody, delegated_request, human_feed};
use crate::problem::{ApiProblem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;

/// Item states of `cos_inbox_items` accepted by `GET /cos/inbox?state=`.
const STATES: &[&str] = &[
    "pending",
    "running",
    "answered",
    "observed",
    "escalated",
    "fallback",
    "resolved",
];
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
/// Option key of a free-text question: the human answers in the web screen.
pub const REPLY_OPTION: &str = "reply";
/// Task labels a person sets to keep every wait of the task with the human.
const HUMAN_REQUIRED_LABELS: &[&str] = &["human_required", "human-required"];
const MAX_SUMMARY_CHARS: usize = 600;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/cos/inbox", get(list))
        .route("/api/v1/cos/inbox/{i}/resolve", post(resolve))
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CosInboxList {
    pub items: Vec<CosInboxItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolveOutcome {
    Answer,
    Observe,
    Escalate,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EscalationOption {
    pub key: String,
    pub label: String,
}

/// D3 escalation packet. `recommended` is one of `options[].key` or null (then the reason says why).
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EscalationPacket {
    pub summary: String,
    pub options: Vec<EscalationOption>,
    pub recommended: Option<String>,
    pub recommendation_reason: String,
    pub web_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveBody {
    pub idempotency_key: String,
    /// The `source_revision` of the item the worker judged.
    pub expected_revision: String,
    pub outcome: ResolveOutcome,
    pub reason: String,
    pub policy_version: String,
    #[serde(default)]
    pub answer: Option<InboxAnswerBody>,
    #[serde(default)]
    pub escalation: Option<EscalationPacket>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ResolveResponse {
    pub item: Option<CosInboxItem>,
    pub operation: OperationView,
    /// The outbox notification of `escalate` (null when the item was already routed).
    pub notification_id: Option<String>,
}

fn unprocessable(code: &'static str, detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, code, detail)
}

fn revision_conflict(detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::CONFLICT, "cos_inbox_revision_conflict", detail)
}

pub(crate) fn human_required_problem(why: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::FORBIDDEN,
        "cos_human_required",
        format!("this wait is explicitly human_required ({why}); escalate it instead"),
    )
}

async fn list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let q = QueryParams::parse(raw.as_deref(), &["state", "limit"])?;
    let filter = q.single("state")?.map(str::to_owned);
    if let Some(s) = filter.as_deref().filter(|s| !STATES.contains(s)) {
        return Err(ApiProblem::bad_request(format!("unknown item state: {s}")));
    }
    let limit = match q.single("limit")? {
        None => DEFAULT_LIMIT,
        Some(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| ApiProblem::bad_request(format!("limit must be 1..={MAX_LIMIT}")))?,
    };
    let db = triage_db(&state)?;
    let items = state
        .blocking(move |_| triage_view::items(&db, filter.as_deref(), limit).map_err(cos_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &CosInboxList { items }))
}

/// The derived human-inbox item a triage source points at. Inbox sources use the derived item id as
/// `source_key` (or `<kind>-<key>`); notices have no inbox item.
fn source_item<'a>(items: &'a [InboxItem], item: &CosInboxItem) -> Option<&'a InboxItem> {
    if item.source_kind == "notice" {
        return None;
    }
    let prefixed = format!("{}-{}", item.source_kind, item.source_key);
    items
        .iter()
        .find(|x| x.id == item.source_key || x.id == prefixed)
}

/// The same-app path the escalation link must point at, derived from the target only.
pub(crate) fn derived_web_path(item: &CosInboxItem, source: Option<&InboxItem>) -> String {
    match source {
        Some(found) => match &found.task {
            Some(task) => format!("/tasks/{}", task.id),
            None => "/inbox".to_string(),
        },
        None if item.source_kind == "notice" => "/notifications".to_string(),
        None => "/inbox".to_string(),
    }
}

/// Explicit `human_required` (D3「人に回す基準」): a human check (acceptance with `Check::Human`
/// waiting for its verdict), or any wait — decision, approval, plan/phase gate — of a task the person
/// marked with a `human_required` label. Policy skills and config cannot lift it. Shared by resolve
/// and `/cos/operations`.
pub(crate) fn human_required_reason(
    store: &SqliteStore,
    kind: Option<InboxKind>,
    task_id: Option<TaskId>,
) -> Result<Option<String>, ApiProblem> {
    if kind == Some(InboxKind::AcceptanceCheck) {
        return Ok(Some("human check".into()));
    }
    let Some(task_id) = task_id else {
        return Ok(None);
    };
    let Some(task) = store.get(task_id).map_err(store_problem)? else {
        return Ok(None);
    };
    if task
        .labels
        .iter()
        .any(|l| HUMAN_REQUIRED_LABELS.contains(&l.as_str()))
    {
        return Ok(Some("the task is labeled human_required".into()));
    }
    Ok(None)
}

/// `human_required` of a `/cos/operations` request that answers a wait (decision・approval・gate).
pub(crate) async fn human_required_for_operation(
    state: &ApiState,
    matched: &Matched,
    path: &str,
) -> Result<Option<String>, ApiProblem> {
    if !matches!(
        matched.action,
        "decision.answer" | "approval.decide" | "execution.phase_gate"
    ) {
        return Ok(None);
    }
    let feed = human_feed(state).await?;
    let found = feed.items.iter().find(|x| {
        x.answer
            .native
            .as_ref()
            .is_some_and(|native| native.path == path)
    });
    let kind = found
        .map(|x| x.kind)
        .or_else(|| (matched.action == "execution.phase_gate").then_some(InboxKind::PhaseGate));
    let task_id = found
        .and_then(|x| x.task.as_ref().map(|t| t.id))
        .or_else(|| {
            (matched.action == "execution.phase_gate")
                .then(|| matched.id.as_deref().and_then(|id| id.parse().ok()))
                .flatten()
        });
    state
        .blocking(move |store| human_required_reason(store, kind, task_id))
        .await
}

fn validate_packet(
    packet: &EscalationPacket,
    item: &CosInboxItem,
    source: Option<&InboxItem>,
) -> Result<(), ApiProblem> {
    let invalid = |detail: String| unprocessable("cos_escalation_invalid", detail);
    let summary = packet.summary.trim();
    if summary.is_empty() || summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(invalid(format!(
            "escalation.summary must be 1..={MAX_SUMMARY_CHARS} characters"
        )));
    }
    if packet.options.is_empty() {
        return Err(invalid(
            "escalation.options must list the current question's options (free text: key=reply)"
                .into(),
        ));
    }
    let free_text = source.is_none_or(|s| {
        s.kind == InboxKind::Question
            || s.options.is_empty()
            || s.options.iter().any(|o| o.needs_note)
    });
    let mut seen = std::collections::HashSet::new();
    for option in &packet.options {
        if option.label.trim().is_empty() {
            return Err(invalid(format!("option {} has an empty label", option.key)));
        }
        if !seen.insert(option.key.as_str()) {
            return Err(invalid(format!("option key {} is repeated", option.key)));
        }
        let offered = source.is_some_and(|s| s.options.iter().any(|o| o.key == option.key));
        if !offered && !(option.key == REPLY_OPTION && free_text) {
            return Err(invalid(format!(
                "option key {} is not an option of the current question",
                option.key
            )));
        }
    }
    if let Some(recommended) = &packet.recommended
        && !packet.options.iter().any(|o| &o.key == recommended)
    {
        return Err(invalid(format!(
            "escalation.recommended {recommended} is not one of escalation.options"
        )));
    }
    if packet.recommendation_reason.trim().is_empty() {
        return Err(invalid(
            "escalation.recommendation_reason must say why (also when recommended is null)".into(),
        ));
    }
    let web_path = packet.web_path.as_str();
    if !web_path.starts_with('/') || web_path.starts_with("//") || web_path.contains("://") {
        return Err(invalid(
            "escalation.web_path must be a path inside the Celeris web app, not a URL".into(),
        ));
    }
    let expected = derived_web_path(item, source);
    if web_path != expected {
        return Err(invalid(format!(
            "escalation.web_path must be {expected} (derived from the target)"
        )));
    }
    Ok(())
}

fn body_problem(error: serde_json::Error) -> ApiProblem {
    let detail = error.to_string();
    if detail.starts_with("unknown field") {
        ApiProblem::bad_request(detail)
    } else {
        unprocessable("validation", detail)
    }
}

async fn resolve(
    State(state): State<ApiState>,
    caller: Option<Extension<CosCaller>>,
    Params(i): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    crate::handlers::no_query(&raw)?;
    let Some(Extension(caller)) = caller else {
        return Err(cos_only(
            "/api/v1/cos/inbox/{i}/resolve accepts only a CoS run credential; people answer in /api/v1/inbox",
        ));
    };
    let value: Value = crate::handlers::read_json(body, false).await?;
    reject_identity_claims(&value)?;
    let req: ResolveBody = serde_json::from_value(value.clone()).map_err(body_problem)?;
    for (name, field) in [
        ("reason", &req.reason),
        ("policy_version", &req.policy_version),
        ("idempotency_key", &req.idempotency_key),
        ("expected_revision", &req.expected_revision),
    ] {
        if field.trim().is_empty() {
            return Err(unprocessable(
                "validation",
                format!("{name} must not be empty"),
            ));
        }
    }
    let path = format!("/api/v1/cos/inbox/{i}/resolve");
    let action = match req.outcome {
        ResolveOutcome::Answer => "inbox.answer",
        ResolveOutcome::Observe => "inbox.observe",
        ResolveOutcome::Escalate => "inbox.escalate",
    };
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
        request_hash: request_hash("POST", &path, &value),
        expected_revision: Some(req.expected_revision.clone()),
        payload: value,
        item: None,
    };
    // The derived inbox is built before the blocking section; the store re-checks the item state.
    let feed = human_feed(&state).await?;
    let env = DispatchEnv::of(&state);
    let item_key = i.clone();
    let lookup = i.clone();
    let db = triage_db(&state)?;
    let lookup_db = db.clone();
    let (operation, notification_id) = state
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
                return Ok((existing, None));
            }
            let rejecter = audit.clone();
            let reject =
                |problem: ApiProblem| rejecter.reject(store, "cos_inbox_item", &item_key, problem);
            let Some(item) = triage_view::item_get(&db, &i).map_err(cos_problem)? else {
                return Err(reject(cos_problem(ChatError::NotFound {
                    kind: "triage item",
                    id: i.clone(),
                })));
            };
            if item.is_terminal() {
                return Err(reject(revision_conflict(format!(
                    "triage item {i} is already {}",
                    item.state
                ))));
            }
            if item.source_revision != req.expected_revision {
                return Err(reject(revision_conflict(format!(
                    "triage item {i} is revision {}, expected {}",
                    item.source_revision, req.expected_revision
                ))));
            }
            if triage_view::superseded(&db, &item).map_err(cos_problem)? {
                return Err(reject(revision_conflict(format!(
                    "a newer revision of {}:{} was ingested; judge that one",
                    item.source_kind, item.source_key
                ))));
            }
            let source = source_item(&feed.items, &item);
            let inbox_source = item.source_kind != "notice";
            match req.outcome {
                ResolveOutcome::Answer => {
                    if req.escalation.is_some() {
                        return Err(reject(unprocessable(
                            "validation",
                            "escalation must be null for outcome=answer",
                        )));
                    }
                    let Some(input) = req.answer.as_ref() else {
                        return Err(reject(unprocessable(
                            "validation",
                            "answer is required for outcome=answer",
                        )));
                    };
                    let Some(found) = source else {
                        return Err(reject(if inbox_source {
                            revision_conflict(format!(
                                "the source wait of {i} is no longer open (answered or changed)"
                            ))
                        } else {
                            unprocessable("validation", "a notice has nothing to answer")
                        }));
                    };
                    if let Some(why) = human_required_reason(
                        store,
                        Some(found.kind),
                        found.task.as_ref().map(|t| t.id),
                    )? {
                        return Err(reject(human_required_problem(&why)));
                    }
                    let Some(selected) = found.options.iter().find(|o| o.key == input.option)
                    else {
                        return Err(reject(unprocessable(
                            "validation",
                            "answer.option is not offered by this inbox item",
                        )));
                    };
                    if selected.needs_note
                        && input.note.as_deref().is_none_or(|s| s.trim().is_empty())
                    {
                        return Err(reject(unprocessable(
                            "validation",
                            "answer.note is required for this option",
                        )));
                    }
                    let (method, domain_path, domain_body) =
                        delegated_request(found, input).map_err(&reject)?;
                    audit.item = Some(ItemMark {
                        item_id: item.id.clone(),
                        outcome: "answered",
                    });
                    let op = dispatch(
                        store,
                        &env,
                        &audit,
                        method.as_str(),
                        &domain_path,
                        domain_body,
                    )?;
                    Ok((op, None))
                }
                ResolveOutcome::Observe => {
                    if req.escalation.is_some() || req.answer.is_some() {
                        return Err(reject(unprocessable(
                            "validation",
                            "answer and escalation must be null for outcome=observe",
                        )));
                    }
                    if source.is_some() {
                        return Err(reject(unprocessable(
                            "cos_observe_needs_judgment",
                            "the source is an unresolved wait that needs a judgment; answer or escalate it",
                        )));
                    }
                    audit.item = Some(ItemMark {
                        item_id: item.id.clone(),
                        outcome: "observed",
                    });
                    let op = audit.apply(store, "cos_inbox_item", &item.id, action, |_| {
                        Ok(serde_json::json!({"item_id": item.id, "outcome": "observed"}))
                    })?;
                    Ok((op, None))
                }
                ResolveOutcome::Escalate => {
                    if req.answer.is_some() {
                        return Err(reject(unprocessable(
                            "validation",
                            "answer must be null for outcome=escalate",
                        )));
                    }
                    let Some(packet) = req.escalation.as_ref() else {
                        return Err(reject(unprocessable(
                            "cos_escalation_invalid",
                            "escalation is required for outcome=escalate",
                        )));
                    };
                    if inbox_source && source.is_none() {
                        return Err(reject(revision_conflict(format!(
                            "the source wait of {i} is no longer open (answered or changed)"
                        ))));
                    }
                    validate_packet(packet, &item, source).map_err(&reject)?;
                    let outbox = serde_json::json!({
                        "item_id": item.id,
                        "source_kind": item.source_kind,
                        "source_key": item.source_key,
                        "source_revision": item.source_revision,
                        "operation_id": audit.ctx.operation_id,
                        "summary": packet.summary,
                        "options": packet.options,
                        "recommended": packet.recommended,
                        "recommendation_reason": packet.recommendation_reason,
                        "blocking": source.map(|s| s.blocking.summary.clone()),
                        "web_path": packet.web_path,
                    });
                    // Claim first: a crash after the claim leaves the outbox, and a retried resolve
                    // finds the route taken and still records the outcome.
                    let notification_id = store
                        .cos_triage_outbox_claim(
                            &item.id,
                            "escalation",
                            &outbox.to_string(),
                            OffsetDateTime::now_utc(),
                        )
                        .map_err(cos_problem)?;
                    audit.item = Some(ItemMark {
                        item_id: item.id.clone(),
                        outcome: "escalated",
                    });
                    let result = serde_json::json!({
                        "item_id": item.id,
                        "outcome": "escalated",
                        "notification_id": notification_id,
                        "already_routed": notification_id.is_none(),
                    });
                    let op = audit.apply(store, "cos_inbox_item", &item.id, action, |_| {
                        Ok(result)
                    })?;
                    Ok((op, notification_id))
                }
            }
        })
        .await?;
    state.chat.events.notify_waiters();
    let item = state
        .blocking(move |_| triage_view::item_get(&lookup_db, &lookup).map_err(cos_problem))
        .await?;
    Ok(json_response(
        StatusCode::OK,
        &ResolveResponse {
            item,
            operation: OperationView::from(operation),
            notification_id,
        },
    ))
}

/// The daemon DB the triage view reads (the API's own connection; see [`triage_view`]).
fn triage_db(state: &ApiState) -> Result<std::path::PathBuf, ApiProblem> {
    state.chat.attachment_db_path.clone().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "cos_inbox_unavailable",
            "the CoS inbox needs a file-backed database",
        )
    })
}
