//! ADR 2026-11-04-lane-rules-exhaustive-evidence D7: `lane_rules` の全入力を本番の daemon の isolate で流す網羅の
//! 起動と、最新の証拠の読み出し。
//!
//! - `POST /script/domains/{domain}/exhaustive-runs` → 202 `ScriptExhaustiveRun`（実行は裏で続く）。本文は
//!   `ScriptExhaustiveRunBody`（`generation` と `reason`、どちらも必須）。人（`human:*`）と外部 operator（`operator:*`）の
//!   token だけが起こせる。他（CoS・MCP・`system:*`）は 403 `script_exhaustive_requires_operator`。
//!   未知の領域は 404、`lane_rules` 以外は 422 `script_exhaustive_domain_unsupported`、mode が `compare`・`canary` でない・
//!   generation が今の行と違うは 409 `script_exhaustive_mode_conflict`、走っている間は 409 `script_exhaustive_running`、
//!   全体の停止の間は 409 `script_guard_halted`、runtime が無い・standby は 503 `script_runtime_unavailable`。
//!   どの拒否でも表を変えない。
//! - `GET /script/domains/{domain}/exhaustive-runs` → 200 `ScriptExhaustiveRunList`（`standing`・`reference_sha256`・
//!   新しい順の最大 50 行）。認証された誰でも読める。
//! - CoS の operations には足さない（registry の EXCLUDED `script_api_only`）。昇格は今までどおり人の token だけ。

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use task_core::{
    script_compare::{ScriptDomain, ScriptDomainError, actor_may_run_exhaustive},
    script_exhaustive::{ScriptExhaustiveRun, ScriptExhaustiveStanding},
    store::current_reference_sha256,
};

use crate::{
    handlers::{ApiResult, json_response, no_query, read_json},
    problem::{ApiProblem, store_problem},
    script_domains::domain_problem,
    script_generations::generation_problem,
    state::ApiState,
};

/// 起こせない主体（CoS・MCP・`system:*`）。403。
pub const SCRIPT_EXHAUSTIVE_REQUIRES_OPERATOR: &str = "script_exhaustive_requires_operator";
/// 一覧に返す行の上限。
pub const EXHAUSTIVE_LIST_LIMIT: u32 = 50;

/// `POST /script/domains/{domain}/exhaustive-runs` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptExhaustiveRunBody {
    /// 流す generation の `bundle_sha256`（領域の mode の行が指す generation と同じ値）。
    pub generation: String,
    /// 起こす理由（`script_exhaustive_runs.reason`）。
    pub reason: String,
}

/// `GET /script/domains/{domain}/exhaustive-runs` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptExhaustiveRunList {
    pub domain: ScriptDomain,
    /// 領域の今の generation（mode の行が指す）の `bundle_sha256`。`rust` では `null`。
    pub generation: Option<String>,
    /// 今の generation と今の binary の参照に対する証拠の立場。`lane_rules` 以外は `not_applicable`、`rust` では `missing`。
    pub standing: ScriptExhaustiveStanding,
    /// 今の binary の参照の hash（これと違う行は証拠に数えない）。
    pub reference_sha256: String,
    /// 新しい順、最大 50 行（generation を問わない）。
    pub runs: Vec<ScriptExhaustiveRun>,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route(
        "/api/v1/script/domains/{domain}/exhaustive-runs",
        axum::routing::get(list_runs).post(start_run),
    )
}

/// 領域名の解釈（未知は 404）。
fn parse_domain(domain: &str) -> Result<ScriptDomain, ApiProblem> {
    ScriptDomain::parse(domain)
        .ok_or_else(|| domain_problem(ScriptDomainError::UnknownDomain(domain.to_owned())))
}

/// `POST …/exhaustive-runs` → 202 `ScriptExhaustiveRun`。
async fn start_run(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    Path(domain): Path<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptExhaustiveRunBody = read_json(body, false).await?;
    let actor = caller.actor();
    let control = state.script_generations.clone();
    let run = state
        .blocking(move |_store| start_effect(control.as_deref(), &domain, &body, &actor))
        .await?;
    tracing::info!(
        op = "script_exhaustive_start",
        run = %run.id,
        domain = %run.domain.as_str(),
        "script exhaustive"
    );
    Ok(json_response(StatusCode::ACCEPTED, &run))
}

