//! ADR 2026-11-23-project-observability-publish D1: `GET /api/v1/projects/{id}/overview`（読み取り専用）。
//!
//! 14 項目の導出は `task_ops::project_overview` が持ち、この handler は案件の確認と、O14 の manifest の状態を
//! `crate::publish::manifest_status`（manifest の GET・署名の前の検査と同じ関数）から写す口だけを持つ。
//! どの表にも書かない。読める主体は `GET /projects/{id}` と同じ（同じ router の認可の層を通る）。MCP の道具は足さない。

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::routing::get;
use task_ops::project_overview::{self, ManifestFacts};

use super::*;
use crate::http_io::{ApiResult, Params, json_response, no_query};
use crate::problem::ops_problem;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route("/api/v1/projects/{id}/overview", get(get_overview))
}

async fn get_overview(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let project_id = parse_project_id(&id)?;
    let env = crate::cos::operations::DispatchEnv::of(&state);
    let generated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| ApiProblem::internal(e.to_string()))?;
    let view = state
        .blocking(move |store| {
            let roots = &env.api.inner.view.workspace_roots;
            let mut manifests = |task_id, manifest_id: &str, version| {
                crate::publish::manifest_status(
                    store,
                    roots,
                    task_id,
                    manifest_id,
                    Some(version),
                    &mut |project, r| {
                        crate::cos::strategy_gate::Reader::new(store, &env, project.id.to_string())
                            .lookup(project, r)
                    },
                )
                .ok()
                .map(|v| ManifestFacts {
                    name: v.name,
                    sha256: v.recorded_sha256,
                    signoff: v.signoff.state,
                    violations: v.violations.into_iter().map(|x| x.code).collect(),
                    strong: v.strong_warnings,
                    file_matches_record: v.file_matches_record,
                })
            };
            project_overview::overview(store, roots, project_id, &generated_at, &mut manifests)
                .map_err(|e| ops_problem(store, e, None))?
                .ok_or_else(|| ApiProblem::project_not_found(&project_id.to_string()))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}
