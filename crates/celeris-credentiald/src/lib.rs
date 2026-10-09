//! Local credential broker. Legacy plugin secret retrieval is disabled (ADR-0103).
#![cfg(unix)]

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Permission,
    TrustedInjectionRequired,
    VaultLocked,
    NotFound,
    Denied,
    Expired,
    Used,
    NeedsHuman,
    AuditUnavailable,
    Io,
}
impl Error {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_request",
            Self::Permission => "permission_denied",
            Self::TrustedInjectionRequired => "trusted_injection_required",
            Self::VaultLocked => "vault_locked",
            Self::NotFound => "not_found",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Used => "used",
            Self::NeedsHuman => "needs_human",
            Self::AuditUnavailable => "audit_unavailable",
            Self::Io => "io_error",
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for Error {}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn canonical_origin(input: &str) -> Result<String, Error> {
    let rest = input.strip_prefix("https://").ok_or(Error::Invalid)?;
    if rest.is_empty()
        || rest
            .bytes()
            .any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | b'\\' | b'%' | b' '))
    {
        return Err(Error::Invalid);
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    if host.len() > 253
        || host.is_empty()
        || host.starts_with('-')
        || host.contains('*')
        || host.split('.').any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(Error::Invalid);
    }
    let port = match port {
        Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            p.parse::<u16>().map_err(|_| Error::Invalid)?
        }
        Some(_) => return Err(Error::Invalid),
        None => 443,
    };
    if port == 0 {
        return Err(Error::Invalid);
    }
    let host = host.to_ascii_lowercase();
    Ok(if port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    })
}
/// lease の policy 断面が持つ trusted selector（照合 4b）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LeaseSelectors {
    pub(crate) password: Option<String>,
    pub(crate) username: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub credential_id: String,
    pub provider: String,
    pub policy_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialPolicy {
    pub policy_id: String,
    pub revision: u64,
    pub exact_origin: String,
    pub task_id: String,
    pub max_ttl_seconds: u64,
    pub require_approval: bool,
    pub allow_persistence: bool,
    /// ADR-0110 D2: 管理者が設定するログイン URL（`exact_origin` 直下）。モデル・worker は指定できない。
    /// 旧い policy は欄が無くても読めるが、trusted selector が無いので注入は拒否される。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    /// 管理者が設定する top-level の password 欄 selector（ADR-0110 D2 の文法）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_selector: Option<String>,
    /// 管理者が設定する任意の submit selector（同じ文法）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_selector: Option<String>,
    /// ADR 2026-10-09 credential username / post-login D1: 管理者が設定する username 欄の selector。
    /// あれば注入要求は username 欄を伴わなければならない（照合 4b）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_selector: Option<String>,
    /// 同 D2: ログイン後の読み取りの opt-in（承認画面・TrustedLogin に写すだけで、broker は使わない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_login: Option<task_core::browser_wait::PostLogin>,
}
impl CredentialPolicy {
    /// 注入に使える管理者の password selector（`login_url` と揃っているときだけ）。
    pub fn trusted_password_selector(&self) -> Option<&str> {
        self.login_url.as_ref()?;
        self.password_selector.as_deref()
    }
    /// 注入に使える管理者の username selector（`login_url` と password selector が揃っているときだけ）。
    pub fn trusted_username_selector(&self) -> Option<&str> {
        self.trusted_password_selector()?;
        self.username_selector.as_deref()
    }
    pub fn validate(&self) -> Result<(), Error> {
        match (&self.login_url, &self.password_selector) {
            (None, None)
                if self.submit_selector.is_none()
                    && self.username_selector.is_none()
                    && self.post_login.is_none() => {}
            (Some(url), Some(password)) => task_core::browser_wait::validate_trusted_login_full(
                url,
                &self.exact_origin,
                password,
                self.submit_selector.as_deref(),
                self.username_selector.as_deref(),
                self.post_login.as_ref(),
            )
            .map_err(|_| Error::Invalid)?,
            _ => return Err(Error::Invalid),
        }
        if !valid_id(&self.policy_id)
            || !valid_id(&self.task_id)
            || self.revision == 0
            || self.max_ttl_seconds == 0
            || self.max_ttl_seconds > 300
            || !self.require_approval
            || self.allow_persistence
            || canonical_origin(&self.exact_origin)? != self.exact_origin
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedLeaseContext {
    pub lease_id: String,
    pub task_id: String,
    pub run_id: String,
    pub session_id: String,
    pub exact_origin: String,
    pub expires_at: u64,
    pub credential_revision: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretEnvelope {
    pub username: String,
    pub password: String,
}
impl std::fmt::Debug for SecretEnvelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretEnvelope(<redacted>)")
    }
}
impl Drop for SecretEnvelope {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub manual_registration: bool,
    pub interactive_unlock: bool,
    pub totp: bool,
    pub revoke: bool,
}
pub trait CredentialProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;
    fn resolve(
        &self,
        reference: &CredentialRef,
        context: &AuthorizedLeaseContext,
    ) -> Result<SecretEnvelope, Error>;
    fn revoke(&self, lease_id: &str) -> Result<(), Error>;
}
fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn check_dir(path: &Path) -> Result<(), Error> {
    let m = fs::symlink_metadata(path).map_err(|_| Error::Permission)?;
    if !m.is_dir() || m.is_symlink() || m.uid() != uid() || m.mode() & 0o777 != 0o700 {
        return Err(Error::Permission);
    }
    Ok(())
}
fn check_file(path: &Path) -> Result<(), Error> {
    let m = fs::symlink_metadata(path).map_err(|_| Error::Permission)?;
    if !m.is_file()
        || m.is_symlink()
        || m.nlink() != 1
        || m.uid() != uid()
        || m.mode() & 0o777 != 0o600
    {
        return Err(Error::Permission);
    }
    Ok(())
}
fn ensure_dir(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| Error::Io)?;
    }
    check_dir(path)
}
fn read_private(path: &Path) -> Result<Vec<u8>, Error> {
    check_file(path)?;
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| Error::Io)?;
    let m = f.metadata().map_err(|_| Error::Permission)?;
    if !m.is_file() || m.nlink() != 1 || m.uid() != uid() || m.mode() & 0o777 != 0o600 {
        return Err(Error::Permission);
    }
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|_| Error::Io)?;
    Ok(b)
}
fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).map_err(|_| Error::Io)?;
    Ok(b)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| Error::Io)?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|_| Error::Io)
}
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> Result<(), Error> {
    check_dir(dir)?;
    if path.exists() {
        check_file(path)?;
    }
    let tmp = dir.join(format!(".tmp-{}", hex::encode(random::<16>()?)));
    write_new(&tmp, bytes)?;
    let res = fs::rename(&tmp, path).and_then(|_| File::open(dir)?.sync_all());
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
        return Err(Error::Io);
    }
    Ok(())
}
fn now() -> Result<u64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| Error::Io)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    format_version: u8,
    key_id: String,
    credential_id: String,
    provider: String,
    revision: u64,
    exact_origin: String,
    policy_id: String,
    #[serde(default)]
    policy_revision: u64,
    #[serde(default)]
    login_url: Option<String>,
    #[serde(default)]
    password_selector: Option<String>,
    #[serde(default)]
    submit_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    post_login: Option<task_core::browser_wait::PostLogin>,
    nonce: String,
    ciphertext: String,
    tag: String,
}
fn aad(s: &Stored) -> Vec<u8> {
    let mut out = vec![s.format_version];
    for part in [&s.credential_id, &s.provider, &s.exact_origin, &s.policy_id] {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part.as_bytes());
    }
    out.extend_from_slice(&s.revision.to_be_bytes());
    out
}

