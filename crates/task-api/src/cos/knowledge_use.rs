//! ADR 2026-11-12-knowledge-usage-per-run D1・D3: CoS run の KB の操作を API の受け側で数える。
//!
//! CoS run の `celerisctl knowledge search|get|record` は、操作のたびに `POST /api/v1/cos/knowledge-use`
//! （`{"op": "search"|"get"|"record", "ok": bool}`）を run credential で送る。どの run の数かは credential
//! から決め（body に run id は無い）、`chat_runs.knowledge_use_json` に 1 回を足す。数は観測値で、権限・判断の
//! 根拠には使わない。

use axum::Extension;
use axum::body::Body;
use axum::extract::State;
use axum::routing::post;
use serde::Deserialize;
use task_core::{KnowledgeOp, KnowledgeUseStore};

use super::{CosCaller, cos_only};
use crate::handlers::{ApiResult, json_response};
use crate::problem::ApiProblem;
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new().route("/api/v1/cos/knowledge-use", post(record))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeUseRequest {
    op: KnowledgeOp,
    ok: bool,
}

async fn record(
    State(state): State<ApiState>,
    caller: Option<Extension<CosCaller>>,
    body: Body,
) -> ApiResult {
    let Some(Extension(caller)) = caller else {
        return Err(cos_only(
            "knowledge-use accepts only the CoS run credential of the current run",
        ));
    };
    let req: KnowledgeUseRequest = crate::handlers::read_json(body, false).await?;
    let run_id = caller.run_id.clone();
    let used = state
        .blocking(move |store| {
            store
                .chat_run_knowledge_use_add(&run_id, req.op, req.ok)
                .map_err(|e| ApiProblem::internal(e.to_string()))
        })
        .await?;
    Ok(json_response(
        axum::http::StatusCode::OK,
        &serde_json::json!({ "run_id": caller.run_id, "knowledge_use": used }),
    ))
}
