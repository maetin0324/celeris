//! ADR 2026-11-12-knowledge-usage-per-run D1・D3: CoS run の KB の操作を API の受け側で数える。
//!
//! 数える受け側は次の 4 つで、どれも [`note`] を通る。どの run の数かは run credential（[`CosCaller`]）から
//! 決め（body に run id は無い）、`chat_runs.knowledge_use_json` に 1 回を足す。数は観測値で、権限・判断の
//! 根拠には使わない。
//!
//! - `GET /api/v1/knowledge/tree`（検索。CoS の常設の文脈が案内する入口）→ `search`
//! - `GET /api/v1/knowledge/page`（取得）→ `get`
//! - `POST /api/v1/cos/operations` の `knowledge.record`（`POST /api/v1/knowledge/inbox`）→ `record`
//! - `POST /api/v1/cos/knowledge-use`（`{"op": "search"|"get", "ok": bool}`）: CoS run の
//!   `celerisctl knowledge search|get` は KB の根を直接読み API を通らないので、入口がこの route に 1 回を送る。

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
    if req.op == KnowledgeOp::Record {
        // 記録は API を通る（operation `knowledge.record`）ので受け側が数える。ここでも数えると二重になる。
        return Err(ApiProblem::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "record is counted by the knowledge.record operation; send only search or get here",
        ));
    }
    let used = add(&state, &caller, req.op, req.ok).await?;
    Ok(json_response(
        axum::http::StatusCode::OK,
        &serde_json::json!({ "run_id": caller.run_id, "knowledge_use": used }),
    ))
}

/// CoS run `caller` の回数に 1 回を足す（行が無い・計測していない run なら `None`）。
async fn add(
    state: &ApiState,
    caller: &CosCaller,
    op: KnowledgeOp,
    ok: bool,
) -> Result<Option<task_core::KnowledgeUse>, ApiProblem> {
    let run_id = caller.run_id.clone();
    state
        .blocking(move |store| {
            store
                .chat_run_knowledge_use_add(&run_id, op, ok)
                .map_err(|e| ApiProblem::internal(e.to_string()))
        })
        .await
}

/// KB の受け側（tree・page・`knowledge.record`）の 1 回を数える。CoS run でない呼び手（人・web・worker）は
/// 数えない。数えられなくても KB の操作は失敗にしない（計測の失敗で作業を止めない。ADR D1）。
pub(crate) async fn note(state: &ApiState, caller: Option<&CosCaller>, op: KnowledgeOp, ok: bool) {
    let Some(caller) = caller else {
        return;
    };
    if let Err(problem) = add(state, caller, op, ok).await {
        tracing::warn!(run_id = %caller.run_id, ?op, ?problem, "knowledge usage: could not count a CoS run operation (ADR 2026-11-12-knowledge-usage-per-run D1)");
    }
}
