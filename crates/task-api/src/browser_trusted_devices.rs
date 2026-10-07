//! ADR 2026-10-07-browser-trusted-devices D2〜D5: `/api/v1/browser/trusted-devices`。
//!
//! 呼ぶのは web gateway だけ。daemon token（管理系の Bearer）に加え、web の Ed25519 assertion
//! （`browser_live::verify_signature`。鍵も検証の仕組みも Live View と同じ）を毎回確かめる。payload は
//! `DeviceClaims` で、`purpose` が端点と一致し、本文・path の値を束縛していなければ拒否する。
//! 秘密の値はここに来ない（web が SHA-256 にした hash だけ）。応答に hash は返さない。時刻は
//! `ApiState` の注入した時計で決める。
//!
//! - `POST   /browser/trusted-devices`              登録（owner session の assertion）
//! - `POST   /browser/trusted-devices/verify`       検証と回転（`readonly: true` なら何も書かない。probe 用）
//! - `GET    /browser/trusted-devices`              一覧（owner session の assertion を header で）
//! - `DELETE /browser/trusted-devices/{id}`         失効（owner session の assertion を header で）

use axum::Json;
use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::{HeaderMap, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::StoreError;
use task_core::trusted_device::{
    NewTrustedDevice, TrustedDevice, TrustedDeviceMethod, TrustedDeviceVerdict,
    is_valid_secret_hash,
};
use ulid::Ulid;

use crate::browser::HumanAttestation;
use crate::browser_live::{parse_body, verify_signature};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;

/// assertion の期限の上限（秒）。Live View の `RelayClaims` と同じ 30 秒。
const MAX_ASSERTION_TTL_SECS: i64 = 30;

/// GET と DELETE で assertion を運ぶ header（本文を持てないため）。
pub const ASSERTION_PAYLOAD_HEADER: &str = "x-celeris-assertion-payload";
pub const ASSERTION_SIGNATURE_HEADER: &str = "x-celeris-assertion-signature";

/// 端点ごとの用途。既存の `RelayClaims` / `AttestationClaims` と取り違えないための欄。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DevicePurpose {
    DeviceRegister,
    DeviceResume,
    DeviceList,
    DeviceRevoke,
}

/// web が署名する assertion の `payload`（ADR D2）。未知の欄は拒否する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceClaims {
    pub purpose: DevicePurpose,
    /// web の login session に結びつく id（空は拒否）。
    pub owner_session_id: String,
    /// web が owner session を持つか。登録・一覧・失効は `true` を要する（検証は要らない）。
    pub owner_session: bool,
    /// events の `actor`（web の `CELERIS_WEB_OWNER_ID`）。
    pub actor_id: String,
    /// 検証・失効の対象。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// 登録: 端末の名前。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 登録: 秘密の hash。検証: 提示された秘密の hash。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presented_hash: Option<String>,
    /// 検証: 回転後の新しい秘密の hash（readonly では無し）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_hash: Option<String>,
    /// 検証: probe 用の読み取りだけの検証か。
    #[serde(default)]
    pub readonly: bool,
    /// UNIX 秒。現在から 30 秒以内の未来でなければ拒否する。
    pub expires_at: i64,
}

/// `POST /browser/trusted-devices` の本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrustedDeviceRegisterBody {
    /// 人が付ける名前（1〜64 文字）。
    pub name: String,
    /// web が計算した秘密の SHA-256 hex（小文字 64 文字）。秘密そのものは送らない。
    pub secret_hash: String,
    pub assertion: HumanAttestation,
}

/// `POST /browser/trusted-devices/verify` の本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrustedDeviceVerifyBody {
    pub device_id: String,
    /// 提示された秘密の hash。
    pub presented_hash: String,
    /// 回転後の新しい秘密の hash。`readonly` でなければ必須。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_hash: Option<String>,
    /// `true` なら判定だけを返し、回転・最終使用・期限・event のいずれも書かない（web-follow の probe 用）。
    #[serde(default)]
    pub readonly: bool,
    pub assertion: HumanAttestation,
}

