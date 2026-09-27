//! `POST /projects/{id}/plan`（ADR-0033 D4 追記。GUI 監査対応 Phase 29）: 分解を起こす。
//!
//! GUI の「この方針で進める」の入口。人が「それで進めて」と言ったときに、案件の依頼・途中目標・
//! 人の一言・秘書との直近のやり取りをまとめた `kind = plan` のタスクを 1 件作る（`task_ops::project_plan`）。
//! **管理系**（`token_file` 未設定でも 401）: `POST /projects` や `POST /org/{id}/messages` と同じ規律
//! （人格を持つノードに仕事を起こす経路）。応答は 202 `{task_id}`（run を待たない）。

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{MilestoneId, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::handlers::{ApiResult, Params, json_response, no_query, parse_project_id, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem, store_problem};
use crate::state::ApiState;

/// ADR-0074 D3.3（Phase F4a (b)）: `POST /projects/{id}/plan` の `mode`。省略時は従来どおりの分解
/// （`decompose`）。`milestones` は案件全体のマイルストーン DAG を CoS に設計させる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPlanMode {
    #[default]
    Decompose,
    Milestones,
}

/// `POST /projects/{id}/plan` の要求本文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlanBody {
    /// 分解の対象にする途中目標。省略すると `approved` / `in_progress` のものを文脈として渡すだけで、
    /// 特定の 1 件をこのタスクに紐づけない。`mode = "milestones"` では使わない（422）。
    #[serde(default)]
    pub milestone_id: Option<MilestoneId>,
    /// 人の一言（任意）。
    #[serde(default)]
    pub note: Option<String>,
    /// ADR-0074 D3.3（Phase F4a）: `"decompose"`（既定、従来どおり）か `"milestones"`
    /// （案件レベルの計画。マイルストーン Task の DAG を提案させる）。
    #[serde(default)]
    pub mode: ProjectPlanMode,
}

/// `POST /projects/{id}/plan` の応答（202）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectPlanAccepted {
    pub task_id: TaskId,
}

pub(crate) async fn create_project_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let post: ProjectPlanBody = read_json(body, true).await?;
    if post.mode == ProjectPlanMode::Milestones && post.milestone_id.is_some() {
        return Err(ApiProblem::validation(vec![
            crate::types::ValidationError {
                field: Some("milestone_id".into()),
                message: "not used with mode=\"milestones\" (it plans the whole project)".into(),
            },
        ]));
    }
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let started = state
        .blocking(move |store| {
            let Some(project) = store.project_get(project_id).map_err(store_problem)? else {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            };
            match post.mode {
                ProjectPlanMode::Decompose => task_ops::project_plan::start(
                    store,
                    &project,
                    post.milestone_id,
                    post.note.as_deref(),
                    &roles,
                    &genres,
                    OffsetDateTime::now_utc(),
                ),
                ProjectPlanMode::Milestones => task_ops::project_plan::start_milestones(
                    store,
                    &project,
                    post.note.as_deref(),
                    &roles,
                    &genres,
                    OffsetDateTime::now_utc(),
                ),
            }
            .map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    tracing::info!(
        who = "admin",
        op = "project_plan",
        project_id = %project_id,
        task_id = %started.task.id,
        "admin: decomposition started for a project"
    );
    Ok(json_response(
        StatusCode::ACCEPTED,
        &ProjectPlanAccepted {
            task_id: started.task.id,
        },
    ))
}

/// ADR-0074 D3.3（Phase F4a (c)）: `POST /projects/{id}/project-plan/{version}/decide` の `decision`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPlanDecisionInput {
    Approve,
    Reject,
}

impl From<ProjectPlanDecisionInput> for task_ops::project_plan::ProjectPlanDecision {
    fn from(value: ProjectPlanDecisionInput) -> Self {
        match value {
            ProjectPlanDecisionInput::Approve => {
                task_ops::project_plan::ProjectPlanDecision::Approve
            }
            ProjectPlanDecisionInput::Reject => task_ops::project_plan::ProjectPlanDecision::Reject,
        }
    }
}

/// `POST /projects/{id}/project-plan/{version}/decide` の要求本文（ADR-0074 D3.3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlanDecideBody {
    pub decision: ProjectPlanDecisionInput,
    /// `reject` では必須（空なら 422）。`approve` では任意で、秘書へは送らない。
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /projects/{id}/project-plan/{version}/decide` の応答（202）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectPlanDecided {
    pub decision: ProjectPlanDecisionInput,
    pub plan_task_id: TaskId,
    /// `approve` は `approved`、`reject` は `redesigned` にした途中目標。
    pub milestones: Vec<MilestoneId>,
    /// `approve` は `ready`、`reject` は `cancelled` にした Task。
    pub tasks: Vec<TaskId>,
}

fn parse_version(raw: &str) -> Result<u32, ApiProblem> {
    raw.parse::<u32>()
        .map_err(|_| ApiProblem::bad_request(format!("`{raw}` is not a project plan version")))
}

pub(crate) async fn decide_project_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
    Params((id, version)): Params<(String, String)>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let version = parse_version(&version)?;
    let post: ProjectPlanDecideBody = read_json(body, false).await?;
    if post.decision == ProjectPlanDecisionInput::Reject
        && post.note.as_deref().map(str::trim).unwrap_or("").is_empty()
    {
        return Err(ApiProblem::validation(vec![
            crate::types::ValidationError {
                field: Some("note".into()),
                message: "is required to reject a project plan".into(),
            },
        ]));
    }
    let roles = state.inner.roles.clone();
    let genres = state.inner.genres.clone();
    let conversation_genre = state.inner.conversation_genre.clone();
    let decided = state
        .blocking(move |store| {
            let Some(project) = store.project_get(project_id).map_err(store_problem)? else {
                return Err(ApiProblem::project_not_found(&project_id.to_string()));
            };
            task_ops::project_plan::decide(
                store,
                &project,
                version,
                post.decision.into(),
                post.note.as_deref(),
                &roles,
                &genres,
                &conversation_genre,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| ops_problem(store, e, None))
        })
        .await?;
    tracing::info!(
        who = "admin",
        op = "project_plan_decide",
        project_id = %project_id,
        version,
        decision = ?post.decision,
        milestones = decided.milestones.len(),
        tasks = decided.tasks.len(),
        "admin: a project plan proposal was decided"
    );
    Ok(json_response(
        StatusCode::ACCEPTED,
        &ProjectPlanDecided {
            decision: post.decision,
            plan_task_id: decided.plan_task_id,
            milestones: decided.milestones,
            tasks: decided.tasks,
        },
    ))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/projects/{id}/plan",
            axum::routing::post(create_project_plan),
        )
        .route(
            "/api/v1/projects/{id}/project-plan/{version}/decide",
            axum::routing::post(decide_project_plan),
        )
}
