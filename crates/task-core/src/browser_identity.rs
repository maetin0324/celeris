//! ADR-0101 D2: Browser Identity（P3-A）の束縛・期限・失効・混入拒否の規則。
//!
//! I/O も時計も暗号も持たない。呼び出し側が `now`（UNIX 秒）を渡す。ここが決めるのは
//! 「どの identity をどの project/origin の browser session に戻してよいか」だけで、
//! 封緘（AEAD）は broker（celeris-credentiald）が `key_label` と `aad` を使って行う。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::browser::normalize_https_origin;

/// identity の期限の既定（秒）。H5: 期限付き。
pub const IDENTITY_TTL_DEFAULT_SECS: u64 = 7 * 24 * 3600;
/// identity の期限の上限（秒）。
pub const IDENTITY_TTL_MAX_SECS: u64 = 30 * 24 * 3600;
const KEY_LABEL_PREFIX: &str = "celeris-browser-identity-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityState {
    Active,
    Revoked,
    Deleted,
}

/// browser session の隔離（P4-A との契約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// ADR-0080 H6 の trusted local。identity の利用は許さない。
    TrustedLocal,
    /// P4-A の隔離（container / 別 UID / egress 制限）が証明された session。
    Isolated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRegistration {
    pub identity_id: String,
    pub project_id: String,
    pub origin: String,
    /// H5: 需要が確認された project+origin だけ。人の承認の参照。
    pub demand_confirmed_by: Option<String>,
    /// 0 は既定。
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserIdentity {
    pub identity_id: String,
    pub project_id: String,
    /// 正規化した https origin。
    pub origin: String,
    pub demand_confirmed_by: String,
    /// 失効のたびに増える。古い封緘は世代の不一致で拒否する。
    pub generation: u64,
    pub created_at: u64,
    pub expires_at: u64,
    pub state: IdentityState,
}

/// 封緘した state の外側（平文）。中身は broker だけが開く。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedEnvelope {
    pub identity_id: String,
    pub project_id: String,
    pub origin: String,
    pub generation: u64,
    pub key_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityUse {
    pub project_id: String,
    pub origin: String,
    pub isolation: Isolation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityDenied {
    DemandNotConfirmed,
    InvalidOrigin,
    InvalidId,
    TtlTooLong,
    Expired,
    Revoked,
    Deleted,
    OtherProject,
    OtherOrigin,
    OtherIdentity,
    StaleGeneration,
    KeyMismatch,
    ForeignOrigin,
    EmptyState,
    IsolationRequired,
}

impl IdentityDenied {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DemandNotConfirmed => "demand_not_confirmed",
            Self::InvalidOrigin => "invalid_origin",
            Self::InvalidId => "invalid_id",
            Self::TtlTooLong => "ttl_too_long",
            Self::Expired => "identity_expired",
            Self::Revoked => "identity_revoked",
            Self::Deleted => "identity_deleted",
            Self::OtherProject => "other_project",
            Self::OtherOrigin => "other_origin",
            Self::OtherIdentity => "other_identity",
            Self::StaleGeneration => "stale_generation",
            Self::KeyMismatch => "key_mismatch",
            Self::ForeignOrigin => "foreign_origin",
            Self::EmptyState => "empty_state",
            Self::IsolationRequired => "isolation_required",
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// project+origin 単位の鍵の label。broker はこれから鍵を導出する。
/// 区切りは NUL（id と origin に現れない）なので、別の組が同じ label にならない。
pub fn key_label(project_id: &str, origin: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(KEY_LABEL_PREFIX.as_bytes());
    hasher.update([0u8]);
    hasher.update(project_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(origin.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{KEY_LABEL_PREFIX}:{hex}")
}

/// AEAD の追加認証データ。identity・project・origin・世代を束縛する。
pub fn aad(identity: &BrowserIdentity) -> Vec<u8> {
    let mut out = Vec::new();
    for part in [
        KEY_LABEL_PREFIX,
        identity.identity_id.as_str(),
        identity.project_id.as_str(),
        identity.origin.as_str(),
    ] {
        out.extend_from_slice(part.as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&identity.generation.to_be_bytes());
    out
}

pub fn register(req: &IdentityRegistration, now: u64) -> Result<BrowserIdentity, IdentityDenied> {
    let confirmed = match req.demand_confirmed_by.as_deref().map(str::trim) {
        Some(by) if !by.is_empty() => by.to_string(),
        _ => return Err(IdentityDenied::DemandNotConfirmed),
    };
    if !valid_id(&req.identity_id) || !valid_id(&req.project_id) {
        return Err(IdentityDenied::InvalidId);
    }
    let origin = normalize_https_origin(&req.origin).ok_or(IdentityDenied::InvalidOrigin)?;
    if req.ttl_secs > IDENTITY_TTL_MAX_SECS {
        return Err(IdentityDenied::TtlTooLong);
    }
    let ttl = if req.ttl_secs == 0 {
        IDENTITY_TTL_DEFAULT_SECS
    } else {
        req.ttl_secs
    };
    Ok(BrowserIdentity {
        identity_id: req.identity_id.clone(),
        project_id: req.project_id.clone(),
        origin,
        demand_confirmed_by: confirmed,
        generation: 1,
        created_at: now,
        expires_at: now.saturating_add(ttl),
        state: IdentityState::Active,
    })
}

pub fn envelope_for(identity: &BrowserIdentity) -> SealedEnvelope {
    SealedEnvelope {
        identity_id: identity.identity_id.clone(),
        project_id: identity.project_id.clone(),
        origin: identity.origin.clone(),
        generation: identity.generation,
        key_label: key_label(&identity.project_id, &identity.origin),
    }
}

fn check_live(identity: &BrowserIdentity, now: u64) -> Result<(), IdentityDenied> {
    match identity.state {
        IdentityState::Deleted => return Err(IdentityDenied::Deleted),
        IdentityState::Revoked => return Err(IdentityDenied::Revoked),
        IdentityState::Active => {}
    }
    if now >= identity.expires_at {
        return Err(IdentityDenied::Expired);
    }
    Ok(())
}

/// identity を browser session に戻してよいか。隔離の無い session には戻さない。
pub fn authorize_use(
    identity: &BrowserIdentity,
    request: &IdentityUse,
    now: u64,
) -> Result<(), IdentityDenied> {
    check_live(identity, now)?;
    if request.project_id != identity.project_id {
        return Err(IdentityDenied::OtherProject);
    }
    let origin = normalize_https_origin(&request.origin).ok_or(IdentityDenied::InvalidOrigin)?;
    if origin != identity.origin {
        return Err(IdentityDenied::OtherOrigin);
    }
    if request.isolation != Isolation::Isolated {
        return Err(IdentityDenied::IsolationRequired);
    }
    Ok(())
}

/// 封緘が この identity のものか。開く前に呼ぶ。
pub fn check_envelope(
    identity: &BrowserIdentity,
    envelope: &SealedEnvelope,
    now: u64,
) -> Result<(), IdentityDenied> {
    check_live(identity, now)?;
    if envelope.identity_id != identity.identity_id {
        return Err(IdentityDenied::OtherIdentity);
    }
    if envelope.project_id != identity.project_id {
        return Err(IdentityDenied::OtherProject);
    }
    if envelope.origin != identity.origin {
        return Err(IdentityDenied::OtherOrigin);
    }
    if envelope.generation != identity.generation {
        return Err(IdentityDenied::StaleGeneration);
    }
    if envelope.key_label != key_label(&identity.project_id, &identity.origin) {
        return Err(IdentityDenied::KeyMismatch);
    }
    Ok(())
}

/// 保存・復元する state の各項目（cookie / storage）の origin が identity の origin だけか。
/// 1 件でも外れたら全体を拒否する。部分的に捨てて通すことはしない。
pub fn check_state_origins(
    identity: &BrowserIdentity,
    entry_origins: &[String],
) -> Result<(), IdentityDenied> {
    if entry_origins.is_empty() {
        return Err(IdentityDenied::EmptyState);
    }
    for entry in entry_origins {
        match normalize_https_origin(entry) {
            Some(origin) if origin == identity.origin => {}
            _ => return Err(IdentityDenied::ForeignOrigin),
        }
    }
    Ok(())
}

/// 失効。世代を上げるので、以前の封緘は開けない。削除済みは変えない。
pub fn revoke(identity: &mut BrowserIdentity) -> bool {
    if identity.state != IdentityState::Active {
        return false;
    }
    identity.state = IdentityState::Revoked;
    identity.generation = identity.generation.saturating_add(1);
    true
}

/// 削除。呼び出し側は封緘と鍵を消し、この tombstone だけを残す。
pub fn delete(identity: &mut BrowserIdentity) -> bool {
    if identity.state == IdentityState::Deleted {
        return false;
    }
    identity.state = IdentityState::Deleted;
    identity.generation = identity.generation.saturating_add(1);
    true
}

/// 期限切れなら失効させる。変えたら true。
pub fn expire(identity: &mut BrowserIdentity, now: u64) -> bool {
    if identity.state == IdentityState::Active && now >= identity.expires_at {
        return revoke(identity);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration() -> IdentityRegistration {
        IdentityRegistration {
            identity_id: "id-1".into(),
            project_id: "proj-a".into(),
            origin: "https://app.example.com".into(),
            demand_confirmed_by: Some("human:h5".into()),
            ttl_secs: 0,
        }
    }

    fn usage(project: &str, origin: &str, isolation: Isolation) -> IdentityUse {
        IdentityUse {
            project_id: project.into(),
            origin: origin.into(),
            isolation,
        }
    }

    #[test]
    fn register_requires_confirmed_demand_and_https_origin() {
        let mut req = registration();
        req.demand_confirmed_by = None;
        assert_eq!(register(&req, 10), Err(IdentityDenied::DemandNotConfirmed));
        req.demand_confirmed_by = Some("  ".into());
        assert_eq!(register(&req, 10), Err(IdentityDenied::DemandNotConfirmed));
        let mut req = registration();
        req.origin = "http://app.example.com".into();
        assert_eq!(register(&req, 10), Err(IdentityDenied::InvalidOrigin));
        let mut req = registration();
        req.project_id = "a/b".into();
        assert_eq!(register(&req, 10), Err(IdentityDenied::InvalidId));
    }

    #[test]
    fn register_bounds_the_ttl() {
        let identity = register(&registration(), 100).expect("register");
        assert_eq!(identity.expires_at, 100 + IDENTITY_TTL_DEFAULT_SECS);
        assert_eq!(identity.generation, 1);
        let mut req = registration();
        req.ttl_secs = IDENTITY_TTL_MAX_SECS + 1;
        assert_eq!(register(&req, 100), Err(IdentityDenied::TtlTooLong));
        req.ttl_secs = 3600;
        assert_eq!(register(&req, 100).expect("register").expires_at, 3700);
    }

    #[test]
    fn key_label_is_per_project_and_origin() {
        let base = key_label("proj-a", "https://app.example.com");
        assert_eq!(base, key_label("proj-a", "https://app.example.com"));
        assert_ne!(base, key_label("proj-b", "https://app.example.com"));
        assert_ne!(base, key_label("proj-a", "https://other.example.com"));
        assert!(!base.contains("proj-a"));
        assert!(!base.contains("example.com"));
    }

    #[test]
    fn use_is_denied_without_isolation() {
        let identity = register(&registration(), 100).expect("register");
        let origin = identity.origin.clone();
        assert_eq!(
            authorize_use(
                &identity,
                &usage("proj-a", &origin, Isolation::TrustedLocal),
                200
            ),
            Err(IdentityDenied::IsolationRequired)
        );
        assert_eq!(
            authorize_use(
                &identity,
                &usage("proj-a", &origin, Isolation::Isolated),
                200
            ),
            Ok(())
        );
    }

    #[test]
    fn use_rejects_other_project_and_origin() {
        let identity = register(&registration(), 100).expect("register");
        let origin = identity.origin.clone();
        assert_eq!(
            authorize_use(
                &identity,
                &usage("proj-b", &origin, Isolation::Isolated),
                200
            ),
            Err(IdentityDenied::OtherProject)
        );
        assert_eq!(
            authorize_use(
                &identity,
                &usage("proj-a", "https://evil.example.com", Isolation::Isolated),
                200
            ),
            Err(IdentityDenied::OtherOrigin)
        );
    }

    #[test]
    fn expiry_revocation_and_deletion_stop_use() {
        let mut identity = register(&registration(), 100).expect("register");
        let origin = identity.origin.clone();
        let request = usage("proj-a", &origin, Isolation::Isolated);
        let at_expiry = identity.expires_at;
        assert_eq!(
            authorize_use(&identity, &request, at_expiry),
            Err(IdentityDenied::Expired)
        );
        assert!(!expire(&mut identity, at_expiry - 1));
        assert!(expire(&mut identity, at_expiry));
        assert_eq!(identity.state, IdentityState::Revoked);
        assert_eq!(
            authorize_use(&identity, &request, 200),
            Err(IdentityDenied::Revoked)
        );
        assert!(!revoke(&mut identity));
        assert!(delete(&mut identity));
        assert!(!delete(&mut identity));
        assert_eq!(
            authorize_use(&identity, &request, 200),
            Err(IdentityDenied::Deleted)
        );
    }

    #[test]
    fn envelope_of_another_identity_is_rejected() {
        let identity = register(&registration(), 100).expect("register");
        let good = envelope_for(&identity);
        assert_eq!(check_envelope(&identity, &good, 200), Ok(()));

        let mut other = good.clone();
        other.identity_id = "id-2".into();
        assert_eq!(
            check_envelope(&identity, &other, 200),
            Err(IdentityDenied::OtherIdentity)
        );
        let mut other = good.clone();
        other.project_id = "proj-b".into();
        assert_eq!(
            check_envelope(&identity, &other, 200),
            Err(IdentityDenied::OtherProject)
        );
        let mut other = good.clone();
        other.origin = "https://other.example.com".into();
        assert_eq!(
            check_envelope(&identity, &other, 200),
            Err(IdentityDenied::OtherOrigin)
        );
        let mut other = good.clone();
        other.key_label = key_label("proj-b", &identity.origin);
        assert_eq!(
            check_envelope(&identity, &other, 200),
            Err(IdentityDenied::KeyMismatch)
        );
    }

    #[test]
    fn old_envelope_is_stale_after_revocation() {
        let mut identity = register(&registration(), 100).expect("register");
        let old = envelope_for(&identity);
        let before = aad(&identity);
        assert!(revoke(&mut identity));
        assert_ne!(before, aad(&identity));
        // 失効したままでは開けない。再登録（Active に戻した新しい世代）でも古い封緘は拒否する。
        assert_eq!(
            check_envelope(&identity, &old, 200),
            Err(IdentityDenied::Revoked)
        );
        identity.state = IdentityState::Active;
        assert_eq!(
            check_envelope(&identity, &old, 200),
            Err(IdentityDenied::StaleGeneration)
        );
    }

    #[test]
    fn state_with_a_foreign_origin_is_rejected_whole() {
        let identity = register(&registration(), 100).expect("register");
        let own = identity.origin.clone();
        assert_eq!(
            check_state_origins(&identity, std::slice::from_ref(&own)),
            Ok(())
        );
        assert_eq!(
            check_state_origins(
                &identity,
                &[own.clone(), "https://other.example.com".into()]
            ),
            Err(IdentityDenied::ForeignOrigin)
        );
        assert_eq!(
            check_state_origins(&identity, &[own, "not a url".into()]),
            Err(IdentityDenied::ForeignOrigin)
        );
        assert_eq!(
            check_state_origins(&identity, &[]),
            Err(IdentityDenied::EmptyState)
        );
    }

    #[test]
    fn codes_are_fixed_words() {
        assert_eq!(
            IdentityDenied::IsolationRequired.code(),
            "isolation_required"
        );
        assert_eq!(IdentityDenied::ForeignOrigin.code(), "foreign_origin");
        assert_eq!(IdentityDenied::StaleGeneration.code(), "stale_generation");
    }
}
