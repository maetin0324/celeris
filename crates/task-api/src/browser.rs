//! ADR-0080 D5: browser の人待ち（credential の登録依頼・承認）の API。
//!
//! - `POST /tasks/{id}/browser/requests` — trusted supervisor（bearer）が登録待ち・承認待ちを開く。
//! - `GET /tasks/{id}/browser/waits` / `GET /browser/waits` — wait の一覧（秘密・鍵・socket・token なし）。
//! - `POST /tasks/{id}/browser/waits/{wait_id}/credential` — 本人の手動登録。秘密は broker の control
//!   IPC へ渡すだけで、DB・event・log・応答に書かない（`SecretText` は `Serialize`/`Debug` で値を出さない）。
//! - `POST /tasks/{id}/browser/waits/{wait_id}/registered` — GUI が broker で登録した receipt を通知する。
//! - `POST /tasks/{id}/browser/waits/{wait_id}/decision` — `approve_once` / `deny`。
//! - `POST /tasks/{id}/browser/waits/{wait_id}/revoke` — 未消費の承認の失効。
//!
//! 人の操作（登録・決定・失効）は bearer に加えて、GUI 専用鍵（Ed25519）で署名した human attestation を
//! 必須にする。API token を持つだけの相手（LLM を含む）は `approved_by` を自称できない。
//! 全応答 `Cache-Control: no-store`。エラーは固定コードだけで、要求の値を反射しない。LLM は呼ばない。

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::BrowserTaskPolicy;
use task_core::browser_wait::{
    BrowserDecision, BrowserWait, BrowserWaitError, BrowserWaitStore, CredentialRecord,
    HumanDecision, NewBrowserWait,
};
use task_core::{Status, TaskId, TaskStore};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::handlers::{ApiResult, Params, json_response, no_query, read_body};
use crate::middleware::require_admin;
use crate::problem::{ApiProblem, store_problem};
use crate::query::parse_task_id;
use crate::state::ApiState;

/// attestation の有効期間の上限（ADR-0080 D5: 30 秒以内）。
pub const ATTESTATION_MAX_SECS: i64 = 30;
/// 手動登録の username / password の上限（bytes）。
pub const USERNAME_MAX_BYTES: usize = 256;
pub const PASSWORD_MAX_BYTES: usize = 1024;
/// browser の要求本文の上限（bytes）。
pub const BROWSER_BODY_MAX_BYTES: usize = 16 * 1024;

// ---- broker（celeris-credentiald）の control IPC の抽象 ----

/// 秘密の文字列。`Serialize` を持たず、`Debug` は値を出さず、drop で zeroize する。
pub struct SecretText(Zeroizing<String>);

impl SecretText {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// broker の IPC に書き込むときだけ使う。
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretText(<redacted>)")
    }
}

/// broker へ渡す手動登録（manual v1: username/password）。origin/policy は保存済みの wait が正。
#[derive(Debug)]
pub struct ManualRegistration {
    pub task_id: TaskId,
    pub wait_id: String,
    pub origin: String,
    pub policy_id: String,
    pub username: SecretText,
    pub password: SecretText,
}

/// broker の登録 receipt（秘密ではない）。
pub type BrokerReceipt = CredentialRecord;

/// broker の失敗（固定コード）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerFailure {
    /// broker に届かない・未初期化・vault_locked。
    Unavailable,
    /// broker が断った（固定コード）。
    Rejected(&'static str),
}

/// celeris-credentiald の control socket（本人の登録・receipt の照合だけ）。resolve は含まない。
pub trait CredentialBrokerControl: Send + Sync {
    fn register(&self, registration: ManualRegistration) -> Result<BrokerReceipt, BrokerFailure>;
    /// receipt が broker の登録と一致するか（存在・origin・policy・revision）。
    fn verify_receipt(&self, wait_id: &str, receipt: &BrokerReceipt)
    -> Result<bool, BrokerFailure>;
}

