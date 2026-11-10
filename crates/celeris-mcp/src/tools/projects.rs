//! ADR-0056 D2: `projects_list` / `projects_get`（読むだけ）。
//!
//! ADR 2026-11-10-taskstore-consumer-ports: 本体（`list_projects`・`get_project`）は同期の関数で、
//! `&dyn OrgDirectory`・`&dyn TaskQuery` だけを受ける（試験は `tools/fake_ports.rs` の fake を渡す）。
//! async 側は引数を検査し、`McpState::blocking_on` で本体を 1 回呼ぶ。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    ListFilter, ListOrder, McpScope, Milestone, OrgDirectory, ProjectId, ProjectStatus, TaskQuery,
};

use super::{ToolDef, ToolError, ToolOutput, clamp_limit, schema};
use crate::auth::AuthedClient;
use crate::state::McpState;

// ---- projects_list ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProjectSummary {
    pub id: String,
    pub title: String,
    pub status: ProjectStatus,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    pub items: Vec<ProjectSummary>,
}

async fn list_impl(
    state: &Arc<McpState>,
    _client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: ListArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    let limit = clamp_limit(args.limit);
    let status = match args.status.as_deref() {
        Some(s) => Some(
            serde_json::from_value::<ProjectStatus>(serde_json::Value::String(s.to_string()))
                .map_err(|_| ToolError::invalid_params(format!("unknown status {s:?}")))?,
        ),
        None => None,
    };
    let items = state
        .blocking_on(&state.org_directory, move |org| {
            list_projects(org, status, limit)
        })
        .await?;
    ToolOutput::from_serialize(&ListOutput { items })
}

/// `projects_list` の本体: 案件を新しい順に読み、`status` で絞り、`limit` 件で切る。
pub(super) fn list_projects(
    org: &dyn OrgDirectory,
    status: Option<ProjectStatus>,
    limit: usize,
) -> Result<Vec<ProjectSummary>, ToolError> {
    let mut items: Vec<_> = org
        .project_list()
        .map_err(|e| ToolError::internal(e.to_string()))?
        .into_iter()
        .filter(|p| match status {
            Some(s) => p.status == s,
            None => true,
        })
        .map(|p| ProjectSummary {
            id: p.id.to_string(),
            title: p.title,
            status: p.status,
        })
        .collect();
    items.truncate(limit);
    Ok(items)
}

fn list_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(list_impl(state, client, args))
}

pub fn list_def() -> ToolDef {
    ToolDef {
        name: "projects_list",
        description: "案件の一覧（状態で絞れる）。",
        scope: McpScope::TasksRead,
        input_schema: schema::<ListArgs>,
        call: list_call,
    }
}

// ---- projects_get ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetOutput {
    pub id: String,
    pub title: String,
    pub request: String,
    pub status: ProjectStatus,
    pub milestones: Vec<Milestone>,
    pub tasks: Vec<super::tasks::TaskSummary>,
}

async fn get_impl(
    state: &Arc<McpState>,
    _client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: GetArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    let id: ProjectId = args
        .id
        .parse()
        .map_err(|_| ToolError::invalid_params(format!("{:?} is not a project id", args.id)))?;
    let tasks = Arc::clone(&state.task_query);
    let out = state
        .blocking_on(&state.org_directory, move |org| {
            get_project(org, tasks.as_ref(), id)
        })
        .await?;
    match out {
        Some(o) => ToolOutput::from_serialize(&o),
        None => Err(ToolError::not_found(format!(
            "project {} was not found",
            args.id
        ))),
    }
}

/// `projects_get` が返す task の数の上限（新しく更新された順）。
const GET_TASKS_LIMIT: usize = 100;

/// `projects_get` の本体: 案件 1 件と、その途中目標（`seq` 順）と task（更新の新しい順に
/// `GET_TASKS_LIMIT` 件まで）。案件が無ければ `None`。
pub(super) fn get_project(
    org: &dyn OrgDirectory,
    tasks: &dyn TaskQuery,
    id: ProjectId,
) -> Result<Option<GetOutput>, ToolError> {
    let Some(project) = org
        .project_get(id)
        .map_err(|e| ToolError::internal(e.to_string()))?
    else {
        return Ok(None);
    };
    let milestones = org
        .milestone_list(id)
        .map_err(|e| ToolError::internal(e.to_string()))?;
    let filter = ListFilter {
        project_id: Some(id),
        ..ListFilter::default()
    };
    let page = tasks
        .list_page(&filter, ListOrder::UpdatedDesc, None, GET_TASKS_LIMIT)
        .map_err(|e| ToolError::internal(e.to_string()))?;
    Ok(Some(GetOutput {
        id: project.id.to_string(),
        title: project.title,
        request: project.request,
        status: project.status,
        milestones,
        tasks: page
            .items
            .into_iter()
            .map(super::tasks::task_summary)
            .collect(),
    }))
}

fn get_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(get_impl(state, client, args))
}

pub fn get_def() -> ToolDef {
    ToolDef {
        name: "projects_get",
        description: "案件 1 件（途中目標とタスクの一覧）。",
        scope: McpScope::TasksRead,
        input_schema: schema::<GetArgs>,
        call: get_call,
    }
}
