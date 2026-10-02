//! ADR-0129 (4)(6): owner の target を repo の seed から reflink で作る。
//!
//! seed は `<scratch>/seeds/<repo-key>/current/{target/,manifest.json}`（作成・更新は seed の更新側）。新しい owner の
//! `target/` が無いとき、互換な seed があれば `cp -a --reflink=auto` で owner の一時ディレクトリへ写し、写した
//! ファイルの extent が共有されている（FIEMAP の `FIEMAP_EXTENT_SHARED`）ことを確かめてから `target/` へ rename する。
//! container の `/local` では `FICLONE` が EPERM でも `copy_file_range(2)` が extent を共有するので、
//! `--reflink=always` の成否は判定に使わない。共有を確かめられない（通常コピーに落ちた・EXDEV・cp の失敗・
//! FIEMAP 非対応）なら一時ディレクトリを消し、空の target から始める（従来の方式）。
//! どちらで作ったかは `<owner>/target-origin.json` とログに残す。**LLM は呼ばない**。

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{CargoTuning, Pool, TARGET_SUBDIR, rfc3339};

pub const SEEDS_DIR: &str = "seeds";
/// `seeds/<repo-key>/current`（世代ディレクトリへの symlink）。
pub const SEED_CURRENT: &str = "current";
pub const SEED_MANIFEST: &str = "manifest.json";
/// owner のディレクトリの中の「どう作ったか」の印。
pub const TARGET_ORIGIN_FILE: &str = "target-origin.json";
pub const TARGET_ORIGIN_SCHEMA: &str = "celeris.scratch-target-origin/1";
/// owner のディレクトリの中の作りかけ（`.seed-copy-<nanos>`）。途中で落ちた残骸は次の割り当てで消す。
pub const SEED_COPY_PREFIX: &str = ".seed-copy-";
/// 共有の判定に使うファイルの最小の大きさ（これより小さいファイルは btrfs の inline extent になりうる）。
pub const SHARE_PROBE_MIN_BYTES: u64 = 64 * 1024;

/// seed の `manifest.json`（ADR-0129 (4): main commit、Cargo/rustc profile、作成日時）。未知の欄は無視する。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// `rustc -V` の 1 行。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustc: Option<String>,
    /// `[scratch.cargo] incremental`（seed を build したときの値）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental: Option<bool>,
    /// `[scratch.cargo] dev_debug`（無し = 空文字列）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_debug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// `seeds/<repo-key>/current`。
pub fn seed_current_dir(pool: &Pool, repo_key: &str) -> PathBuf {
    pool.root()
        .join(SEEDS_DIR)
        .join(repo_key)
        .join(SEED_CURRENT)
}

/// seed の互換の条件（ADR-0129 (4): compiler version、profile、Cargo 設定が違えばコピーしない）。
pub struct SeedPolicy<'a> {
    pub enabled: bool,
    /// `Some` なら manifest の `incremental` / `dev_debug` がこれと一致するときだけ使う。
    pub cargo: Option<CargoTuning>,
    /// `Some` なら manifest の `rustc` がこの関数の値（`rustc -V`）と一致するときだけ使う。seed があるときだけ呼ぶ。
    pub rustc: Option<&'a dyn Fn() -> Option<String>>,
}

impl SeedPolicy<'_> {
    /// 互換の検査をしない（`allocate` の既定。repo key だけ見る）。
    pub fn unchecked() -> Self {
        Self {
            enabled: true,
            cargo: None,
            rustc: None,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            cargo: None,
            rustc: None,
        }
    }
}

/// 写しと共有の判定（試験で差し替える）。
pub struct SeedCopyOps<'a> {
    /// `from`（ディレクトリかファイル）を `to`（まだ無い）へ写す。
    pub copy: &'a dyn Fn(&Path, &Path) -> io::Result<()>,
    /// ファイルの extent が全て共有されているか。
    pub is_shared: &'a dyn Fn(&Path) -> io::Result<bool>,
}

