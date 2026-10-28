//! ADR 2026-10-28-cos-integrate-task D1/D2: `POST /api/v1/tasks/{id}/integration-requests/{request}/integrate-task`.
//!
//! Files one ordinary root task that resolves the conflict of an open delivery integration request,
//! and links it to the original task (`Event::IntegrateTaskRequested`) in the creating transaction.
//! The worker merges and resolves, the acceptance checks and the reviewer decide, and the delivery
//! lands it; neither this route nor CoS touches git. People call the route with the admin
//! credential; CoS reaches the same [`integrate_task_op`] through `/cos/operations`
//! (`integration.integrate_task`).

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde::Deserialize;
use task_core::integration_request::{DELIVERY_ORIGIN, IntegrateTaskLink};
use task_core::store::SqliteStore;
use task_core::{Event, Task, TaskId, TaskStore};

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::query::parse_task_id;
use crate::state::ApiState;

pub(crate) const PATTERN: &str = "/api/v1/tasks/{id}/integration-requests/{request}/integrate-task";

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route(PATTERN, post(integrate_task))
}

/// Body of the route (and of the inbox's `integrate_task` answer).
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntegrateTaskBody {
    /// A note for the worker, copied into the new task's objective.
    #[serde(default)]
    pub note: Option<String>,
}

async fn integrate_task(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    headers: HeaderMap,
    Params((id, request)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let input: IntegrateTaskBody = read_json(body, true).await?;
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let by = caller.actor();
    let task = state
        .blocking(move |store| {
            integrate_task_op(store, &roles, &genres, task_id, &request, input, &by, None)?.direct()
        })
        .await?;
    Ok(json_response(StatusCode::CREATED, &task))
}

fn problem(status: StatusCode, code: &'static str, detail: String) -> ApiProblem {
    ApiProblem::new(status, code, detail)
}

/// D1/D2: validate the request and the link, then create the task and its link in one transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn integrate_task_op(
    store: &SqliteStore,
    roles: &[task_core::RoleSpec],
    genres: &[task_core::GenreSpec],
    task_id: TaskId,
    request_id: &str,
    input: IntegrateTaskBody,
    by: &str,
    audit: Option<&OperationAudit>,
) -> Result<Applied<Task>, ApiProblem> {
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "task", &task_id.to_string(), problem),
        None => problem,
    };
    let original = store.get(task_id).map_err(store_problem)?.ok_or_else(|| {
        reject(problem(
            StatusCode::NOT_FOUND,
            "task_not_found",
            format!("task {task_id} not found"),
        ))
    })?;
    let open = task_ops::integrate_task::open_requests(store)
        .map_err(store_problem)?
        .into_iter()
        .find(|o| o.task_id == task_id && o.request_id == request_id)
        .ok_or_else(|| {
            reject(problem(
                StatusCode::GONE,
                "integration_request_gone",
                format!("task {task_id} has no open integration request {request_id}"),
            ))
        })?;
    if open.origin != DELIVERY_ORIGIN {
        return Err(reject(problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "integrate_task_unsupported_origin",
            format!(
                "integration request {request_id} comes from {} (an integrate task resolves delivery requests only)",
                open.origin
            ),
        )));
    }
    match task_ops::integrate_task::link_for(store, task_id, request_id).map_err(store_problem)? {
        None => {}
        Some(IntegrateTaskLink::Escalated {
            task_id: linked, ..
        }) => {
            return Err(reject(problem(
                StatusCode::CONFLICT,
                "integrate_task_escalated",
                format!("integrate task {linked} failed for this request; a person decides it"),
            )));
        }
        Some(linked) => {
            return Err(reject(problem(
                StatusCode::CONFLICT,
                "integrate_task_exists",
                format!(
                    "integrate task {} is already linked to this request",
                    linked.task_id()
                ),
            )));
        }
    }
    if original.project_id.is_none() || original.repos.is_empty() {
        return Err(reject(problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            task_ops::repo_requirement::REPOSITORY_REQUIRED_CODE,
            format!("task {task_id} has no project repository to integrate"),
        )));
    }
    let checks = task_ops::integrate_task::repo_checks(store, &original).map_err(store_problem)?;
    let spec = task_ops::integrate_task::spec(
        &original,
        &open.request,
        request_id,
        &checks,
        input.note.as_deref(),
    )
    .map_err(|e| reject(ApiProblem::internal(e.to_string())))?;
    let body = crate::handlers::tasks::NewTaskBody::from_spec(spec);
    let request_id = request_id.to_owned();
    let link_by = by.to_owned();
    let link = move |integrate_task_id: TaskId| {
        (
            task_id,
            Event::IntegrateTaskRequested {
                request_id: request_id.clone(),
                integrate_task_id,
                by: link_by.clone(),
            },
        )
    };
    crate::handlers::tasks::create_task_linked_op(
        store,
        roles,
        genres,
        body,
        None,
        by,
        audit,
        "integration.integrate_task",
        Some(&link),
    )
}
