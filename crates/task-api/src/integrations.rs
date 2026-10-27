//! Human API for stranded task branches and their durable integration state.

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{RepoId, TaskId, TaskRepoIntegrationState, TaskRepoIntegrationStatus, TaskStore};

use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::query::{QueryParams, parse_task_id};
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/integrations", get(list))
        .route("/api/v1/tasks/{id}/integrations", get(task_integrations))
        .route("/api/v1/tasks/{id}/integrations/{repo}/merge", post(merge))
        .route(
            "/api/v1/tasks/{id}/integrations/{repo}/discard",
            post(discard),
        )
        .route("/api/v1/tasks/{id}/integrations/{repo}/hold", post(hold))
        .route(
            "/api/v1/tasks/{id}/integrations/{repo}/reopen",
            post(reopen),
        )
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IntegrationList {
    pub items: Vec<TaskRepoIntegrationState>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReasonBody {
    pub reason: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct IntegrationActionResult {
    pub state: TaskRepoIntegrationState,
    /// `true` means the task head is an ancestor of the configured default branch.
    pub integrated: bool,
    pub reason: Option<String>,
}

async fn list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["state"])?;
    let filter = query
        .single("state")?
        .map(|value| {
            TaskRepoIntegrationStatus::parse(value)
                .ok_or_else(|| ApiProblem::bad_request("unknown integration state"))
        })
        .transpose()?;
    let items = state
        .blocking(move |store| store.task_integration_list(filter).map_err(store_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &IntegrationList { items }))
}

