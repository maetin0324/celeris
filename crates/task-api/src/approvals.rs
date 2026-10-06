//! 認可の API（ADR-0033 D5。Phase 26。`docs/api/v1/gui-api.md` §3.56〜3.60）。
//!
//! - `GET /approvals?pending=&project=&node=` — 一覧（読み取り、通常の認証）。`pending` は三値:
//!   `true` = 未決定だけ、`false` = 決定済みだけ、省略 = 全件（GUI からの依頼 R5。Phase 27）。
//! - `POST /approvals/{id}/decide` — 人が `once` / `standing` / `denied` で答える（**管理系**）。
//!   タスクの再開は既存の「質問に答える」経路（`task_ops::gate::answer`）に相乗りする（`task_ops::approval`）。
//!   Phase F7: 認可元のタスクが終端なら決定だけ記録して 200 + `note`、終端でも `blocked` でもなければ
//!   409（何も書かない）。`withdrawn` は celeris だけが書く（人が送ると 400）。
//! - `GET /standing-rules?node=` — 一覧（読み取り）。
//! - `POST /standing-rules` / `DELETE /standing-rules/{id}` — GUI から直接編集する（**管理系**）。
//!
//! `DaemonSnapshot.approvals_pending` は `reports`（Phase 25）と同じ理由で **API が応答を組むときに埋める**
//! （`crate::handlers::system::daemon_snapshot_with_reports` から呼ぶ `approvals_pending` 参照）。

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::approval::{
    Approval, ApprovalId, ApprovalStore, Decision, StandingRule, StandingRuleId,
};
use task_core::{ProjectId, SqliteStore};
use task_ops::approval::Scope;
use time::OffsetDateTime;

use crate::cos::operations::{Applied, OperationAudit};
use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, ops_problem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;

/// `GET /approvals` の応答。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalList {
    pub items: Vec<Approval>,
}

/// `POST /approvals/{id}/decide` の要求本文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalDecideBody {
    pub decision: Decision,
    pub answer: String,
    /// `decision = "standing"` のときだけ意味を持つ。`"node"`（既定）| `"all"`。
    #[serde(default)]
    pub scope: Option<String>,
}

/// `POST /approvals/{id}/decide` の応答。`transition`（`task_ops::gate::TransitionResult`）は
/// `Deserialize` を持たないので、この型も応答専用（`Serialize` だけ）にする。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ApprovalDecideResult {
    pub approval: Approval,
    /// `decision = "standing"` のときだけ `Some`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_rule: Option<StandingRule>,
    /// 元の質問にタスクが紐づいていたときだけ `Some`（既存の「質問に答える」経路の結果）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<task_ops::gate::TransitionResult>,
    /// Phase F7: 認可元のタスクが既に終端（または無い）ため、決定だけ記録してタスクには答えなかった
    /// ときの説明。タスクに答えたとき・タスクの無い approval では出ない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `GET /standing-rules` の応答。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StandingRuleList {
    pub items: Vec<StandingRule>,
}

/// `POST /standing-rules` の要求本文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandingRuleCreateBody {
    /// 省略すると全員向け（`node_id = NULL`）。
    #[serde(default)]
    pub node_id: Option<String>,
    pub rule: String,
}

fn approval_not_found(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "approval_not_found",
        format!("no approval {id}"),
    )
}

fn parse_approval_id(raw: &str) -> Result<ApprovalId, ApiProblem> {
    raw.parse::<ApprovalId>()
        .map_err(|_| approval_not_found(raw))
}

fn standing_rule_not_found(id: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "standing_rule_not_found",
        format!("no standing rule {id}"),
    )
}

fn parse_standing_rule_id(raw: &str) -> Result<StandingRuleId, ApiProblem> {
    raw.parse::<StandingRuleId>()
        .map_err(|_| standing_rule_not_found(raw))
}