/// 検査の順: 領域名（404）→ 主体（403）→ runtime の有無（503）→ 本文の形（422）→ daemon の `exhaustive_start`
/// （store の拒否 409・422、runtime の 503）。
pub(crate) fn start_effect(
    control: Option<&dyn crate::script_generations::ScriptGenerationControl>,
    domain: &str,
    body: &ScriptExhaustiveRunBody,
    actor: &str,
) -> Result<ScriptExhaustiveRun, ApiProblem> {
    let domain = parse_domain(domain)?;
    if !actor_may_run_exhaustive(actor) {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            SCRIPT_EXHAUSTIVE_REQUIRES_OPERATOR,
            format!(
                "only a human or operator API token may start an exhaustive run (actor `{actor}`)"
            ),
        ));
    }
    let control = control.ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            crate::script_generations::SCRIPT_RUNTIME_UNAVAILABLE,
            "the script runtime is not wired into this daemon ([script] is not configured)",
        )
    })?;
    if body.reason.trim().is_empty() {
        return Err(ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            crate::script_domains::SCRIPT_GENERATION_INVALID,
            "reason must not be empty",
        ));
    }
    control
        .exhaustive_start(domain, &body.generation, actor, &body.reason)
        .map_err(generation_problem)
}

/// `GET …/exhaustive-runs` → 200 `ScriptExhaustiveRunList`。
async fn list_runs(
    State(state): State<ApiState>,
    Path(domain): Path<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let list = state
        .blocking(move |store| {
            let domain = parse_domain(&domain)?;
            let reference = current_reference_sha256().map_err(store_problem)?;
            let row = store
                .script_domain_mode(domain.as_str())
                .map_err(domain_problem)?;
            let generation = match row.generation_id {
                Some(id) => store.script_generation_get(&id).map_err(store_problem)?,
                None => None,
            };
            let standing = match &generation {
                Some(g) => store
                    .script_exhaustive_standing(domain, &g.id, reference)
                    .map_err(store_problem)?,
                None if domain == ScriptDomain::LaneRules => ScriptExhaustiveStanding::Missing,
                None => ScriptExhaustiveStanding::NotApplicable,
            };
            let runs = store
                .script_exhaustive_runs(domain, None, EXHAUSTIVE_LIST_LIMIT)
                .map_err(store_problem)?;
            Ok(ScriptExhaustiveRunList {
                domain,
                generation: generation.map(|g| g.bundle_sha256),
                standing,
                reference_sha256: reference.to_owned(),
                runs,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> ScriptExhaustiveRunBody {
        ScriptExhaustiveRunBody {
            generation: "a".repeat(64),
            reason: "r".into(),
        }
    }

    /// handler の主体の検査: `human:*`・`operator:*` 以外（CoS・MCP・daemon・空の名前）は、runtime の有無より先に 403。
    #[test]
    fn exhaustive_start_actor_check_precedes_runtime() {
        for actor in [
            "cos:thread-1",
            "mcp:client",
            "system:script-guard",
            "human:",
            "operator: ",
        ] {
            let err = start_effect(None, "lane_rules", &body(), actor).expect_err(actor);
            assert_eq!(err.status(), StatusCode::FORBIDDEN, "{actor}");
            assert_eq!(err.code(), SCRIPT_EXHAUSTIVE_REQUIRES_OPERATOR, "{actor}");
        }
        // 許される主体は runtime が無ければ 503（403 ではない）。
        for actor in ["human:web", "operator:claude-code"] {
            let err = start_effect(None, "lane_rules", &body(), actor).expect_err(actor);
            assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE, "{actor}");
        }
        let err = start_effect(None, "nope", &body(), "human:web").expect_err("domain");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);
    }
}