impl SeedCopyOps<'static> {
    /// `cp -a --reflink=auto` と FIEMAP。
    pub fn real() -> Self {
        Self {
            copy: &cp_reflink_auto,
            is_shared: &fiemap_all_shared,
        }
    }
}

/// owner の `target/` をどう作ったか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetOrigin {
    /// 既にあった（割り当て直し）。
    Existing,
    /// 回収可能な owner の target を rename で引き継いだ（ADR-0075 D3）。
    Adopted { from: String },
    /// seed から reflink で写した。
    Seed {
        seed: PathBuf,
        commit: Option<String>,
    },
    /// 空から作った。`reason` は seed を使わなかった理由（seed が無い・無効のときは `None`）。
    Empty { reason: Option<String> },
}

impl TargetOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetOrigin::Existing => "existing",
            TargetOrigin::Adopted { .. } => "adopted",
            TargetOrigin::Seed { .. } => "seed",
            TargetOrigin::Empty { .. } => "empty",
        }
    }
}

/// `target-origin.json`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetOriginRecord {
    pub schema: String,
    /// `seed` / `empty` / `adopted`。
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub created_at: String,
}

impl TargetOriginRecord {
    pub fn new(origin: &TargetOrigin, at: SystemTime) -> Self {
        let mut r = Self {
            schema: TARGET_ORIGIN_SCHEMA.to_string(),
            origin: origin.as_str().to_string(),
            seed: None,
            seed_commit: None,
            adopted_from: None,
            reason: None,
            created_at: rfc3339(at),
        };
        match origin {
            TargetOrigin::Existing => {}
            TargetOrigin::Adopted { from } => r.adopted_from = Some(from.clone()),
            TargetOrigin::Seed { seed, commit } => {
                r.seed = Some(seed.display().to_string());
                r.seed_commit = commit.clone();
            }
            TargetOrigin::Empty { reason } => r.reason = reason.clone(),
        }
        r
    }
}