#[derive(Clone)]
pub struct ManualProvider {
    key_dir: PathBuf,
    vault_dir: PathBuf,
}
impl ManualProvider {
    pub fn open(key_dir: PathBuf, vault_dir: PathBuf) -> Result<Self, Error> {
        check_dir(key_dir.parent().ok_or(Error::Invalid)?)?;
        check_dir(vault_dir.parent().ok_or(Error::Invalid)?)?;
        ensure_dir(&key_dir)?;
        ensure_dir(&vault_dir)?;
        Ok(Self { key_dir, vault_dir })
    }
    pub fn initialize_key(&self) -> Result<(), Error> {
        check_dir(&self.key_dir)?;
        check_dir(&self.vault_dir)?;
        let p = self.key_dir.join("master-v1.key");
        if p.exists() {
            check_file(&p)?;
            return Err(Error::Denied);
        }
        if fs::read_dir(&self.vault_dir)
            .map_err(|_| Error::Io)?
            .next()
            .is_some()
        {
            return Err(Error::VaultLocked);
        }
        let mut k = random::<32>()?;
        let res = write_new(&p, &k).and_then(|_| {
            File::open(&self.key_dir)
                .map_err(|_| Error::Io)?
                .sync_all()
                .map_err(|_| Error::Io)
        });
        k.zeroize();
        res
    }
    fn key(&self) -> Result<[u8; 32], Error> {
        check_dir(&self.key_dir)?;
        let mut b =
            read_private(&self.key_dir.join("master-v1.key")).map_err(|_| Error::VaultLocked)?;
        if b.len() != 32 {
            b.zeroize();
            return Err(Error::VaultLocked);
        }
        let mut k = [0; 32];
        k.copy_from_slice(&b);
        b.zeroize();
        Ok(k)
    }
    pub fn register(
        &self,
        reference: &CredentialRef,
        policy: &CredentialPolicy,
        revision: u64,
        secret: &SecretEnvelope,
    ) -> Result<(), Error> {
        policy.validate()?;
        if reference.provider != "manual"
            || !valid_id(&reference.credential_id)
            || reference.policy_id != policy.policy_id
            || revision == 0
        {
            return Err(Error::Invalid);
        }
        let path = self
            .vault_dir
            .join(format!("{}.json", reference.credential_id));
        let mut s = Stored {
            format_version: 1,
            key_id: "master-v1".into(),
            credential_id: reference.credential_id.clone(),
            provider: "manual".into(),
            revision,
            exact_origin: policy.exact_origin.clone(),
            policy_id: policy.policy_id.clone(),
            policy_revision: policy.revision,
            login_url: policy.login_url.clone(),
            password_selector: policy.password_selector.clone(),
            submit_selector: policy.submit_selector.clone(),
            username_selector: policy.username_selector.clone(),
            post_login: policy.post_login.clone(),
            nonce: String::new(),
            ciphertext: String::new(),
            tag: String::new(),
        };
        let mut key = self.key()?;
        let nonce = random::<24>()?;
        let cipher = XChaCha20Poly1305::new((&key).into());
        #[derive(Serialize)]
        struct Plaintext<'a> {
            username: &'a str,
            password: &'a str,
        }
        let mut plaintext = serde_json::to_vec(&Plaintext {
            username: &secret.username,
            password: &secret.password,
        })
        .map_err(|_| Error::Invalid)?;
        let result = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plaintext,
                    aad: &aad(&s),
                },
            )
            .map_err(|_| Error::Io);
        plaintext.zeroize();
        key.zeroize();
        let mut output = result?;
        if output.len() < 16 {
            output.zeroize();
            return Err(Error::Io);
        }
        let tag = output.split_off(output.len() - 16);
        s.nonce = hex::encode(nonce);
        s.ciphertext = hex::encode(&output);
        s.tag = hex::encode(tag);
        output.zeroize();
        let bytes = serde_json::to_vec(&s).map_err(|_| Error::Io)?;
        write_atomic(&self.vault_dir, &path, &bytes)
    }
    /// Return only the administrator's non-secret login metadata for approval pinning.
    pub fn describe_registered(
        &self,
        reference: &CredentialRef,
        origin: &str,
        credential_revision: Option<u64>,
    ) -> Result<task_core::browser_wait::TrustedLogin, Error> {
        if reference.provider != "manual"
            || !valid_id(&reference.credential_id)
            || !valid_id(&reference.policy_id)
            || canonical_origin(origin)? != origin
        {
            return Err(Error::Invalid);
        }
        let bytes = read_private(
            &self
                .vault_dir
                .join(format!("{}.json", reference.credential_id)),
        )?;
        let s: Stored = serde_json::from_slice(&bytes).map_err(|_| Error::VaultLocked)?;
        if s.format_version != 1
            || s.credential_id != reference.credential_id
            || s.provider != reference.provider
            || s.policy_id != reference.policy_id
            || s.exact_origin != origin
            || credential_revision.is_some_and(|revision| s.revision != revision)
        {
            return Err(Error::Denied);
        }
        let login_url = s.login_url.ok_or(Error::Denied)?;
        let password_selector = s.password_selector.ok_or(Error::Denied)?;
        task_core::browser_wait::validate_trusted_login_full(
            &login_url,
            origin,
            &password_selector,
            s.submit_selector.as_deref(),
            s.username_selector.as_deref(),
            s.post_login.as_ref(),
        )
        .map_err(|_| Error::Invalid)?;
        Ok(task_core::browser_wait::TrustedLogin {
            policy_id: s.policy_id,
            revision: s.policy_revision,
            login_url,
            password_selector,
            submit_selector: s.submit_selector,
            username_selector: s.username_selector,
            post_login: s.post_login,
        })
    }
    pub fn resolve_registered(
        &self,
        reference: &CredentialRef,
        revision: u64,
        origin: &str,
    ) -> Result<SecretEnvelope, Error> {
        check_dir(&self.vault_dir)?;
        if reference.provider != "manual"
            || !valid_id(&reference.credential_id)
            || !valid_id(&reference.policy_id)
            || canonical_origin(origin)? != origin
        {
            return Err(Error::Invalid);
        }
        let path = self
            .vault_dir
            .join(format!("{}.json", reference.credential_id));
        let bytes = read_private(&path)?;
        let s: Stored = serde_json::from_slice(&bytes).map_err(|_| Error::VaultLocked)?;
        if s.format_version != 1
            || s.key_id != "master-v1"
            || s.credential_id != reference.credential_id
            || s.provider != reference.provider
            || s.policy_id != reference.policy_id
            || s.exact_origin != origin
            || s.revision != revision
        {
            return Err(Error::Denied);
        }
        let nonce = hex::decode(&s.nonce).map_err(|_| Error::VaultLocked)?;
        let mut data = hex::decode(&s.ciphertext).map_err(|_| Error::VaultLocked)?;
        let tag = hex::decode(&s.tag).map_err(|_| Error::VaultLocked)?;
        if nonce.len() != 24 || tag.len() != 16 {
            return Err(Error::VaultLocked);
        }
        data.extend_from_slice(&tag);
        let mut key = self.key()?;
        let cipher = XChaCha20Poly1305::new((&key).into());
        let result = cipher
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &data,
                    aad: &aad(&s),
                },
            )
            .map_err(|_| Error::Denied);
        key.zeroize();
        data.zeroize();
        let mut plain = result?;
        let secret = serde_json::from_slice(&plain).map_err(|_| Error::VaultLocked);
        plain.zeroize();
        secret
    }
    pub fn remove(&self, reference: &CredentialRef) -> Result<(), Error> {
        check_dir(&self.vault_dir)?;
        if reference.provider != "manual" || !valid_id(&reference.credential_id) {
            return Err(Error::Invalid);
        }
        let p = self
            .vault_dir
            .join(format!("{}.json", reference.credential_id));
        check_file(&p)?;
        fs::remove_file(p).map_err(|_| Error::Io)?;
        File::open(&self.vault_dir)
            .map_err(|_| Error::Io)?
            .sync_all()
            .map_err(|_| Error::Io)
    }
}
impl CredentialProvider for ManualProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            manual_registration: true,
            interactive_unlock: false,
            totp: false,
            revoke: true,
        }
    }
    fn resolve(
        &self,
        r: &CredentialRef,
        c: &AuthorizedLeaseContext,
    ) -> Result<SecretEnvelope, Error> {
        self.resolve_registered(r, c.credential_revision, &c.exact_origin)
    }
    fn revoke(&self, _: &str) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub token: String,
    pub task_id: String,
    pub run_id: String,
    pub session_id: String,
    pub exact_origin: String,
    pub policy_hash: String,
    pub expires_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRequest {
    pub reference: CredentialRef,
    pub policy: CredentialPolicy,
    pub credential_revision: u64,
    pub task_id: String,
    pub run_id: String,
    pub session_id: String,
    pub approval_id: String,
    pub approved_by: String,
    pub policy_hash: String,
    pub idempotency_key: String,
    pub ttl_seconds: u64,
    pub approval_expires_at: u64,
    pub session_expires_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseGrant {
    pub lease_id: String,
    pub expires_at: u64,
    pub max_uses: u8,
}
#[derive(Serialize)]
struct AuditRecord<'a> {
    sequence: u64,
    timestamp: u64,
    action: &'a str,
    decision_code: &'a str,
    actor_id: &'a str,
    task_id: &'a str,
    run_id: &'a str,
    session_id: &'a str,
    credential_id: &'a str,
    provider: &'a str,
    policy_id: &'a str,
    policy_hash: &'a str,
    lease_id: &'a str,
    approval_id: &'a str,
}
struct Lease {
    request: LeaseRequest,
    grant: LeaseGrant,
    used: bool,
    revoked: bool,
}
struct State {
    leases: HashMap<String, Lease>,
    bindings: HashMap<String, Binding>,
    sequence: u64,
}
pub struct Broker {
    manual: ManualProvider,
    providers: HashMap<String, Arc<dyn CredentialProvider>>,
    audit_dir: PathBuf,
    state: Mutex<State>,
}
impl Broker {
    pub fn new(provider: ManualProvider, audit_dir: PathBuf) -> Result<Self, Error> {
        check_dir(audit_dir.parent().ok_or(Error::Invalid)?)?;
        ensure_dir(&audit_dir)?;
        let p = audit_dir.join("journal.jsonl");
        let sequence = if p.exists() {
            let b = read_private(&p)?;
            if !b.is_empty()
                && (b.last() != Some(&b'\n')
                    || b.split(|v| *v == b'\n')
                        .take_while(|line| !line.is_empty())
                        .any(|line| serde_json::from_slice::<serde_json::Value>(line).is_err()))
            {
                return Err(Error::AuditUnavailable);
            }
            b.iter().filter(|v| **v == b'\n').count() as u64
        } else {
            0
        };
        let mut providers: HashMap<String, Arc<dyn CredentialProvider>> = HashMap::new();
        providers.insert("manual".into(), Arc::new(provider.clone()));
        Ok(Self {
            manual: provider,
            providers,
            audit_dir,
            state: Mutex::new(State {
                leases: HashMap::new(),
                bindings: HashMap::new(),
                sequence,
            }),
        })
    }
    pub fn add_provider(
        &mut self,
        name: String,
        provider: Arc<dyn CredentialProvider>,
    ) -> Result<(), Error> {
        if !valid_id(&name) || name == "manual" || self.providers.contains_key(&name) {
            return Err(Error::Invalid);
        }
        self.providers.insert(name, provider);
        Ok(())
    }
    fn audit(
        &self,
        state: &mut State,
        action: &str,
        decision: &str,
        actor: &str,
        req: &LeaseRequest,
        lease_id: &str,
    ) -> Result<(), Error> {
        check_dir(&self.audit_dir).map_err(|_| Error::AuditUnavailable)?;
        let p = self.audit_dir.join("journal.jsonl");
        if p.exists() {
            check_file(&p).map_err(|_| Error::AuditUnavailable)?
        }
        let record = AuditRecord {
            sequence: state.sequence + 1,
            timestamp: now().map_err(|_| Error::AuditUnavailable)?,
            action,
            decision_code: decision,
            actor_id: actor,
            task_id: &req.task_id,
            run_id: &req.run_id,
            session_id: &req.session_id,
            credential_id: &req.reference.credential_id,
            provider: &req.reference.provider,
            policy_id: &req.reference.policy_id,
            policy_hash: &req.policy_hash,
            lease_id,
            approval_id: &req.approval_id,
        };
        let mut bytes = serde_json::to_vec(&record).map_err(|_| Error::AuditUnavailable)?;
        bytes.push(b'\n');
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&p)
            .map_err(|_| Error::AuditUnavailable)?;
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| Error::AuditUnavailable)?;
        state.sequence += 1;
        Ok(())
    }
    pub fn register_binding(&self, mut binding: Binding) -> Result<String, Error> {
        let t = now()?;
        if binding.expires_at <= t
            || binding.expires_at > t + 300
            || !valid_id(&binding.task_id)
            || !valid_id(&binding.run_id)
            || !valid_id(&binding.session_id)
            || binding.policy_hash.is_empty()
            || canonical_origin(&binding.exact_origin)? != binding.exact_origin
        {
            return Err(Error::Invalid);
        }
        binding.token = hex::encode(random::<32>()?);
        let token = binding.token.clone();
        let mut s = self.state.lock().map_err(|_| Error::Denied)?;
        s.bindings.insert(token.clone(), binding);
        Ok(token)
    }
    pub fn grant(&self, req: LeaseRequest) -> Result<LeaseGrant, Error> {
        req.policy.validate()?;
        // A caller cannot replace the administrator's registered site selector
        // in a freshly issued lease. Legacy leases without H3 metadata retain
        // their existing validation path.
        if req.reference.provider == "manual" && req.policy.login_url.is_some() {
            let stored = self.manual.describe_registered(
                &req.reference,
                &req.policy.exact_origin,
                Some(req.credential_revision),
            )?;
            if stored.revision != req.policy.revision
                || Some(&stored.login_url) != req.policy.login_url.as_ref()
                || Some(&stored.password_selector) != req.policy.password_selector.as_ref()
                || stored.submit_selector != req.policy.submit_selector
                || stored.username_selector != req.policy.username_selector
                || stored.post_login != req.policy.post_login
            {
                return Err(Error::Denied);
            }
        }
        let t = now()?;
        if req.policy.task_id != req.task_id
            || req.reference.policy_id != req.policy.policy_id
            || !self.providers.contains_key(&req.reference.provider)
            || req.credential_revision == 0
            || !valid_id(&req.reference.credential_id)
            || !valid_id(&req.reference.provider)
            || !valid_id(&req.run_id)
            || !valid_id(&req.session_id)
            || !valid_id(&req.approval_id)
            || !valid_id(&req.approved_by)
            || !valid_id(&req.policy_hash)
            || !valid_id(&req.idempotency_key)
            || req.ttl_seconds == 0
            || req.approval_expires_at <= t
            || req.session_expires_at <= t
        {
            return Err(Error::Denied);
        }
        let ttl = req
            .ttl_seconds
            .min(req.policy.max_ttl_seconds)
            .min(300)
            .min(req.approval_expires_at - t)
            .min(req.session_expires_at - t);
        if ttl == 0 {
            return Err(Error::Expired);
        }
        let grant = LeaseGrant {
            lease_id: hex::encode(random::<32>()?),
            expires_at: t + ttl,
            max_uses: 1,
        };
        let mut s = self.state.lock().map_err(|_| Error::Denied)?;
        self.audit(
            &mut s,
            "request",
            "requested",
            &req.approved_by,
            &req,
            &grant.lease_id,
        )?;
        if s.leases
            .values()
            .any(|l| l.request.idempotency_key == req.idempotency_key)
        {
            self.audit(
                &mut s,
                "deny",
                "used",
                &req.approved_by,
                &req,
                &grant.lease_id,
            )?;
            return Err(Error::Used);
        }
        let context = AuthorizedLeaseContext {
            lease_id: grant.lease_id.clone(),
            task_id: req.task_id.clone(),
            run_id: req.run_id.clone(),
            session_id: req.session_id.clone(),
            exact_origin: req.policy.exact_origin.clone(),
            expires_at: grant.expires_at,
            credential_revision: req.credential_revision,
        };
        if let Err(e) = self
            .providers
            .get(&req.reference.provider)
            .ok_or(Error::Denied)?
            .resolve(&req.reference, &context)
        {
            self.audit(
                &mut s,
                "deny",
                e.code(),
                &req.approved_by,
                &req,
                &grant.lease_id,
            )?;
            return Err(e);
        }
        self.audit(
            &mut s,
            "grant",
            "granted",
            &req.approved_by,
            &req,
            &grant.lease_id,
        )?;
        s.leases.insert(
            grant.lease_id.clone(),
            Lease {
                request: req,
                grant: grant.clone(),
                used: false,
                revoked: false,
            },
        );
        Ok(grant)
    }
    pub fn resolve(
        &self,
        token: &str,
        lease_id: &str,
        observed_origin: &str,
    ) -> Result<SecretEnvelope, Error> {
        let t = now()?;
        let mut s = self.state.lock().map_err(|_| Error::Denied)?;
        let (req, grant, used, revoked) = {
            let lease = s.leases.get(lease_id).ok_or(Error::NotFound)?;
            (
                lease.request.clone(),
                lease.grant.clone(),
                lease.used,
                lease.revoked,
            )
        };
        let binding = match s.bindings.get(token).cloned() {
            Some(binding) => binding,
            None => {
                self.audit(&mut s, "deny", "denied", "plugin", &req, lease_id)?;
                return Err(Error::Denied);
            }
        };
        let origin = match canonical_origin(observed_origin) {
            Ok(origin) => origin,
            Err(_) => {
                self.audit(&mut s, "deny", "invalid_request", "plugin", &req, lease_id)?;
                return Err(Error::Invalid);
            }
        };
        let decision = if binding.expires_at <= t || grant.expires_at <= t {
            Some(Error::Expired)
        } else if used {
            Some(Error::Used)
        } else if revoked
            || binding.task_id != req.task_id
            || binding.run_id != req.run_id
            || binding.session_id != req.session_id
            || binding.policy_hash != req.policy_hash
            || binding.exact_origin != req.policy.exact_origin
            || origin != req.policy.exact_origin
        {
            Some(Error::Denied)
        } else {
            None
        };
        if let Some(e) = decision {
            self.audit(
                &mut s,
                if e == Error::Expired {
                    "expire"
                } else {
                    "deny"
                },
                e.code(),
                "plugin",
                &req,
                lease_id,
            )?;
            return Err(e);
        }
        // Durable consume is committed before decryption. A lost response cannot be replayed.
        self.audit(&mut s, "use", "consumed", "plugin", &req, lease_id)?;
        if let Some(lease) = s.leases.get_mut(lease_id) {
            lease.used = true;
        }
        drop(s);
        let context = AuthorizedLeaseContext {
            lease_id: lease_id.into(),
            task_id: req.task_id,
            run_id: req.run_id,
            session_id: req.session_id,
            exact_origin: origin,
            expires_at: grant.expires_at,
            credential_revision: req.credential_revision,
        };
        self.providers
            .get(&req.reference.provider)
            .ok_or(Error::Denied)?
            .resolve(&req.reference, &context)
    }
    pub fn revoke(&self, lease_id: &str, actor: &str) -> Result<(), Error> {
        if !valid_id(actor) {
            return Err(Error::Invalid);
        }
        let mut s = self.state.lock().map_err(|_| Error::Denied)?;
        let req = s
            .leases
            .get(lease_id)
            .ok_or(Error::NotFound)?
            .request
            .clone();
        self.audit(&mut s, "revoke", "revoked", actor, &req, lease_id)?;
        if let Some(l) = s.leases.get_mut(lease_id) {
            l.revoked = true;
        }
        self.providers
            .get(&req.reference.provider)
            .ok_or(Error::Denied)?
            .revoke(lease_id)
    }
    pub fn provider(&self) -> &ManualProvider {
        &self.manual
    }
    /// ADR-0110 D2 照合 3: lease が保持する policy 断面の trusted password selector と（ADR 2026-10-09
    /// credential username / post-login D1-3）username selector を消費せずに読む。
    /// lease が無ければ `None`（その拒否は消費の段で出す）。
    pub(crate) fn lease_trusted_selector(&self, lease_id: &str) -> Option<LeaseSelectors> {
        let s = self.state.lock().ok()?;
        let lease = s.leases.get(lease_id)?;
        let policy = &lease.request.policy;
        Some(LeaseSelectors {
            password: policy.trusted_password_selector().map(str::to_owned),
            username: policy.trusted_username_selector().map(str::to_owned),
        })
    }
    /// ADR-0109 D3 step 5: the trusted-injection path consumes a lease without a
    /// plugin binding. Same lease checks as [`Broker::resolve`]; the durable
    /// consume is committed before the provider is contacted.
    pub(crate) fn consume_for_injection(
        &self,
        lease_id: &str,
        session_id: &str,
        origin: &str,
    ) -> Result<
        (
            CredentialRef,
            AuthorizedLeaseContext,
            Arc<dyn CredentialProvider>,
        ),
        LeaseDenied,
    > {
        let t = now().map_err(|_| LeaseDenied::Invalid)?;
        let mut s = self.state.lock().map_err(|_| LeaseDenied::Invalid)?;
        let (req, grant, used, revoked) = {
            let lease = s.leases.get(lease_id).ok_or(LeaseDenied::Invalid)?;
            (
                lease.request.clone(),
                lease.grant.clone(),
                lease.used,
                lease.revoked,
            )
        };
        let decision = if grant.expires_at <= t {
            Some(LeaseDenied::Expired)
        } else if used {
            Some(LeaseDenied::Used)
        } else if req.session_id != session_id {
            Some(LeaseDenied::OtherSession)
        } else if revoked || req.policy.exact_origin != origin {
            Some(LeaseDenied::Invalid)
        } else {
            None
        };
        if let Some(d) = decision {
            let action = if d == LeaseDenied::Expired {
                "expire"
            } else {
                "deny"
            };
            self.audit(&mut s, action, d.code(), "injector", &req, lease_id)
                .map_err(|_| LeaseDenied::Audit)?;
            return Err(d);
        }
        self.audit(&mut s, "use", "consumed", "injector", &req, lease_id)
            .map_err(|_| LeaseDenied::Audit)?;
        if let Some(lease) = s.leases.get_mut(lease_id) {
            lease.used = true;
        }
        drop(s);
        let provider = self
            .providers
            .get(&req.reference.provider)
            .cloned()
            .ok_or(LeaseDenied::Invalid)?;
        let context = AuthorizedLeaseContext {
            lease_id: lease_id.into(),
            task_id: req.task_id,
            run_id: req.run_id,
            session_id: req.session_id,
            exact_origin: origin.into(),
            expires_at: grant.expires_at,
            credential_revision: req.credential_revision,
        };
        Ok((req.reference, context, provider))
    }
    /// Non-secret injection decision record (ADR-0109 D1 audit).
    pub(crate) fn audit_injection(&self, record: &serde_json::Value) -> Result<(), Error> {
        let mut s = self.state.lock().map_err(|_| Error::AuditUnavailable)?;
        check_dir(&self.audit_dir).map_err(|_| Error::AuditUnavailable)?;
        let p = self.audit_dir.join("journal.jsonl");
        if p.exists() {
            check_file(&p).map_err(|_| Error::AuditUnavailable)?
        }
        let mut record = record.clone();
        if let Some(o) = record.as_object_mut() {
            o.insert("sequence".into(), (s.sequence + 1).into());
            o.insert(
                "timestamp".into(),
                now().map_err(|_| Error::AuditUnavailable)?.into(),
            );
        }
        let mut bytes = serde_json::to_vec(&record).map_err(|_| Error::AuditUnavailable)?;
        bytes.push(b'\n');
        let mut f = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&p)
            .map_err(|_| Error::AuditUnavailable)?;
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| Error::AuditUnavailable)?;
        s.sequence += 1;
        Ok(())
    }
}
/// Lease-stage refusals of the injection path (ADR-0109 D3 step 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeaseDenied {
    Expired,
    Used,
    OtherSession,
    Invalid,
    Audit,
}
impl LeaseDenied {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Expired => "lease_expired",
            Self::Used => "lease_used",
            Self::OtherSession => "other_session",
            Self::Invalid => "lease_invalid",
            Self::Audit => "audit_unavailable",
        }
    }
}
pub mod identity_seal;
pub mod injection;
pub mod injection_ipc;
pub mod ipc;
#[derive(Debug, Serialize)]
pub struct CredentialUseResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<&'static str>,
}
impl CredentialUseResult {
    pub fn from_result(result: Result<(), Error>) -> Self {
        match result {
            Ok(()) => Self {
                success: true,
                failure_code: None,
            },
            Err(e) => Self {
                success: false,
                failure_code: Some(e.code()),
            },
        }
    }
}
