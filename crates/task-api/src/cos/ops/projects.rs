//! projects registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{audited, decode_optional, decode_problem};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations.
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    ("PATCH", "/api/v1/projects/{id}", "project.update"),
    ("POST", "/api/v1/projects/{id}/cancel", "project.cancel"),
    ("POST", "/api/v1/projects/{id}/pause", "project.pause"),
    ("POST", "/api/v1/projects/{id}/resume", "project.resume"),
    ("POST", "/api/v1/projects/{id}/archive", "project.archive"),
    (
        "POST",
        "/api/v1/projects/{id}/unarchive",
        "project.unarchive",
    ),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[
    (
        "PATCH",
        "/api/v1/milestones/{id}",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/milestones/{id}/cancel",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/milestones/{id}/decide",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/milestones/{id}/pause",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/milestones/{id}/resume",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/projects/{id}/milestones",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/projects/{id}/plan",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
    (
        "POST",
        "/api/v1/projects/{id}/project-plan/{version}/decide",
        "removed_by_adr_0079: ADR-0079 D13/R5a で撤去済み。人にも変更できない 410 の互換入口を復活させない。",
    ),
];

/// Assigned mutations awaiting audited implementation. Move a row to ALLOWED when implemented.
pub(crate) const PENDING: &[(&str, &str)] = &[
    ("POST", "/api/v1/projects"),
    ("POST", "/api/v1/projects/{id}/docs/init"),
    ("POST", "/api/v1/projects/{id}/docs/maintenance"),
    ("DELETE", "/api/v1/projects/{id}/docs/page"),
    ("PUT", "/api/v1/projects/{id}/docs/page"),
    ("POST", "/api/v1/reports/notified"),
    ("POST", "/api/v1/reports/read"),
    ("POST", "/api/v1/standing-rules"),
    ("DELETE", "/api/v1/standing-rules/{id}"),
];

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
        "project.update" => {
            let raw_id = matched.id.unwrap_or_default();
            let id = crate::handlers::parse_project_id(&raw_id)
                .map_err(|problem| audit.reject(store, "project", &raw_id, problem))?;
            let input = serde_json::from_value(body).map_err(decode)?;
            audited(crate::handlers::projects::cos_patch_project(
                store, id, input, audit,
            )?)
        }
        "project.cancel" | "project.pause" | "project.resume" | "project.archive"
        | "project.unarchive" => {
            use task_ops::lifecycle::ProjectAction;
            let action = match matched.action {
                "project.cancel" => ProjectAction::Cancel,
                "project.pause" => ProjectAction::Pause,
                "project.resume" => ProjectAction::Resume,
                "project.archive" => ProjectAction::Archive,
                _ => ProjectAction::Unarchive,
            };
            let raw_id = matched.id.unwrap_or_default();
            let id = crate::handlers::parse_project_id(&raw_id)
                .map_err(|problem| audit.reject(store, "project", &raw_id, problem))?;
            let crate::lifecycle::EmptyBody {} = decode_optional(body).map_err(decode)?;
            audited(crate::lifecycle::project_action_op(
                store,
                id,
                action,
                Some(audit),
            )?)
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in projects"
        ))),
    }
}