pub fn read_target_origin(owner_dir: &Path) -> io::Result<Option<TargetOriginRecord>> {
    match std::fs::read(owner_dir.join(TARGET_ORIGIN_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn write_target_origin(owner_dir: &Path, origin: &TargetOrigin) -> io::Result<()> {
    let body = serde_json::to_vec_pretty(&TargetOriginRecord::new(origin, SystemTime::now()))
        .map_err(io::Error::other)?;
    let tmp = owner_dir.join(format!(".{TARGET_ORIGIN_FILE}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, owner_dir.join(TARGET_ORIGIN_FILE))
}

/// owner のディレクトリに残った作りかけ（`.seed-copy-*`）を消す。
pub fn remove_partial_copies(owner_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(owner_dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name()
            .to_string_lossy()
            .starts_with(SEED_COPY_PREFIX)
        {
            let _ = remove_any(&e.path());
        }
    }
}

fn remove_any(path: &Path) -> io::Result<()> {
    let r = match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) => Err(e),
    };
    match r {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn check_manifest(
    manifest: &SeedManifest,
    repo_key: &str,
    policy: &SeedPolicy<'_>,
) -> Result<(), String> {
    if let Some(k) = &manifest.repo_key
        && k != repo_key
    {
        return Err(format!("seed manifest is for repo {k:?}, not {repo_key:?}"));
    }
    if let Some(t) = &policy.cargo {
        let Some(incremental) = manifest.incremental else {
            return Err("seed manifest has no cargo settings".to_string());
        };
        let dev_debug = manifest.dev_debug.clone().filter(|v| !v.is_empty());
        if incremental != t.incremental || dev_debug != t.dev_debug {
            return Err(format!(
                "seed cargo settings (incremental={incremental}, dev_debug={dev_debug:?}) differ from [scratch.cargo] (incremental={}, dev_debug={:?})",
                t.incremental, t.dev_debug
            ));
        }
    }
    if let Some(rustc) = policy.rustc {
        let Some(seed_rustc) = &manifest.rustc else {
            return Err("seed manifest has no rustc version".to_string());
        };
        match rustc() {
            Some(v) if v.trim() == seed_rustc.trim() => {}
            Some(v) => {
                return Err(format!(
                    "seed rustc {seed_rustc:?} differs from {:?}",
                    v.trim()
                ));
            }
            None => return Err("rustc version unknown".to_string()),
        }
    }
    Ok(())
}

/// 一番大きい通常ファイル（symlink は辿らない）。共有の判定に使う。
fn largest_file(dir: &Path) -> Option<(PathBuf, u64)> {
    let mut best: Option<(PathBuf, u64)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(m) = e.path().symlink_metadata() else {
                continue;
            };
            if m.is_dir() {
                stack.push(e.path());
            } else if m.is_file() && best.as_ref().is_none_or(|(_, s)| m.len() > *s) {
                best = Some((e.path(), m.len()));
            }
        }
    }
    best
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// `target/` がまだ無い owner に seed を写す。成功すれば `TargetOrigin::Seed`、使えなければ一時ディレクトリを
/// 消して `TargetOrigin::Empty`（`target/` は作らない。呼び出し側が空で作る）。`.lock` は呼び出し側が持つ。
pub fn seed_target(
    pool: &Pool,
    owner_dir: &Path,
    repo_key: &str,
    policy: &SeedPolicy<'_>,
    ops: &SeedCopyOps<'_>,
) -> TargetOrigin {
    remove_partial_copies(owner_dir);
    if !policy.enabled {
        return TargetOrigin::Empty { reason: None };
    }
    let current = seed_current_dir(pool, repo_key);
    let seed = current.join(TARGET_SUBDIR);
    if !seed.is_dir() {
        return TargetOrigin::Empty { reason: None };
    }
    let empty = |reason: String| TargetOrigin::Empty {
        reason: Some(reason),
    };
    let manifest: SeedManifest = match std::fs::read(current.join(SEED_MANIFEST)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(m) => m,
            Err(e) => return empty(format!("seed manifest is unreadable: {e}")),
        },
        Err(e) => return empty(format!("seed manifest is missing: {e}")),
    };
    if let Err(reason) = check_manifest(&manifest, repo_key, policy) {
        return empty(reason);
    }
    // 世代ディレクトリ（`current` の先）を固定して写す（途中で `current` が切り替わっても混ざらない）。
    let seed = match seed.canonicalize() {
        Ok(p) => p,
        Err(e) => return empty(format!("seed is not readable: {e}")),
    };
    let Some((probe_src, size)) = largest_file(&seed) else {
        return empty("seed has no file to verify extent sharing".to_string());
    };
    if size < SHARE_PROBE_MIN_BYTES {
        return empty(format!(
            "seed has no file of at least {SHARE_PROBE_MIN_BYTES} bytes to verify extent sharing"
        ));
    }
    let tmp = owner_dir.join(format!("{SEED_COPY_PREFIX}{}", nanos()));
    // 先に 1 ファイルだけ写して共有を確かめる（共有できない filesystem で target 全体を実体コピーしない）。
    let probe = tmp.with_extension("probe");
    let shared = (ops.copy)(&probe_src, &probe).and_then(|()| (ops.is_shared)(&probe));
    let _ = remove_any(&probe);
    match shared {
        Ok(true) => {}
        Ok(false) => return empty("reflink is not available (copy did not share extents)".into()),
        Err(e) => return empty(format!("reflink probe failed: {e}")),
    }
    let rel = probe_src.strip_prefix(&seed).map(Path::to_path_buf);
    let result = (ops.copy)(&seed, &tmp).and_then(|()| {
        // 写した target でも共有を確かめる（途中で通常コピーへ落ちていないか）。
        let copied = rel
            .as_ref()
            .map(|r| tmp.join(r))
            .map_err(|_| io::Error::other("probe file outside the seed"))?;
        if !(ops.is_shared)(&copied)? {
            return Err(io::Error::other("copied target did not share extents"));
        }
        std::fs::rename(&tmp, owner_dir.join(TARGET_SUBDIR))
    });
    match result {
        Ok(()) => TargetOrigin::Seed {
            seed,
            commit: manifest.commit,
        },
        Err(e) => {
            if let Err(rm) = remove_any(&tmp) {
                tracing::warn!(tmp = %tmp.display(), error = %rm, "scratch: could not remove a partial seed copy");
            }
            empty(format!("seed copy failed: {e}"))
        }
    }
}

/// `cp -a --reflink=auto -T <from> <to>`（GNU coreutils。FICLONE が失敗すると copy_file_range に落ちる）。
pub fn cp_reflink_auto(from: &Path, to: &Path) -> io::Result<()> {
    let out = Command::new("cp")
        .arg("-a")
        .arg("--reflink=auto")
        .arg("-T")
        .arg(from)
        .arg(to)
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "cp --reflink=auto exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

const FS_IOC_FIEMAP: nix::libc::c_ulong = 0xC020_660B;
const FIEMAP_FLAG_SYNC: u32 = 0x0001;
const FIEMAP_EXTENT_LAST: u32 = 0x0001;
const FIEMAP_EXTENT_SHARED: u32 = 0x2000;
const FIEMAP_BATCH: usize = 64;

/// `struct fiemap_extent`（linux/fiemap.h）。予約欄は kernel だけが読み書きする。
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct FiemapExtent {
    fe_logical: u64,
    fe_physical: u64,
    fe_length: u64,
    fe_reserved64: [u64; 2],
    fe_flags: u32,
    fe_reserved: [u32; 3],
}

/// `struct fiemap`。
#[repr(C)]
#[allow(dead_code)]
struct Fiemap {
    fm_start: u64,
    fm_length: u64,
    fm_flags: u32,
    fm_mapped_extents: u32,
    fm_extent_count: u32,
    fm_reserved: u32,
    fm_extents: [FiemapExtent; FIEMAP_BATCH],
}

/// FIEMAP（`filefrag` と同じ情報）で、ファイルの extent が 1 つ以上あり全て `FIEMAP_EXTENT_SHARED` か。
/// FIEMAP に対応しない filesystem（tmpfs など）はエラー。
pub fn fiemap_all_shared(path: &Path) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path)?;
    let mut start = 0u64;
    let mut seen = 0usize;
    let mut flags = FIEMAP_FLAG_SYNC;
    loop {
        let mut fm = Fiemap {
            fm_start: start,
            fm_length: u64::MAX - start,
            fm_flags: flags,
            fm_mapped_extents: 0,
            fm_extent_count: FIEMAP_BATCH as u32,
            fm_reserved: 0,
            fm_extents: [FiemapExtent::default(); FIEMAP_BATCH],
        };
        // SAFETY: `fm` は struct fiemap と同じ配置で、`fm_extent_count` 個の extent の領域を持つ。fd は有効。
        let rc = unsafe {
            nix::libc::ioctl(file.as_raw_fd(), FS_IOC_FIEMAP as _, &mut fm as *mut Fiemap)
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        flags = 0;
        let n = (fm.fm_mapped_extents as usize).min(FIEMAP_BATCH);
        if n == 0 {
            return Ok(seen > 0);
        }
        for ext in &fm.fm_extents[..n] {
            seen += 1;
            if ext.fe_flags & FIEMAP_EXTENT_SHARED == 0 {
                return Ok(false);
            }
            if ext.fe_flags & FIEMAP_EXTENT_LAST != 0 {
                return Ok(true);
            }
            start = ext.fe_logical.saturating_add(ext.fe_length);
        }
    }
}
