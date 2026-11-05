//! ADR 2026-11-04-project-controller A1: 案件の controller の API（`/projects/{id}/controller`）。
//!
//! - `GET …/controller` → `ProjectControllerView`（行が無ければ mode `off` の既定）。`summary` に未達・達成・
//!   待機の理由・許可の範囲・次の wake・guard の上限・最後の判断の追跡（第 3 フェーズ P2-J）。
//! - `PUT …/controller` `{mode, expected_revision, reason}`: mode を上げるのは人の token だけ（operator・MCP・
//!   CoS は `off` へ下げるだけ）。`shadow` 以上は `[project_controller] allowed_project_ids` の案件だけ（403）。
//!   `expected_revision` が違えば 409。CoS は `/cos/operations` の `project.controller_put`。
//! - `POST …/controller/wake` `{external_ref, note, idempotency_key}`: 外部待ちの解除の wake
//!   `external:<ref>:<key>`（同じ key は 1 件）。CoS は `project.controller_wake`。
//! - `GET …/controller/decisions?before=&limit=` → 判断の記録（新しい順）。

use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use task_core::chat::CosOperation;
use task_core::project_wake::{
    ControllerMode, NewProjectWake, ProjectControllerDecision, ProjectControllerView,
    ProjectWakeKind,
};
use task_core::store::{ModeChange, SqliteStore, project_controller_default_state};
use time::OffsetDateTime;

use super::*;
use crate::cos::operations::OperationAudit;
use crate::middleware::require_admin;
use crate::principal::{Caller, Principal};
use crate::problem::ops_problem;
use crate::query::QueryParams;

/// `[project_controller]` のうち API が使う欄。
#[derive(Debug, Clone, Default)]
pub struct ControllerApiConfig {
    pub enabled: bool,
    pub allowed_project_ids: Vec<String>,
    /// `[project_controller.budget]` の実効値（起動時。`GET …/controller` の `summary.budget_limits`）。
    pub budget: task_ops::project_budget::BudgetLimits,
}

/// `PUT …/controller` の本文。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControllerPut {
    pub mode: ControllerMode,
    /// 今の `revision`（行が無ければ 0）。違えば 409 `project_controller_revision_conflict`。
    pub expected_revision: i64,
    pub reason: String,
    /// A7: `enabled` に上げるときの `launch_root` の許可の期限（RFC3339）。無ければ 30 日後。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_grant_expires_at: Option<String>,
    /// A7: 同じ許可の回数。無ければ 10。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_grant_budget: Option<i64>,
}

/// A7: 人が `enabled` に上げる操作は、その理由を根拠に `launch_root` の許可を 1 件作る（同じ tx。前の同種の許可は取り消す）。
fn enable_grant_tx(
    tx: &rusqlite::Transaction<'_>,
    project: &str,
    revision: i64,
    body: &ControllerPut,
    by: &str,
    at: OffsetDateTime,
) -> Result<(), ApiProblem> {
    use task_core::authority as a;
    SqliteStore::authority_supersede_operation_grants_tx(tx, project, by, at)
        .map_err(store_problem)?;
    let expires_at = match &body.launch_grant_expires_at {
        Some(v) => v.clone(),
        None => (at + time::Duration::days(a::DEFAULT_LAUNCH_GRANT_DAYS))
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| ApiProblem::internal(e.to_string()))?,
    };
    let new = a::NewAuthorityGrant {
        id: ulid::Ulid::new().to_string(),
        project_id: project.to_owned(),
        scope: a::GrantScope {
            kind: a::ScopeKind::Project,
            reference: project.to_owned(),
        },
        action_class: a::ActionClass::LaunchRoot,
        source: a::GrantSource {
            kind: a::SourceKind::HumanOperation,
            reference: format!("project.controller_put:{project}:{revision}"),
        },
        granted_by: by.to_owned(),
        question_key: None,
        note: body.reason.clone(),
        expires_at,
        budget: body
            .launch_grant_budget
            .unwrap_or(a::DEFAULT_LAUNCH_GRANT_BUDGET),
    };
    SqliteStore::authority_grant_insert_tx(tx, &new, at)
        .map_err(store_problem)?
        .map_err(|r| ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, r.code, r.detail))?;
    Ok(())
}

/// A9: `POST /projects` の直後の案件は `proposed` で、これを `active` へ移す API は無い。人が `enabled` に上げる操作を
/// 人の返事として同じ tx で `active` にする（`proposed` のままだと TS と Rust の検証が起票を断り続ける）。
fn activate_proposed_tx(
    tx: &rusqlite::Transaction<'_>,
    project_id: task_core::ProjectId,
) -> Result<(), ApiProblem> {
    let proposed: bool = tx
        .query_row(
            "SELECT status = 'proposed' FROM projects WHERE id = ?1",
            [project_id.to_string()],
            |r| r.get(0),
        )
        .map_err(|e| ApiProblem::internal(e.to_string()))?;
    if proposed {
        SqliteStore::project_set_status_tx(tx, project_id, task_core::ProjectStatus::Active)
            .map_err(store_problem)?;
    }
    Ok(())
}

