//! ADR-0083 D2-4/5/6: Browser Identity の state の封緘・開封・鍵消去。
//!
//! 鍵は project+origin 単位（`browser_identity::key_label`）。label ごとに 32 byte の乱数
//! （鍵素材）を `key_dir` に 0600 で置き、AEAD の鍵は鍵素材と label から SHA-256 で導出する。
//! AAD は `browser_identity::aad`（identity・project・origin・世代）。開封は
//! `check_envelope` と `check_state_origins`（foreign_origin 等）を通った state だけを返す。
//! agent-browser の restore には依存しない（ADR-0083 D4）。平文・鍵・cookie の値は
//! Debug にもエラーにも出さない。

use super::{Error, check_file, ensure_dir, random, read_private, write_atomic};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use task_core::browser_identity::{self as bi, BrowserIdentity, IdentityDenied, SealedEnvelope};
use zeroize::{Zeroize, Zeroizing};

const DERIVE_PREFIX: &[u8] = b"celeris-identity-seal-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealError {
    /// task-core の規則が拒否した（別 identity・旧世代・別 origin・foreign_origin 等）。
    Denied(IdentityDenied),
    /// 鍵が無い（消去済み）。
    KeyErased,
    /// AEAD の検証に失敗した（AAD・暗号文の改変、鍵の不一致）。
    Tampered,
    Broker(Error),
}
impl SealError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Denied(d) => d.code(),
            Self::KeyErased => "key_erased",
            Self::Tampered => "seal_tampered",
            Self::Broker(e) => e.code(),
        }
    }
}
impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for SealError {}
impl From<Error> for SealError {
    fn from(e: Error) -> Self {
        Self::Broker(e)
    }
}
impl From<IdentityDenied> for SealError {
    fn from(d: IdentityDenied) -> Self {
        Self::Denied(d)
    }
}

/// state の 1 項目（cookie / storage）。値は Debug に出さない。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateEntry {
    pub origin: String,
    pub kind: String,
    pub name: String,
    pub value: String,
}
impl std::fmt::Debug for StateEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateEntry")
            .field("origin", &self.origin)
            .field("kind", &self.kind)
            .field("name", &"<redacted>")
            .field("value", &"<redacted>")
            .finish()
    }
}
impl Drop for StateEntry {
    fn drop(&mut self) {
        self.name.zeroize();
        self.value.zeroize();
    }
}

/// 封緘する browser state の平文。
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IdentityStatePlain {
    pub entries: Vec<StateEntry>,
}
impl std::fmt::Debug for IdentityStatePlain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IdentityStatePlain {{ entries: {} <redacted> }}",
            self.entries.len()
        )
    }
}
impl IdentityStatePlain {
    fn origins(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.origin.clone()).collect()
    }
}

/// 封緘済みの state。外側（envelope）は平文、中身は暗号文。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedIdentityState {
    pub envelope: SealedEnvelope,
    pub nonce: String,
    pub ciphertext: String,
}
impl std::fmt::Debug for SealedIdentityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedIdentityState")
            .field("envelope", &self.envelope)
            .field("ciphertext_len", &(self.ciphertext.len() / 2))
            .finish()
    }
}

/// project+origin 鍵の保管と AEAD。鍵素材はメモリに持ち続けない（使うたびに読む）。
pub struct IdentitySealer {
    key_dir: PathBuf,
    /// `open_state` が呼ばれた回数（ADR-0088 D5: 拒否経路で開封しないことの観測用。秘密は含まない）。
    open_attempts: std::sync::atomic::AtomicU64,
}
impl std::fmt::Debug for IdentitySealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentitySealer").finish_non_exhaustive()
    }
}

fn label_file(key_dir: &Path, label: &str) -> Result<PathBuf, Error> {
    // label は "prefix:<64 hex>"。ファイル名には hex だけを使う。
    let hex_part = label.rsplit(':').next().ok_or(Error::Invalid)?;
    if hex_part.len() != 64 || !hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Invalid);
    }
    Ok(key_dir.join(format!("{hex_part}.key")))
}

