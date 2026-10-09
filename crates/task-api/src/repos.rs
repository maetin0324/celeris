//! 案件のリポジトリ（ADR-0043 D1。Phase 52）:
//! `GET|POST /projects/{id}/repos`、`PATCH|DELETE /repos/{id}`。
//!
//! - 読み取り（`GET`）は他の読み取りと同じで無認証でよい。
//! - **管理系**（`token_file` 未設定でも 401）: `POST` / `PATCH` / `DELETE`。案件の作業場所を変えるのは
//!   「人がリポジトリを登録する」操作なので、`POST /projects` と同じ規律にする。
//! - 検証は `task_core::repos::validate_upsert`（決定的。LLM も I/O も無い）+ `[[clusters]]` の存在確認。
//! - 未終端のタスクが参照しているリポジトリは消せない（409）。

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse as _;
use axum::routing::{get, patch};
use task_core::{ProjectRepo, RepoId, RepoKind, RepoRun, TaskStore};
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{
    ApiResult, Params, json_response, no_query, parse_project_id, read_json, validated_workspace,
};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;
use crate::types::{RepoCreateBody, RepoList, RepoPatchBody, ValidationError};
use task_core::store::SqliteStore;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/projects/{id}/repos",
            get(list_repos).post(create_repo),
        )
        .route("/api/v1/repos/{id}", patch(patch_repo).delete(delete_repo))
}

fn parse_repo_id(raw: &str) -> Result<RepoId, ApiProblem> {
    raw.parse::<RepoId>()
        .map_err(|_| ApiProblem::bad_request(format!("invalid repo id: {raw}")))
}

fn repo_not_found(id: RepoId) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "repo_not_found",
        format!("repo not found: {id}"),
    )
}

