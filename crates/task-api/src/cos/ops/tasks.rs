//! tasks registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{audited, decode_problem};
use crate::problem::ApiProblem;
use crate::query::parse_task_id;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    ("POST", "/api/v1/tasks", "task.create"),
    ("POST", "/api/v1/tasks/{id}/comments", "comment.create"),
    (
        "POST",
        "/api/v1/tasks/{id}/execution/phase-gate",
        "execution.phase_gate",
    ),
    ("POST", "/api/v1/tasks/{id}/answer", "question.answer"),
    (
        "POST",
        "/api/v1/tasks/{id}/execution/plan-gate",
        "execution.plan_gate",
    ),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[(
    "POST",
    "/api/v1/plans",
    "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
)];

/// Assigned mutations awaiting audited implementation. Move a row to ALLOWED when implemented.
pub(crate) const PENDING: &[(&str, &str)] = &[
    ("PATCH", "/api/v1/tasks/{id}"),
    ("POST", "/api/v1/tasks/{id}/accept"),
    ("POST", "/api/v1/tasks/{id}/approve"),
    ("POST", "/api/v1/tasks/{id}/cancel"),
    ("POST", "/api/v1/tasks/{id}/changes/{repo}/integrate"),
    ("POST", "/api/v1/tasks/{id}/changes/{repo}/pr/merge"),
    ("POST", "/api/v1/tasks/{id}/execution-plan"),
    ("PUT", "/api/v1/tasks/{id}/execution-plan"),
    ("POST", "/api/v1/tasks/{id}/execution/decompose"),
    ("POST", "/api/v1/tasks/{id}/pause"),
    ("POST", "/api/v1/tasks/{id}/reject"),
    ("POST", "/api/v1/tasks/{id}/reopen"),
    ("POST", "/api/v1/tasks/{id}/rereview"),
    ("POST", "/api/v1/tasks/{id}/resume"),
    ("POST", "/api/v1/tasks/{id}/retry"),
    ("POST", "/api/v1/tasks/{id}/tree/adopt"),
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
        "task.create" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::handlers::tasks::create_task_op(
                store,
                &env.roles,
                &env.genres,
                input,
                Some(audit),
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
                Some(audit),
            )?)
        }
        "execution.phase_gate" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = parse_task_id(&raw_id)
                .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::execution::phase_gate_op(
                store,
                id,
                input,
                Some(audit),
            )?)
        }
        "question.answer" | "execution.plan_gate" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = parse_task_id(&raw_id)
                .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
            if matched.action == "question.answer" {
                let input = serde_json::from_value(body).map_err(decode)?;
                audited(crate::handlers::task_actions::answer_op(
                    store, id, input, audit,
                )?)
            } else {
                let input = serde_json::from_value(body).map_err(decode)?;
                audited(crate::execution::plan_gate_op(store, id, input, audit)?)
            }
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in tasks"
        ))),
    }
}