/// ADR-0110 D2: 管理者の site policy（daemon の設定から来る。モデル・worker・HTTP 要求は指定できない）。
/// 手動登録の `policy_id` と `origin` が両方一致したものだけが broker の `CredentialPolicy` に入る。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedSitePolicy {
    pub policy_id: String,
    pub exact_origin: String,
    pub login_url: String,
    pub password_selector: String,
    #[serde(default)]
    pub submit_selector: Option<String>,
}

impl TrustedSitePolicy {
    /// ADR-0110 D2 の形式検証（broker と同じ検査）。
    pub fn validate(&self) -> Result<(), &'static str> {
        task_core::browser_wait::validate_trusted_login(
            &self.login_url,
            &self.exact_origin,
            &self.password_selector,
            self.submit_selector.as_deref(),
        )
    }
}

/// Production control client. Only the daemon PID admitted by credentiald may use this socket.
#[derive(Debug, Clone, Default)]
pub struct UnixCredentialBrokerControl {
    pub socket: std::path::PathBuf,
    /// 管理者の site policy。該当が無い登録は selector 無しの policy になり、broker は注入を拒否する。
    pub site_policies: Vec<TrustedSitePolicy>,
}

impl UnixCredentialBrokerControl {
    fn site_policy(&self, policy_id: &str, origin: &str) -> Option<&TrustedSitePolicy> {
        self.site_policies
            .iter()
            .find(|p| p.policy_id == policy_id && p.exact_origin == origin)
    }
}

impl CredentialBrokerControl for UnixCredentialBrokerControl {
    fn register(&self, registration: ManualRegistration) -> Result<BrokerReceipt, BrokerFailure> {
        use celeris_credentiald::{CredentialPolicy, CredentialRef};
        #[derive(Serialize)]
        struct Secret<'a> {
            username: &'a str,
            password: &'a str,
        }
        #[derive(Serialize)]
        struct Request<'a> {
            op: &'static str,
            reference: &'a CredentialRef,
            policy: &'a CredentialPolicy,
            revision: u64,
            secret: Secret<'a>,
        }
        let site = self.site_policy(&registration.policy_id, &registration.origin);
        if site.is_some_and(|p| p.validate().is_err()) {
            return Err(BrokerFailure::Rejected("site_policy_invalid"));
        }
        let credential_id = format!("cred-{}", registration.wait_id);
        let reference = CredentialRef {
            credential_id: credential_id.clone(),
            provider: "manual".into(),
            policy_id: registration.policy_id.clone(),
        };
        let policy = CredentialPolicy {
            policy_id: registration.policy_id.clone(),
            revision: 1,
            exact_origin: registration.origin.clone(),
            task_id: registration.task_id.to_string(),
            max_ttl_seconds: 60,
            require_approval: true,
            allow_persistence: false,
            login_url: site.map(|p| p.login_url.clone()),
            password_selector: site.map(|p| p.password_selector.clone()),
            submit_selector: site.and_then(|p| p.submit_selector.clone()),
        };
        let request = Request {
            op: "register",
            reference: &reference,
            policy: &policy,
            revision: 1,
            secret: Secret {
                username: registration.username.expose(),
                password: registration.password.expose(),
            },
        };
        let bytes =
            Zeroizing::new(serde_json::to_vec(&request).map_err(|_| BrokerFailure::Unavailable)?);
        let reply = celeris_credentiald::ipc::call(&self.socket, &bytes)
            .map_err(|_| BrokerFailure::Unavailable)?;
        if !reply.success {
            return Err(BrokerFailure::Rejected("broker_rejected"));
        }
        Ok(BrokerReceipt {
            credential_id,
            provider: reference.provider,
            policy_id: reference.policy_id,
            credential_revision: 1,
            origin: registration.origin,
            receipt_id: format!("reg-{}", registration.wait_id),
        })
    }

    fn verify_receipt(
        &self,
        wait_id: &str,
        receipt: &BrokerReceipt,
    ) -> Result<bool, BrokerFailure> {
        if receipt.credential_id != format!("cred-{wait_id}")
            || receipt.receipt_id != format!("reg-{wait_id}")
            || receipt.provider != "manual"
        {
            return Ok(false);
        }
        let request = serde_json::json!({
            "op": "inspect",
            "reference": {"credential_id": receipt.credential_id, "provider": receipt.provider, "policy_id": receipt.policy_id},
            "revision": receipt.credential_revision, "origin": receipt.origin,
        });
        let bytes = serde_json::to_vec(&request).map_err(|_| BrokerFailure::Unavailable)?;
        let reply = celeris_credentiald::ipc::call(&self.socket, &bytes)
            .map_err(|_| BrokerFailure::Unavailable)?;
        Ok(reply.success)
    }
}

