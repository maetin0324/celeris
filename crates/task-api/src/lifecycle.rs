//! ADR-0044 D6（Phase 55）: 案件・途中目標の **中止・一時停止・アーカイブ**。
//!
//! - `POST /projects/{id}/{cancel|pause|resume|archive|unarchive}`
//! - `POST /tasks/{id}/{pause|resume}`（ADR-0079 D13、Phase R5a: task の subtree の一時停止）
//! - `POST /milestones/{id}/{cancel|pause|resume}` は ADR-0079 D13（Phase R5a）で 410 Gone
//!
//! どれも**管理系**（`token_file` 未設定でも 401。ADR-0044 §5 Phase 53 追記「変更を伴う API は
//! すべて管理系に揃える」）。本文は取らない（`{}` でも空でもよい）。
//!
//! ハンドラは HTTP への写像だけで、判断と連鎖は `task_ops::lifecycle` が行う（DESIGN 原則 1〜4）。
//! 応答は 200 で、`cancel` は連鎖で `cancelled` になったタスク（と途中目標）を添える。
//! 404（知らない id）／409（その状態ではできない）／401（トークン無し）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use task_ops::lifecycle;
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, parse_project_id, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem};
use crate::query::parse_task_id;
use crate::state::ApiState;

/// `POST /milestones/{id}/{cancel|pause|resume}` の旧い応答（ADR-0044 D6）。ADR-0079 D13（Phase R5a）で入口は 410 に
/// なったが、`api-v1.schema.json` と GUI の生成型の互換のために形だけ残す（R5b 以降で外す）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct MilestoneLifecycle {
    pub milestone: task_core::Milestone,
    #[serde(default)]
    pub cancelled_tasks: Vec<task_ops::view::TaskRef>,
}

/// 本文は取らない（`{}` か空）。余計なキーは 422。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EmptyBody {}

use lifecycle::ProjectAction;

fn project_trigger(action: ProjectAction) -> &'static str {
    match action {
        ProjectAction::Cancel => "project_cancel",
        ProjectAction::Pause => "project_pause",
        ProjectAction::Resume => "project_resume",
        ProjectAction::Archive => "project_archive",
        ProjectAction::Unarchive => "project_unarchive",
    }
}

/// The CoS action name of a project lifecycle operation.
pub(crate) fn project_action_name(action: ProjectAction) -> &'static str {
    match action {
        ProjectAction::Cancel => "project.cancel",
        ProjectAction::Pause => "project.pause",
        ProjectAction::Resume => "project.resume",
        ProjectAction::Archive => "project.archive",
        ProjectAction::Unarchive => "project.unarchive",
    }
}

