//! ADR 2026-10-11-llm-source-model-routing-evaluation D3: genre×harness ごとの段階・根拠表の読み取りと、
//! 人の段階変更（`enforce` へ進めるのは人だけ。根拠が立たなければ 409）。本体は `task_ops::routing_stage`。
use axum::{
    body::Body,
    extract::{RawQuery, State},
    http::StatusCode,
};
use serde::Deserialize;
use task_core::TaskId;
use task_core::route_select::RoutingStage;
use task_ops::routing_stage::{self as ops, HumanStageChange, StageChangeError, StageViewError};
use time::format_description::well_known::Rfc3339;

use crate::{
    handlers::{ApiResult, json_response, read_json},
    problem::{ApiProblem, store_problem},
    query::QueryParams,
    state::ApiState,
};

/// `POST /routing/stages` の本文。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutingStageChangeBody {
    /// event を記録する task（受信箱の承認の対象の task など）。
    pub task_id: TaskId,
    pub genre: String,
    pub harness_id: String,
    /// `shadow` | `enforce`。
    #[schemars(with = "StageName")]
    pub stage: RoutingStage,
    /// 人の理由（空白だけは 422）。
    pub reason: String,
}

/// schema 用の段階名（`task_core::route_select::RoutingStage` と同じ wire）。
#[derive(schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum StageName {
    Shadow,
    Enforce,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/routing/stages",
            axum::routing::get(stages).post(set_stage),
        )
        .route("/api/v1/routing/evidence", axum::routing::get(evidence))
}

fn filters(raw: &Option<String>) -> Result<(Option<String>, Option<String>), ApiProblem> {
    let q = QueryParams::parse(raw.as_deref(), &["genre", "harness_id"])?;
    Ok((
        q.single("genre")?.map(str::to_string),
        q.single("harness_id")?.map(str::to_string),
    ))
}

fn view_problem(e: StageViewError) -> ApiProblem {
    ApiProblem::internal(format!("routing stage view: {e}"))
}

fn change_problem(e: StageChangeError) -> ApiProblem {
    match e {
        StageChangeError::TaskNotFound(id) => ApiProblem::task_not_found(id),
        StageChangeError::EmptyReason => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "reason must not be empty",
        ),
        e @ StageChangeError::EvidenceInsufficient { .. } => ApiProblem::new(
            StatusCode::CONFLICT,
            "routing_evidence_insufficient",
            e.to_string(),
        ),
        StageChangeError::View(e) => view_problem(e),
        StageChangeError::Store(e) => store_problem(e),
    }
}

/// `GET /routing/stages?genre=&harness_id=` → 200 `RoutingStageList`。
async fn stages(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let (genre, harness) = filters(&raw)?;
    let db = state.inner.db_path.clone();
    let list = state
        .blocking(move |_| {
            ops::list_stages(&db, genre.as_deref(), harness.as_deref()).map_err(view_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

/// `GET /routing/evidence?genre=&harness_id=` → 200 `RoutingEvidenceList`（組ごとの最新版）。
async fn evidence(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let (genre, harness) = filters(&raw)?;
    let db = state.inner.db_path.clone();
    let list = state
        .blocking(move |store| {
            ops::list_evidence(store, &db, genre.as_deref(), harness.as_deref())
                .map_err(change_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

/// `POST /routing/stages` → 200 `RoutingStageView`（actor は要求の主体）。
async fn set_stage(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    QueryParams::parse(raw.as_deref(), &[])?;
    let body: RoutingStageChangeBody = read_json(body, false).await?;
    let db = state.inner.db_path.clone();
    let view = state
        .blocking(move |store| {
            let since = time::OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|e| ApiProblem::internal(format!("clock format: {e}")))?;
            ops::set_stage_by_human(
                store,
                &db,
                &HumanStageChange {
                    task_id: body.task_id,
                    genre: body.genre,
                    harness_id: body.harness_id,
                    stage: body.stage,
                    reason: body.reason,
                    actor: caller.actor(),
                },
                &since,
            )
            .map_err(change_problem)
        })
        .await?;
    tracing::info!(op = "routing_stage_set", genre = %view.genre, harness_id = %view.harness_id, stage = %view.stage, "routing stage changed by human");
    Ok(json_response(StatusCode::OK, &view))
}
