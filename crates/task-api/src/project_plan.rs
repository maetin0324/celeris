//! 案件計画の入口（ADR-0033 D4 追記 Phase 29 / ADR-0074 D3.3）。**ADR-0079 D13（Phase R5a）で撤去**:
//! `POST /projects/{id}/plan`（`mode: decompose` / `milestones` とも）と
//! `POST /projects/{id}/project-plan/{version}/decide` は 410 Gone（`type: urn:celeris:problem:removed_by_adr_0079`）。
//! 要求・応答の型は `api-v1.schema.json` の互換のためにだけ残す（GUI の生成型。R5b 以降で外す）。

use axum::http::HeaderMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{MilestoneId, TaskId};

use crate::handlers::ApiResult;
use crate::middleware::require_admin;
use crate::problem::ApiProblem;
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

/// ADR-0079 D13（Phase R5a）: 案件は計画を持たない。`POST /projects/{id}/plan`（`decompose` / `milestones` とも）は
/// 410。本文は読まない（管理系のまま: トークン無しは 401）。
pub(crate) const PROJECT_PLAN_GONE: &str = "ADR-0079: 案件は計画を持たない。root task を作る";

pub(crate) async fn create_project_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        PROJECT_PLAN_GONE,
        "POST /api/v1/tasks with project_id (a root task; name the stages in stages_hint)",
    ))
}

/// ADR-0074 D3.3（Phase F4a (c)）: `POST /projects/{id}/project-plan/{version}/decide` の `decision`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPlanDecisionInput {
    Approve,
    Reject,
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

/// ADR-0079 D13（Phase R5a）: `POST /projects/{id}/project-plan/{version}/decide` も 410（案件計画の提案は作られない）。
pub(crate) async fn decide_project_plan(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        PROJECT_PLAN_GONE,
        "POST /api/v1/tasks with project_id (a root task); decisions inside a task tree use POST /api/v1/decisions/{id}/answer",
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