fn derive(material: &[u8], label: &str) -> Zeroizing<[u8; 32]> {
    let mut h = Sha256::new();
    h.update(DERIVE_PREFIX);
    h.update([0u8]);
    h.update(material);
    h.update([0u8]);
    h.update(label.as_bytes());
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&h.finalize());
    out
}

impl IdentitySealer {
    pub fn open(key_dir: PathBuf) -> Result<Self, Error> {
        ensure_dir(&key_dir)?;
        Ok(Self {
            key_dir,
            open_attempts: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn key(&self, label: &str, create: bool) -> Result<Zeroizing<[u8; 32]>, SealError> {
        let path = label_file(&self.key_dir, label)?;
        if !path.exists() {
            if !create {
                return Err(SealError::KeyErased);
            }
            let material = Zeroizing::new(random::<32>()?);
            write_atomic(&self.key_dir, &path, material.as_ref())?;
        }
        let material = Zeroizing::new(read_private(&path)?);
        if material.len() != 32 {
            return Err(SealError::Broker(Error::Invalid));
        }
        Ok(derive(&material, label))
    }

    /// state を封緘する。foreign origin を含む state は封緘しない。
    pub fn seal(
        &self,
        identity: &BrowserIdentity,
        state: &IdentityStatePlain,
        now: u64,
    ) -> Result<SealedIdentityState, SealError> {
        let envelope = bi::envelope_for(identity);
        bi::check_envelope(identity, &envelope, now)?;
        bi::check_state_origins(identity, &state.origins())?;
        let key = self.key(&envelope.key_label, true)?;
        let nonce = random::<24>()?;
        let plain = Zeroizing::new(
            serde_json::to_vec(state).map_err(|_| SealError::Broker(Error::Invalid))?,
        );
        let aad = bi::aad(identity);
        let ciphertext = XChaCha20Poly1305::new(key.as_ref().into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: &aad,
                },
            )
            .map_err(|_| SealError::Broker(Error::Io))?;
        Ok(SealedIdentityState {
            envelope,
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
        })
    }

    /// `open_state` が呼ばれた回数（成否を問わない）。
    pub fn open_attempts(&self) -> u64 {
        self.open_attempts.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 封緘を開く。envelope の束縛・AEAD・state の origin の全てを通ったときだけ返す。
    pub fn open_state(
        &self,
        identity: &BrowserIdentity,
        sealed: &SealedIdentityState,
        now: u64,
    ) -> Result<IdentityStatePlain, SealError> {
        self.open_attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        bi::check_envelope(identity, &sealed.envelope, now)?;
        let key = self.key(&sealed.envelope.key_label, false)?;
        let nonce = hex::decode(&sealed.nonce).map_err(|_| SealError::Tampered)?;
        if nonce.len() != 24 {
            return Err(SealError::Tampered);
        }
        let ciphertext = hex::decode(&sealed.ciphertext).map_err(|_| SealError::Tampered)?;
        let aad = bi::aad(identity);
        let plain = Zeroizing::new(
            XChaCha20Poly1305::new(key.as_ref().into())
                .decrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| SealError::Tampered)?,
        );
        let state: IdentityStatePlain =
            serde_json::from_slice(&plain).map_err(|_| SealError::Tampered)?;
        bi::check_state_origins(identity, &state.origins())?;
        Ok(state)
    }

    /// project+origin の鍵を消去する（上書きしてから削除）。以後その label の封緘は開けない。
    /// 消したら true、元から無ければ false。
    pub fn erase_key(&self, project_id: &str, origin: &str) -> Result<bool, Error> {
        let label = bi::key_label(project_id, origin);
        let path = label_file(&self.key_dir, &label)?;
        if !path.exists() {
            return Ok(false);
        }
        check_file(&path)?;
        let mut f = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|_| Error::Io)?;
        f.write_all(&[0u8; 32])
            .and_then(|_| f.sync_all())
            .map_err(|_| Error::Io)?;
        drop(f);
        fs::remove_file(&path).map_err(|_| Error::Io)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_core::browser_identity::{IdentityRegistration, register};

    const NOW: u64 = 1_000_000;

    fn ident(id: &str, project: &str, origin: &str) -> BrowserIdentity {
        match register(
            &IdentityRegistration {
                identity_id: id.into(),
                project_id: project.into(),
                origin: origin.into(),
                demand_confirmed_by: Some("human:rmaeda".into()),
                ttl_secs: 0,
            },
            NOW,
        ) {
            Ok(i) => i,
            Err(e) => panic!("register: {e:?}"),
        }
    }

    fn state(origin: &str) -> IdentityStatePlain {
        IdentityStatePlain {
            entries: vec![StateEntry {
                origin: origin.into(),
                kind: "cookie".into(),
                name: "session".into(),
                value: "SECRET-COOKIE-VALUE-42".into(),
            }],
        }
    }

    fn sealer() -> (tempfile::TempDir, IdentitySealer) {
        let dir = tempfile::tempdir().unwrap();
        let s = IdentitySealer::open(dir.path().join("identity-keys")).unwrap();
        (dir, s)
    }

    #[test]
    fn round_trip() {
        let (_d, s) = sealer();
        let id = ident("id-a", "proj", "https://example.com");
        let sealed = s.seal(&id, &state("https://example.com"), NOW).unwrap();
        assert!(
            !sealed
                .ciphertext
                .contains(&hex::encode("SECRET-COOKIE-VALUE-42"))
        );
        let opened = s.open_state(&id, &sealed, NOW + 1).unwrap();
        assert_eq!(opened, state("https://example.com"));
    }

    #[test]
    fn other_identity_same_key_cannot_open() {
        let (_d, s) = sealer();
        let a = ident("id-a", "proj", "https://example.com");
        let b = ident("id-b", "proj", "https://example.com");
        let sealed = s.seal(&a, &state("https://example.com"), NOW).unwrap();
        // 外側の検査で拒否される。
        assert_eq!(
            s.open_state(&b, &sealed, NOW),
            Err(SealError::Denied(IdentityDenied::OtherIdentity))
        );
        // envelope を書き換えて外側を通しても、AAD の不一致で開けない（鍵は同じ label）。
        let mut forged = sealed.clone();
        forged.envelope = bi::envelope_for(&b);
        assert_eq!(s.open_state(&b, &forged, NOW), Err(SealError::Tampered));
    }

    #[test]
    fn stale_generation_cannot_open() {
        let (_d, s) = sealer();
        let id = ident("id-a", "proj", "https://example.com");
        let sealed = s.seal(&id, &state("https://example.com"), NOW).unwrap();
        let mut revoked = id.clone();
        assert!(bi::revoke(&mut revoked));
        assert_eq!(
            s.open_state(&revoked, &sealed, NOW),
            Err(SealError::Denied(IdentityDenied::Revoked))
        );
        // 世代だけ進んだ active の identity: 外側は旧世代で拒否、envelope を合わせても AAD で拒否。
        let mut next = id.clone();
        next.generation += 1;
        assert_eq!(
            s.open_state(&next, &sealed, NOW),
            Err(SealError::Denied(IdentityDenied::StaleGeneration))
        );
        let mut forged = sealed.clone();
        forged.envelope.generation = next.generation;
        assert_eq!(s.open_state(&next, &forged, NOW), Err(SealError::Tampered));
    }

    #[test]
    fn other_origin_cannot_open() {
        let (_d, s) = sealer();
        let a = ident("id-a", "proj", "https://example.com");
        let other = ident("id-a", "proj", "https://evil.example.org");
        let sealed = s.seal(&a, &state("https://example.com"), NOW).unwrap();
        assert_eq!(
            s.open_state(&other, &sealed, NOW),
            Err(SealError::Denied(IdentityDenied::OtherOrigin))
        );
        // envelope を別 origin のものに差し替えると、別の鍵（未作成）になる。
        let mut forged = sealed.clone();
        forged.envelope = bi::envelope_for(&other);
        assert_eq!(
            s.open_state(&other, &forged, NOW),
            Err(SealError::KeyErased)
        );
        // 別 origin の鍵が存在しても開けない。
        s.seal(&other, &state("https://evil.example.org"), NOW)
            .unwrap();
        assert_eq!(s.open_state(&other, &forged, NOW), Err(SealError::Tampered));
    }

    #[test]
    fn tampered_aad_or_ciphertext_rejected() {
        let (_d, s) = sealer();
        let id = ident("id-a", "proj", "https://example.com");
        let sealed = s.seal(&id, &state("https://example.com"), NOW).unwrap();
        // AAD の元になる identity を改変（demand 以外の束縛項目は外側検査と整合させる）。
        let mut renamed = id.clone();
        renamed.identity_id = "id-z".into();
        let mut forged = sealed.clone();
        forged.envelope.identity_id = "id-z".into();
        assert_eq!(
            s.open_state(&renamed, &forged, NOW),
            Err(SealError::Tampered)
        );
        // 暗号文の 1 byte 改変。
        let mut ct = hex::decode(&sealed.ciphertext).unwrap();
        ct[0] ^= 1;
        let mut flipped = sealed.clone();
        flipped.ciphertext = hex::encode(ct);
        assert_eq!(s.open_state(&id, &flipped, NOW), Err(SealError::Tampered));
    }

    #[test]
    fn foreign_origin_state_rejected() {
        let (_d, s) = sealer();
        let id = ident("id-a", "proj", "https://example.com");
        let mut mixed = state("https://example.com");
        mixed.entries.push(StateEntry {
            origin: "https://tracker.example.net".into(),
            kind: "cookie".into(),
            name: "t".into(),
            value: "v".into(),
        });
        assert_eq!(
            s.seal(&id, &mixed, NOW),
            Err(SealError::Denied(IdentityDenied::ForeignOrigin))
        );
        assert_eq!(
            s.seal(&id, &IdentityStatePlain::default(), NOW),
            Err(SealError::Denied(IdentityDenied::EmptyState))
        );
    }

    #[test]
    fn erased_key_cannot_open() {
        let (d, s) = sealer();
        let id = ident("id-a", "proj", "https://example.com");
        let sealed = s.seal(&id, &state("https://example.com"), NOW).unwrap();
        let key_path =
            label_file(&d.path().join("identity-keys"), &sealed.envelope.key_label).unwrap();
        assert!(key_path.exists());
        assert!(s.erase_key("proj", "https://example.com").unwrap());
        assert!(!key_path.exists());
        assert_eq!(s.open_state(&id, &sealed, NOW), Err(SealError::KeyErased));
        assert!(!s.erase_key("proj", "https://example.com").unwrap());
        // 新しい鍵で封緘し直しても、古い封緘は開けない。
        s.seal(&id, &state("https://example.com"), NOW).unwrap();
        assert_eq!(s.open_state(&id, &sealed, NOW), Err(SealError::Tampered));
    }

    #[test]
    fn debug_and_errors_do_not_leak_secrets() {
        let (d, s) = sealer();
        let id = ident("id-a", "proj", "https://example.com");
        let plain = state("https://example.com");
        let sealed = s.seal(&id, &plain, NOW).unwrap();
        let key_path =
            label_file(&d.path().join("identity-keys"), &sealed.envelope.key_label).unwrap();
        let material = fs::read(&key_path).unwrap();
        let derived = derive(&material, &sealed.envelope.key_label);
        let opened = s.open_state(&id, &sealed, NOW).unwrap();
        let err = s
            .open_state(&ident("id-b", "proj", "https://example.com"), &sealed, NOW)
            .unwrap_err();
        let texts = [
            format!("{plain:?}"),
            format!("{opened:?}"),
            format!("{:?}", plain.entries[0]),
            format!("{sealed:?}"),
            format!("{s:?}"),
            format!("{err:?}"),
            err.to_string(),
        ];
        let secrets = [
            "SECRET-COOKIE-VALUE-42".to_string(),
            "session".to_string(),
            hex::encode(&material),
            hex::encode(derived.as_ref()),
            sealed.ciphertext.clone(),
        ];
        for t in &texts {
            for secret in &secrets {
                assert!(!t.contains(secret.as_str()), "leak in {t}");
            }
        }
    }
}
