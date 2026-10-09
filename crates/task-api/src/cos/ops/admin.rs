//! admin registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{
    audited, decode_optional, decode_problem, path_param, unprocessable,
};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations. The first match wins, so a literal segment
/// (`assignments/roles/{tier}`) is listed before the placeholder row it would also match.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "PUT",
        "/api/v1/llm/models/assignments/roles/{tier}",
        "model_role.replace",
    ),
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
    (
        "POST",
        "/api/v1/llm/models/assignments/preview",
        "model_assignment.preview",
    ),
    (
        "POST",
        "/api/v1/llm/models/assignments/roles/{tier}/preview",
        "model_role.preview",
    ),
    (
        "PUT",
        "/api/v1/llm/models/{source}/{model_id}/override",
        "model_override.put",
    ),
    (
        "DELETE",
        "/api/v1/llm/models/{source}/{model_id}/override",
        "model_override.delete",
    ),
    ("POST", "/api/v1/cron-jobs", "cron_job.create"),
    ("PATCH", "/api/v1/cron-jobs/{id}", "cron_job.update"),
    ("DELETE", "/api/v1/cron-jobs/{id}", "cron_job.delete"),
    ("POST", "/api/v1/cron-jobs/{id}/pause", "cron_job.pause"),
    ("POST", "/api/v1/cron-jobs/{id}/resume", "cron_job.resume"),
    ("POST", "/api/v1/cron-jobs/{id}/run", "cron_job.run"),
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
    ("POST", "/api/v1/llm/models/discover"),
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
const ROLE: &str = "/api/v1/llm/models/assignments/roles/{tier}";
const OVERRIDE: &str = "/api/v1/llm/models/{source}/{model_id}/override";

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ApiProblem> {
    serde_json::to_value(value).map_err(|e| ApiProblem::internal(e.to_string()))
}

/// The CoS form of a route whose domain request has no body: `null` or `{}` only.
fn require_empty_body(
    store: &SqliteStore,
    audit: &OperationAudit,
    kind: &str,
    path: &str,
    body: &Value,
) -> Result<(), ApiProblem> {
    if body.is_null() || *body == serde_json::json!({}) {
        return Ok(());
    }
    Err(audit.reject(
        store,
        kind,
        path,
        unprocessable("validation", "request.body must be empty"),
    ))
}

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
            require_empty_body(store, audit, "model_assignment", path, &body)?;
            crate::model_assignments::delete_assignment_audited(
                store,
                &path_param(ASSIGNMENT, path, "{source}"),
                &path_param(ASSIGNMENT, path, "{tier}"),
                audit,
            )
        }
        "model_assignment.preview" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            let impact = crate::model_assignments::ImpactEnv::of(&env.api);
            let outcome = crate::model_assignments::preview_impact(store, &impact, &input)
                .and_then(|response| to_value(&response));
            audit.record(store, "model_assignment", path, matched.action, outcome)
        }
        "model_role.replace" | "model_role.preview" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            let impact = crate::model_assignments::ImpactEnv::of(&env.api);
            crate::model_assignments::edit_role_audited(
                store,
                &impact,
                &path_param(ROLE, path, "{tier}"),
                input,
                matched.action == "model_role.preview",
                audit,
            )
        }
        "model_override.put" => {
            let input = decode_optional(body).map_err(decode)?;
            let routing = env.api.inner.routing_catalog.as_ref().map(|r| r.view());
            audited(crate::model_catalog::put_override_op(
                store,
                routing.as_ref(),
                &path_param(OVERRIDE, path, "{source}"),
                &path_param(OVERRIDE, path, "{model_id}"),
                input,
                Some(audit),
            )?)
        }
        "model_override.delete" => {
            require_empty_body(store, audit, "model_override", path, &body)?;
            audited(crate::model_catalog::delete_override_op(
                store,
                &path_param(OVERRIDE, path, "{source}"),
                &path_param(OVERRIDE, path, "{model_id}"),
                Some(audit),
            )?)
        }
        "cron_job.create" => {
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::cron_jobs::create_job_op(
                store,
                &env.roles,
                &env.genres,
                input,
                Some(audit),
            )?)
        }
        "cron_job.update" => {
            let input = decode_optional(body).map_err(decode)?;
            audited(crate::cron_jobs::update_job_op(
                store,
                &env.roles,
                &env.genres,
                &matched.id.unwrap_or_default(),
                input,
                Some(audit),
            )?)
        }
        "cron_job.delete" | "cron_job.pause" | "cron_job.resume" | "cron_job.run" => {
            require_empty_body(store, audit, "cron_job", path, &body)?;
            let key = matched.id.unwrap_or_default();
            match matched.action {
                "cron_job.delete" => {
                    audited(crate::cron_jobs::delete_job_op(store, &key, Some(audit))?)
                }
                "cron_job.pause" => {
                    audited(crate::cron_jobs::pause_job_op(store, &key, Some(audit))?)
                }
                "cron_job.resume" => {
                    audited(crate::cron_jobs::resume_job_op(store, &key, Some(audit))?)
                }
                _ => audited(crate::cron_jobs::run_job_op(
                    store,
                    &env.roles,
                    &env.genres,
                    &key,
                    Some(audit),
                    time::OffsetDateTime::now_utc(),
                )?),
            }
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in admin"
        ))),
    }
}
