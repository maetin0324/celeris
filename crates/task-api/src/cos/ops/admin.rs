//! admin registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{decode_problem, path_param, unprocessable};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "PUT",
        "/api/v1/llm/models/assignments/{source}/{tier}",
        "model_assignment.put",
    ),
    (
        "DELETE",
        "/api/v1/llm/models/assignments/{source}/{tier}",
        "model_assignment.delete",
    ),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[
    (
        "DELETE",
        "/api/v1/accounts/{id}/login",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/accounts/{id}/login",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/accounts/{id}/login/code",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "DELETE",
        "/api/v1/clusters/{id}/connect",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/clusters/{id}/connect",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "POST",
        "/api/v1/clusters/{id}/connect/code",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "DELETE",
        "/api/v1/secrets/{id}",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
    (
        "PUT",
        "/api/v1/secrets/{id}",
        "secret_operations: 人の決定 secrets=exclude。秘密値を扱う系列を全て除外（取消・削除も含む）。",
    ),
];

/// Assigned mutations awaiting audited implementation. Move a row to ALLOWED when implemented.
pub(crate) const PENDING: &[(&str, &str)] = &[
    ("POST", "/api/v1/accounts"),
    ("DELETE", "/api/v1/accounts/{id}"),
    ("POST", "/api/v1/accounts/{id}/check"),
    ("PUT", "/api/v1/clusters/{id}/settings"),
    ("POST", "/api/v1/cron-jobs"),
    ("DELETE", "/api/v1/cron-jobs/{id}"),
    ("PATCH", "/api/v1/cron-jobs/{id}"),
    ("POST", "/api/v1/cron-jobs/{id}/pause"),
    ("POST", "/api/v1/cron-jobs/{id}/resume"),
    ("POST", "/api/v1/cron-jobs/{id}/run"),
    ("POST", "/api/v1/llm/models/assignments/preview"),
    ("PUT", "/api/v1/llm/models/assignments/roles/{tier}"),
    (
        "POST",
        "/api/v1/llm/models/assignments/roles/{tier}/preview",
    ),
    ("POST", "/api/v1/llm/models/discover"),
    ("DELETE", "/api/v1/llm/models/{source}/{model_id}/override"),
    ("PUT", "/api/v1/llm/models/{source}/{model_id}/override"),
    ("POST", "/api/v1/notify/test"),
    ("POST", "/api/v1/org"),
    ("DELETE", "/api/v1/org/{id}"),
    ("PATCH", "/api/v1/org/{id}"),
    ("PATCH", "/api/v1/org/{id}/browser-settings"),
    ("POST", "/api/v1/org/{id}/skills"),
    ("DELETE", "/api/v1/org/{id}/skills/{skill}"),
    ("POST", "/api/v1/projects/{id}/repos"),
    ("POST", "/api/v1/providers"),
    ("DELETE", "/api/v1/providers/{id}"),
    ("PATCH", "/api/v1/providers/{id}"),
    ("POST", "/api/v1/providers/{id}/check"),
    ("POST", "/api/v1/releases/{sha12}/promote"),
    ("POST", "/api/v1/reload"),
    ("POST", "/api/v1/replay"),
    ("DELETE", "/api/v1/repos/{id}"),
    ("PATCH", "/api/v1/repos/{id}"),
    ("DELETE", "/api/v1/skills/{name}"),
    ("PUT", "/api/v1/skills/{name}"),
];

const ASSIGNMENT: &str = "/api/v1/llm/models/assignments/{source}/{tier}";

pub(crate) fn dispatch(
    store: &SqliteStore,
    _env: &DispatchEnv,
    audit: &OperationAudit,
    matched: Matched,
    path: &str,
    body: Value,
) -> Result<CosOperation, ApiProblem> {
    let decode = |error| decode_problem(store, audit, path, error);
    match matched.action {
        "model_assignment.put" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            crate::model_assignments::put_assignment_audited(
                store,
                &path_param(ASSIGNMENT, path, "{source}"),
                &path_param(ASSIGNMENT, path, "{tier}"),
                input,
                audit,
            )
        }
        "model_assignment.delete" => {
            if !body.is_null() && body != serde_json::json!({}) {
                return Err(audit.reject(
                    store,
                    "model_assignment",
                    path,
                    unprocessable("validation", "request.body must be empty"),
                ));
            }
            crate::model_assignments::delete_assignment_audited(
                store,
                &path_param(ASSIGNMENT, path, "{source}"),
                &path_param(ASSIGNMENT, path, "{tier}"),
                audit,
            )
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in admin"
        ))),
    }
}
