//! ADR 2026-11-03-ts-intended-behavior-change D4: TS の方針改善の仕様の承認・取り消し・一覧。
//!
//! - `GET /script/behavior-changes` → 200 `ScriptBehaviorChangeList`（全版の行と追記専用の記録）。
//! - `POST /script/behavior-changes` → 200 `ScriptBehaviorChangeApproveResult`。本文 `ScriptBehaviorChangeApproveBody`。
//!   人の API token の主体（`human:*`）だけ。他は 403 `script_behavior_change_requires_human`（表も記録も変えない）。
//! - `POST /script/behavior-changes/{spec_sha256}/revoke` → 200 `ScriptBehaviorChangeRevokeResult`（認証された誰でも）。
//! - CoS の operations には無い（承認は人だけ、取り消しは API で行う）。

use axum::{
    body::Body,
    extract::{Path, RawQuery, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use task_core::script_behavior_change::{
    BehaviorChangeError, BehaviorChangeEvent, BehaviorChangeRow, BehaviorChangeSpec,
};

use crate::{
    handlers::{ApiResult, json_response, no_query, read_json},
    problem::{ApiProblem, store_problem},
    state::ApiState,
};

/// 承認を人以外の主体が求めた。403。
pub const BEHAVIOR_CHANGE_REQUIRES_HUMAN: &str = "script_behavior_change_requires_human";
/// 仕様の形の不備・`supersedes` の食い違い。422。
pub const BEHAVIOR_CHANGE_INVALID: &str = "behavior_change_invalid";
/// 比較の結線が無い領域。422。
pub const BEHAVIOR_CHANGE_DOMAIN_UNSUPPORTED: &str = "behavior_change_domain_unsupported";
/// `spec_sha256` が計算と違う。422。
pub const BEHAVIOR_CHANGE_HASH_MISMATCH: &str = "behavior_change_hash_mismatch";
/// 同じ `id`・`version` に別の仕様がある。409。
pub const BEHAVIOR_CHANGE_VERSION_TAKEN: &str = "behavior_change_version_taken";
/// `version` が過去の版以下。409。
pub const BEHAVIOR_CHANGE_VERSION_NOT_NEWER: &str = "behavior_change_version_not_newer";
/// 取り消した・置き換えた版を承認し直そうとした。409。
pub const BEHAVIOR_CHANGE_RETIRED: &str = "behavior_change_retired";
/// 未知の hash。404。
pub const BEHAVIOR_CHANGE_NOT_FOUND: &str = "behavior_change_not_found";

/// `POST /script/behavior-changes` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptBehaviorChangeApproveBody {
    /// `control-ts/behavior-changes/<domain>/<id>.v<version>.json` の中身。
    pub spec: BehaviorChangeSpec,
    /// 人が確かめた仕様の hash（64 桁 hex。`script-host-runner behavior` の出力の `spec_sha256`）。
    pub spec_sha256: String,
    /// 承認の理由（記録の `reason`）。
    pub reason: String,
}

/// `POST /script/behavior-changes` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptBehaviorChangeApproveResult {
    /// 表を変えたか（同じ hash がすでに承認済みなら `false`）。
    pub changed: bool,
    /// 同じ transaction で置き換えた前の版の hash。
    pub superseded: Option<String>,
    pub change: BehaviorChangeRow,
}

/// `POST /script/behavior-changes/{spec_sha256}/revoke` の本文。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptBehaviorChangeRevokeBody {
    pub reason: String,
}

/// `POST /script/behavior-changes/{spec_sha256}/revoke` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptBehaviorChangeRevokeResult {
    /// 取り消したか（承認済みでなければ `false` で、表も記録も変えない）。
    pub revoked: bool,
}

/// `GET /script/behavior-changes` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct ScriptBehaviorChangeList {
    pub changes: Vec<BehaviorChangeRow>,
    pub events: Vec<BehaviorChangeEvent>,
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/script/behavior-changes",
            axum::routing::get(list).post(approve),
        )
        .route(
            "/api/v1/script/behavior-changes/{spec_sha256}/revoke",
            axum::routing::post(revoke),
        )
}

async fn list(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    no_query(&raw)?;
    let body = state
        .blocking(|store| {
            Ok::<_, ApiProblem>(ScriptBehaviorChangeList {
                changes: store.script_behavior_changes().map_err(store_problem)?,
                events: store
                    .script_behavior_change_events()
                    .map_err(store_problem)?,
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &body))
}

async fn approve(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptBehaviorChangeApproveBody = read_json(body, false).await?;
    let actor = caller.actor();
    let result = state
        .blocking(move |store| {
            store
                .script_behavior_change_approve(
                    &body.spec,
                    &body.spec_sha256,
                    &actor,
                    &body.reason,
                    time::OffsetDateTime::now_utc(),
                )
                .map_err(problem)
        })
        .await?;
    tracing::info!(
        op = "script_behavior_change_approve",
        id = %result.row.id,
        version = result.row.version,
        changed = result.changed,
        "script behavior change"
    );
    Ok(json_response(
        StatusCode::OK,
        &ScriptBehaviorChangeApproveResult {
            changed: result.changed,
            superseded: result.superseded,
            change: result.row,
        },
    ))
}

async fn revoke(
    State(state): State<ApiState>,
    caller: crate::principal::Caller,
    Path(sha): Path<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: ScriptBehaviorChangeRevokeBody = read_json(body, false).await?;
    let actor = caller.actor();
    let revoked = state
        .blocking(move |store| {
            store
                .script_behavior_change_revoke(
                    &sha,
                    &actor,
                    &body.reason,
                    time::OffsetDateTime::now_utc(),
                )
                .map_err(problem)
        })
        .await?;
    tracing::info!(
        op = "script_behavior_change_revoke",
        revoked,
        "script behavior change"
    );
    Ok(json_response(
        StatusCode::OK,
        &ScriptBehaviorChangeRevokeResult { revoked },
    ))
}

fn problem(err: BehaviorChangeError) -> ApiProblem {
    let detail = err.to_string();
    let (status, code) = match err {
        BehaviorChangeError::RequiresHuman(_) => {
            (StatusCode::FORBIDDEN, BEHAVIOR_CHANGE_REQUIRES_HUMAN)
        }
        BehaviorChangeError::Invalid(_) => {
            (StatusCode::UNPROCESSABLE_ENTITY, BEHAVIOR_CHANGE_INVALID)
        }
        BehaviorChangeError::DomainUnsupported(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            BEHAVIOR_CHANGE_DOMAIN_UNSUPPORTED,
        ),
        BehaviorChangeError::HashMismatch { .. } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            BEHAVIOR_CHANGE_HASH_MISMATCH,
        ),
        BehaviorChangeError::VersionTaken { .. } => {
            (StatusCode::CONFLICT, BEHAVIOR_CHANGE_VERSION_TAKEN)
        }
        BehaviorChangeError::VersionNotNewer { .. } => {
            (StatusCode::CONFLICT, BEHAVIOR_CHANGE_VERSION_NOT_NEWER)
        }
        BehaviorChangeError::Retired { .. } => (StatusCode::CONFLICT, BEHAVIOR_CHANGE_RETIRED),
        BehaviorChangeError::NotFound(_) => (StatusCode::NOT_FOUND, BEHAVIOR_CHANGE_NOT_FOUND),
        BehaviorChangeError::Store(e) => return store_problem(e),
    };
    ApiProblem::new(status, code, detail)
}
