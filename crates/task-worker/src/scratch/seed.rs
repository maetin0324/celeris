//! ADR-0129 (4)(5): repo の seed の更新と、seed の GC。
//!
//! 置き場は `<scratch>/seeds/<repo-key>/`:
//! - `gen-<commit12>-<nanos>/{target/,manifest.json}` … 世代（build に成功したものだけ）
//! - `current` … 世代への相対 symlink。一時 symlink の rename で原子的に切り替える
//! - `.building-<gen>/` … build 中の世代（成功すれば `gen-*` へ rename、失敗すれば消す）
//! - `checkout/` … main の固定 commit を build する専用の安定した checkout（`git worktree --detach`）
//! - `.refresh.lock` … 同じ repo の更新・GC を排他する POSIX record lock（`claim_seed`。GC は取れたときだけ触る）
//! - `.unregistered-since` … repo が登録から外れたのを最初に見た時刻（mtime）。猶予を過ぎたら seed 全体を消す
//!
//! seed は lease 付きの owner ではない（`targets/` の外）ので、ADR-0075 D2 の semantic GC（P0〜P3・warm seed）
//! の対象にならない。ここの GC は「current 以外の世代」「放棄された `.building-*`」「猶予を過ぎた登録外の repo」
//! だけを消す。消す・切り替えるは pool の `.lock` の下で行う（owner への写しも `.lock` の下なので、写している
//! 最中の世代は消えない）。**LLM は呼ばない**（seed の build は cargo を走らせるだけ）。

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use super::{
    CARGO_TARGET_DIR_VAR, CargoTuning, Pool, SEED_CURRENT, SEED_MANIFEST, SEEDS_DIR, SeedManifest,
    TARGET_SUBDIR, cargo_tuning_env, deleting_name, measure_tree, mtime, rfc3339, set_mtime,
};

/// 世代ディレクトリの接頭辞。
pub const SEED_GEN_PREFIX: &str = "gen-";
/// build 中の世代の接頭辞（`.building-gen-*`）。
pub const SEED_BUILDING_PREFIX: &str = ".building-";
/// seed を build する専用の checkout。
pub const SEED_CHECKOUT_DIR: &str = "checkout";
pub const SEED_REFRESH_LOCK: &str = ".refresh.lock";
/// 登録から外れたのを最初に見た時刻の印（mtime が時刻）。
pub const SEED_UNREGISTERED_MARK: &str = ".unregistered-since";
/// 登録から外れた repo の seed を消すまでの猶予（ADR-0129 (5)。人が確かめられるよう 7 日）。
pub const SEED_UNREGISTERED_GRACE_SECS: u64 = 7 * 86400;
/// dispatcher が main の前進を確かめる間隔（ADR-0129 (4)。起動直後〈＝昇格の後〉は待たずに確かめる）。
pub const SEED_CHECK_INTERVAL_SECS: u64 = 300;

/// `<scratch>/seeds`。
pub fn seeds_dir(pool: &Pool) -> PathBuf {
    pool.root().join(SEEDS_DIR)
}

/// `<scratch>/seeds/<repo-key>`。
pub fn seed_repo_dir(pool: &Pool, repo_key: &str) -> PathBuf {
    seeds_dir(pool).join(repo_key)
}

/// `current` が指す世代の名前（`gen-*`）。無い・壊れているなら `None`。
pub fn current_generation(pool: &Pool, repo_key: &str) -> Option<String> {
    let link = seed_repo_dir(pool, repo_key).join(SEED_CURRENT);
    let target = std::fs::read_link(&link).ok()?;
    let name = target.file_name()?.to_string_lossy().into_owned();
    seed_repo_dir(pool, repo_key)
        .join(&name)
        .is_dir()
        .then_some(name)
}

