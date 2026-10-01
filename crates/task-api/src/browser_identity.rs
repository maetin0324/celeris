//! ADR-0101 / H5: Browser Identity の登録・一覧・失効・削除・利用（P3-A）。
//!
//! 封緘は credentiald の [`IdentitySealer`]、metadata と封緘 blob は store が持つ。
//! 応答は metadata だけで、封緘 blob・鍵・cookie は返さない。
//! 利用（session への復元）は Isolated の経路が無いので trusted local では常に
//! `isolation_required`（ADR-0101 D3、P4-A の後に解除）。agent に state は渡さない。

use axum::http::StatusCode;
use celeris_credentiald::identity_seal::{
    IdentitySealer, IdentityStatePlain, SealError, SealedIdentityState,
};
use serde::{Deserialize, Serialize};
use task_core::SqliteStore;
use task_core::browser_identity::{
    self as bi, IDENTITY_TTL_DEFAULT_SECS, IdentityDenied, IdentityRegistration, IdentityState,
    IdentityUse, Isolation,
};
use task_core::browser_isolation::IsolationAttestation;
use task_core::browser_store::{BrowserStoreError, StoredIdentity};

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;

use crate::handlers::{ApiResult, Params, json_response, no_query, read_body};
use crate::middleware::require_admin;
use crate::problem::ApiProblem;
use crate::state::ApiState;

/// 登録の入力。`state` は取り込む cookie / storage（封緘してから保存し、平文は残さない）。
#[derive(Deserialize)]
pub struct IdentityRegisterInput {
    pub identity_id: String,
    pub project_id: String,
    pub origin: String,
    /// 需要を確認した人。無ければ `demand_not_confirmed`。
    pub demand_confirmed_by: Option<String>,
    /// 既定 7 日・上限 30 日。超過は切り詰めずに `ttl_too_long`。
    pub ttl_secs: Option<u64>,
    pub state: IdentityStatePlain,
}

/// 一覧・応答に出す metadata。封緘 blob と鍵 label は含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityView {
    pub identity_id: String,
    pub project_id: String,
    pub origin: String,
    pub demand_confirmed_by: String,
    pub generation: u64,
    pub created_at: u64,
    pub expires_at: u64,
    pub state: IdentityState,
}

impl From<&StoredIdentity> for IdentityView {
    fn from(s: &StoredIdentity) -> Self {
        let i = &s.identity;
        Self {
            identity_id: i.identity_id.clone(),
            project_id: i.project_id.clone(),
            origin: i.origin.clone(),
            demand_confirmed_by: i.demand_confirmed_by.clone(),
            generation: i.generation,
            created_at: i.created_at,
            expires_at: i.expires_at,
            state: i.state,
        }
    }
}

/// 固定コードの拒否。本文に値（origin・cookie 等）は含めない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityApiError {
    Denied(IdentityDenied),
    NotFound,
    Conflict,
    SealFailed,
    Store,
}

impl IdentityApiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Denied(d) => d.code(),
            Self::NotFound => "identity_not_found",
            Self::Conflict => "identity_conflict",
            Self::SealFailed => "identity_seal_failed",
            Self::Store => "identity_store_failed",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Denied(IdentityDenied::IsolationRequired) => StatusCode::FORBIDDEN,
            Self::Denied(
                IdentityDenied::Expired | IdentityDenied::Revoked | IdentityDenied::Deleted,
            ) => StatusCode::GONE,
            Self::Denied(IdentityDenied::StaleGeneration) => StatusCode::CONFLICT,
            Self::Denied(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::SealFailed | Self::Store => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub(crate) fn problem(&self) -> ApiProblem {
        ApiProblem::new(self.status(), self.code(), self.code())
    }
}

impl From<IdentityDenied> for IdentityApiError {
    fn from(d: IdentityDenied) -> Self {
        Self::Denied(d)
    }
}

impl From<SealError> for IdentityApiError {
    fn from(e: SealError) -> Self {
        match e {
            SealError::Denied(d) => Self::Denied(d),
            _ => Self::SealFailed,
        }
    }
}

impl From<BrowserStoreError> for IdentityApiError {
    fn from(e: BrowserStoreError) -> Self {
        match e {
            BrowserStoreError::Identity(d) => Self::Denied(d),
            _ => Self::Store,
        }
    }
}

impl From<task_core::StoreError> for IdentityApiError {
    fn from(e: task_core::StoreError) -> Self {
        BrowserStoreError::Store(e).into()
    }
}

/// identity の API。store と credentiald の封緘をまとめる（LLM 呼び出しは無い）。
pub struct IdentityService<'a> {
    pub store: &'a SqliteStore,
    pub sealer: &'a IdentitySealer,
}