/// browser API の設定。既定（鍵も broker も無い）では人の操作は 503 `browser_unavailable`。
#[derive(Clone, Default)]
pub struct BrowserApiConfig {
    /// GUI 専用の human attestation 鍵の公開鍵（Ed25519、raw 32 bytes）。
    pub attestation_public_key: Option<Vec<u8>>,
    pub broker: Option<Arc<dyn CredentialBrokerControl>>,
}

impl std::fmt::Debug for BrowserApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserApiConfig")
            .field(
                "attestation_public_key",
                &self.attestation_public_key.as_ref().map(|_| "<ed25519>"),
            )
            .field("broker", &self.broker.as_ref().map(|_| "<broker>"))
            .finish()
    }
}

impl BrowserApiConfig {
    /// 公開鍵のファイル（raw 32 bytes か、64 文字の hex）を読む。
    pub fn read_public_key(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
        let raw = std::fs::read(path)?;
        if raw.len() == 32 {
            return Ok(raw);
        }
        let text = String::from_utf8_lossy(&raw);
        match decode_hex(text.trim()) {
            Some(key) if key.len() == 32 => Ok(key),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "attestation public key must be 32 raw bytes or 64 hex characters",
            )),
        }
    }
}

// ---- human attestation ----

/// GUI が署名する human attestation。`payload` は `AttestationClaims` の JSON（そのまま署名対象）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanAttestation {
    pub payload: String,
    /// Ed25519 署名（hex）。
    pub signature: String,
}

/// attestation が束縛する値（ADR-0080 D5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttestationClaims {
    pub actor_id: String,
    pub owner_session_hash: String,
    pub task_id: String,
    pub wait_id: String,
    pub version: u64,
    /// `approve_once` | `deny` | `revoke` | `register`。
    pub decision: String,
    pub policy_hash: String,
    pub nonce: String,
    /// Unix 秒。発行から 30 秒以内。
    pub expires_at: i64,
}

pub(crate) fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    s.as_bytes()
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

fn attestation_invalid() -> ApiProblem {
    ApiProblem::new(
        StatusCode::FORBIDDEN,
        "attestation_invalid",
        "a valid human attestation from the owner session is required",
    )
}

fn browser_unavailable(what: &str) -> ApiProblem {
    ApiProblem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "browser_unavailable",
        format!("{what} is not configured"),
    )
}

fn body_invalid() -> ApiProblem {
    // serde のメッセージは値を含み得るので返さない（固定コードだけ）。
    ApiProblem::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "browser_body_invalid",
        "the request body does not match the schema",
    )
}