/// `POST …/controller/wake` の本文。
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControllerWake {
    pub external_ref: String,
    #[serde(default)]
    pub note: String,
    pub idempotency_key: String,
}

/// `POST …/controller/wake` の応答。
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ControllerWakeResult {
    pub wake_key: String,
    /// 新しく積んだか（同じ key の再送・mode `off` は `false`）。
    pub added: bool,
}

const TEXT_MAX: usize = 200;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/projects/{id}/controller",
            get(get_controller).put(put_controller),
        )
        .route(
            "/api/v1/projects/{id}/controller/wake",
            post(post_controller_wake),
        )
        .route(
            "/api/v1/projects/{id}/controller/decisions",
            get(get_controller_decisions),
        )
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn require_project(store: &SqliteStore, id: task_core::ProjectId) -> Result<(), ApiProblem> {
    match store.project_get(id).map_err(store_problem)? {
        Some(_) => Ok(()),
        None => Err(ApiProblem::project_not_found(&id.to_string())),
    }
}

/// 行が無ければ既定（mode `off`）の view。
pub(crate) fn controller_view(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    limits: &task_ops::project_budget::BudgetLimits,
) -> Result<ProjectControllerView, ApiProblem> {
    let project = project_id.to_string();
    let project = project.as_str();
    let at = now()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let state = store
        .project_controller_state(project)
        .map_err(store_problem)?
        .unwrap_or_else(|| project_controller_default_state(project, &at));
    let pending_wakes = store
        .project_wakes_pending(project)
        .map_err(store_problem)?;
    let last_decision = match &state.last_decision_id {
        Some(id) => store
            .project_controller_decision(id)
            .map_err(store_problem)?,
        None => None,
    };
    let used = task_ops::project_budget::usage(store, project, now()).map_err(store_problem)?;
    let summary = task_ops::project_controller_view::summary(
        store,
        project_id,
        &state,
        last_decision.as_ref(),
        &pending_wakes,
        limits,
        now(),
    )
    .map_err(|e| ops_problem(store, e, None))?;
    Ok(ProjectControllerView {
        state,
        pending_wakes,
        last_decision,
        budget_today: task_core::project_wake::ProjectBudgetToday {
            day: used.day,
            cos_inputs: used.cos_inputs,
            roots: used.roots,
            tokens: used.tokens,
            chat_starts: used.chat_starts,
        },
        summary,
    })
}

async fn get_controller(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let project_id = parse_project_id(&id)?;
    let limits = state.project_controller.budget.clone();
    let view = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            controller_view(store, project_id, &limits)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

/// mode の変更の規則（D8・D10・D11）。`human` は人の token（または loopback の無認証）。
pub(crate) fn check_mode_change(
    cfg: &ControllerApiConfig,
    project: &str,
    current: ControllerMode,
    next: ControllerMode,
    human: bool,
) -> Result<(), ApiProblem> {
    if next.rank() > current.rank() && !human {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "project_controller_mode_requires_human",
            "only a person may raise the project controller mode; others may only turn it off",
        ));
    }
    if next.wakes() && !cfg.enabled {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "project_controller_disabled",
            "[project_controller] enabled is false on this daemon",
        ));
    }
    if next.wakes() && !cfg.allowed_project_ids.iter().any(|p| p == project) {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "project_controller_not_allowed",
            format!("project {project} is not in [project_controller] allowed_project_ids"),
        ));
    }
    Ok(())
}

fn validate_put(body: &ControllerPut) -> Result<(), ApiProblem> {
    if body.reason.trim().is_empty() || body.reason.chars().count() > 2000 {
        return Err(ApiProblem::bad_request(
            "reason must be 1 to 2000 characters and not blank",
        ));
    }
    Ok(())
}

fn mode_outcome(change: ModeChange) -> Result<Value, ApiProblem> {
    match change {
        ModeChange::Changed(state) => {
            serde_json::to_value(state).map_err(|e| ApiProblem::internal(e.to_string()))
        }
        ModeChange::Stale { current } => Err(ApiProblem::new(
            StatusCode::CONFLICT,
            "project_controller_revision_conflict",
            format!("expected_revision does not match the current revision {current}"),
        )),
    }
}

async fn put_controller(
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
    let body: ControllerPut = read_json(body, false).await?;
    validate_put(&body)?;
    let cfg = state.project_controller.clone();
    let human = matches!(
        caller.0,
        Principal::Human { .. } | Principal::Unauthenticated
    );
    let by = caller.actor();
    let view = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let project = project_id.to_string();
            let current = store
                .project_controller_state(&project)
                .map_err(store_problem)?
                .map_or(ControllerMode::Off, |s| s.mode);
            check_mode_change(&cfg, &project, current, body.mode, human)?;
            let raises_to_enabled =
                body.mode == ControllerMode::Enabled && current != ControllerMode::Enabled;
            store.write_tx::<_, ApiProblem, _>(|tx| {
                let at = now();
                let change = SqliteStore::project_controller_set_mode_tx(
                    tx,
                    &project,
                    body.mode,
                    Some(body.expected_revision),
                    &by,
                    at,
                )
                .map_err(store_problem)?;
                let revision = match &change {
                    ModeChange::Changed(state) => state.revision,
                    ModeChange::Stale { current } => *current,
                };
                mode_outcome(change)?;
                if raises_to_enabled {
                    enable_grant_tx(tx, &project, revision, &body, &by, at)?;
                    activate_proposed_tx(tx, project_id)?;
                }
                Ok(())
            })?;
            controller_view(store, project_id, &cfg.budget)
        })
        .await?;
    Ok(json_response(StatusCode::OK, &view))
}

