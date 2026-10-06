//! ADR-0079 D7（Phase R3a）: 人への決定の要求（`docs/api/v1/gui-api.md` の「決定の要求」）。
//!
//! - `GET /decisions?open=&root_id=` — 一覧（読み取り、通常の認証）。`open=true` は未回答だけ、`false` は
//!   回答済み・取り下げ済みだけ、省略は全件。`root_id` で 1 つの木に絞る。
//! - `GET /tasks/{id}/decisions?open=` — その task の subtree（その task か子孫が出した決定）。
//! - `POST /decisions/{id}/answer` `{option?, note?}` — 回答（**管理系**）。`option` は決定の選択肢の key。
//!   `choice` の決定だけ `option` を省いて `note` に自由記述で答えられる。404（無い id）/ 409（`open` でない・
//!   決定を出した節点が終端）/ 422（選択肢の外・daemon の決定で `option` 無し・note が長すぎる）。
//! - `POST /decisions/{id}/withdraw` `{reason?}` — 人の取り下げ（**管理系**。止めていた unit を取り下げ、
//!   `needed_before: [self]` なら節点を中止）。
//! - `POST /decisions/{id}/revise` `{option?, note?}` — 回答済みの `choice` の答えを変える（**管理系**）。
//!
//! ハンドラは HTTP への写像だけで、判断と効き目は `task_ops::decision`（決定的、LLM なし）。
//! `[execution.tree] enabled = false`（既定）では決定は作られないので、一覧は空、回答は 404 になる。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use task_ops::decision::{DecisionAnswerBody, DecisionFilter, DecisionWithdrawBody};
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem};
use crate::query::{QueryParams, parse_task_id};
use crate::state::ApiState;

/// API から答えたときの `DecisionAnswered.by`（管理系のトークン = 人）。
const HUMAN: &str = "human";
/// CoS の `/cos/operations` から答えたときの `DecisionAnswered.by`。
const COS: &str = "cos";

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/v1/decisions", get(list_decisions))
        .route("/api/v1/decisions/{id}/answer", post(answer))
        .route("/api/v1/decisions/{id}/withdraw", post(withdraw))
        .route("/api/v1/decisions/{id}/revise", post(revise))
        .route("/api/v1/tasks/{id}/decisions", get(task_decisions))
}

async fn list_decisions(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["open", "root_id"])?;
    let open = query.bool("open")?;
    let root_id = match query.single("root_id")? {
        Some(raw) => Some(parse_task_id(raw).map_err(|_| {
            ApiProblem::bad_request("query parameter `root_id` must be a task id (ULID)")
        })?),
        None => None,
    };
    let list = state
        .blocking(move |store| {
            task_ops::decision::list(store, &DecisionFilter { open, root_id })
                .map_err(|e| ops_problem(store, e, Some("decision_list")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

async fn task_decisions(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["open"])?;
    let open = query.bool("open")?;
    let task_id = parse_task_id(&id)?;
    let list = state
        .blocking(move |store| {
            task_ops::decision::for_subtree(store, task_id, open)
                .map_err(|e| ops_problem(store, e, Some("decision_list")))
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

async fn answer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let payload: DecisionAnswerBody = read_json(body, false).await?;
    let decision_id = id.clone();
    let outcome = state
        .blocking(move |store| answer_op(store, &decision_id, payload, None)?.direct())
        .await?;
    tracing::info!(who = "admin", op = "decision_answer", decision_id = %id, effect = outcome.effect.as_str(), resumed = outcome.resumed.len(), cancelled = outcome.cancelled.len(), "admin: decision answered");
    Ok(json_response(StatusCode::OK, &outcome))
}

/// `POST /decisions/{id}/answer` の本体。handler（`audit = None`、`by = human`）と CoS の
/// `/cos/operations`（ADR 2026-10-05 D3。`by = cos`、回答・unit 更新・監査を同じ transaction で書く）が
/// 共有する。
pub(crate) fn answer_op(
    store: &task_core::store::SqliteStore,
    decision_id: &str,
    payload: DecisionAnswerBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<task_ops::decision::DecisionOutcome>, ApiProblem> {
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        return task_ops::decision::answer(
            store,
            decision_id,
            payload.option.as_deref(),
            payload.note.as_deref(),
            HUMAN,
            now,
        )
        .map(Applied::Direct)
        .map_err(|e| ops_problem(store, e, Some("decision_answer")));
    };
    let plan = task_ops::decision::plan_answer(
        store,
        decision_id,
        payload.option.as_deref(),
        payload.note.as_deref(),
        COS,
        now,
    )
    .map_err(|e| {
        audit.reject(
            store,
            "decision",
            decision_id,
            ops_problem(store, e, Some("decision_answer")),
        )
    })?;
    let operation = audit.apply(store, "decision", decision_id, "decision.answer", |tx| {
        let applied = task_core::store::SqliteStore::decision_resolve_apply_tx(
            tx,
            plan.node_id,
            &plan.decision_id,
            task_core::decision::DecisionStatus::Open,
            plan.rows(),
            plan.events(),
        )?;
        if !applied {
            return Err(task_core::chat::ChatError::Conflict(format!(
                "decision {decision_id} is no longer open"
            )));
        }
        Ok(serde_json::json!({"decision_id": decision_id, "task_id": plan.node_id.to_string()}))
    })?;
    if operation.id == audit.ctx.operation_id
        && let Err(error) = task_ops::decision::finish_answer(store, plan, now)
    {
        tracing::warn!(operation_id = %operation.id, %error, "cos decision follow-up failed");
    }
    Ok(Applied::Audited(Box::new(operation)))
}

async fn withdraw(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let payload: DecisionWithdrawBody = read_json(body, true).await?;
    let decision_id = id.clone();
    let outcome = state
        .blocking(move |store| {
            task_ops::decision::withdraw(
                store,
                &decision_id,
                payload.reason.as_deref(),
                HUMAN,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| ops_problem(store, e, Some("decision_withdraw")))
        })
        .await?;
    tracing::info!(who = "admin", op = "decision_withdraw", decision_id = %id, cancelled = outcome.cancelled.len(), "admin: decision withdrawn");
    Ok(json_response(StatusCode::OK, &outcome))
}

async fn revise(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let payload: DecisionAnswerBody = read_json(body, false).await?;
    let decision_id = id.clone();
    let outcome = state
        .blocking(move |store| {
            task_ops::decision::revise(
                store,
                &decision_id,
                payload.option.as_deref(),
                payload.note.as_deref(),
                HUMAN,
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| ops_problem(store, e, Some("decision_revise")))
        })
        .await?;
    tracing::info!(who = "admin", op = "decision_revise", decision_id = %id, notified = outcome.notified_children.len(), "admin: decision revised");
    Ok(json_response(StatusCode::OK, &outcome))
}