async fn list_repos(
    axum::extract::State(state): axum::extract::State<ApiState>,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let project_id = parse_project_id(&id)?;
    let items = state
        .blocking(move |store| {
            if store
                .project_get(project_id)
                .map_err(store_problem)?
                .is_none()
            {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            }
            store.repo_list(project_id).map_err(store_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &RepoList { items }))
}

async fn create_repo(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let create: RepoCreateBody = read_json(body, false).await?;
    let repo = plan_create(&state, &id, create)?;
    let created = state
        .blocking(move |store| create_repo_op(store, repo, None)?.direct())
        .await?;
    Ok(json_response(StatusCode::CREATED, &created))
}

/// `POST /projects/{id}/repos`: validate the body into the repo to create.
pub(crate) fn plan_create(
    state: &ApiState,
    project: &str,
    create: RepoCreateBody,
) -> Result<ProjectRepo, ApiProblem> {
    let project_id = parse_project_id(project)?;
    // ADR-0039 D5 / ADR-0043 D1: `~` を展開し、知らないクラスタは 422。
    let location = validated_workspace(state, create.location)?;
    let name = match create.name {
        Some(name) => name.trim().to_string(),
        None => task_core::default_repo_name(&location),
    };
    check_name(&name)?;
    Ok(ProjectRepo {
        id: RepoId::new(),
        project_id,
        name,
        kind: create
            .kind
            .unwrap_or_else(|| task_core::store::detect_repo_kind(&location)),
        location,
        default_branch: create.default_branch.filter(|b| !b.trim().is_empty()),
        sync: create.sync,
        run: create.run.unwrap_or(RepoRun::Auto),
        is_primary: create.is_primary,
        created_at: OffsetDateTime::now_utc(),
    })
}

fn check_name(name: &str) -> Result<(), ApiProblem> {
    if task_core::valid_repo_name(name) {
        return Ok(());
    }
    Err(ApiProblem::validation(vec![ValidationError {
        field: Some("name".into()),
        message: format!("repo name must be a lowercase slug ([a-z0-9._-], 1..64 chars): {name:?}"),
    }]))
}

fn repo_value(
    store: &task_core::store::SqliteStore,
    id: RepoId,
) -> Result<ProjectRepo, ApiProblem> {
    store
        .repo_get(id)
        .map_err(store_problem)?
        .ok_or_else(|| repo_not_found(id))
}

/// In the CoS transaction: run `write`, then return the stored repo as the operation result.
fn audited_write<F>(
    store: &task_core::store::SqliteStore,
    audit: &OperationAudit,
    id: RepoId,
    action: &str,
    write: F,
) -> Result<Applied<ProjectRepo>, ApiProblem>
where
    F: FnOnce(&rusqlite::Connection) -> Result<serde_json::Value, ApiProblem>,
{
    let operation = audit.apply_checked(store, "repo", &id.to_string(), action, |tx| write(tx))?;
    Ok(Applied::Audited(Box::new(operation)))
}

fn stored_value(tx: &rusqlite::Connection, id: RepoId) -> Result<serde_json::Value, ApiProblem> {
    let stored = SqliteStore::repo_get_tx(tx, id)
        .map_err(store_problem)?
        .ok_or_else(|| repo_not_found(id))?;
    serde_json::to_value(stored).map_err(|e| ApiProblem::internal(e.to_string()))
}

/// `POST /projects/{id}/repos`, shared by the handler and CoS (`repo.create`). A missing project
/// is 404 before any write.
pub(crate) fn create_repo_op(
    store: &SqliteStore,
    repo: ProjectRepo,
    audit: Option<&OperationAudit>,
) -> Result<Applied<ProjectRepo>, ApiProblem> {
    let project_id = repo.project_id;
    let missing = match store.project_get(project_id).map_err(store_problem)? {
        None => Some(ApiProblem::project_not_found(&project_id.to_string())),
        Some(_) => None,
    };
    let Some(audit) = audit else {
        if let Some(problem) = missing {
            return Err(problem);
        }
        store.repo_create(&repo).map_err(store_problem)?;
        tracing::info!(who = "admin", op = "repo_create", project_id = %project_id, repo = %repo.name, "admin: project repo created");
        return Ok(Applied::Direct(repo_value(store, repo.id)?));
    };
    if let Some(problem) = missing {
        return Err(audit.reject(store, "repo", &project_id.to_string(), problem));
    }
    audited_write(store, audit, repo.id, "repo.create", |tx| {
        SqliteStore::repo_create_tx(tx, &repo).map_err(store_problem)?;
        stored_value(tx, repo.id)
    })
}

async fn patch_repo(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let repo_id = parse_repo_id(&id)?;
    let p: RepoPatchBody = read_json(body, false).await?;
    let location = check_patch(&state, &p)?;
    let updated = state
        .blocking(move |store| {
            let next = plan_patch(store, repo_id, p, location)?;
            patch_repo_op(store, next, None)?.direct()
        })
        .await?;
    Ok(json_response(StatusCode::OK, &updated))
}

/// `PATCH /repos/{id}` body checks that do not read the store; returns the validated location.
pub(crate) fn check_patch(
    state: &ApiState,
    p: &RepoPatchBody,
) -> Result<Option<task_core::WorkspaceSpec>, ApiProblem> {
    if p.name.is_none()
        && p.kind.is_none()
        && p.location.is_none()
        && p.default_branch.is_none()
        && p.sync.is_none()
        && p.run.is_none()
        && p.is_primary.is_none()
    {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: None,
            message: "specify at least one field to change".into(),
        }]));
    }
    let location = match p.location.clone() {
        Some(spec) => Some(validated_workspace(state, spec)?),
        None => None,
    };
    if let Some(name) = &p.name {
        check_name(name.trim())?;
    }
    Ok(location)
}

