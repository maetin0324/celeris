//! ADR-0083 / H5: Browser Identity の登録・一覧・失効・削除・利用（P3-A）。
//!
//! 封緘は credentiald の [`IdentitySealer`]、metadata と封緘 blob は store が持つ。
//! 応答は metadata だけで、封緘 blob・鍵・cookie は返さない。
//! 利用（session への復元）は Isolated の経路が無いので trusted local では常に
//! `isolation_required`（ADR-0083 D3、P4-A の後に解除）。agent に state は渡さない。

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

    /// 隔離下の復元（ADR-0084 D4）。[`IsolationAttestation`] は P4-A の検査
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
}

/// 利用の入口。trusted local では常に 403 `isolation_required`（ADR-0083 D3）。
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
    run(&state, move |svc| {
        svc.restore(&id, &req.project_id, &req.origin, now)
    })
    .await?;
    Ok(json_response(
        StatusCode::NO_CONTENT,
        &serde_json::json!({}),
    ))
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
mod tests {
    use super::*;
    use celeris_credentiald::identity_seal::StateEntry;
    use task_core::browser_identity::IDENTITY_TTL_MAX_SECS;

    const NOW: u64 = 1_800_000_000;
    const ORIGIN: &str = "https://example.com";
    const SECRET: &str = "cookie-secret-value-zz9";

    fn state(origin: &str) -> IdentityStatePlain {
        IdentityStatePlain {
            entries: vec![StateEntry {
                origin: origin.into(),
                kind: "cookie".into(),
                name: "sid".into(),
                value: SECRET.into(),
            }],
        }
    }

    fn input(id: &str) -> IdentityRegisterInput {
        IdentityRegisterInput {
            identity_id: id.into(),
            project_id: "proj".into(),
            origin: ORIGIN.into(),
            demand_confirmed_by: Some("rmaeda".into()),
            ttl_secs: None,
            state: state(ORIGIN),
        }
    }

    fn fixture() -> (SqliteStore, IdentitySealer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let sealer = IdentitySealer::open(dir.path().join("keys")).unwrap();
        (SqliteStore::open_in_memory().unwrap(), sealer, dir)
    }