fn validate_wake(body: &ControllerWake) -> Result<(), ApiProblem> {
    let bad = |s: &str| s.trim().is_empty() || s.chars().count() > TEXT_MAX || s.contains(':');
    if bad(&body.external_ref) || bad(&body.idempotency_key) {
        return Err(ApiProblem::bad_request(format!(
            "external_ref and idempotency_key must be 1 to {TEXT_MAX} characters without ':'"
        )));
    }
    if body.note.chars().count() > 2000 {
        return Err(ApiProblem::bad_request(
            "note must be at most 2000 characters",
        ));
    }
    Ok(())
}

fn wake_of(project: &str, body: &ControllerWake) -> NewProjectWake {
    NewProjectWake {
        project_id: project.into(),
        wake_key: format!("external:{}:{}", body.external_ref, body.idempotency_key),
        kind: ProjectWakeKind::ExternalReleased,
        source_event_id: None,
        source_ref: Some(body.external_ref.clone()),
    }
}

async fn post_controller_wake(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let project_id = parse_project_id(&id)?;
    let body: ControllerWake = read_json(body, false).await?;
    validate_wake(&body)?;
    let out = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            let wake = wake_of(&project_id.to_string(), &body);
            let added = store
                .project_wake_ingest(std::slice::from_ref(&wake), None, 30, now())
                .map_err(store_problem)?
                > 0;
            Ok(ControllerWakeResult {
                wake_key: wake.wake_key,
                added,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &out))
}

async fn get_controller_decisions(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let q = QueryParams::parse(raw.as_deref(), &["before", "limit"])?;
    let before = q.single("before")?.map(str::to_owned);
    let limit = match q.single("limit")? {
        None => 50,
        Some(v) => v
            .parse::<usize>()
            .map_err(|_| ApiProblem::bad_request("limit must be an integer"))?
            .clamp(1, 200),
    };
    let project_id = parse_project_id(&id)?;
    let items: Vec<ProjectControllerDecision> = state
        .blocking(move |store| {
            require_project(store, project_id)?;
            store
                .project_controller_decisions(&project_id.to_string(), before.as_deref(), limit)
                .map_err(store_problem)
        })
        .await?;
    Ok(json_response(
        StatusCode::OK,
        &serde_json::json!({ "items": items }),
    ))
}

// ---- CoS（`/cos/operations`）----

/// `project.controller_put`: CoS は `off` へ下げるだけ（D8）。判断と監査は同じ transaction。
pub(crate) fn cos_controller_put(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    body: ControllerPut,
    audit: &OperationAudit,
) -> Result<CosOperation, ApiProblem> {
    let target = project_id.to_string();
    validate_put(&body).map_err(|p| audit.reject(store, "project", &target, p))?;
    if body.mode != ControllerMode::Off {
        return Err(audit.reject(
            store,
            "project",
            &target,
            ApiProblem::new(
                StatusCode::FORBIDDEN,
                "project_controller_mode_requires_human",
                "the CoS may only turn the project controller off",
            ),
        ));
    }
    require_project(store, project_id).map_err(|p| audit.reject(store, "project", &target, p))?;
    audit.apply_checked(store, "project", &target, "project.controller_put", |tx| {
        let change = SqliteStore::project_controller_set_mode_tx(
            tx,
            &target,
            ControllerMode::Off,
            Some(body.expected_revision),
            &audit.by(),
            now(),
        )
        .map_err(store_problem)?;
        mode_outcome(change)
    })
}

/// `project.controller_wake`: 外部待ちの解除の wake を積む（同じ key は 1 件）。
pub(crate) fn cos_controller_wake(
    store: &SqliteStore,
    project_id: task_core::ProjectId,
    body: ControllerWake,
    audit: &OperationAudit,
) -> Result<CosOperation, ApiProblem> {
    let target = project_id.to_string();
    validate_wake(&body).map_err(|p| audit.reject(store, "project", &target, p))?;
    require_project(store, project_id).map_err(|p| audit.reject(store, "project", &target, p))?;
    let wake = wake_of(&target, &body);
    audit.apply_checked(store, "project", &target, "project.controller_wake", |tx| {
        let added =
            SqliteStore::project_wake_insert_tx(tx, &wake, 30, now()).map_err(store_problem)?;
        Ok(serde_json::json!({ "wake_key": wake.wake_key, "added": added }))
    })
}