/// 今の seed の manifest。
pub fn read_current_manifest(pool: &Pool, repo_key: &str) -> Option<SeedManifest> {
    let generation = current_generation(pool, repo_key)?;
    let bytes = std::fs::read(
        seed_repo_dir(pool, repo_key)
            .join(generation)
            .join(SEED_MANIFEST),
    )
    .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 更新が要るか（ADR-0129 (4): main が進んだ・rustc や `[scratch.cargo]` が変わった・seed が無い）。
/// 要るなら理由を返す。`rustc` が `None`（分からない）なら版の比較はしない。
pub fn seed_refresh_reason(
    manifest: Option<&SeedManifest>,
    head_commit: &str,
    cargo: &CargoTuning,
    rustc: Option<&str>,
) -> Option<String> {
    let Some(m) = manifest else {
        return Some("no seed yet".to_string());
    };
    if m.commit.as_deref() != Some(head_commit) {
        return Some(format!(
            "main moved from {} to {head_commit}",
            m.commit.as_deref().unwrap_or("?")
        ));
    }
    let dev_debug = m.dev_debug.clone().filter(|v| !v.is_empty());
    if m.incremental != Some(cargo.incremental) || dev_debug != cargo.dev_debug {
        return Some("[scratch.cargo] changed".to_string());
    }
    if let Some(v) = rustc
        && m.rustc.as_deref().map(str::trim) != Some(v.trim())
    {
        return Some(format!("rustc changed to {:?}", v.trim()));
    }
    None
}

/// ADR-0129 (5): 更新を保留するか。seed の build は owner と extent を共有しない実体を書くので、空きが
/// `min_free + 今の seed の大きさ（無ければ 0）` を下回る、または pool が watermark を超えているなら保留する。
pub fn seed_refresh_hold(
    pressure_high: bool,
    fs_free: Option<u64>,
    min_free_bytes: u64,
    current_seed_bytes: Option<u64>,
) -> Option<String> {
    if pressure_high {
        return Some("scratch pool is above the watermark".to_string());
    }
    let need = min_free_bytes.saturating_add(current_seed_bytes.unwrap_or(0));
    match fs_free {
        Some(free) if free < need => Some(format!(
            "free space {free} bytes is below {need} bytes (min_free + current seed)"
        )),
        _ => None,
    }
}

/// `last` から `interval` 経ったか（`None` = まだ一度も見ていない＝起動直後）。
pub fn seed_check_due(
    last: Option<std::time::Instant>,
    now: std::time::Instant,
    interval: Duration,
) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= interval)
}

/// seed の build（`checkout` と env を受ける）。
pub type SeedBuildFn = dyn Fn(&Path, &[(String, String)]) -> io::Result<()>;

/// seed の build の手順（試験で差し替える）。
pub struct SeedBuildOps<'a> {
    /// 専用の checkout（`checkout`）を `repo` の `commit` にする。
    pub prepare_checkout: &'a dyn Fn(&Path, &Path, &str) -> io::Result<()>,
    /// `checkout` で build する（env に `CARGO_TARGET_DIR` と `[scratch.cargo]`）。
    pub build: &'a SeedBuildFn,
    /// `checkout` の `rustc -V`。
    pub rustc: &'a dyn Fn(&Path) -> Option<String>,
}

impl SeedBuildOps<'static> {
    /// `git worktree` と `cargo build --workspace --all-targets`。
    pub fn real() -> Self {
        Self {
            prepare_checkout: &git_detached_checkout,
            build: &cargo_build_all_targets,
            rustc: &rustc_version,
        }
    }
}

/// 更新の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedRefreshOutcome {
    /// 新しい世代に切り替えた。`replaced` は前の世代。
    Refreshed {
        generation: String,
        commit: String,
        replaced: Option<String>,
    },
    /// 他が更新中・GC 中（`claim_seed` が取れない）。
    Busy,
    /// 失敗（旧 seed はそのまま）。
    Failed { reason: String },
}

/// 同じ process の中で更新中・GC 中の repo（`seeds/<key>`）。
static SEED_CLAIMS: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());

/// repo の seed を触る権利（更新・GC）。同じ process の中は `SEED_CLAIMS`、process の間（handoff 中の新旧 daemon
/// など）は `.refresh.lock` の POSIX record lock（`F_SETLK`）で排他する。flock と違い record lock は fork した子に
/// 引き継がれないので、並行して子を起こす daemon でも、解放した直後に「誰かが持っている」と誤らない。
pub struct SeedClaim {
    repo_dir: PathBuf,
    _file: std::fs::File,
}

impl Drop for SeedClaim {
    fn drop(&mut self) {
        SEED_CLAIMS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.repo_dir);
    }
}

fn posix_try_lock(file: &std::fs::File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: `flock` は C の構造体で、全て 0 は有効な値。
    let mut fl: nix::libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = nix::libc::F_WRLCK as _;
    fl.l_whence = nix::libc::SEEK_SET as _;
    // SAFETY: fd は有効で、`fl` は F_SETLK の引数の形。
    let rc = unsafe { nix::libc::fcntl(file.as_raw_fd(), nix::libc::F_SETLK, &fl) };
    if rc == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(nix::libc::EACCES) | Some(nix::libc::EAGAIN) => Ok(false),
        _ => Err(e),
    }
}