    fn code(r: Result<IdentityView, IdentityApiError>) -> &'static str {
        r.unwrap_err().code()
    }

    #[test]
    fn register_defaults_to_seven_days_and_lists_metadata_only() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        let v = svc.register(input("a"), NOW).unwrap();
        assert_eq!(v.expires_at, NOW + IDENTITY_TTL_DEFAULT_SECS);
        assert_eq!(v.state, IdentityState::Active);
        let list = svc.list("proj", NOW).unwrap();
        assert_eq!(list, vec![v]);
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains(SECRET));
        assert!(!json.contains("ciphertext"));
        assert!(!json.contains("key_label"));
        assert!(!json.contains("sealed"));
        // 保存されたのは封緘だけ（平文の cookie は store に無い）。
        let stored = store.browser_identity_get("a").unwrap().unwrap();
        let blob = stored.sealed_blob.clone().unwrap();
        assert!(!String::from_utf8_lossy(&blob).contains(SECRET));
        assert!(!format!("{stored:?}").contains(SECRET));
    }

    #[test]
    fn register_rejects_with_fixed_codes() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        let mut i = input("a");
        i.demand_confirmed_by = None;
        assert_eq!(code(svc.register(i, NOW)), "demand_not_confirmed");
        let mut i = input("a");
        i.ttl_secs = Some(IDENTITY_TTL_MAX_SECS + 1);
        assert_eq!(code(svc.register(i, NOW)), "ttl_too_long");
        let mut i = input("a");
        i.origin = "http://example.com".into();
        assert_eq!(code(svc.register(i, NOW)), "invalid_origin");
        let mut i = input("a");
        i.state = state("https://other.example");
        assert_eq!(code(svc.register(i, NOW)), "foreign_origin");
        let mut i = input("a");
        i.state
            .entries
            .extend(state("https://evil.example").entries.clone());
        assert_eq!(code(svc.register(i, NOW)), "foreign_origin");
        assert!(svc.list("proj", NOW).unwrap().is_empty());
        // 上限ちょうどは通る（切り詰めではない）。
        let mut i = input("b");
        i.ttl_secs = Some(IDENTITY_TTL_MAX_SECS);
        assert_eq!(
            svc.register(i, NOW).unwrap().expires_at,
            NOW + IDENTITY_TTL_MAX_SECS
        );
        assert_eq!(code(svc.register(input("b"), NOW)), "identity_conflict");
    }

    #[test]
    fn revoked_old_generation_cannot_be_opened() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        svc.register(input("a"), NOW).unwrap();
        let before = store.browser_identity_get("a").unwrap().unwrap();
        let sealed = sealed_of(&before).unwrap();
        assert!(sealer.open_state(&before.identity, &sealed, NOW).is_ok());
        let v = svc.revoke("a").unwrap();
        assert_eq!(v.state, IdentityState::Revoked);
        assert_eq!(v.generation, before.identity.generation + 1);
        let after = store.browser_identity_get("a").unwrap().unwrap();
        assert!(after.sealed_blob.is_none());
        let err = sealer
            .open_state(&after.identity, &sealed, NOW)
            .unwrap_err();
        assert!(matches!(err, SealError::Denied(IdentityDenied::Revoked)));
        let mut reactivated = after.identity.clone();
        reactivated.state = IdentityState::Active;
        let err = sealer.open_state(&reactivated, &sealed, NOW).unwrap_err();
        assert!(matches!(
            err,
            SealError::Denied(IdentityDenied::StaleGeneration)
        ));
        assert_eq!(svc.revoke("nope").unwrap_err().code(), "identity_not_found");
    }

    #[test]
    fn deleted_leaves_tombstone_and_seal_cannot_be_opened() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        svc.register(input("a"), NOW).unwrap();
        let before = store.browser_identity_get("a").unwrap().unwrap();
        let sealed = sealed_of(&before).unwrap();
        let v = svc.delete("a").unwrap();
        assert_eq!(v.state, IdentityState::Deleted);
        let tomb = store.browser_identity_get("a").unwrap().unwrap();
        assert!(tomb.sealed_blob.is_none());
        assert_eq!(svc.list("proj", NOW).unwrap().len(), 1);
        // 鍵が消えているので、世代が合う元の metadata でも開けない。
        let err = sealer
            .open_state(&before.identity, &sealed, NOW)
            .unwrap_err();
        assert!(matches!(err, SealError::KeyErased | SealError::Tampered));
        let err = sealer.open_state(&tomb.identity, &sealed, NOW).unwrap_err();
        assert!(matches!(err, SealError::Denied(IdentityDenied::Deleted)));
    }

    fn attestation() -> task_core::browser_isolation::IsolationAttestation {
        use task_core::browser_isolation::{
            CdpEndpoint, REQUIRED_NAMESPACES, RuntimeFacts, verify_isolation,
        };
        verify_isolation(&RuntimeFacts {
            session_id: "s1".into(),
            host_uid: 1000,
            runtime_uid: 200_001,
            namespaces: REQUIRED_NAMESPACES.into_iter().collect(),
            root_readonly: true,
            writable_mounts: vec!["/session/profile".into()],
            visible_paths: vec!["/usr".into()],
            cdp: CdpEndpoint::Pipe,
            no_new_privs: true,
            capabilities_dropped: true,
            pgid: 4242,
        })
        .unwrap()
    }

    #[test]
    fn restore_is_allowed_only_under_verified_isolation() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        svc.register(input("a"), NOW).unwrap();
        // trusted local は拒否のまま
        assert_eq!(
            svc.restore("a", "proj", ORIGIN, NOW).unwrap_err().code(),
            "isolation_required"
        );
        let att = attestation();
        let plain = svc
            .restore_isolated("a", "proj", ORIGIN, &att, NOW)
            .unwrap();
        assert_eq!(plain.entries[0].value, SECRET);
        // 隔離下でも他 project / 他 origin / 期限切れは拒否
        assert_eq!(
            svc.restore_isolated("a", "other", ORIGIN, &att, NOW)
                .unwrap_err()
                .code(),
            "other_project"
        );
        assert_eq!(
            svc.restore_isolated("a", "proj", "https://other.example", &att, NOW)
                .unwrap_err()
                .code(),
            "other_origin"
        );
        assert!(
            svc.restore_isolated(
                "a",
                "proj",
                ORIGIN,
                &att,
                NOW + IDENTITY_TTL_DEFAULT_SECS + 1
            )
            .is_err()
        );
    }

    #[test]
    fn restore_is_isolation_required_on_trusted_local() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        svc.register(input("a"), NOW).unwrap();
        let err = svc.restore("a", "proj", ORIGIN, NOW).unwrap_err();
        assert_eq!(err.code(), "isolation_required");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
        // 他 project / 他 origin は先に拒否する。
        assert_eq!(
            svc.restore("a", "other", ORIGIN, NOW).unwrap_err().code(),
            "other_project"
        );
        assert_eq!(
            svc.restore("a", "proj", "https://other.example", NOW)
                .unwrap_err()
                .code(),
            "other_origin"
        );
    }

    #[test]
    fn sealed_state_cannot_cross_project_or_identity() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        svc.register(input("a"), NOW).unwrap();
        let source = store.browser_identity_get("a").unwrap().unwrap();
        let sealed = sealed_of(&source).unwrap();

        let mut other_project = input("b");
        other_project.project_id = "other".into();
        svc.register(other_project, NOW).unwrap();
        let target = store.browser_identity_get("b").unwrap().unwrap();
        assert_eq!(
            sealer.open_state(&target.identity, &sealed, NOW),
            Err(SealError::Denied(IdentityDenied::OtherIdentity))
        );
        // Even if the outer envelope is forged, AEAD binds the original project and identity.
        let mut forged = sealed.clone();
        forged.envelope = bi::envelope_for(&target.identity);
        assert_eq!(
            sealer.open_state(&target.identity, &forged, NOW),
            Err(SealError::Tampered)
        );
        let mut same_id_other_project = target.identity.clone();
        same_id_other_project.identity_id = "a".into();
        assert_eq!(
            sealer.open_state(&same_id_other_project, &sealed, NOW),
            Err(SealError::Denied(IdentityDenied::OtherProject))
        );
    }

    #[test]
    fn public_output_and_debug_never_include_sealed_state_or_cookie() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        let view = svc.register(input("a"), NOW).unwrap();
        let stored = store.browser_identity_get("a").unwrap().unwrap();
        let sealed = sealed_of(&stored).unwrap();
        let response = serde_json::to_string(&serde_json::json!({"identity": view})).unwrap();
        let log = format!(
            "{:?} {:?} {:?}",
            input("a").state,
            view,
            IdentityApiError::SealFailed.problem()
        );
        for output in [&response, &log] {
            assert!(!output.contains(SECRET));
            assert!(!output.contains(&sealed.ciphertext));
            assert!(!output.contains(&sealed.envelope.key_label));
        }
        assert!(!response.contains("sealed_blob"));
        assert!(!response.contains("key_label"));
    }

    #[test]
    fn expired_identities_are_revoked_on_read() {
        let (store, sealer, _d) = fixture();
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        let v = svc.register(input("a"), NOW).unwrap();
        let later = v.expires_at;
        let list = svc.list("proj", later).unwrap();
        assert_eq!(list[0].state, IdentityState::Revoked);
        assert!(
            store
                .browser_identity_get("a")
                .unwrap()
                .unwrap()
                .sealed_blob
                .is_none()
        );
        assert_eq!(
            svc.restore("a", "proj", ORIGIN, later).unwrap_err().code(),
            "identity_revoked"
        );
    }

    #[test]
    fn problem_body_is_fixed_code_only() {
        let p = IdentityApiError::Denied(IdentityDenied::IsolationRequired).problem();
        let s = format!("{p:?}");
        assert!(s.contains("isolation_required"));
        assert!(!s.contains(SECRET));
    }
}
