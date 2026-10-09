//! tasks registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{audited, decode_optional, decode_problem};
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
    ("PATCH", "/api/v1/tasks/{id}", "task.update"),
    ("POST", "/api/v1/tasks/{id}/reopen", "task.reopen"),
    ("POST", "/api/v1/tasks/{id}/retry", "task.retry"),
    ("POST", "/api/v1/tasks/{id}/accept", "task.accept"),
    ("POST", "/api/v1/tasks/{id}/approve", "task.approve"),
    ("POST", "/api/v1/tasks/{id}/reject", "task.reject"),
    ("POST", "/api/v1/tasks/{id}/cancel", "task.cancel"),
    ("POST", "/api/v1/tasks/{id}/rereview", "task.rereview"),
    (
        "POST",
        "/api/v1/tasks/{id}/execution/decompose",
        "execution.decompose",
    ),
    ("POST", "/api/v1/tasks/{id}/pause", "task.pause"),
    ("POST", "/api/v1/tasks/{id}/resume", "task.resume"),
    (
        "PUT",
        "/api/v1/tasks/{id}/execution-plan",
        "execution.put_plan",
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
    ("POST", "/api/v1/tasks/{id}/changes/{repo}/integrate"),
    ("POST", "/api/v1/tasks/{id}/changes/{repo}/pr/merge"),
    ("POST", "/api/v1/tasks/{id}/execution-plan"),
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
        "task.accept" | "task.approve" | "task.reject" | "task.cancel" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = parse_task_id(&raw_id)
                .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
            let (note, expected_status) = match matched.action {
                "task.approve" | "task.reject" => {
                    let input: crate::types::DecisionBody =
                        serde_json::from_value(body).map_err(decode)?;
                    (input.note, input.expected_status)
                }
                "task.cancel" => {
                    let input: crate::types::CancelBody =
                        serde_json::from_value(body).map_err(decode)?;
                    (None, input.expected_status)
                }
                _ => {
                    let input: crate::types::ReopenBody =
                        serde_json::from_value(body).map_err(decode)?;
                    (None, input.expected_status)
                }
            };
            audited(crate::handlers::task_actions::gate_action_op(
                store,
                id,
                matched.action,
                note,
                expected_status,
                audit,
            )?)
        }
        "task.rereview" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = parse_task_id(&raw_id)
                .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
            let input: crate::types::ReopenBody = serde_json::from_value(body).map_err(decode)?;
            audited(crate::handlers::task_actions::rereview_op(
                store,
                id,
                input.expected_status,
                Some(audit),
            )?)
        }
        "execution.decompose" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = parse_task_id(&raw_id)
                .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
            let input: task_ops::regate::DecomposeRequest =
                serde_json::from_value(body).map_err(decode)?;
            audited(crate::execution::decompose_op(
                store,
                id,
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
        "task.update" | "task.reopen" | "task.retry" | "task.pause" | "task.resume"
        | "execution.put_plan" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = parse_task_id(&raw_id)
                .map_err(|problem| audit.reject(store, "task", &raw_id, problem))?;
            match matched.action {
                "task.update" => {
                    let input = serde_json::from_value(body).map_err(decode)?;
                    audited(crate::handlers::task_actions::patch_task_op(
                        store,
                        &env.genres,
                        &env.clusters,
                        id,
                        input,
                        Some(audit),
                    )?)
                }
                "task.reopen" => {
                    let input = decode_optional(body).map_err(decode)?;
                    audited(crate::handlers::task_actions::reopen_op(
                        store,
                        id,
                        input,
                        Some(audit),
                    )?)
                }
                "task.retry" => {
                    let input = decode_optional(body).map_err(decode)?;
                    audited(crate::handlers::task_actions::retry_op(
                        store,
                        &env.clusters,
                        id,
                        input,
                        Some(audit),
                    )?)
                }
                "execution.put_plan" => {
                    let input = serde_json::from_value(body).map_err(decode)?;
                    crate::execution::put_plan_audited(store, id, input, env.tree_limits, audit)
                }
                pause => {
                    let _: crate::lifecycle::EmptyBody = decode_optional(body).map_err(decode)?;
                    audited(crate::lifecycle::task_pause_op(
                        store,
                        id,
                        pause == "task.pause",
                        Some(audit),
                    )?)
                }
            }
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in tasks"
        ))),
    }
}