/// `PATCH /repos/{id}`: the repo with the patch applied (404 when absent).
pub(crate) fn plan_patch(
    store: &SqliteStore,
    repo_id: RepoId,
    p: RepoPatchBody,
    location: Option<task_core::WorkspaceSpec>,
) -> Result<ProjectRepo, ApiProblem> {
    let Some(current) = store.repo_get(repo_id).map_err(store_problem)? else {
        return Err(repo_not_found(repo_id));
    };
    let location_changed = location.is_some();
    let next = ProjectRepo {
        name: p.name.map(|n| n.trim().to_string()).unwrap_or(current.name),
        // `location` を変えて `kind` を書かなかったときは、新しい場所から決め直す。
        kind: p.kind.unwrap_or_else(|| match &location {
            Some(spec) => task_core::store::detect_repo_kind(spec),
            None => current.kind,
        }),
        location: location.unwrap_or(current.location),
        default_branch: match p.default_branch {
            Some(v) => v.filter(|b| !b.trim().is_empty()),
            None if location_changed => current.default_branch,
            None => current.default_branch,
        },
        sync: match p.sync {
            Some(v) => v,
            None => current.sync,
        },
        run: p.run.unwrap_or(current.run),
        is_primary: p.is_primary.unwrap_or(current.is_primary) || current.is_primary,
        ..current
    };
    // `kind = dir` に変えたら `default_branch` は意味を失う（検証に落ちるので先に落とす）。
    let next = if next.kind == RepoKind::Dir {
        ProjectRepo {
            default_branch: None,
            ..next
        }
    } else {
        next
    };
    // `Local` に変えたら `sync` も意味を失う。
    Ok(
        if matches!(next.location, task_core::WorkspaceSpec::Local { .. }) {
            ProjectRepo { sync: None, ..next }
        } else {
            next
        },
    )
}

/// `PATCH /repos/{id}`, shared by the handler and CoS (`repo.update`).
pub(crate) fn patch_repo_op(
    store: &SqliteStore,
    next: ProjectRepo,
    audit: Option<&OperationAudit>,
) -> Result<Applied<ProjectRepo>, ApiProblem> {
    let repo_id = next.id;
    let Some(audit) = audit else {
        if !store.repo_update(&next).map_err(store_problem)? {
            return Err(repo_not_found(repo_id));
        }
        tracing::info!(who = "admin", op = "repo_update", repo_id = %repo_id, "admin: project repo updated");
        return Ok(Applied::Direct(repo_value(store, repo_id)?));
    };
    audited_write(store, audit, repo_id, "repo.update", |tx| {
        if !SqliteStore::repo_update_tx(tx, &next).map_err(store_problem)? {
            return Err(repo_not_found(repo_id));
        }
        stored_value(tx, repo_id)
    })
}

async fn delete_repo(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let repo_id = parse_repo_id(&id)?;
    state
        .blocking(move |store| delete_repo_op(store, repo_id, None)?.direct())
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /repos/{id}`, shared by the handler and CoS (`repo.delete`). Absent is 404, a repo
/// with unfinished tasks 409.
pub(crate) fn delete_repo_op(
    store: &SqliteStore,
    repo_id: RepoId,
    audit: Option<&OperationAudit>,
) -> Result<Applied<()>, ApiProblem> {
    let Some(audit) = audit else {
        if store.repo_get(repo_id).map_err(store_problem)?.is_none()
            || !store.repo_delete(repo_id).map_err(store_problem)?
        {
            return Err(repo_not_found(repo_id));
        }
        tracing::info!(who = "admin", op = "repo_delete", repo_id = %repo_id, "admin: project repo deleted");
        return Ok(Applied::Direct(()));
    };
    let operation =
        audit.apply_checked(store, "repo", &repo_id.to_string(), "repo.delete", |tx| {
            if !SqliteStore::repo_delete_tx(tx, repo_id).map_err(store_problem)? {
                return Err(repo_not_found(repo_id));
            }
            Ok(serde_json::json!({"id": repo_id.to_string(), "deleted": true}))
        })?;
    Ok(Applied::Audited(Box::new(operation)))
}

/// Parse a repo id from a CoS path (`400` like the route).
pub(crate) fn repo_id_of(raw: &str) -> Result<RepoId, ApiProblem> {
    parse_repo_id(raw)
}
