//! bundle の manifest（`celeris.script-manifest/1`）の検証と配備（境界 ADR 2026-11-02 (d)）。
//!
//! bundle は Rust に焼き込まない。`dist` の `bundle.mjs` と `manifest.json` を実行時に読み、次を検証する。
//! - manifest に未知の欄・未知の capability が無い（`deny_unknown_fields`）。
//! - `bundle_sha256` が bundle の byte の sha256 と一致する。
//! - `host_api` が `^<major>` の形で、major がこの binary の Host API と一致する。
//! - `schema_sha256` がこの binary の Host API schema（`host-v1.schema.json` と同じ byte）の sha256 と一致する。
//!
//! 配備先は `<state_dir>/script-bundles/<bundle_sha256>/` で、読み込むたびに同じ検証をやり直す。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::dto::{Capability, HOST_API_VERSION, host_api_schema_json};
use crate::handle::ScriptSource;

/// manifest の `schema` 欄の値。
pub const MANIFEST_SCHEMA: &str = "celeris.script-manifest/1";
/// bundle の file 名。
pub const BUNDLE_FILE: &str = "bundle.mjs";
/// manifest の file 名。
pub const MANIFEST_FILE: &str = "manifest.json";
/// 配備先の dir 名（state dir の直下）。
pub const BUNDLES_DIR: &str = "script-bundles";

/// bundle を作った道具の版。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
    pub typescript: String,
    pub esbuild: String,
    pub node: String,
}

/// `celeris.script-manifest/1`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptManifest {
    pub schema: String,
    pub bundle_sha256: String,
    /// 要求する Host API の版。`^<major>` の形だけを受ける。
    pub host_api: String,
    pub schema_sha256: String,
    pub toolchain: Toolchain,
    pub source_commit: String,
    pub domains: Vec<String>,
    pub capabilities: BTreeSet<Capability>,
    /// 引き継げる checkpoint の版（`celeris.script-checkpoint/<n>` の n）。
    pub checkpoint_versions: Vec<u32>,
}

impl ScriptManifest {
    /// 同じ bundle の再配備で食い違ってはいけない部分が同じか。`toolchain`・`source_commit` は来歴なので比べない。
    fn same_contract(&self, other: &Self) -> bool {
        self.host_api == other.host_api
            && self.schema_sha256 == other.schema_sha256
            && self.domains == other.domains
            && self.capabilities == other.capabilities
            && self.checkpoint_versions == other.checkpoint_versions
    }
}

/// manifest の検証・配備の誤り。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot access {path}: {reason}")]
    Io { path: String, reason: String },
    #[error("invalid manifest: {reason}")]
    Invalid { reason: String },
    #[error("unsupported manifest schema {found:?} (expected {MANIFEST_SCHEMA})")]
    UnsupportedSchema { found: String },
    #[error("{field} is not a lowercase sha256 hex: {value:?}")]
    InvalidSha256 { field: &'static str, value: String },
    #[error("host_api must have the form ^<major>: {found:?}")]
    InvalidHostApi { found: String },
    #[error("host_api {required:?} does not match this binary's Host API {host}")]
    HostApiMajorMismatch {
        required: String,
        host: &'static str,
    },
    #[error("schema_sha256 {manifest} does not match this binary's Host API schema {host}")]
    SchemaMismatch { manifest: String, host: String },
    #[error("bundle_sha256 {manifest} does not match the bundle's sha256 {actual}")]
    HashMismatch { manifest: String, actual: String },
    #[error("bundle is not UTF-8")]
    BundleNotUtf8,
    #[error(
        "{path} is already deployed with a different contract (host_api, schema_sha256, domains, capabilities or checkpoint_versions)"
    )]
    DeployConflict { path: String },
}

/// 検証に通った bundle。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBundle {
    pub manifest: ScriptManifest,
    pub code: String,
}

impl VerifiedBundle {
    /// actor に渡す形にする。module 名は bundle の hash から決める。
    pub fn source(&self) -> ScriptSource {
        ScriptSource {
            name: format!("celeris:bundle/{}.mjs", self.manifest.bundle_sha256),
            code: self.code.clone(),
            capabilities: self.manifest.capabilities.clone(),
        }
    }
}

/// byte 列の sha256（小文字の 16 進）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// この binary の Host API schema の sha256。
pub fn host_schema_sha256() -> String {
    sha256_hex(host_api_schema_json().as_bytes())
}

/// manifest と bundle の byte を検証する。file を読まない部分の全体。
pub fn verify_bundle(
    manifest_bytes: &[u8],
    bundle: &[u8],
) -> Result<VerifiedBundle, ManifestError> {
    let manifest: ScriptManifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| ManifestError::Invalid {
            reason: e.to_string(),
        })?;
    if manifest.schema != MANIFEST_SCHEMA {
        return Err(ManifestError::UnsupportedSchema {
            found: manifest.schema,
        });
    }
    check_sha256("bundle_sha256", &manifest.bundle_sha256)?;
    check_sha256("schema_sha256", &manifest.schema_sha256)?;
    check_host_api(&manifest.host_api)?;
    let host = host_schema_sha256();
    if manifest.schema_sha256 != host {
        return Err(ManifestError::SchemaMismatch {
            manifest: manifest.schema_sha256,
            host,
        });
    }
    let actual = sha256_hex(bundle);
    if manifest.bundle_sha256 != actual {
        return Err(ManifestError::HashMismatch {
            manifest: manifest.bundle_sha256,
            actual,
        });
    }
    let code = std::str::from_utf8(bundle)
        .map_err(|_| ManifestError::BundleNotUtf8)?
        .to_owned();
    Ok(VerifiedBundle { manifest, code })
}

