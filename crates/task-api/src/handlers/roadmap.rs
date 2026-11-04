//! ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap の API（`/projects/{id}/roadmap`）。
//!
//! - `GET …/roadmap[?version=]` → [`Roadmap`]（今の approved の版。無ければ 404 `roadmap_not_found`）。
//! - `GET …/roadmap/history` → 版の行と Outcome の記録の全部。
//! - `POST …/roadmap/proposals` [`RoadmapProposalInput`] → 201 の版の行（`proposed`。今の版は変えない）。
//!   CoS は `/cos/operations` の `project.roadmap_propose`。
//! - `POST …/roadmap/proposals/{version}/decide` `{decision: approve|reject, note}`: **人の token だけ**（403
//!   `roadmap_decide_requires_human`）。approve で今の版が進み、前の版は `superseded`。CoS は registry の
//!   EXCLUDED（human_only）。
//! - `POST …/roadmap/outcomes/{oid}/status` `{status, evidence_refs, reason}`: 人の token だけ（controller の
//!   `mark_milestone` の適用は `task_ops::roadmap::record_status_tx` を直接使う）。`reached` は evidence の
//!   root を Rust が確かめる。
//!
//! `/milestones` は使わない（D2。旧 milestones の 410 はそのまま）。

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use task_core::chat::CosOperation;
use task_core::project_proposal::sha256_hex;
use task_core::roadmap::{RoadmapProposalInput, RoadmapVersion};
use task_core::store::{RoadmapWriteError, SqliteStore};
use task_ops::roadmap::OutcomeStatusInput;
use time::OffsetDateTime;

use super::*;
use crate::cos::operations::OperationAudit;
use crate::human_epoch::HumanEpoch;
use crate::middleware::require_admin;
use crate::principal::{Caller, Principal};
use crate::query::QueryParams;

/// 決定の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapDecision {
    Approve,
    Reject,
}

/// `POST …/roadmap/proposals/{version}/decide` の本文。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoadmapDecideInput {
    pub decision: RoadmapDecision,
    #[serde(default)]
    pub note: String,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route("/api/v1/projects/{id}/roadmap", get(get_roadmap))
        .route("/api/v1/projects/{id}/roadmap/history", get(get_history))
        .route(
            "/api/v1/projects/{id}/roadmap/proposals",
            post(post_proposal),
        )
        .route(
            "/api/v1/projects/{id}/roadmap/proposals/{version}/decide",
            post(post_decide),
        )
        .route(
            "/api/v1/projects/{id}/roadmap/outcomes/{oid}/status",
            post(post_outcome_status),
        )
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

pub(crate) fn roadmap_problem(e: RoadmapWriteError) -> ApiProblem {
    match e {
        RoadmapWriteError::Invalid(i) => {
            ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, i.code, i.detail)
        }
        RoadmapWriteError::Stale { current } => ApiProblem::new(
            StatusCode::CONFLICT,
            "roadmap_version_conflict",
            format!("the approved roadmap version is now {current:?}; propose on top of it"),
        ),
        RoadmapWriteError::NotFound(v) => ApiProblem::new(
            StatusCode::NOT_FOUND,
            "roadmap_version_not_found",
            format!("roadmap version {v} not found"),
        ),
        RoadmapWriteError::NotProposed(s) => ApiProblem::new(
            StatusCode::CONFLICT,
            "roadmap_already_decided",
            format!("roadmap version is {s}, not proposed"),
        ),
        RoadmapWriteError::Store(s) => store_problem(s),
    }
}

fn project_request(store: &SqliteStore, id: task_core::ProjectId) -> Result<String, ApiProblem> {
    match store.project_get(id).map_err(store_problem)? {
        Some(p) => Ok(p.request),
        None => Err(ApiProblem::project_not_found(&id.to_string())),
    }
}

fn requires_human(caller: &Caller, what: &'static str) -> Result<(), ApiProblem> {
    match caller.0 {
        Principal::Human { .. } => Ok(()),
        _ => Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            what,
            "only a person's API token may decide the roadmap or record outcome status (ADR project-controller D8)",
        )),
    }
}

fn parse_version(raw: &str) -> Result<i64, ApiProblem> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v >= 1)
        .ok_or_else(|| ApiProblem::bad_request("version must be a positive integer"))
}