/// `repo_dir` の seed を触る権利を取る。他が持っていれば `None`（待たない）。
pub fn claim_seed(repo_dir: &Path) -> io::Result<Option<SeedClaim>> {
    let mut claims = SEED_CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    if claims.contains(repo_dir) {
        return Ok(None);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(repo_dir.join(SEED_REFRESH_LOCK))?;
    if !posix_try_lock(&file)? {
        return Ok(None);
    }
    claims.insert(repo_dir.to_path_buf());
    Ok(Some(SeedClaim {
        repo_dir: repo_dir.to_path_buf(),
        _file: file,
    }))
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

/// `seeds/<key>/current` を `generation` へ原子的に向ける（一時 symlink を作って rename）。
pub fn switch_current(repo_dir: &Path, generation: &str, now: SystemTime) -> io::Result<()> {
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = repo_dir.join(format!(".current.tmp-{nanos}-{}", std::process::id()));
    let _ = remove_any(&tmp);
    std::os::unix::fs::symlink(generation, &tmp)?;
    std::fs::rename(&tmp, repo_dir.join(SEED_CURRENT)).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// ADR-0129 (4): `repo` の `commit` を専用の checkout で build し、成功したら新しい世代に `current` を切り替える。
/// 失敗・中断では旧 seed を残す。切り替えと旧世代の退避は pool の `.lock` の下（owner への写しと直列）。
/// 長い build は lock の外。`now` は世代名と manifest の時刻。
pub fn refresh_seed(
    pool: &Pool,
    repo_path: &Path,
    commit: &str,
    cargo: &CargoTuning,
    ops: &SeedBuildOps<'_>,
    now: SystemTime,
) -> SeedRefreshOutcome {
    let failed = |reason: String| SeedRefreshOutcome::Failed { reason };
    let repo_key = crate::build_cache::repo_cache_key(repo_path);
    let repo_dir = seed_repo_dir(pool, &repo_key);
    if let Err(e) = std::fs::create_dir_all(&repo_dir) {
        return failed(format!("cannot create {}: {e}", repo_dir.display()));
    }
    let _claim = match claim_seed(&repo_dir) {
        Ok(Some(f)) => f,
        Ok(None) => return SeedRefreshOutcome::Busy,
        Err(e) => return failed(format!("cannot take the refresh lock: {e}")),
    };
    // 前回の中断の残骸（lock を取れた＝誰も build していない）。
    remove_building(&repo_dir);
    let short: String = commit.chars().take(12).collect();
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let generation = format!("{SEED_GEN_PREFIX}{short}-{nanos}");
    let building = repo_dir.join(format!("{SEED_BUILDING_PREFIX}{generation}"));
    let checkout = repo_dir.join(SEED_CHECKOUT_DIR);
    let result = (|| -> Result<SeedManifest, String> {
        (ops.prepare_checkout)(repo_path, &checkout, commit)
            .map_err(|e| format!("checkout of {commit} failed: {e}"))?;
        let rustc = (ops.rustc)(&checkout);
        std::fs::create_dir_all(&building).map_err(|e| e.to_string())?;
        let target = building.join(TARGET_SUBDIR);
        let mut env = vec![(
            CARGO_TARGET_DIR_VAR.to_string(),
            target.display().to_string(),
        )];
        env.extend(cargo_tuning_env(cargo));
        (ops.build)(&checkout, &env).map_err(|e| format!("seed build failed: {e}"))?;
        if !target.is_dir() {
            return Err("seed build produced no target directory".to_string());
        }
        let manifest = SeedManifest {
            repo_key: Some(repo_key.clone()),
            commit: Some(commit.to_string()),
            rustc,
            incremental: Some(cargo.incremental),
            dev_debug: Some(cargo.dev_debug.clone().unwrap_or_default()),
            created_at: Some(rfc3339(now)),
            size_bytes: measure_tree(&target).ok().map(|(b, _)| b),
        };
        let body = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(building.join(SEED_MANIFEST), body).map_err(|e| e.to_string())?;
        Ok(manifest)
    })();
    if let Err(reason) = result {
        let _ = remove_any(&building);
        return failed(reason);
    }
    let lock = match pool.lock() {
        Ok(l) => l,
        Err(e) => {
            let _ = remove_any(&building);
            return failed(format!("cannot take the pool lock: {e}"));
        }
    };
    let replaced = current_generation(pool, &repo_key);
    let switched = std::fs::rename(&building, repo_dir.join(&generation))
        .and_then(|()| switch_current(&repo_dir, &generation, now));
    if let Err(e) = switched {
        let _ = remove_any(&building);
        let _ = remove_any(&repo_dir.join(&generation));
        return failed(format!("cannot switch the seed: {e}"));
    }
    // 旧世代は lock の下で `.deleting-*` へ退避する（写している最中の owner は lock を持つので、ここで消えない）。
    let trash = retire_stale_generations(pool, &repo_dir, now);
    drop(lock);
    for t in trash {
        let _ = remove_any(&t);
    }
    SeedRefreshOutcome::Refreshed {
        generation,
        commit: commit.to_string(),
        replaced,
    }
}

fn remove_building(repo_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(repo_dir) else {
        return;
    };
    for e in rd.flatten() {
        if e.file_name()
            .to_string_lossy()
            .starts_with(SEED_BUILDING_PREFIX)
        {
            let _ = remove_any(&e.path());
        }
    }
}

/// `current` 以外の世代を `seeds/.deleting-*` へ rename する（pool の `.lock` を持って呼ぶ）。退避先を返す。
fn retire_stale_generations(pool: &Pool, repo_dir: &Path, now: SystemTime) -> Vec<PathBuf> {
    let key = repo_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let current = current_generation(pool, &key);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(repo_dir) else {
        return out;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(SEED_GEN_PREFIX))
        .collect();
    names.sort();
    for name in names {
        if current.as_deref() == Some(name.as_str()) {
            continue;
        }
        let trash = seeds_dir(pool).join(deleting_name(&format!("seed-{key}-{name}"), now));
        match std::fs::rename(repo_dir.join(&name), &trash) {
            Ok(()) => {
                tracing::info!(repo_key = %key, generation = %name, "scratch: retiring an old seed generation (ADR-0129 (5))");
                out.push(trash);
            }
            Err(e) => {
                tracing::warn!(repo_key = %key, generation = %name, error = %e, "scratch: could not move an old seed aside")
            }
        }
    }
    out
}

/// seed の GC の 1 回の結果（退避したもの）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedGcResult {
    /// 退避した（`seeds/.deleting-*` へ rename した）パス。中身の削除は削除スレッド。
    pub retired: Vec<PathBuf>,
    /// 消した放棄 `.building-*`。
    pub abandoned_builds: Vec<PathBuf>,
    /// 今回 `.unregistered-since` を書いた repo key。
    pub marked_unregistered: Vec<String>,
    /// 猶予を過ぎて seed 全体を退避した repo key。
    pub removed_repos: Vec<String>,
}

/// ADR-0129 (5): seed の GC（tick から呼ぶ。木は辿らず rename だけ）。`registered` は登録されている repo の key。
/// - `current` が指す世代は消さない（容量の圧迫でも黙って失わせない）
/// - `current` 以外の世代は消す（旧 commit の永久保存はしない）
/// - `.building-*` と旧世代は `claim_seed` が取れる（誰も build していない）ときだけ消す
/// - 登録外の repo は `.unregistered-since` を書き、`SEED_UNREGISTERED_GRACE_SECS` を過ぎたら seed 全体を消す
///   （登録に戻れば印を消す）
pub fn seed_gc(pool: &Pool, registered: &BTreeSet<String>, now: SystemTime) -> SeedGcResult {
    let mut out = SeedGcResult::default();
    let root = seeds_dir(pool);
    let Ok(rd) = std::fs::read_dir(&root) else {
        return out;
    };
    let mut keys: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    keys.sort();
    let _lock = match pool.lock() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(dir = %pool.root().display(), error = %e, "scratch: could not take the pool lock; seed GC skipped");
            return out;
        }
    };
    for key in keys {
        let repo_dir = root.join(&key);
        let refresh = claim_seed(&repo_dir).ok().flatten();
        let mark = repo_dir.join(SEED_UNREGISTERED_MARK);
        if registered.contains(&key) {
            let _ = remove_any(&mark);
        } else if refresh.is_some() {
            match mtime(&mark) {
                None => {
                    if std::fs::write(&mark, b"")
                        .and_then(|()| set_mtime(&mark, now))
                        .is_ok()
                    {
                        tracing::warn!(repo_key = %key, grace_secs = SEED_UNREGISTERED_GRACE_SECS, "scratch: the seed's repo is no longer registered; it will be removed after the grace period (ADR-0129 (5))");
                        out.marked_unregistered.push(key.clone());
                    }
                }
                Some(since)
                    if now.duration_since(since).unwrap_or_default()
                        >= Duration::from_secs(SEED_UNREGISTERED_GRACE_SECS) =>
                {
                    let trash = root.join(deleting_name(&format!("seed-{key}"), now));
                    drop(refresh);
                    match std::fs::rename(&repo_dir, &trash) {
                        Ok(()) => {
                            tracing::info!(repo_key = %key, "scratch: removing the seed of an unregistered repo (ADR-0129 (5))");
                            out.retired.push(trash);
                            out.removed_repos.push(key.clone());
                        }
                        Err(e) => {
                            tracing::warn!(repo_key = %key, error = %e, "scratch: could not move an unregistered seed aside")
                        }
                    }
                    continue;
                }
                Some(_) => {}
            }
        }
        if refresh.is_some() {
            let Ok(rd) = std::fs::read_dir(&repo_dir) else {
                continue;
            };
            for e in rd.flatten() {
                if e.file_name()
                    .to_string_lossy()
                    .starts_with(SEED_BUILDING_PREFIX)
                    && remove_any(&e.path()).is_ok()
                {
                    out.abandoned_builds.push(e.path());
                }
            }
            out.retired
                .extend(retire_stale_generations(pool, &repo_dir, now));
        }
    }
    out
}