/// `POST /projects/{id}/{cancel|pause|resume|archive|unarchive}` shared by the handler
/// (`audit = None`) and `/cos/operations` (ADR 2026-10-09-cos-operations-all-mutations D3). The
/// CoS form checks with `plan_project_action` and writes the project change, the cancel cascade
/// and the audit record in one transaction.
pub(crate) fn project_action_op(
    store: &task_core::store::SqliteStore,
    project_id: task_core::ProjectId,
    action: ProjectAction,
    audit: Option<&OperationAudit>,
) -> Result<Applied<lifecycle::ProjectLifecycle>, ApiProblem> {
    let trigger = project_trigger(action);
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let outcome = match action {
            ProjectAction::Cancel => lifecycle::cancel_project(store, project_id),
            ProjectAction::Pause => lifecycle::pause_project(store, project_id),
            ProjectAction::Resume => lifecycle::resume_project(store, project_id),
            ProjectAction::Archive => lifecycle::archive_project(store, project_id, now),
            ProjectAction::Unarchive => lifecycle::unarchive_project(store, project_id),
        };
        return outcome
            .map(Applied::Direct)
            .map_err(|e| ops_problem(store, e, Some(trigger)));
    };
    let target_id = project_id.to_string();
    let change = lifecycle::plan_project_action(store, project_id, action, now).map_err(|e| {
        audit.reject(
            store,
            "project",
            &target_id,
            ops_problem(store, e, Some(trigger)),
        )
    })?;
    let name = project_action_name(action);
    let operation = audit.apply(store, "project", &target_id, name, |tx| {
        let cancelled = lifecycle::apply_project_change_tx(tx, project_id, &change)?;
        Ok(serde_json::json!({
            "project_id": target_id,
            "cancelled_tasks": cancelled.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

async fn project_action(
    state: ApiState,
    headers: HeaderMap,
    raw: Option<String>,
    body: Body,
    id: String,
    action: ProjectAction,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let EmptyBody {} = read_json(body, true).await?;
    let trigger = project_trigger(action);
    let result = state
        .blocking(move |store| project_action_op(store, project_id, action, None)?.direct())
        .await?;
    tracing::info!(who = "admin", op = trigger, project_id = %project_id, status = %result.project.status.as_str(), "admin: project lifecycle");
    Ok(json_response(StatusCode::OK, &result))
}

/// ADR-0079 D13（Phase R5a）: 途中目標の中止・一時停止・再開は 410（中止・一時停止は task の subtree と案件の 2 階層）。
async fn milestone_action(state: ApiState, headers: HeaderMap) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        crate::milestones::MILESTONE_GONE,
        "POST /api/v1/tasks/{id}/pause|resume|cancel (the task subtree) or POST /api/v1/projects/{id}/pause|resume|cancel",
    ))
}

/// ADR-0079 D13（Phase R5a）: task の subtree の一時停止・再開（`POST /tasks/{id}/pause|resume`）。
/// 判断は `task_ops::lifecycle::{pause_task, resume_task}`（`ready_tasks` が祖先を辿って止める）。
async fn task_action(
    state: ApiState,
    headers: HeaderMap,
    raw: Option<String>,
    body: Body,
    id: String,
    pause: bool,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let EmptyBody {} = read_json(body, true).await?;
    let trigger = if pause { "task_pause" } else { "task_resume" };
    let result = state
        .blocking(move |store| task_pause_op(store, task_id, pause, None)?.direct())
        .await?;
    tracing::info!(who = "admin", op = trigger, task_id = %task_id, subtree = result.subtree.len(), "admin: task subtree lifecycle");
    Ok(json_response(StatusCode::OK, &result))
}

/// `POST /tasks/{id}/pause|resume` の本体。handler（`audit = None`、`by = human`）と CoS の
/// `/cos/operations`（ADR 2026-10-09 D3/D5。`by = cos`、task の更新・`Edited` event・監査を同じ transaction）
/// が共有する。
pub(crate) fn task_pause_op(
    store: &task_core::store::SqliteStore,
    task_id: task_core::TaskId,
    pause: bool,
    audit: Option<&OperationAudit>,
) -> Result<Applied<lifecycle::TaskPauseResult>, ApiProblem> {
    let now = OffsetDateTime::now_utc();
    let trigger = if pause { "task_pause" } else { "task_resume" };
    let Some(audit) = audit else {
        let outcome = if pause {
            lifecycle::pause_task(store, task_id, now)
        } else {
            lifecycle::resume_task(store, task_id, now)
        };
        return outcome
            .map(Applied::Direct)
            .map_err(|e| ops_problem(store, e, Some(trigger)));
    };
    let target_id = task_id.to_string();
    let (next, event) =
        lifecycle::plan_set_paused(store, task_id, pause, "cos", now).map_err(|e| {
            audit.reject(
                store,
                "task",
                &target_id,
                ops_problem(store, e, Some(trigger)),
            )
        })?;
    let action = if pause { "task.pause" } else { "task.resume" };
    let operation = audit.apply(store, "task", &target_id, action, |tx| {
        let updated = task_core::store::SqliteStore::edit_task_tx(tx, &next, &event)?;
        Ok(serde_json::json!({
            "task_id": target_id,
            "paused": updated.paused_at.is_some(),
        }))
    })?;
    Ok(Applied::Audited(Box::new(operation)))
}

macro_rules! project_handler {
    ($name:ident, $action:expr) => {
        async fn $name(
            State(state): State<ApiState>,
            headers: HeaderMap,
            Params(id): Params<String>,
            RawQuery(raw): RawQuery,
            body: Body,
        ) -> ApiResult {
            project_action(state, headers, raw, body, id, $action).await
        }
    };
}

macro_rules! milestone_handler {
    ($name:ident) => {
        async fn $name(State(state): State<ApiState>, headers: HeaderMap) -> ApiResult {
            milestone_action(state, headers).await
        }
    };
}

macro_rules! task_handler {
    ($name:ident, $pause:expr) => {
        async fn $name(
            State(state): State<ApiState>,
            headers: HeaderMap,
            Params(id): Params<String>,
            RawQuery(raw): RawQuery,
            body: Body,
        ) -> ApiResult {
            task_action(state, headers, raw, body, id, $pause).await
        }
    };
}

project_handler!(cancel_project, ProjectAction::Cancel);
project_handler!(pause_project, ProjectAction::Pause);
project_handler!(resume_project, ProjectAction::Resume);
project_handler!(archive_project, ProjectAction::Archive);
project_handler!(unarchive_project, ProjectAction::Unarchive);
milestone_handler!(cancel_milestone);
milestone_handler!(pause_milestone);
milestone_handler!(resume_milestone);
task_handler!(pause_task, true);
task_handler!(resume_task, false);

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::post;
    axum::Router::new()
        .route("/api/v1/projects/{id}/cancel", post(cancel_project))
        .route("/api/v1/projects/{id}/pause", post(pause_project))
        .route("/api/v1/projects/{id}/resume", post(resume_project))
        .route("/api/v1/projects/{id}/archive", post(archive_project))
        .route("/api/v1/projects/{id}/unarchive", post(unarchive_project))
        .route("/api/v1/milestones/{id}/cancel", post(cancel_milestone))
        .route("/api/v1/milestones/{id}/pause", post(pause_milestone))
        .route("/api/v1/milestones/{id}/resume", post(resume_milestone))
        // ADR-0079 D13（Phase R5a）: task の subtree の一時停止・再開。
        .route("/api/v1/tasks/{id}/pause", post(pause_task))
        .route("/api/v1/tasks/{id}/resume", post(resume_task))
}