/// 署名・束縛・期限を検証して claims を返す（nonce の一回性は store が確かめる）。
fn verify_attestation(
    state: &ApiState,
    attestation: &HumanAttestation,
    task_id: TaskId,
    wait: &BrowserWait,
    version: u64,
    decision: &str,
    now: OffsetDateTime,
) -> Result<AttestationClaims, ApiProblem> {
    let Some(key) = state.browser.attestation_public_key.as_deref() else {
        return Err(browser_unavailable("human attestation"));
    };
    let signature = decode_hex(&attestation.signature).ok_or_else(attestation_invalid)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(attestation.payload.as_bytes(), &signature)
        .map_err(|_| attestation_invalid())?;
    let claims: AttestationClaims =
        serde_json::from_str(&attestation.payload).map_err(|_| attestation_invalid())?;
    let now_secs = now.unix_timestamp();
    let bound = claims.task_id == task_id.to_string()
        && claims.wait_id == wait.wait_id
        && claims.version == version
        && claims.decision == decision
        && claims.policy_hash == wait.policy_hash
        && wait
            .owner_id
            .as_deref()
            .is_none_or(|owner| owner == claims.actor_id);
    let fresh =
        claims.expires_at >= now_secs && claims.expires_at <= now_secs + ATTESTATION_MAX_SECS;
    if !bound || !fresh {
        return Err(attestation_invalid());
    }
    Ok(claims)
}

// ---- 要求・応答 ----

/// `GET /tasks/{id}/browser/waits` の応答。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BrowserWaitList {
    pub items: Vec<BrowserWait>,
}

/// `GET /browser/waits` の応答（人の対応待ち。inbox の `browser_waits` と同じ形）。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BrowserPendingList {
    pub items: Vec<task_ops::browser::BrowserWaitItem>,
}

/// `POST /tasks/{id}/browser/requests` の応答。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BrowserRequestResult {
    pub wait: BrowserWait,
    /// `false` は同じ `resume_key` の再送（既存の wait）。
    pub created: bool,
}

/// 解決系（登録・決定・失効）の応答。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BrowserWaitResult {
    pub wait: BrowserWait,
    pub task_status: Status,
    /// `true` は冪等な再送（何も書いていない）。
    pub replayed: bool,
}

/// `POST .../credential` の本文。秘密は `SecretText` にしか入らない。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialBody {
    expected_version: u64,
    username: SecretField,
    password: SecretField,
    attestation: HumanAttestation,
}

struct SecretField(SecretText);

impl<'de> Deserialize<'de> for SecretField {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        Ok(Self(SecretText::new(value)))
    }
}

/// `POST .../credential` の本文の形（スキーマ用。秘密の値の例は載せない）。
#[derive(Debug, Clone, JsonSchema)]
#[allow(dead_code)]
pub struct BrowserCredentialBody {
    pub expected_version: u64,
    /// 秘密。broker にだけ渡し、保存・反射しない。
    pub username: String,
    /// 秘密。broker にだけ渡し、保存・反射しない。
    pub password: String,
    pub attestation: HumanAttestation,
}

/// `POST .../registered` の本文。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRegisteredBody {
    pub expected_version: u64,
    pub receipt: CredentialRecord,
    pub attestation: HumanAttestation,
}

/// `POST .../decision` の本文。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserDecisionBody {
    /// `approve_once` | `deny`。
    pub decision: BrowserDecision,
    pub expected_version: u64,
    pub idempotency_key: String,
    pub attestation: HumanAttestation,
}

/// `POST .../revoke` の本文。
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserRevokeBody {
    pub expected_version: u64,
    pub idempotency_key: String,
    pub attestation: HumanAttestation,
}

// ---- 共通 ----

fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn wait_problem(err: BrowserWaitError) -> ApiProblem {
    let code = err.code();
    let status = match &err {
        BrowserWaitError::NotFound => StatusCode::NOT_FOUND,
        BrowserWaitError::Invalid { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        BrowserWaitError::VersionConflict
        | BrowserWaitError::InvalidState { .. }
        | BrowserWaitError::TaskNotRunning => StatusCode::CONFLICT,
        BrowserWaitError::Gone => StatusCode::GONE,
        BrowserWaitError::Replay => StatusCode::FORBIDDEN,
        BrowserWaitError::Store(_) => {
            let BrowserWaitError::Store(e) = err else {
                return ApiProblem::internal("browser wait store error");
            };
            return store_problem(e);
        }
    };
    let problem = ApiProblem::new(status, code, err.to_string());
    match err {
        BrowserWaitError::Invalid { field } => problem.with_extra("field", field),
        _ => problem,
    }
}

async fn read_browser_json<T: serde::de::DeserializeOwned>(body: Body) -> Result<T, ApiProblem> {
    let bytes = Zeroizing::new(read_body(body).await?);
    if bytes.len() > BROWSER_BODY_MAX_BYTES {
        return Err(ApiProblem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            "browser request body is too large",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| body_invalid())
}

fn load_wait(
    store: &task_core::SqliteStore,
    task_id: TaskId,
    wait_id: &str,
) -> Result<BrowserWait, ApiProblem> {
    match store.browser_wait_get(wait_id).map_err(store_problem)? {
        Some(w) if w.task_id == task_id => Ok(w),
        _ => Err(wait_problem(BrowserWaitError::NotFound)),
    }
}

// ---- handlers ----

pub(crate) async fn open_request(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let request: NewBrowserWait = read_browser_json(body).await?;
    let result = state
        .blocking(move |store| {
            if store.get(task_id).map_err(store_problem)?.is_none() {
                return Err(ApiProblem::task_not_found(task_id));
            }
            store
                .browser_wait_open(task_id, &request, OffsetDateTime::now_utc())
                .map_err(wait_problem)
        })
        .await?;
    tracing::info!(
        op = "browser_wait_open",
        task_id = %task_id,
        wait_id = %result.wait.wait_id,
        reason = result.wait.reason.as_str(),
        created = result.created,
        "browser wait opened"
    );
    let status = if result.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(no_store(json_response(
        status,
        &BrowserRequestResult {
            wait: result.wait,
            created: result.created,
        },
    )))
}

pub(crate) async fn task_waits(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let task_id = parse_task_id(&id)?;
    let items = state
        .blocking(move |store| {
            if store.get(task_id).map_err(store_problem)?.is_none() {
                return Err(ApiProblem::task_not_found(task_id));
            }
            store.browser_waits_for_task(task_id).map_err(store_problem)
        })
        .await?;
    Ok(no_store(json_response(
        StatusCode::OK,
        &BrowserWaitList { items },
    )))
}

pub(crate) async fn get_task_policy(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let task_id = parse_task_id(&id)?;
    let policy = state
        .blocking(move |store| {
            if store.get(task_id).map_err(store_problem)?.is_none() {
                return Err(ApiProblem::task_not_found(task_id));
            }
            store
                .browser_task_policy_get(task_id)
                .map_err(store_problem)
        })
        .await?;
    Ok(no_store(json_response(
        StatusCode::OK,
        &serde_json::json!({"policy": policy}),
    )))
}

pub(crate) async fn put_task_policy(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let bytes = read_body(body).await?;
    if bytes.len() > BROWSER_BODY_MAX_BYTES {
        return Err(body_invalid());
    }
    let raw = std::str::from_utf8(&bytes).map_err(|_| body_invalid())?;
    let policy = BrowserTaskPolicy::from_json(raw).map_err(|_| body_invalid())?;
    state
        .blocking(move |store| {
            store
                .browser_task_policy_set(task_id, &policy)
                .map_err(|_| {
                    ApiProblem::new(
                        StatusCode::CONFLICT,
                        "browser_policy_state",
                        "browser policy cannot be changed in this task state",
                    )
                })
        })
        .await?;
    Ok(no_store(json_response(
        StatusCode::OK,
        &serde_json::json!({"updated": true}),
    )))
}

pub(crate) async fn pending_waits(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    let items = state
        .blocking(move |store| {
            let by_id = store
                .list(None)
                .map_err(store_problem)?
                .into_iter()
                .map(|t| (t.id, t))
                .collect();
            task_ops::browser::pending_items(store, &by_id)
                .map_err(|e| ApiProblem::internal(e.to_string()))
        })
        .await?;
    Ok(no_store(json_response(
        StatusCode::OK,
        &BrowserPendingList { items },
    )))
}

pub(crate) async fn register_credential(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((id, wait_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let body: CredentialBody = read_browser_json(body).await?;
    let username_len = body.username.0.expose().len();
    let password_len = body.password.0.expose().len();
    if username_len == 0
        || username_len > USERNAME_MAX_BYTES
        || password_len == 0
        || password_len > PASSWORD_MAX_BYTES
    {
        return Err(body_invalid());
    }
    let Some(broker) = state.browser.broker.clone() else {
        return Err(browser_unavailable("credential broker"));
    };
    let st = state.clone();
    let result = state
        .blocking(move |store| {
            let now = OffsetDateTime::now_utc();
            let wait = load_wait(store, task_id, &wait_id)?;
            let claims = verify_attestation(
                &st,
                &body.attestation,
                task_id,
                &wait,
                body.expected_version,
                "register",
                now,
            )?;
            if wait.version != body.expected_version {
                return Err(wait_problem(BrowserWaitError::VersionConflict));
            }
            if wait.reason != task_core::browser_wait::BrowserWaitReason::WaitingForAuth
                || wait.state != task_core::browser_wait::BrowserWaitState::Pending
            {
                return Err(wait_problem(BrowserWaitError::InvalidState {
                    op: "register",
                }));
            }
            let Some(policy_id) = wait.credential_policy_id.clone() else {
                return Err(wait_problem(BrowserWaitError::InvalidState {
                    op: "register",
                }));
            };
            // 秘密はここで broker にだけ渡る（この関数の外へは出ない。drop で zeroize）。
            let CredentialBody {
                username, password, ..
            } = body;
            let receipt = broker
                .register(ManualRegistration {
                    task_id,
                    wait_id: wait.wait_id.clone(),
                    origin: wait.origin.clone(),
                    policy_id,
                    username: username.0,
                    password: password.0,
                })
                .map_err(broker_problem)?;
            store
                .browser_wait_register(
                    task_id,
                    &wait.wait_id,
                    wait.version,
                    &receipt,
                    &claims.actor_id,
                    now,
                )
                .map_err(wait_problem)
        })
        .await?;
    tracing::info!(
        op = "browser_credential_register",
        task_id = %task_id,
        wait_id = %result.wait.wait_id,
        "browser credential registered"
    );
    Ok(no_store(json_response(
        StatusCode::OK,
        &BrowserWaitResult {
            wait: result.wait,
            task_status: result.task_status,
            replayed: result.replayed,
        },
    )))
}

fn broker_problem(failure: BrokerFailure) -> ApiProblem {
    match failure {
        BrokerFailure::Unavailable => browser_unavailable("credential broker"),
        BrokerFailure::Rejected(code) => ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "credential_rejected",
            "the credential broker rejected the registration",
        )
        .with_extra("broker_code", code),
    }
}

pub(crate) async fn registered(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((id, wait_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&id)?;
    let body: BrowserRegisteredBody = read_browser_json(body).await?;
    let Some(broker) = state.browser.broker.clone() else {
        return Err(browser_unavailable("credential broker"));
    };
    let st = state.clone();
    let result = state
        .blocking(move |store| {
            let now = OffsetDateTime::now_utc();
            let wait = load_wait(store, task_id, &wait_id)?;
            let claims = verify_attestation(
                &st,
                &body.attestation,
                task_id,
                &wait,
                body.expected_version,
                "register",
                now,
            )?;
            if !broker
                .verify_receipt(&wait.wait_id, &body.receipt)
                .map_err(broker_problem)?
            {
                return Err(ApiProblem::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "credential_receipt_invalid",
                    "the credential broker does not know this receipt",
                ));
            }
            store
                .browser_wait_register(
                    task_id,
                    &wait.wait_id,
                    body.expected_version,
                    &body.receipt,
                    &claims.actor_id,
                    now,
                )
                .map_err(wait_problem)
        })
        .await?;
    Ok(no_store(json_response(
        StatusCode::OK,
        &BrowserWaitResult {
            wait: result.wait,
            task_status: result.task_status,
            replayed: result.replayed,
        },
    )))
}

/// `decision` と `revoke` の共通部分。
struct DecideInput {
    id: String,
    wait_id: String,
    decision: BrowserDecision,
    expected_version: u64,
    idempotency_key: String,
    attestation: HumanAttestation,
}

async fn decide_with(state: ApiState, headers: HeaderMap, input: DecideInput) -> ApiResult {
    require_admin(&state, &headers)?;
    let task_id = parse_task_id(&input.id)?;
    let decision = input.decision;
    let st = state.clone();
    let result = state
        .blocking(move |store| {
            let now = OffsetDateTime::now_utc();
            let wait = load_wait(store, task_id, &input.wait_id)?;
            // attestation は要求の `expected_version` に束縛する（同じ idempotency_key の再送は、
            // store が同じ決定かを照合して何も書かずに現在の状態を返す）。
            let claims = verify_attestation(
                &st,
                &input.attestation,
                task_id,
                &wait,
                input.expected_version,
                decision.as_str(),
                now,
            )?;
            store
                .browser_wait_decide(
                    task_id,
                    &wait.wait_id,
                    &HumanDecision {
                        decision,
                        expected_version: input.expected_version,
                        actor_id: claims.actor_id,
                        owner_session_hash: claims.owner_session_hash,
                        policy_hash: claims.policy_hash,
                        nonce: claims.nonce,
                        idempotency_key: input.idempotency_key,
                    },
                    now,
                )
                .map_err(wait_problem)
        })
        .await?;
    tracing::info!(
        op = "browser_wait_decide",
        task_id = %task_id,
        wait_id = %result.wait.wait_id,
        decision = decision.as_str(),
        replayed = result.replayed,
        "browser wait decided"
    );
    Ok(no_store(json_response(
        StatusCode::OK,
        &BrowserWaitResult {
            wait: result.wait,
            task_status: result.task_status,
            replayed: result.replayed,
        },
    )))
}

pub(crate) async fn decision(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((id, wait_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: BrowserDecisionBody = read_browser_json(body).await?;
    if body.decision == BrowserDecision::Revoke {
        return Err(body_invalid());
    }
    decide_with(
        state,
        headers,
        DecideInput {
            id,
            wait_id,
            decision: body.decision,
            expected_version: body.expected_version,
            idempotency_key: body.idempotency_key,
            attestation: body.attestation,
        },
    )
    .await
}

pub(crate) async fn revoke(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((id, wait_id)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let body: BrowserRevokeBody = read_browser_json(body).await?;
    decide_with(
        state,
        headers,
        DecideInput {
            id,
            wait_id,
            decision: BrowserDecision::Revoke,
            expected_version: body.expected_version,
            idempotency_key: body.idempotency_key,
            attestation: body.attestation,
        },
    )
    .await
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/api/v1/tasks/{id}/browser/policy",
            get(get_task_policy).put(put_task_policy),
        )
        .route("/api/v1/tasks/{id}/browser/requests", post(open_request))
        .route("/api/v1/tasks/{id}/browser/waits", get(task_waits))
        .route("/api/v1/browser/waits", get(pending_waits))
        .route(
            "/api/v1/tasks/{id}/browser/waits/{wait_id}/credential",
            post(register_credential),
        )
        .route(
            "/api/v1/tasks/{id}/browser/waits/{wait_id}/registered",
            post(registered),
        )
        .route(
            "/api/v1/tasks/{id}/browser/waits/{wait_id}/decision",
            post(decision),
        )
        .route(
            "/api/v1/tasks/{id}/browser/waits/{wait_id}/revoke",
            post(revoke),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_text_debug_redacts_and_hex_decodes() {
        let s = SecretText::new("hunter2".into());
        assert_eq!(format!("{s:?}"), "SecretText(<redacted>)");
        assert_eq!(decode_hex("0aFf"), Some(vec![0x0a, 0xff]));
        assert_eq!(decode_hex("0g"), None);
        assert_eq!(decode_hex("abc"), None);
    }
}