/// 退避した seed（`seeds/.deleting-*`）の根（削除スレッドの根に足す）。
pub fn seed_deleting_root(pool: &Pool) -> PathBuf {
    seeds_dir(pool)
}

/// 今の seed の `target/` の大きさ（manifest の `size_bytes`。st_blocks の合計で、owner と共有する extent を
/// 含む保守的な上限）。
pub fn current_seed_bytes(pool: &Pool, repo_key: &str) -> Option<u64> {
    read_current_manifest(pool, repo_key).and_then(|m| m.size_bytes)
}

fn git(dir: &Path, args: &[&str]) -> io::Result<()> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "git {} exited with {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// `git rev-parse --verify <branch>^{commit}`。
pub fn resolve_commit(repo: &Path, branch: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{branch}^{{commit}}"))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `commit` の根に `Cargo.toml` があるか（無い repo には seed を作らない）。
pub fn commit_has_cargo_manifest(repo: &Path, commit: &str) -> bool {
    git(repo, &["cat-file", "-e", &format!("{commit}:Cargo.toml")]).is_ok()
}

/// 専用の checkout を `commit` に detach する。無い・壊れていれば `git worktree add --detach` で作り直す
/// （既にあれば `checkout --detach --force`。変わらないファイルの mtime は保たれる）。
pub fn git_detached_checkout(repo: &Path, checkout: &Path, commit: &str) -> io::Result<()> {
    if checkout.join(".git").exists()
        && git(
            checkout,
            &["checkout", "--quiet", "--detach", "--force", commit],
        )
        .is_ok()
    {
        return Ok(());
    }
    remove_any(checkout)?;
    let _ = git(repo, &["worktree", "prune"]);
    let path = checkout.display().to_string();
    git(
        repo,
        &[
            "worktree", "add", "--quiet", "--detach", "--force", &path, commit,
        ],
    )
}

/// `cargo build --workspace --all-targets`（`checkout` で。env は `CARGO_TARGET_DIR` と `[scratch.cargo]`）。
pub fn cargo_build_all_targets(checkout: &Path, env: &[(String, String)]) -> io::Result<()> {
    let out = Command::new("cargo")
        .args(["build", "--workspace", "--all-targets", "--quiet"])
        .current_dir(checkout)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
        Err(io::Error::other(format!(
            "cargo build exited with {}: {}",
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
        )))
    }
}

/// `rustc -V`（`dir` で。rust-toolchain に従う）。
pub fn rustc_version(dir: &Path) -> Option<String> {
    let out = Command::new("rustc")
        .arg("-V")
        .current_dir(dir)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|v| !v.is_empty())
}