/// ADR-0033 D5: スナップショットに載せる未決定の認可の件数（`GET /daemon` から呼ぶ）。
pub(crate) fn approvals_pending(store: &SqliteStore) -> u32 {
    store
        .approval_list(Some(true), None, None)
        .map(|items| u32::try_from(items.len()).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

pub(crate) async fn list_approvals(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["pending", "project", "node"])?;
    // R5（Phase 27）: `pending=true` は未決定だけ、`pending=false` は**決定済みだけ**、省略で全件。
    let pending = query.bool("pending")?;
    let project_id = match query.single("project")? {
        Some(raw) => Some(
            raw.parse::<ProjectId>()
                .map_err(|_| ApiProblem::bad_request("query parameter `project` must be a ULID"))?,
        ),
        None => None,
    };
    let node_id = query.single("node")?.map(str::to_string);
    let items = state
        .blocking(move |store| {
            store
                .approval_list(pending, project_id, node_id.as_deref())
                .map_err(store_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &ApprovalList { items }))
}

pub(crate) async fn decide(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let approval_id = parse_approval_id(&id)?;
    let payload: ApprovalDecideBody = read_json(body, false).await?;
    let decision = payload.decision;
    let outcome = state
        .blocking(move |store| decide_op(store, approval_id, payload, None)?.direct())
        .await?;
    tracing::info!(
        who = "admin",
        op = "approval_decide",
        approval_id = %approval_id,
        decision = decision.as_str(),
        "admin: approval decided"
    );
    Ok(json_response(StatusCode::OK, &outcome))
}

/// `POST /approvals/{id}/decide` の本体。handler（`audit = None`）と CoS の `/cos/operations`
/// （ADR 2026-10-05 D3）が共有する。監査ありでは決定と `standing` の規則を `cos_operation_apply` の
/// transaction 内で書き、紐づく `blocked` のタスクの再開（既存の「質問に答える」経路）は commit 後に行う。
pub(crate) fn decide_op(
    store: &SqliteStore,
    approval_id: ApprovalId,
    payload: ApprovalDecideBody,
    audit: Option<&OperationAudit>,
) -> Result<Applied<ApprovalDecideResult>, ApiProblem> {
    let target_id = approval_id.to_string();
    let reject = |problem: ApiProblem| match audit {
        Some(audit) => audit.reject(store, "approval", &target_id, problem),
        None => problem,
    };
    let scope = match payload.scope.as_deref() {
        None => Scope::Node,
        Some(raw) => Scope::parse(raw).ok_or_else(|| {
            reject(ApiProblem::bad_request(
                "`scope` must be \"node\" or \"all\"",
            ))
        })?,
    };
    let Some(approval) = store
        .approval_get(approval_id)
        .map_err(|e| reject(store_problem(e)))?
    else {
        return Err(reject(approval_not_found(&target_id)));
    };
    let now = OffsetDateTime::now_utc();
    let Some(audit) = audit else {
        let outcome = task_ops::approval::decide(
            store,
            approval,
            payload.decision,
            payload.answer,
            scope,
            now,
        )
        .map_err(|e| ops_problem(store, e, Some("decide")))?;
        return Ok(Applied::Direct(ApprovalDecideResult {
            approval: outcome.approval,
            standing_rule: outcome.standing_rule,
            transition: outcome.transition,
            note: outcome.note,
        }));
    };
    let plan = task_ops::approval::plan_decide(
        store,
        approval,
        payload.decision,
        payload.answer,
        scope,
        now,
    )
    .map_err(|e| reject(ops_problem(store, e, Some("decide"))))?;
    let operation = audit.apply(store, "approval", &target_id, "approval.decide", |tx| {
        SqliteStore::approval_decide_tx(
            tx,
            approval_id,
            plan.decision,
            Some(plan.answer.clone()),
            now,
        )?;
        if let Some(rule) = &plan.standing_rule {
            SqliteStore::standing_rule_append_tx(tx, rule)?;
        }
        Ok(serde_json::json!({
            "approval_id": target_id,
            "decision": plan.decision.as_str(),
            "standing_rule_id": plan.standing_rule.as_ref().map(|r| r.id.to_string()),
        }))
    })?;
    if operation.id == audit.ctx.operation_id
        && let Err(error) = task_ops::approval::finish_decide(store, &plan)
    {
        // The decision is recorded; resuming the task is the same follow-up as on the human path.
        tracing::warn!(operation_id = %operation.id, %error, "cos approval follow-up failed");
    }
    Ok(Applied::Audited(Box::new(operation)))
}

pub(crate) async fn list_standing_rules(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["node"])?;
    let node_id = query.single("node")?.map(str::to_string);
    let items = state
        .blocking(move |store| {
            store
                .standing_rule_list(node_id.as_deref())
                .map_err(store_problem)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &StandingRuleList { items }))
}

pub(crate) async fn create_standing_rule(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let payload: StandingRuleCreateBody = read_json(body, false).await?;
    if payload.rule.trim().is_empty() {
        return Err(ApiProblem::validation(vec![
            crate::types::ValidationError {
                field: Some("rule".to_string()),
                message: "rule must not be blank".to_string(),
            },
        ]));
    }
    let rule = StandingRule {
        id: StandingRuleId::new(),
        node_id: payload.node_id,
        rule: payload.rule,
        created_at: OffsetDateTime::now_utc(),
    };
    let created = rule.clone();
    state
        .blocking(move |store| store.standing_rule_append(&rule).map_err(store_problem))
        .await?;
    tracing::info!(
        who = "admin",
        op = "standing_rule_create",
        standing_rule_id = %created.id,
        node_id = created.node_id.as_deref().unwrap_or("*"),
        "admin: standing rule created"
    );
    Ok(json_response(StatusCode::CREATED, &created))
}

pub(crate) async fn delete_standing_rule(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let rule_id = parse_standing_rule_id(&id)?;
    state
        .blocking(move |store| {
            if !store.standing_rule_delete(rule_id).map_err(store_problem)? {
                return Err(standing_rule_not_found(&rule_id.to_string()));
            }
            tracing::info!(who = "admin", op = "standing_rule_delete", standing_rule_id = %rule_id, "admin: standing rule deleted");
            Ok(())
        })
        .await?;
    Ok(axum::response::IntoResponse::into_response(
        StatusCode::NO_CONTENT,
    ))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/api/v1/approvals", get(list_approvals))
        .route("/api/v1/approvals/{id}/decide", post(decide))
        .route(
            "/api/v1/standing-rules",
            get(list_standing_rules).post(create_standing_rule),
        )
        .route("/api/v1/standing-rules/{id}", delete(delete_standing_rule))
}
