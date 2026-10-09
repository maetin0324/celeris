//! decisions registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{audited, decode_optional, decode_problem};
use super::super::operations::{dispatch as dispatch_operation, unprocessable};
use crate::problem::ApiProblem;
use axum::http::StatusCode;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    ("POST", "/api/v1/decisions/{id}/answer", "decision.answer"),
    ("POST", "/api/v1/approvals/{id}/decide", "approval.decide"),
    ("POST", "/api/v1/knowledge/inbox", "knowledge.record"),
    (
        "POST",
        "/api/v1/knowledge/inbox/{id}/reject",
        "knowledge.reject",
    ),
    ("POST", "/api/v1/inbox/items/{id}/answer", "inbox.answer"),
    ("POST", "/api/v1/decisions/{id}/revise", "decision.revise"),
    (
        "POST",
        "/api/v1/decisions/{id}/withdraw",
        "decision.withdraw",
    ),
    (
        "POST",
        "/api/v1/knowledge/inbox/{id}/accept",
        "knowledge.accept",
    ),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[];

/// Assigned mutations awaiting audited implementation. Move a row to ALLOWED when implemented.
pub(crate) const PENDING: &[(&str, &str)] = &[
    ("PUT", "/api/v1/knowledge/page"),
    ("POST", "/api/v1/notifications/read-all"),
    ("POST", "/api/v1/notifications/{id}/read"),
];

pub(crate) fn dispatch(
    store: &SqliteStore,
    env: &DispatchEnv,
    audit: &OperationAudit,
    matched: Matched,
    path: &str,
    body: Value,
) -> Result<CosOperation, ApiProblem> {
    let decode = |error| decode_problem(store, audit, path, error);
    match matched.action {
        "decision.answer" => {
            let decision_id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::decisions::answer_op(
                store,
                &decision_id,
                input,
                Some(audit),
            )?)
        }
        "decision.revise" => {
            let decision_id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::decisions::revise_op(
                store,
                &decision_id,
                input,
                Some(audit),
            )?)
        }
        "decision.withdraw" => {
            let decision_id = matched.id.unwrap_or_default();
            let input = decode_optional(body).map_err(decode)?;
            audited(crate::decisions::withdraw_op(
                store,
                &decision_id,
                input,
                Some(audit),
            )?)
        }
        "knowledge.accept" => {
            let raw_id = matched.id.unwrap_or_default();
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge", &raw_id, problem))?;
            let input = decode_optional(body).map_err(decode)?;
            audited(crate::knowledge::accept_op(
                store,
                &root,
                raw_id,
                input,
                Some(audit),
            )?)
        }
        "approval.decide" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = raw_id.parse().map_err(|_| {
                audit.reject(
                    store,
                    "approval",
                    &raw_id,
                    ApiProblem::new(
                        StatusCode::NOT_FOUND,
                        "approval_not_found",
                        format!("no approval {raw_id}"),
                    ),
                )
            })?;
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::approvals::decide_op(store, id, input, Some(audit))?)
        }
        "knowledge.reject" => {
            let raw_id = matched.id.unwrap_or_default();
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge", &raw_id, problem))?;
            if !body.is_null() && body != serde_json::json!({}) {
                return Err(audit.reject(
                    store,
                    "knowledge",
                    &raw_id,
                    unprocessable("validation", "request.body must be empty"),
                ));
            }
            audited(crate::knowledge::reject_op(
                store,
                &root,
                raw_id,
                Some(audit),
            )?)
        }
        "knowledge.record" => {
            let root = env
                .kb_root
                .clone()
                .map_err(|problem| audit.reject(store, "knowledge", "new", problem))?;
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::knowledge::record_op(
                store,
                &root,
                None,
                input,
                Some(audit),
            )?)
        }
        "inbox.answer" => {
            let raw_id = matched.id.unwrap_or_default();
            let input: crate::inbox_notifications::InboxAnswerBody =
                serde_json::from_value(body).map_err(decode)?;
            let found = env
                .inbox_feed
                .as_ref()
                .and_then(|items| items.iter().find(|item| item.id == raw_id))
                .ok_or_else(|| {
                    audit.reject(
                        store,
                        "inbox_item",
                        &raw_id,
                        ApiProblem::new(
                            StatusCode::CONFLICT,
                            "cos_revision_conflict",
                            format!("inbox item {raw_id} is not open (answered or changed)"),
                        ),
                    )
                })?;
            let (dm, dp, domain_body) =
                crate::inbox_notifications::delegated_request(found, &input)
                    .map_err(|problem| audit.reject(store, "inbox_item", &raw_id, problem))?;
            dispatch_operation(store, env, audit, dm.as_str(), &dp, domain_body)
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in decisions"
        ))),
    }
}