impl IdentityService<'_> {
    /// 需要確認・期限・https origin を検査し、state を封緘してから保存する。
    pub fn register(
        &self,
        input: IdentityRegisterInput,
        now: u64,
    ) -> Result<IdentityView, IdentityApiError> {
        let req = IdentityRegistration {
            identity_id: input.identity_id,
            project_id: input.project_id,
            origin: input.origin,
            demand_confirmed_by: input.demand_confirmed_by,
            ttl_secs: input.ttl_secs.unwrap_or(IDENTITY_TTL_DEFAULT_SECS),
        };
        let identity = bi::register(&req, now)?;
        if self
            .store
            .browser_identity_get(&identity.identity_id)?
            .is_some()
        {
            return Err(IdentityApiError::Conflict);
        }
        // 他 project/origin の項目が混ざった state は封緘の前に全体を拒否する。
        let sealed = self.sealer.seal(&identity, &input.state, now)?;
        let blob = serde_json::to_vec(&sealed).map_err(|_| IdentityApiError::SealFailed)?;
        let stored = self
            .store
            .browser_identity_register(&req, &sealed.envelope, &blob, now)?;
        Ok(IdentityView::from(&stored))
    }

    /// project の identity の metadata。読み出し時に期限切れを失効させる。
    pub fn list(&self, project_id: &str, now: u64) -> Result<Vec<IdentityView>, IdentityApiError> {
        self.sweep_expired(project_id, now)?;
        Ok(self
            .store
            .browser_identity_list(project_id)?
            .iter()
            .map(IdentityView::from)
            .collect())
    }

    /// 期限切れの active を失効させる（世代を上げ、封緘を捨てる）。変えた件数。
    pub fn sweep_expired(&self, project_id: &str, now: u64) -> Result<usize, IdentityApiError> {
        let mut changed = 0;
        for s in self.store.browser_identity_list(project_id)? {
            let mut probe = s.identity.clone();
            if bi::expire(&mut probe, now) {
                self.store
                    .browser_identity_revoke(&s.identity.identity_id)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// 失効。世代を上げるので以前の封緘は開けない。
    pub fn revoke(&self, identity_id: &str) -> Result<IdentityView, IdentityApiError> {
        let stored = self
            .store
            .browser_identity_revoke(identity_id)?
            .ok_or(IdentityApiError::NotFound)?;
        Ok(IdentityView::from(&stored))
    }

    /// 削除。tombstone を残し、封緘 blob と credentiald の project+origin 鍵を消す。
    pub fn delete(&self, identity_id: &str) -> Result<IdentityView, IdentityApiError> {
        // tombstone は origin を消すので、鍵の label は削除の前の metadata から取る。
        let live = self
            .store
            .browser_identity_get(identity_id)?
            .ok_or(IdentityApiError::NotFound)?;
        let stored = self
            .store
            .browser_identity_delete(identity_id)?
            .ok_or(IdentityApiError::NotFound)?;
        self.sealer
            .erase_key(&live.identity.project_id, &live.identity.origin)
            .map_err(|_| IdentityApiError::SealFailed)?;
        Ok(IdentityView::from(&stored))
    }

    /// 利用（session への復元）の入口。trusted local しか無いので常に `isolation_required`。
    /// 開封は隔離が証明された後にだけ行う（ここでは開かない）。
    pub fn restore(
        &self,
        identity_id: &str,
        project_id: &str,
        origin: &str,
        now: u64,
    ) -> Result<(), IdentityApiError> {
        self.sweep_expired(project_id, now)?;
        let stored = self
            .store
            .browser_identity_get(identity_id)?
            .ok_or(IdentityApiError::NotFound)?;
        let request = IdentityUse {
            project_id: project_id.to_owned(),
            origin: origin.to_owned(),
            isolation: Isolation::TrustedLocal,
        };
        bi::authorize_use(&stored.identity, &request, now)?;
        // authorize_use は TrustedLocal を必ず拒否する。ここに来るのは隔離の経路ができた後だけ。
        Err(IdentityApiError::Denied(IdentityDenied::IsolationRequired))
    }

    /// 隔離下の復元（ADR-0102 D4）。[`IsolationAttestation`] は P4-A の検査
    /// （`verify_isolation`）からしか作れないので、HTTP からは呼べない。開いた state は
    /// runtime の controller に渡すだけで、agent・応答には出さない。
    pub fn restore_isolated(
        &self,
        identity_id: &str,
        project_id: &str,
        origin: &str,
        attestation: &IsolationAttestation,
        now: u64,
    ) -> Result<IdentityStatePlain, IdentityApiError> {
        self.sweep_expired(project_id, now)?;
        let stored = self
            .store
            .browser_identity_get(identity_id)?
            .ok_or(IdentityApiError::NotFound)?;
        let request = IdentityUse {
            project_id: project_id.to_owned(),
            origin: origin.to_owned(),
            isolation: attestation.isolation(),
        };
        bi::authorize_use(&stored.identity, &request, now)?;
        let sealed = sealed_of(&stored).ok_or(IdentityApiError::SealFailed)?;
        self.sealer
            .open_state(&stored.identity, &sealed, now)
            .map_err(|_| IdentityApiError::SealFailed)
    }

    /// 稼働中の隔離 session に結合した復元（ADR-0105 D5）。attestation はこの場で
    /// session から採り直す。session の停止・隔離違反（この host の同一 UID を含む）・
    /// session id の不一致は `IsolationRequired` で拒否する。
    pub fn restore_for_session(
        &self,
        identity_id: &str,
        project_id: &str,
        origin: &str,
        session_id: &str,
        live: &dyn task_core::browser_isolation::LiveIsolation,
        now: u64,
    ) -> Result<IdentityStatePlain, IdentityApiError> {
        let attestation = live
            .current_attestation()
            .map_err(|_| IdentityApiError::Denied(IdentityDenied::IsolationRequired))?;
        if attestation.session_id() != session_id {
            return Err(IdentityApiError::Denied(IdentityDenied::IsolationRequired));
        }
        self.restore_isolated(identity_id, project_id, origin, &attestation, now)
    }

    /// HTTP の復元（ADR-0108 D5）。外側の条件 → registry の登録と種別 → attestation の採り直しと
    /// session id → controller の投入口、の順に判定し、全部通ったときだけ開封して controller に渡す。
    /// 開いた state は返さない。
    pub fn restore_in_session(
        &self,
        identity_id: &str,
        project_id: &str,
        origin: &str,
        session_id: &str,
        registry: Option<&dyn task_core::browser_isolation::LiveSessionRegistry>,
        now: u64,
    ) -> Result<(), IdentityApiError> {
        use task_core::browser_isolation::RuntimeKind;
        let denied = || IdentityApiError::Denied(IdentityDenied::IsolationRequired);
        // 1. 期限・存在・project・origin（authorize_use の非隔離部分。TrustedLocal は最後に必ず落ちる）。
        self.sweep_expired(project_id, now)?;
        let stored = self
            .store
            .browser_identity_get(identity_id)?
            .ok_or(IdentityApiError::NotFound)?;
        let outer = IdentityUse {
            project_id: project_id.to_owned(),
            origin: origin.to_owned(),
            isolation: Isolation::TrustedLocal,
        };
        match bi::authorize_use(&stored.identity, &outer, now) {
            Err(IdentityDenied::IsolationRequired) => {}
            Err(d) => return Err(d.into()),
            Ok(()) => return Err(denied()),
        }
        // 2. 稼働中の隔離 session か。
        let entry = registry
            .and_then(|r| r.get(session_id))
            .ok_or_else(denied)?;
        if entry.kind() != RuntimeKind::Isolated {
            return Err(denied());
        }
        // 3. attestation を採り直し、session id を照合する（この host では SameUid で落ちる）。
        let attestation = entry.current_attestation().map_err(|_| denied())?;
        if attestation.session_id() != session_id {
            return Err(denied());
        }
        // 4. controller に投入口が無い、または Live View の鍵が無ければ開封しない。
        if !entry.accepts_state() {
            return Err(denied());
        }
        let (task_id, run_id) = entry.live_key().ok_or_else(denied)?;
        let plain = self.restore_isolated(identity_id, project_id, origin, &attestation, now)?;
        let bytes = zeroize::Zeroizing::new(
            serde_json::to_vec(&plain).map_err(|_| IdentityApiError::SealFailed)?,
        );
        // 5. 投入の前に観測停止を記録する（ADR-0080 H3 / ADR-0101 D4）。最後の live event が
        // observation_stopped の間は Live View の接続も worker の event 書き込みも拒否され、
        // 解除は session の終了だけ。記録できなければ投入しない。投入に失敗しても停止のまま。
        let stop = task_core::browser_live::ScrubbedLiveEvent::from_event(
            &task_core::browser_live::LiveEvent::Status {
                state: "observation_stopped".into(),
            },
        )
        .ok_or_else(denied)?;
        let key = task_core::browser_store::BrowserSessionKey {
            task_id: &task_id,
            run_id: &run_id,
            session_id,
        };
        self.store
            .browser_live_append(key, &stop)
            .map_err(|_| IdentityApiError::Store)?;
        entry.deliver_state(&bytes).map_err(|_| denied())
    }
}

// ---- HTTP（/api/v1/browser/identities）----

const IDENTITY_BODY_MAX_BYTES: usize = 256 * 1024;

fn unavailable() -> ApiProblem {
    ApiProblem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "identity_unavailable",
        "identity sealing is not configured",
    )
}

fn body_invalid() -> ApiProblem {
    // 本文（state の cookie 等）は応答に写さない。
    ApiProblem::new(
        StatusCode::BAD_REQUEST,
        "identity_body_invalid",
        "identity request body is invalid",
    )
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `name=value` の query から 1 つ取り出す（他の鍵は拒否）。
fn query_param(raw: &Option<String>, name: &str) -> Result<String, ApiProblem> {
    let bad = || {
        ApiProblem::new(
            StatusCode::BAD_REQUEST,
            "identity_query_invalid",
            "identity query is invalid",
        )
    };
    let raw = raw.as_deref().ok_or_else(bad)?;
    let mut found = None;
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').ok_or_else(bad)?;
        if k != name || found.is_some() || v.is_empty() {
            return Err(bad());
        }
        found = Some(v.to_owned());
    }
    found.ok_or_else(bad)
}

async fn run<T, F>(state: &ApiState, f: F) -> Result<T, ApiProblem>
where
    T: Send + 'static,
    F: FnOnce(IdentityService<'_>) -> Result<T, IdentityApiError> + Send + 'static,
{
    let sealer = state.identity_sealer.clone().ok_or_else(unavailable)?;
    state
        .blocking(move |store| {
            f(IdentityService {
                store,
                sealer: &sealer,
            })
            .map_err(|e| e.problem())
        })
        .await
}

pub(crate) async fn register_identity(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let bytes = read_body(body).await?;
    if bytes.len() > IDENTITY_BODY_MAX_BYTES {
        return Err(body_invalid());
    }
    let input: IdentityRegisterInput =
        serde_json::from_slice(&bytes).map_err(|_| body_invalid())?;
    let now = now_secs();
    let view = run(&state, move |svc| svc.register(input, now)).await?;
    Ok(json_response(
        StatusCode::CREATED,
        &serde_json::json!({"identity": view}),
    ))
}

pub(crate) async fn list_identities(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    let project_id = query_param(&raw, "project_id")?;
    let now = now_secs();
    let views = run(&state, move |svc| svc.list(&project_id, now)).await?;
    Ok(json_response(
        StatusCode::OK,
        &serde_json::json!({"identities": views}),
    ))
}

pub(crate) async fn revoke_identity(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let view = run(&state, move |svc| svc.revoke(&id)).await?;
    Ok(json_response(
        StatusCode::OK,
        &serde_json::json!({"identity": view}),
    ))
}

pub(crate) async fn delete_identity(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let view = run(&state, move |svc| svc.delete(&id)).await?;
    Ok(json_response(
        StatusCode::OK,
        &serde_json::json!({"identity": view}),
    ))
}

#[derive(Deserialize)]
struct RestoreBody {
    project_id: String,
    origin: String,
    /// ADR-0108 D5: 復元先の稼働中 session。無ければ従来どおり `isolation_required`。
    #[serde(default)]
    session_id: Option<String>,
}

/// 利用の入口（ADR-0101 D3 / ADR-0108 D5）。`session_id` が稼働中の隔離 session を指し、その場の
/// attestation が通ったときだけ開封して controller に渡し、204（本文なし）を返す。
pub(crate) async fn restore_identity(
    State(state): State<ApiState>,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    require_admin(&state, &headers)?;
    let bytes = read_body(body).await?;
    let req: RestoreBody = serde_json::from_slice(&bytes).map_err(|_| body_invalid())?;
    let now = now_secs();
    let registry = state.live_sessions.clone();
    run(&state, move |svc| match &req.session_id {
        None => svc.restore(&id, &req.project_id, &req.origin, now),
        Some(session) => svc.restore_in_session(
            &id,
            &req.project_id,
            &req.origin,
            session,
            registry.as_deref(),
            now,
        ),
    })
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/api/v1/browser/identities",
            get(list_identities).post(register_identity),
        )
        .route(
            "/api/v1/browser/identities/{id}",
            axum::routing::delete(delete_identity),
        )
        .route(
            "/api/v1/browser/identities/{id}/revoke",
            post(revoke_identity),
        )
        .route(
            "/api/v1/browser/identities/{id}/restore",
            post(restore_identity),
        )
}

/// 保存された封緘 blob を読み戻す（開封は credentiald が行う）。
pub fn sealed_of(stored: &StoredIdentity) -> Option<SealedIdentityState> {
    stored
        .sealed_blob
        .as_deref()
        .and_then(|b| serde_json::from_slice(b).ok())
}

#[cfg(test)]
#[path = "browser_identity/tests.rs"]
mod tests;