/// 登録・検証の応答（hash は含まない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrustedDeviceResult {
    pub device: TrustedDevice,
}

/// 一覧の応答（失効・期限切れも含む。新しい順。hash は含まない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrustedDeviceList {
    pub devices: Vec<TrustedDevice>,
    /// 有効な端末の上限（5）。
    pub limit: usize,
    /// 応答時点の時計（UNIX 秒）。期限切れの表示に使う。
    pub now: i64,
}

/// 失効の応答。`revoked` は今回失効させたか（既に失効済みなら `false`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrustedDeviceRevokeResult {
    pub revoked: bool,
    pub device: TrustedDevice,
}

fn not_owner() -> ApiProblem {
    ApiProblem::new(
        StatusCode::FORBIDDEN,
        "not_owner_session",
        "trusted device access denied",
    )
}

fn rejected() -> ApiProblem {
    // 拒否の理由（未知・不一致・失効・期限切れ・再提示）は events にだけ残し、応答では区別しない（D4）。
    ApiProblem::new(
        StatusCode::FORBIDDEN,
        "device_rejected",
        "trusted device rejected",
    )
}

fn invalid(detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, "device_invalid", detail)
}

fn device_problem(err: StoreError) -> ApiProblem {
    match err {
        StoreError::Invalid(msg) => invalid(msg),
        other => store_problem(other),
    }
}

/// 署名を確かめ、claims を読み、用途・期限・owner session を確かめる。
fn verify_device(
    state: &ApiState,
    assertion: &HumanAttestation,
    purpose: DevicePurpose,
    require_owner: bool,
) -> Result<DeviceClaims, ApiProblem> {
    verify_signature(state, assertion)?;
    let claims: DeviceClaims = serde_json::from_str(&assertion.payload).map_err(|_| not_owner())?;
    let now = state.now_secs();
    if claims.purpose != purpose
        || claims.owner_session_id.is_empty()
        || claims.actor_id.is_empty()
        || claims.expires_at < now
        || claims.expires_at > now + MAX_ASSERTION_TTL_SECS
        || (require_owner && !claims.owner_session)
    {
        return Err(not_owner());
    }
    Ok(claims)
}

fn header_assertion(headers: &HeaderMap) -> Result<HumanAttestation, ApiProblem> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(not_owner)
    };
    Ok(HumanAttestation {
        payload: get(ASSERTION_PAYLOAD_HEADER)?,
        signature: get(ASSERTION_SIGNATURE_HEADER)?,
    })
}

async fn register(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Json<TrustedDeviceRegisterBody>, JsonRejection>,
) -> Result<(StatusCode, Json<TrustedDeviceResult>), ApiProblem> {
    require_admin(&state, &headers)?;
    let body = parse_body(body)?;
    let claims = verify_device(&state, &body.assertion, DevicePurpose::DeviceRegister, true)?;
    if claims.name.as_deref() != Some(body.name.as_str())
        || claims.presented_hash.as_deref() != Some(body.secret_hash.as_str())
    {
        return Err(not_owner());
    }
    let now = state.now_secs();
    let verdict = state
        .blocking(move |store| {
            let id = Ulid::new().to_string();
            store
                .trusted_device_register(
                    &NewTrustedDevice {
                        id: &id,
                        name: &body.name,
                        method: TrustedDeviceMethod::Cookie,
                        secret_hash: &body.secret_hash,
                        // 人の決定 device-policy: 絶対上限なし。
                        absolute_expires_at: None,
                        actor: &claims.actor_id,
                    },
                    now,
                )
                .map_err(device_problem)
        })
        .await?;
    match verdict {
        TrustedDeviceVerdict::Accepted(device) => {
            Ok((StatusCode::CREATED, Json(TrustedDeviceResult { device })))
        }
        TrustedDeviceVerdict::Rejected(_) => Err(ApiProblem::new(
            StatusCode::CONFLICT,
            "device_limit",
            "trusted device limit reached; revoke one from the list first",
        )
        .with_extra("limit", task_core::trusted_device::TRUSTED_DEVICE_LIMIT)),
    }
}

