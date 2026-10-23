//! asset store から K2 の初期 tree を組む（bundle → 固定 commit → submodule snapshot tar）。
//! 入れ子 submodule も lock に平らに載っているので、親と同じく `submodules/<末尾名>-<commit 先頭 8 桁>.tar` を読む。
//! snapshot は `SHA256SUMS` に載り、hash が合い、展開結果が lock の内容 hash と一致しなければならない。
//! 欠落・未登録・改変は `env_failure`（asset の不備。モデル失敗ではない）として build の前に止める。
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

use crate::{
    lock::{LockedInitial, LockedSubmodule},
    lock_tree,
};

/// asset の不備。測定の失敗ではなく環境側の失敗として扱う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvFailure(pub String);

impl fmt::Display for EnvFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "env_failure: {}", self.0)
    }
}

impl std::error::Error for EnvFailure {}

fn fail<T>(m: String) -> Result<T, EnvFailure> {
    Err(EnvFailure(m))
}

/// submodule の snapshot tar の file 名（`submodules/` 配下）。
pub fn snapshot_name(s: &LockedSubmodule) -> String {
    let stem = s.path.rsplit('/').next().unwrap_or(&s.path);
    format!("{stem}-{}.tar", &s.commit[..8.min(s.commit.len())])
}

fn parse_sums(assets: &Path) -> Result<BTreeMap<String, String>, EnvFailure> {
    let p = assets.join("SHA256SUMS");
    let text = fs::read_to_string(&p)
        .map_err(|e| EnvFailure(format!("{} unreadable: {e}", p.display())))?;
    let mut m = BTreeMap::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Some((h, f)) = l.split_once(char::is_whitespace) {
            m.insert(f.trim().trim_start_matches('*').to_string(), h.to_string());
        }
    }
    Ok(m)
}

fn sha_file(p: &Path) -> Result<String, EnvFailure> {
    let b = fs::read(p).map_err(|e| EnvFailure(format!("{}: {e}", p.display())))?;
    Ok(format!("{:x}", Sha256::digest(b)))
}

/// lock の全 submodule（入れ子を含む）の snapshot が揃い、SHA256SUMS に載り、hash が合うことを確かめる。
/// build や展開の前に呼ぶ。返すのは (path, snapshot) の組で、親が先になる順。
pub fn check_snapshots(
    lock: &LockedInitial,
    assets: &Path,
) -> Result<Vec<(String, PathBuf)>, EnvFailure> {
    let sums = parse_sums(assets)?;
    let mut subs: Vec<&LockedSubmodule> = lock.submodules.iter().collect();
    subs.sort_by_key(|s| (s.path.matches('/').count(), s.path.clone()));
    let mut names: BTreeMap<String, &str> = BTreeMap::new();
    let mut out = Vec::new();
    for s in subs {
        let name = snapshot_name(s);
        if let Some(other) = names.insert(name.clone(), &s.path) {
            return fail(format!(
                "snapshot name {name} is ambiguous for {} and {other}",
                s.path
            ));
        }
        let tar = assets.join("submodules").join(&name);
        if !tar.is_file() {
            return fail(format!(
                "snapshot for submodule {} missing: submodules/{name}",
                s.path
            ));
        }
        let key = format!("submodules/{name}");
        let Some(want) = sums.get(&key) else {
            return fail(format!(
                "snapshot {key} (submodule {}) is not listed in SHA256SUMS",
                s.path
            ));
        };
        if &sha_file(&tar)? != want {
            return fail(format!(
                "snapshot {key} (submodule {}) differs from SHA256SUMS",
                s.path
            ));
        }
        out.push((s.path.clone(), tar));
    }
    Ok(out)
}

fn run(dir: &Path, prog: &str, args: &[&str]) -> Result<(), EnvFailure> {
    let o = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| EnvFailure(format!("{prog}: {e}")))?;
    if !o.status.success() {
        return fail(format!(
            "{prog} {args:?}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    Ok(())
}

fn utf8(p: &Path) -> Result<&str, EnvFailure> {
    p.to_str()
        .ok_or_else(|| EnvFailure(format!("non-utf8 path {}", p.display())))
}

/// `bundle`（固定 commit を含む）と snapshot から `dest` に初期 tree を組み、lock と一致を確かめる。
/// `dest` は未作成の path（親 dir は存在する）。
pub fn assemble_initial(
    lock: &LockedInitial,
    assets: &Path,
    bundle: &Path,
    dest: &Path,
) -> Result<(), EnvFailure> {
    if !bundle.is_file() {
        return fail(format!("asset missing: {}", bundle.display()));
    }
    let snaps = check_snapshots(lock, assets)?;
    let parent = dest
        .parent()
        .ok_or_else(|| EnvFailure("dest has no parent".into()))?;
    run(parent, "git", &["clone", "-q", utf8(bundle)?, utf8(dest)?])?;
    run(dest, "git", &["checkout", "-q", &lock.commit])?;
    lock_tree::verify_repo(lock, dest)
        .map_err(|e| EnvFailure(format!("bundle does not match the lock: {e}")))?;
    for (path, tar) in snaps {
        let d = dest.join(&path);
        fs::create_dir_all(&d).map_err(|e| EnvFailure(format!("{}: {e}", d.display())))?;
        run(dest, "tar", &["-xf", utf8(&tar)?, "-C", utf8(&d)?])?;
    }
    lock_tree::verify_archive(lock, dest)
        .map_err(|e| EnvFailure(format!("assembled tree does not match the lock: {e}")))
}

/// 固定入力の tgz を `dest` へ展開する（grader の再計算検査用。grader 自身は子 process を起動しない）。
/// `src_dir` の中身（`.`）を gzip tar にする（評価側の固定入力の写し・試験 fixture 用）。
pub fn create_tgz(src_dir: &Path, archive: &Path) -> Result<(), String> {
    let out = std::process::Command::new("tar")
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(src_dir)
        .arg(".")
        .output()
        .map_err(|e| format!("tar: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "tar -czf {}: {}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

pub fn extract_tgz(archive: &Path, dest: &Path) -> Result<(), String> {
    let out = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .output()
        .map_err(|e| format!("tar: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