async fn task_integrations(
    State(state): State<ApiState>,
    Params(raw_id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let task_id = parse_task_id(&raw_id)?;
    let items = state
        .blocking(move |store| {
            if store.get(task_id).map_err(store_problem)?.is_none() {
                return Err(ApiProblem::task_not_found(task_id));
            }
            Ok(store
                .task_integration_list(None)
                .map_err(store_problem)?
                .into_iter()
                .filter(|i| i.task_id == task_id)
                .collect::<Vec<_>>())
        })
        .await?;
    Ok(json_response(StatusCode::OK, &IntegrationList { items }))
}

async fn discard(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    headers: HeaderMap,
    Params((raw_id, repo)): Params<(String, String)>,
    body: Body,
) -> ApiResult {
    let input: ReasonBody = read_json(body, false).await?;
    disposition(
        state,
        headers,
        caller.actor(),
        raw_id,
        repo,
        input.reason,
        true,
    )
    .await
}
async fn hold(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    headers: HeaderMap,
    Params((raw_id, repo)): Params<(String, String)>,
    body: Body,
) -> ApiResult {
    let input: ReasonBody = read_json(body, false).await?;
    disposition(
        state,
        headers,
        caller.actor(),
        raw_id,
        repo,
        input.reason,
        false,
    )
    .await
}
async fn reopen(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    headers: HeaderMap,
    Params((raw_id, repo)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_human(&state, &headers)?;
    mutate_simple(state, caller.actor(), raw_id, repo, Action::Reopen).await
}
async fn merge(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    headers: HeaderMap,
    Params((raw_id, repo)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_human(&state, &headers)?;
    mutate_simple(state, caller.actor(), raw_id, repo, Action::Merge).await
}

async fn disposition(
    state: ApiState,
    headers: HeaderMap,
    by: String,
    raw_id: String,
    repo: String,
    reason: String,
    discard: bool,
) -> ApiResult {
    require_human(&state, &headers)?;
    if reason.trim().is_empty() {
        return Err(ApiProblem::bad_request("reason is required"));
    }
    let id = parse_task_id(&raw_id)?;
    let repo_name = repo;
    let now = time::OffsetDateTime::now_utc();
    let result = state
        .blocking(move |store| {
            let repo_id = resolve_repo(store, id, &repo_name)?;
            let result = if discard {
                task_ops::stranded::discard(store, id, repo_id, &reason, &by, now)
            } else {
                task_ops::stranded::hold(store, id, repo_id, &reason, &by, now)
            };
            result.map_err(invalid_problem)?;
            state_row(store, id, repo_id)
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        &IntegrationActionResult {
            state: result,
            integrated: false,
            reason: None,
        },
    ))
}

/// Integration dispositions are human decisions. The shared admin helper also accepts CoS run
/// credentials for some API surfaces, so explicitly reject those credentials here.
fn require_human(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiProblem> {
    if crate::cos::cos_bearer_secret(headers).is_some() {
        return Err(ApiProblem::unauthorized());
    }
    require_admin(state, headers)
}

#[derive(Clone, Copy)]
enum Action {
    Reopen,
    Merge,
}
async fn mutate_simple(
    state: ApiState,
    by: String,
    raw_id: String,
    repo: String,
    action: Action,
) -> ApiResult {
    let id = parse_task_id(&raw_id)?;
    let now = time::OffsetDateTime::now_utc();
    let roots = state.inner.view.workspace_roots.clone();
    let result = state
        .blocking(move |store| {
            let repo_id = resolve_repo(store, id, &repo)?;
            let (integrated, reason) = match action {
                Action::Reopen => {
                    task_ops::stranded::reopen(store, id, repo_id, &by, now)
                        .map_err(invalid_problem)?;
                    (false, None)
                }
                Action::Merge => {
                    let task = store
                        .get(id)
                        .map_err(store_problem)?
                        .ok_or_else(|| ApiProblem::task_not_found(id))?;
                    let marker = crate::tree::marker_of(store, &roots, &task)?;
                    let marker_repo = crate::tree::marker_repos(&marker)
                        .into_iter()
                        .find(|r| r.name == repo)
                        .ok_or_else(|| {
                            ApiProblem::file_not_found(format!("repo not found in task: {repo}"))
                        })?;
                    let row = task
                        .project_id
                        .map(|project| store.repo_list(project))
                        .transpose()
                        .map_err(store_problem)?
                        .unwrap_or_default()
                        .into_iter()
                        .find(|r| r.id == repo_id);
                    let path = std::path::PathBuf::from(marker_repo.source);
                    let default_branch = task_ops::changes::resolve_default_branch(
                        &path,
                        row.as_ref().and_then(|r| r.default_branch.as_deref()),
                    )
                    .map_err(|error| {
                        ApiProblem::new(
                            StatusCode::CONFLICT,
                            "default_branch_unresolvable",
                            format!("{repo}: {error}"),
                        )
                    })?;
                    let outcome = task_ops::stranded::merge(
                        store,
                        &task_ops::stranded::MergeRequest {
                            task_id: id,
                            repo_id,
                            repo: &path,
                            branch: &state_row(store, id, repo_id)?.branch,
                            head: &state_row(store, id, repo_id)?.head_sha,
                            default_branch: &default_branch,
                            actor: &by,
                            now,
                        },
                    )
                    .map_err(invalid_problem)?;
                    match outcome {
                        task_ops::stranded::MergeOutcome::Integrated
                        | task_ops::stranded::MergeOutcome::AlreadyIntegrated => (true, None),
                        task_ops::stranded::MergeOutcome::NotMerged { reason } => {
                            (false, Some(reason))
                        }
                        task_ops::stranded::MergeOutcome::Busy { reason } => {
                            return Err(ApiProblem::new(
                                StatusCode::CONFLICT,
                                "checkout_busy",
                                reason,
                            ));
                        }
                    }
                }
            };
            Ok((state_row(store, id, repo_id)?, integrated, reason))
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        &IntegrationActionResult {
            state: result.0,
            integrated: result.1,
            reason: result.2,
        },
    ))
}

fn resolve_repo(store: &dyn TaskStore, id: TaskId, name: &str) -> Result<RepoId, ApiProblem> {
    let task = store
        .get(id)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::task_not_found(id))?;
    let mut repos = match task.project_id {
        Some(project) => store.repo_list(project).map_err(store_problem)?,
        None => Vec::new(),
    };
    // ADR 2026-10-11 D3: a task without a project was resolved by the sweep through a registered
    // repository's `celeris/<task id>` branch; its integration rows name that repository.
    for state in store.task_integration_list(None).map_err(store_problem)? {
        if state.task_id == id
            && let Some(repo) = store.repo_get(state.repo_id).map_err(store_problem)?
        {
            repos.push(repo);
        }
    }
    repos
        .into_iter()
        .find(|repo| repo.name == name)
        .map(|repo| repo.id)
        .ok_or_else(|| ApiProblem::file_not_found(format!("repo not found: {name}")))
}

fn state_row(
    store: &dyn TaskStore,
    id: TaskId,
    repo: RepoId,
) -> Result<TaskRepoIntegrationState, ApiProblem> {
    store
        .task_integration_get(id, repo)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::file_not_found("task integration not found"))
}
fn invalid_problem(error: task_core::StoreError) -> ApiProblem {
    match error {
        task_core::StoreError::Invalid(message) if message.contains("not found") => {
            ApiProblem::file_not_found(message)
        }
        task_core::StoreError::Invalid(message) if message.starts_with("checkout_busy:") => {
            ApiProblem::new(StatusCode::CONFLICT, "checkout_busy", message)
        }
        task_core::StoreError::Invalid(message) => {
            ApiProblem::new(StatusCode::CONFLICT, "invalid_transition", message)
        }
        other => store_problem(other),
    }
}