/// `dir` の `manifest.json` と `bundle.mjs` を読んで検証する。
pub fn load_bundle_dir(dir: &Path) -> Result<VerifiedBundle, ManifestError> {
    let (manifest, bundle) = read_pair(dir)?;
    verify_bundle(&manifest, &bundle)
}

/// `<state_dir>/script-bundles/<bundle_sha256>/` を読み、配備先の名前と hash の照合を含めて検証する。
pub fn load_deployed(
    state_dir: &Path,
    bundle_sha256: &str,
) -> Result<VerifiedBundle, ManifestError> {
    // 外から来た値を path に使う前に形を確かめる（`../` などで dir の外を読まない）。
    check_sha256("bundle_sha256", bundle_sha256)?;
    let bundle = load_bundle_dir(&deployed_dir(state_dir, bundle_sha256))?;
    if bundle.manifest.bundle_sha256 != bundle_sha256 {
        return Err(ManifestError::HashMismatch {
            manifest: bundle_sha256.to_owned(),
            actual: bundle.manifest.bundle_sha256,
        });
    }
    Ok(bundle)
}

/// 配備先の dir。
pub fn deployed_dir(state_dir: &Path, bundle_sha256: &str) -> PathBuf {
    state_dir.join(BUNDLES_DIR).join(bundle_sha256)
}

/// `dist` の bundle と manifest を検証し、`<state_dir>/script-bundles/<bundle_sha256>/` へ写す。
///
/// 一時 dir に書いて rename するので、配備先は 2 file が揃った状態でだけ現れる。既に在れば検証して再利用する。
/// bundle が同じで `toolchain`・`source_commit` だけが違う再配備は既存を残す（配備先の manifest は最初の来歴のまま）。
/// 契約（[`ScriptManifest::same_contract`]）が違えば [`ManifestError::DeployConflict`]。既存が壊れていれば検証の誤り。
pub fn deploy_bundle(dist: &Path, state_dir: &Path) -> Result<VerifiedBundle, ManifestError> {
    let (manifest_bytes, bundle_bytes) = read_pair(dist)?;
    let verified = verify_bundle(&manifest_bytes, &bundle_bytes)?;
    let sha = verified.manifest.bundle_sha256.as_str();
    let target = deployed_dir(state_dir, sha);
    if target.exists() {
        return reuse_deployed(state_dir, &verified);
    }
    let bundles = state_dir.join(BUNDLES_DIR);
    fs::create_dir_all(&bundles).map_err(io_error(&bundles))?;
    let tmp = bundles.join(format!(".tmp-{}", Ulid::new()));
    let written = write_pair(&tmp, &manifest_bytes, &bundle_bytes)
        .and_then(|()| fs::rename(&tmp, &target).map_err(io_error(&target)));
    match written {
        Ok(()) => {
            sync_path(&bundles)?;
            Ok(verified)
        }
        Err(err) => {
            let _ = fs::remove_dir_all(&tmp);
            // 同時に走った配備が先に rename した。
            if target.exists() {
                reuse_deployed(state_dir, &verified)
            } else {
                Err(err)
            }
        }
    }
}

fn reuse_deployed(state_dir: &Path, new: &VerifiedBundle) -> Result<VerifiedBundle, ManifestError> {
    let sha = &new.manifest.bundle_sha256;
    let existing = load_deployed(state_dir, sha)?;
    if existing.manifest.same_contract(&new.manifest) {
        Ok(existing)
    } else {
        Err(ManifestError::DeployConflict {
            path: deployed_dir(state_dir, sha).display().to_string(),
        })
    }
}

fn read_pair(dir: &Path) -> Result<(Vec<u8>, Vec<u8>), ManifestError> {
    let read = |name: &str| {
        let path = dir.join(name);
        fs::read(&path).map_err(io_error(&path))
    };
    Ok((read(MANIFEST_FILE)?, read(BUNDLE_FILE)?))
}

fn write_pair(dir: &Path, manifest: &[u8], bundle: &[u8]) -> Result<(), ManifestError> {
    fs::create_dir(dir).map_err(io_error(dir))?;
    for (name, bytes) in [(BUNDLE_FILE, bundle), (MANIFEST_FILE, manifest)] {
        let path = dir.join(name);
        fs::write(&path, bytes).map_err(io_error(&path))?;
        sync_path(&path)?;
    }
    sync_path(dir)
}

fn sync_path(path: &Path) -> Result<(), ManifestError> {
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(io_error(path))
}

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> ManifestError {
    let path = path.display().to_string();
    move |e| ManifestError::Io {
        path: path.clone(),
        reason: e.to_string(),
    }
}

fn check_sha256(field: &'static str, value: &str) -> Result<(), ManifestError> {
    let valid = value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if valid {
        Ok(())
    } else {
        Err(ManifestError::InvalidSha256 {
            field,
            value: value.to_owned(),
        })
    }
}

fn check_host_api(required: &str) -> Result<(), ManifestError> {
    let major = required
        .strip_prefix('^')
        .filter(|m| !m.is_empty() && m.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| ManifestError::InvalidHostApi {
            found: required.to_owned(),
        })?;
    if HOST_API_VERSION.split('.').next() == Some(major) {
        Ok(())
    } else {
        Err(ManifestError::HostApiMajorMismatch {
            required: required.to_owned(),
            host: HOST_API_VERSION,
        })
    }
}