async fn verify(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Json<TrustedDeviceVerifyBody>, JsonRejection>,
) -> Result<Json<TrustedDeviceResult>, ApiProblem> {
    require_admin(&state, &headers)?;
    let body = parse_body(body)?;
    let claims = verify_device(&state, &body.assertion, DevicePurpose::DeviceResume, false)?;
    if claims.device_id.as_deref() != Some(body.device_id.as_str())
        || claims.presented_hash.as_deref() != Some(body.presented_hash.as_str())
        || claims.next_hash != body.next_hash
        || claims.readonly != body.readonly
    {
        return Err(not_owner());
    }
    if !is_valid_secret_hash(&body.presented_hash) {
        return Err(invalid("presented_hash must be a lowercase SHA-256 hex"));
    }
    let next_hash = match (&body.next_hash, body.readonly) {
        (Some(h), false) => Some(h.clone()),
        (None, true) => None,
        (None, false) => return Err(invalid("next_hash is required unless readonly")),
        (Some(_), true) => return Err(invalid("readonly verify must not carry next_hash")),
    };
    let now = state.now_secs();
    let verdict = state
        .blocking(move |store| {
            match next_hash {
                Some(next) => store.trusted_device_verify_and_rotate(
                    &body.device_id,
                    &body.presented_hash,
                    &next,
                    &claims.actor_id,
                    now,
                ),
                None => {
                    store.trusted_device_verify_readonly(&body.device_id, &body.presented_hash, now)
                }
            }
            .map_err(device_problem)
        })
        .await?;
    match verdict {
        TrustedDeviceVerdict::Accepted(device) => Ok(Json(TrustedDeviceResult { device })),
        TrustedDeviceVerdict::Rejected(_) => Err(rejected()),
    }
}

async fn list(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<TrustedDeviceList>, ApiProblem> {
    require_admin(&state, &headers)?;
    let assertion = header_assertion(&headers)?;
    verify_device(&state, &assertion, DevicePurpose::DeviceList, true)?;
    let now = state.now_secs();
    let devices = state
        .blocking(|store| store.trusted_device_list().map_err(device_problem))
        .await?;
    Ok(Json(TrustedDeviceList {
        devices,
        limit: task_core::trusted_device::TRUSTED_DEVICE_LIMIT,
        now,
    }))
}

async fn revoke(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<TrustedDeviceRevokeResult>, ApiProblem> {
    require_admin(&state, &headers)?;
    let assertion = header_assertion(&headers)?;
    let claims = verify_device(&state, &assertion, DevicePurpose::DeviceRevoke, true)?;
    if claims.device_id.as_deref() != Some(id.as_str()) {
        return Err(not_owner());
    }
    let now = state.now_secs();
    let (revoked, device) = state
        .blocking(move |store| {
            let revoked = store
                .trusted_device_revoke(&id, &claims.actor_id, now)
                .map_err(device_problem)?;
            let device = store.trusted_device_get(&id).map_err(device_problem)?;
            Ok((revoked, device))
        })
        .await?;
    let device = device.ok_or_else(|| {
        ApiProblem::new(
            StatusCode::NOT_FOUND,
            "device_not_found",
            "trusted device not found",
        )
    })?;
    Ok(Json(TrustedDeviceRevokeResult { revoked, device }))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{delete, post};
    axum::Router::new()
        .route("/api/v1/browser/trusted-devices", post(register).get(list))
        .route("/api/v1/browser/trusted-devices/verify", post(verify))
        .route("/api/v1/browser/trusted-devices/{id}", delete(revoke))
}
