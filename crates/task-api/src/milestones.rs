//! 途中目標の判定（ADR-0038 D2。Phase 41）: `POST /milestones/{id}/decide`。
//!
//! **ADR-0079 D13（Phase R5a）で撤去**: 410 Gone。途中目標の書き込みの入口（`POST /projects/{id}/milestones`、
//! `PATCH /milestones/{id}`、`POST /milestones/{id}/{cancel,pause,resume}`）も同じ 410（`MILESTONE_GONE`）。
//! 要求・応答の型は `api-v1.schema.json` の互換のためにだけ残す。

use axum::http::HeaderMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{Milestone, MilestoneDecision, TaskId};

use crate::handlers::ApiResult;
use crate::middleware::require_admin;
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// `POST /milestones/{id}/decide` の要求本文（ADR-0038 D2）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MilestoneDecideBody {
    /// `"ok"` / `"discuss"` / `"ng"`。
    pub decision: MilestoneDecision,
    /// 人の自由記述。`discuss` / `ng` では**必須**（空なら 422）。`ok` では任意で、計画の `note` と
    /// 秘書への `messages` に渡る。
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /milestones/{id}/decide` の応答（202）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MilestoneDecided {
    /// 適用した答え。
    pub decision: MilestoneDecision,
    /// 判定した途中目標（更新後）。
    pub milestone: Milestone,
    /// `ok` で承認して分解を始めた次の途中目標、`ng` で `redesigned` にした提案（無ければ省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_milestone: Option<Milestone>,
    /// `ok` で起きた分解（計画 run）のタスク。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_task_id: Option<TaskId>,
    /// `discuss` / `ng` で秘書に送った `role = "user"` の行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// その返事のために起きた対話用タスク。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_task_id: Option<TaskId>,
}

/// ADR-0079 D13（Phase R5a）: 途中目標は凍結した（判定 run も `MilestoneReady` も無い）。人の判定の入口も 410。
pub(crate) const MILESTONE_GONE: &str = "ADR-0079: 途中目標は root task の段階で表す（既存の途中目標は凍結。読み取りは GET /projects/{id}?include_frozen=true）";

pub(crate) async fn decide_milestone(
    axum::extract::State(state): axum::extract::State<ApiState>,
    headers: HeaderMap,
) -> ApiResult {
    require_admin(&state, &headers)?;
    Err(ApiProblem::gone(
        MILESTONE_GONE,
        "a stage with review: human in the root task's plan (POST /api/v1/tasks/{id}/execution/phase-gate)",
    ))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route(
        "/api/v1/milestones/{id}/decide",
        axum::routing::post(decide_milestone),
    )
}
