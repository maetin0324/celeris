//! ADR 2026-10-08-browser-prod-enablement D3: `/api/v1/browser/site-policies`。
//!
//! 管理者の site policy（ログインの exact origin・URL・selector）の正本は DB（`browser_site_policies`）。
//! 書くのは admin token だけ（web は owner session を要求してから呼ぶ）。daemon の broker control は
//! 手動登録のたびに DB を読む（`browser::StoreSitePolicies`）ので、ここでの変更は再起動なしに次の判定から効く。
//!
//! - `GET    /browser/site-policies`               一覧（`policy_id` 昇順）
//! - `PUT    /browser/site-policies/{policy_id}`   作成または置換（ADR-0110 D2 の形式検証。失敗は 422）
//! - `DELETE /browser/site-policies/{policy_id}`   削除（grant・未解決の wait が参照していれば 409）

use axum::Json;
use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::{HeaderMap, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{BrowserSitePolicy, BrowserSitePolicyDelete, BrowserSitePolicyRecord};
use time::OffsetDateTime;

use crate::browser::TrustedSitePolicy;
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;

/// `policy_id` の長さの上限。
pub const SITE_POLICY_ID_MAX_LEN: usize = 64;

/// `policy_id` は `[A-Za-z0-9._-]` の 1〜64 文字（path と grant の id に使うので狭く取る）。
pub fn valid_site_policy_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= SITE_POLICY_ID_MAX_LEN
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// `PUT /browser/site-policies/{policy_id}` の本文（`policy_id` は path）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SitePolicyPutBody {
    /// 正規形の origin（`https://host[:port]`）。
    pub exact_origin: String,
    /// `exact_origin` 直下のログイン URL。
    pub login_url: String,
    pub password_selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_selector: Option<String>,
    /// ADR 2026-10-09 credential username / post-login D1-1: password 欄と同じ頁の username 欄（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_selector: Option<String>,
    /// 同 D2-1: ログイン後に読み取ってよい origin と action（任意。無ければ観測停止のまま）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_login: Option<task_core::browser_wait::PostLogin>,
    /// 同 付記 2026-10-10b: IdP の属性送信の同意頁で controller が 1 回だけ押す固定ボタン（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<task_core::browser_wait::ConsentPolicy>,
}

/// `GET /browser/site-policies` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SitePolicyList {
    pub items: Vec<BrowserSitePolicyRecord>,
}

/// `PUT` の応答（新規作成は 201、置換は 200）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SitePolicyPutResult {
    pub created: bool,
    pub policy: BrowserSitePolicyRecord,
}

fn invalid_id() -> ApiProblem {
    ApiProblem::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "site_policy_invalid",
        "policy_id must be 1-64 characters of [A-Za-z0-9._-]",
    )
    .with_extra("reason", "policy_id")
}

fn not_found() -> ApiProblem {
    ApiProblem::new(
        StatusCode::NOT_FOUND,
        "site_policy_not_found",
        "site policy not found",
    )
}

async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<SitePolicyList>, ApiProblem> {
    require_admin(&state, &headers)?;
    let items = state
        .blocking(|store| store.browser_site_policy_list().map_err(store_problem))
        .await?;
    Ok(Json(SitePolicyList { items }))
}

async fn put(
    State(state): State<ApiState>,
    Path(policy_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<SitePolicyPutBody>, JsonRejection>,
) -> Result<(StatusCode, Json<SitePolicyPutResult>), ApiProblem> {
    require_admin(&state, &headers)?;
    if !valid_site_policy_id(&policy_id) {
        return Err(invalid_id());
    }
    let body = body.map(|Json(v)| v).map_err(|e| {
        ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "site_policy_invalid",
            format!("invalid site policy body: {}", e.body_text()),
        )
        .with_extra("reason", "body")
    })?;
    let policy = validated_policy(policy_id, body)?;
    let result = state
        .blocking(move |store| upsert_policy(store, &policy, "admin"))
        .await?;
    let status = if result.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(result)))
}

/// Create or replace a validated site policy (route and CoS `browser.site_policy_put`).
pub(crate) fn upsert_policy(
    store: &task_core::SqliteStore,
    policy: &BrowserSitePolicy,
    actor: &str,
) -> Result<SitePolicyPutResult, ApiProblem> {
    let (policy, created) = store
        .browser_site_policy_upsert(policy, actor, OffsetDateTime::now_utc())
        .map_err(store_problem)?;
    Ok(SitePolicyPutResult { created, policy })
}

async fn delete(
    State(state): State<ApiState>,
    Path(policy_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiProblem> {
    require_admin(&state, &headers)?;
    if !valid_site_policy_id(&policy_id) {
        return Err(not_found());
    }
    state
        .blocking(move |store| delete_policy(store, &policy_id, "admin"))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// ADR-0110 D2: the broker's own format check of a site policy (fixed `reason` code). Shared by
/// the route and the CoS operation `browser.site_policy_put`.
pub(crate) fn validated_policy(
    policy_id: String,
    body: SitePolicyPutBody,
) -> Result<BrowserSitePolicy, ApiProblem> {
    if !valid_site_policy_id(&policy_id) {
        return Err(invalid_id());
    }
    let trusted = TrustedSitePolicy {
        policy_id,
        exact_origin: body.exact_origin,
        login_url: body.login_url,
        password_selector: body.password_selector,
        submit_selector: body.submit_selector,
        username_selector: body.username_selector,
        post_login: body.post_login,
        consent: body.consent,
    };
    trusted.validate().map_err(|code| {
        ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "site_policy_invalid",
            format!("site policy rejected: {code}"),
        )
        .with_extra("reason", code)
    })?;
    Ok(trusted.into())
}

/// Delete a site policy (404 missing, 409 still referenced). Shared by the route and the CoS
/// operation `browser.site_policy_delete`; `actor` is recorded by the store.
pub(crate) fn delete_policy(
    store: &task_core::SqliteStore,
    policy_id: &str,
    actor: &str,
) -> Result<(), ApiProblem> {
    if !valid_site_policy_id(policy_id) {
        return Err(not_found());
    }
    match store
        .browser_site_policy_delete(policy_id, actor, OffsetDateTime::now_utc())
        .map_err(store_problem)?
    {
        BrowserSitePolicyDelete::Deleted => Ok(()),
        BrowserSitePolicyDelete::NotFound => Err(not_found()),
        BrowserSitePolicyDelete::InUse(refs) => Err(ApiProblem::new(
            StatusCode::CONFLICT,
            "site_policy_in_use",
            "site policy is referenced by a browser grant or an open browser wait",
        )
        .with_extra("node_ids", refs.node_ids)
        .with_extra("wait_ids", refs.wait_ids)),
    }
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, put as put_route};
    axum::Router::new()
        .route("/api/v1/browser/site-policies", get(list))
        .route(
            "/api/v1/browser/site-policies/{policy_id}",
            put_route(put).delete(delete),
        )
}