async fn get_roadmap(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let q = QueryParams::parse(raw.as_deref(), &["version"])?;
    let version = q.single("version")?.map(parse_version).transpose()?;
    let project_id = parse_project_id(&id)?;
    let view = state
        .blocking(move |store| {
            project_request(store, project_id)?;
            task_ops::roadmap::view(store, project_id, version)
                .map_err(|e| crate::problem::ops_problem(store, e, None))?
                .ok_or_else(|| {
                    ApiProblem::new(
                        StatusCode::NOT_FOUND,
                        "roadmap_not_found",
                        "the project has no roadmap version (it still reads the request outcome block)",
                    )
                })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

async fn get_history(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let project_id = parse_project_id(&id)?;
    let history = state
        .blocking(move |store| {
            project_request(store, project_id)?;
            task_ops::roadmap::history(store, project_id)
                .map_err(|e| crate::problem::ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &history))
}

fn propose_tx(
    tx: &rusqlite::Connection,
    project: &str,
    request: &str,
    input: &RoadmapProposalInput,
    by: &str,
) -> Result<RoadmapVersion, ApiProblem> {
    SqliteStore::roadmap_propose_tx(tx, project, input, &sha256_hex(request), by, now())
        .map_err(roadmap_problem)
}

async fn post_proposal(
    State(state): State<ApiState>,
    caller: Caller,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let input: RoadmapProposalInput = read_json(body, false).await?;
    let by = caller.actor();
    let row = state
        .blocking(move |store| {
            let request = project_request(store, project_id)?;
            store.write_tx(|tx| propose_tx(tx, &project_id.to_string(), &request, &input, &by))
        })
        .await?;
    Ok(json_response(StatusCode::CREATED, &row))
}

async fn post_decide(
    State(state): State<ApiState>,
    caller: Caller,
    Params((id, version)): Params<(String, String)>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    requires_human(&caller, "roadmap_decide_requires_human")?;
    let project_id = parse_project_id(&id)?;
    let version = parse_version(&version)?;
    let input: RoadmapDecideInput = read_json(body, false).await?;
    if input.note.chars().count() > task_core::roadmap::ROADMAP_TEXT_MAX {
        return Err(ApiProblem::bad_request("note is too long"));
    }
    let by = caller.actor();
    let epoch = HumanEpoch::of(&caller);
    let row = state
        .blocking(move |store| {
            project_request(store, project_id)?;
            store.write_tx(|tx| {
                let row = SqliteStore::roadmap_decide_tx(
                    tx,
                    &project_id.to_string(),
                    version,
                    input.decision == RoadmapDecision::Approve,
                    (!input.note.trim().is_empty()).then_some(input.note.as_str()),
                    &by,
                    now(),
                )
                .map_err(roadmap_problem)?;
                // 人の方針の変更（版が進んだ）を controller の再評価のきっかけにする（I7。off の案件では何もしない）。
                if input.decision == RoadmapDecision::Approve {
                    epoch.bump_tx(tx, Some(project_id)).map_err(store_problem)?;
                }
                Ok::<_, ApiProblem>(row)
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &row))
}

async fn post_outcome_status(
    State(state): State<ApiState>,
    caller: Caller,
    Params((id, oid)): Params<(String, String)>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    requires_human(&caller, "roadmap_status_requires_human")?;
    let project_id = parse_project_id(&id)?;
    let input: OutcomeStatusInput = read_json(body, false).await?;
    let by = caller.actor();
    let epoch = HumanEpoch::of(&caller);
    let view = state
        .blocking(move |store| {
            project_request(store, project_id)?;
            let checked = task_ops::roadmap::check_status(store, project_id, &oid, &input, now())
                .map_err(roadmap_problem)?;
            let project = project_id.to_string();
            store.write_tx(|tx| {
                task_ops::roadmap::record_status_tx(tx, &project, &oid, &checked, &by, now())
                    .map_err(roadmap_problem)?;
                epoch.bump_tx(tx, Some(project_id)).map_err(store_problem)?;
                Ok::<_, ApiProblem>(())
            })?;
            task_ops::roadmap::view(store, project_id, None)
                .map_err(|e| crate::problem::ops_problem(store, e, None))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

// ---- CoS（`/cos/operations`）----

/// `project.roadmap_propose`: CoS は提案だけ（版は進めない）。版の行と監査は同じ transaction。
pub(crate) fn cos_roadmap_propose(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    input: RoadmapProposalInput,
    audit: &OperationAudit,
) -> Result<CosOperation, ApiProblem> {
    let target = project_id.to_string();
    let request = project_request(store, project_id)
        .map_err(|p| audit.reject(store, "project", &target, p))?;
    audit.apply_checked(store, "project", &target, "project.roadmap_propose", |tx| {
        let row = propose_tx(tx, &target, &request, &input, &audit.by())?;
        serde_json::to_value(row).map_err(|e| ApiProblem::internal(e.to_string()))
    })
}
