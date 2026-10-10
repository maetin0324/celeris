//! ADR 2026-10-07-build-tmp-hygiene D1.2 規則 4〜6・D1.3: 共有 cargo target の掃除の **I/O 層**。
//!
//! 純粋な計画（どの項目を消すか）は `task_worker::target_sweep::plan` が持つ。ここは
//! 1. 走査（root の直下と `<root>/<repo-key>/wu-*` を target dir、`.cargo-lock` を持つ `<profile>` と
//!    `<triple>/<profile>` を profile dir。symlink は辿らない。大きさは `st_blocks × 512`）
//! 2. profile ごとに `<profile>/.cargo-lock` を `flock(LOCK_EX | LOCK_NB)`（取れなければ `build_in_progress`）
//! 3. `plan` → lock を持ったまま消す項目を `<profile>/.deleting-<ulid>/` へ rename
//!    （放置 target dir は `<親>/.deleting-<ulid>` へ）
//! 4. lock を放してから `remove_dir_all`（前回の `.deleting-*` の残りも消す）
//!
//! を行う。`dry_run` は rename も削除もしない（lock は読み取りで開いて確かめるだけで、木は変えない）。
//! root の列挙は呼び出し側の仕事で、roots に入れない dir（scratch pool の `release-build` lease 等）には触らない。
//! 時計と大きさ・使用時刻は [`SweepEnv`] で差し替えられる（試験は決定的）。**LLM は呼ばない。**

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};
use task_core::Event;
use task_core::model::{
    TargetSweepByReason, TargetSweepMode, TargetSweepRootReport, TargetSweepSkip,
};
use task_worker::scratch::DELETING_PREFIX;
use task_worker::target_sweep::{
    self as sweep_plan, DeleteReason, ItemKind, SkipReason, SweepParams, TargetSnapshot,
};

/// `TargetSweepRan.skipped` に載せる先頭の件数（D1.5）。
pub const SKIPPED_EVENT_LIMIT: usize = 50;

const CARGO_LOCK: &str = ".cargo-lock";

/// 時計と、項目の大きさ・使用時刻の測り方。試験は `now` を固定し、必要なら大きさも差し替える。
pub trait SweepEnv {
    fn now(&self) -> SystemTime;

    /// 1 つの file・dir の実使用量（既定 `st_blocks × 512`）。
    fn bytes(&self, _path: &Path, meta: &Metadata) -> u64 {
        meta.blocks().saturating_mul(512)
    }

    /// 1 つの file・dir の使用時刻。file は `max(mtime, atime)`、dir は `mtime` だけ
    /// （掃除の走査そのものが `read_dir` で dir の atime を進めてしまう〈relatime〉ため）。
    fn used_at(&self, _path: &Path, meta: &Metadata) -> SystemTime {
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if meta.is_dir() {
            return mtime;
        }
        let atime = meta.accessed().unwrap_or(SystemTime::UNIX_EPOCH);
        mtime.max(atime)
    }
}

/// 本物の時計と `stat` の値。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemEnv;

impl SweepEnv for SystemEnv {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// root 1 つ分の結果（`TargetSweepRootReport` と同じ形）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootReport {
    pub root: PathBuf,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub deleted_bytes: u64,
    pub deleted_items: u64,
    pub by_reason: ByReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByReason {
    pub age: u64,
    pub cap: u64,
    pub stale_target: u64,
}

/// 消した（dry_run なら消す予定の）項目 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedItem {
    pub root: PathBuf,
    pub path: PathBuf,
    pub bytes: u64,
    /// `age` / `cap` / `stale_target`。
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedItem {
    pub path: PathBuf,
    /// `build_in_progress` / `outside_root` / `symlink` / `rename_failed`。
    pub reason: String,
}

/// 1 回の掃除の結果（D1.5 の JSON。`celerisctl target sweep --json` の stdout にもそのまま出す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepReport {
    pub mode: TargetSweepMode,
    pub roots: Vec<RootReport>,
    pub deleted: Vec<DeletedItem>,
    pub skipped: Vec<SkippedItem>,
    pub over_cap_unresolved: bool,
    /// 消した（dry_run なら消す予定の）前回までの `.deleting-*` の残り。
    pub leftovers: Vec<PathBuf>,
    /// 走査・rename・削除で起きた失敗（掃除は止めずに続ける）。
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

impl SweepReport {
    pub fn deleted_bytes(&self) -> u64 {
        self.roots.iter().map(|r| r.deleted_bytes).sum()
    }

    /// `Event::TargetSweepRan` の root 1 つ分へ写す。
    pub fn root_reports(&self) -> Vec<TargetSweepRootReport> {
        self.roots
            .iter()
            .map(|r| TargetSweepRootReport {
                root: r.root.display().to_string(),
                before_bytes: r.before_bytes,
                after_bytes: r.after_bytes,
                deleted_bytes: r.deleted_bytes,
                deleted_items: r.deleted_items,
                by_reason: TargetSweepByReason {
                    age: r.by_reason.age,
                    cap: r.by_reason.cap,
                    stale_target: r.by_reason.stale_target,
                },
            })
            .collect()
    }

    /// skip の先頭 [`SKIPPED_EVENT_LIMIT`] 件。
    pub fn skipped_head(&self) -> Vec<TargetSweepSkip> {
        self.skipped
            .iter()
            .take(SKIPPED_EVENT_LIMIT)
            .map(|s| TargetSweepSkip {
                path: s.path.display().to_string(),
                reason: s.reason.clone(),
            })
            .collect()
    }

    /// D1.5 の `TargetSweepRan`（cron 由来の task に追記する）。
    pub fn to_event(&self) -> Event {
        Event::TargetSweepRan {
            mode: self.mode,
            roots: self.root_reports(),
            skipped: self.skipped_head(),
            skipped_total: self.skipped.len() as u64,
            over_cap_unresolved: self.over_cap_unresolved,
            duration_ms: self.duration_ms,
        }
    }
}

/// 走査の結果。
#[derive(Debug, Default)]
struct Scan {
    items: Vec<TargetSnapshot>,
    /// profile dir → その target dir。
    profiles: BTreeMap<PathBuf, PathBuf>,
    /// 前回の `.deleting-*`。
    leftovers: Vec<PathBuf>,
    errors: Vec<String>,
    skipped: Vec<SkippedItem>,
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_dir())
        .unwrap_or(false)
}

/// `read_dir` の名前つき一覧（名前順。読めなければ空）。
fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    out.sort();
    out
}

fn is_deleting(name: &str) -> bool {
    name.starts_with(DELETING_PREFIX)
}

/// `<target>/<profile>` と `<target>/<triple>/<profile>` のうち `.cargo-lock` を持つ実 dir。
fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    let has_lock = |p: &Path| {
        std::fs::symlink_metadata(p.join(CARGO_LOCK))
            .map(|m| m.file_type().is_file())
            .unwrap_or(false)
    };
    let mut out = Vec::new();
    for (name, p) in entries(target) {
        if is_deleting(&name) || !is_real_dir(&p) {
            continue;
        }
        if has_lock(&p) {
            out.push(p);
            continue;
        }
        for (sub, q) in entries(&p) {
            if !is_deleting(&sub) && is_real_dir(&q) && has_lock(&q) {
                out.push(q);
            }
        }
    }
    out
}

/// 木を辿らずに（symlink は lstat の値だけ）大きさと使用時刻を合計する。
fn measure(env: &dyn SweepEnv, path: &Path, meta: &Metadata) -> (u64, SystemTime) {
    let mut bytes = env.bytes(path, meta);
    let mut used = env.used_at(path, meta);
    if meta.file_type().is_dir() {
        for (_, child) in entries(path) {
            if let Ok(m) = std::fs::symlink_metadata(&child) {
                let (b, u) = measure(env, &child, &m);
                bytes = bytes.saturating_add(b);
                used = used.max(u);
            }
        }
    }
    (bytes, used)
}

fn scan_profile(env: &dyn SweepEnv, root: &Path, target: &Path, profile: &Path, scan: &mut Scan) {
    scan.profiles
        .insert(profile.to_path_buf(), target.to_path_buf());
    for (name, p) in entries(profile) {
        if is_deleting(&name) {
            scan.leftovers.push(p);
        }
    }
    let sub: [(&str, ItemKind); 4] = [
        ("deps", ItemKind::Deps),
        (".fingerprint", ItemKind::Fingerprint),
        ("build", ItemKind::Build),
        ("incremental", ItemKind::Incremental),
    ];
    for (dir, kind) in sub {
        let base = profile.join(dir);
        if !is_real_dir(&base) {
            continue;
        }
        for (name, path) in entries(&base) {
            let key = match kind {
                ItemKind::Deps => sweep_plan::deps_key(&name),
                _ => sweep_plan::dir_key(&name),
            };
            let Some(key) = key else {
                continue;
            };
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    scan.errors.push(format!("stat {}: {e}", path.display()));
                    continue;
                }
            };
            let symlink = meta.file_type().is_symlink();
            let (bytes, used_at) = measure(env, &path, &meta);
            scan.items.push(TargetSnapshot {
                root: root.to_path_buf(),
                target_dir: target.to_path_buf(),
                profile_dir: profile.to_path_buf(),
                path,
                kind,
                key,
                bytes,
                used_at,
                symlink,
            });
        }
    }
}

/// scratch pool の root（`<scratch>/targets`）の中で触らない owner（lease の本人が掃除する。D1.1）。
pub const SCRATCH_EXCLUDED_OWNERS: &[&str] = &["release-build"];

/// scratch の owner 配置（`<owner>/target`・`<owner>/wu-*/target`）の target dir を走査する（付記 A2）。
/// owner dir の file（`lease.json` 等）と `<root>` 直下の `.deleting-*`（scratch_gc のもの）には触れない。
fn scan_scratch_root(
    env: &dyn SweepEnv,
    root: &Path,
    lookup: &dyn task_worker::scratch::StatusLookup,
    scan: &mut Scan,
) {
    for (name, owner) in entries(root) {
        if name.starts_with('.') || SCRATCH_EXCLUDED_OWNERS.contains(&name.as_str()) {
            continue;
        }
        if !is_real_dir(&owner) {
            continue;
        }
        // A task may run tests between cargo invocations, when no cargo lock is held.
        // Unknown/external owners stay protected: only a confirmed terminal task is eligible.
        let finished = name
            .strip_prefix("task-")
            .and_then(|id| lookup.task_status(id))
            .is_some_and(|s| s.is_terminal());
        if !finished {
            scan.skipped.push(SkippedItem {
                path: owner,
                reason: "active_or_unknown_owner".into(),
            });
            continue;
        }
        let mut owner_dirs = vec![owner.clone()];
        for (sub, d2) in entries(&owner) {
            if sub.starts_with("wu-") && is_real_dir(&d2) {
                owner_dirs.push(d2);
            }
        }
        for dir in owner_dirs {
            for (sub, p) in entries(&dir) {
                if is_deleting(&sub) && is_real_dir(&p) {
                    scan.leftovers.push(p);
                }
            }
            let target = dir.join(task_worker::workspace_targets::WORKSPACE_TARGET_NAME);
            if !is_real_dir(&target) {
                continue;
            }
            for p in profile_dirs(&target) {
                scan_profile(env, root, &target, &p, scan);
            }
        }
    }
}

/// root の直下（深さ 1）と `<root>/<repo-key>/wu-*`（深さ 2）の target dir を走査する。
/// `scratch_roots` に入っている root は scratch の owner 配置で走査する（付記 A2）。
fn scan_roots(
    env: &dyn SweepEnv,
    roots: &[PathBuf],
    scratch_roots: &[PathBuf],
    lookup: &dyn task_worker::scratch::StatusLookup,
) -> Scan {
    let mut scan = Scan::default();
    for root in roots {
        // root 自体は呼び出し側が明示したものなので、symlink でも辿る（中は辿らない）。
        if !root.is_dir() {
            scan.errors
                .push(format!("root {} is not a directory", root.display()));
            continue;
        }
        if scratch_roots.contains(root) {
            scan_scratch_root(env, root, lookup, &mut scan);
            continue;
        }
        for (name, d1) in entries(root) {
            if is_deleting(&name) {
                scan.leftovers.push(d1);
                continue;
            }
            if !is_real_dir(&d1) {
                continue;
            }
            let profiles = profile_dirs(&d1);
            if !profiles.is_empty() {
                for p in profiles {
                    scan_profile(env, root, &d1, &p, &mut scan);
                }
                continue;
            }
            for (sub, d2) in entries(&d1) {
                if is_deleting(&sub) {
                    scan.leftovers.push(d2);
                    continue;
                }
                if !sub.starts_with("wu-") || !is_real_dir(&d2) {
                    continue;
                }
                for p in profile_dirs(&d2) {
                    scan_profile(env, root, &d2, &p, &mut scan);
                }
            }
        }
    }
    scan
}

/// `<profile>/.cargo-lock` の非 blocking 排他 lock。取れなければ `None`（build 中か、開けない）。
/// 読み取りで開くので file を作らない・書かない。drop で解放する。
fn try_lock(profile: &Path) -> Option<Flock<File>> {
    let file = File::open(profile.join(CARGO_LOCK)).ok()?;
    Flock::lock(file, FlockArg::LockExclusiveNonblock).ok()
}

fn skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::BuildInProgress => "build_in_progress",
        SkipReason::OutsideRoot => "outside_root",
        SkipReason::Symlink => "symlink",
    }
}

fn delete_reason(reason: DeleteReason) -> &'static str {
    match reason {
        DeleteReason::Age => "age",
        DeleteReason::Cap => "cap",
        DeleteReason::StaleTarget => "stale_target",
    }
}

fn deleting_name() -> String {
    format!("{DELETING_PREFIX}{}", ulid::Ulid::new())
}

/// `src` を `dest_root` の下の同じ相対 path へ rename する。
fn rename_into(src: &Path, base: &Path, dest_root: &Path) -> std::io::Result<()> {
    let rel = src
        .strip_prefix(base)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let dest = dest_root.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(src, dest)
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// 掃除を 1 回走らせる（D1.3）。`roots` は呼び出し側が列挙した root（`params.roots` は無視して上書きする）。
/// `dry_run` は木を変えない（計画と、前回の `.deleting-*` の一覧を report に出すだけ）。
pub fn run_sweep(
    roots: &[PathBuf],
    params: &SweepParams,
    mode: TargetSweepMode,
    env: &dyn SweepEnv,
) -> SweepReport {
    run_sweep_with_scratch(roots, &[], params, mode, env, &task_worker::scratch::NoDb)
}

/// [`run_sweep`] に scratch pool の root（`<scratch>/targets`。`roots` にも入れる）を足したもの（付記 A2）。
/// scratch の root は owner 配置（`<owner>/target`・`<owner>/wu-*/target`）で走査し、`release-build` は除く。
pub fn run_sweep_with_scratch(
    roots: &[PathBuf],
    scratch_roots: &[PathBuf],
    params: &SweepParams,
    mode: TargetSweepMode,
    env: &dyn SweepEnv,
    lookup: &dyn task_worker::scratch::StatusLookup,
) -> SweepReport {
    let started = Instant::now();
    let apply = mode == TargetSweepMode::Apply;
    let params = SweepParams {
        roots: roots.to_vec(),
        ..params.clone()
    };
    let scan = scan_roots(env, roots, scratch_roots, lookup);
    let mut errors = scan.errors;

    // 規則 4: profile ごとの lock。apply は rename まで持ち続け、dry_run は確かめたらすぐ放す。
    let mut held: Vec<Flock<File>> = Vec::new();
    let mut locked: BTreeSet<PathBuf> = BTreeSet::new();
    for profile in scan.profiles.keys() {
        match try_lock(profile) {
            Some(lock) if apply => held.push(lock),
            Some(_) => {}
            None => {
                locked.insert(profile.clone());
            }
        }
    }

    let plan = sweep_plan::plan(&scan.items, &locked, env.now(), &params);

    let mut skipped: Vec<SkippedItem> = plan
        .skip
        .iter()
        .map(|s| SkippedItem {
            path: s.path.clone(),
            reason: skip_reason(s.reason).to_string(),
        })
        .collect();

    skipped.extend(scan.skipped);

    let item_profile: BTreeMap<&Path, &Path> = scan
        .items
        .iter()
        .map(|i| (i.path.as_path(), i.profile_dir.as_path()))
        .collect();
    let entry = |d: &sweep_plan::PlannedDelete| DeletedItem {
        root: d.root.clone(),
        path: d.path.clone(),
        bytes: d.bytes,
        reason: delete_reason(d.reason).to_string(),
    };

    let mut deleted: Vec<DeletedItem> = Vec::new();
    let mut to_remove: Vec<PathBuf> = Vec::new();
    if !apply {
        deleted.extend(plan.delete.iter().map(entry));
    } else {
        // 放置 target dir を先に `<親>/.deleting-<ulid>` へ動かす。中の項目は個別に rename しない。
        let mut moved_targets: Vec<&Path> = Vec::new();
        for d in plan
            .delete
            .iter()
            .filter(|d| d.reason == DeleteReason::StaleTarget)
        {
            let parent = d.path.parent().unwrap_or(&d.root);
            let dest = parent.join(deleting_name());
            match std::fs::rename(&d.path, &dest) {
                Ok(()) => {
                    moved_targets.push(d.path.as_path());
                    to_remove.push(dest);
                    deleted.push(entry(d));
                }
                Err(e) => {
                    errors.push(format!("rename {}: {e}", d.path.display()));
                    skipped.push(SkippedItem {
                        path: d.path.clone(),
                        reason: "rename_failed".to_string(),
                    });
                }
            }
        }
        // 残りの項目は profile ごとに 1 つの `<profile>/.deleting-<ulid>/` へ同じ相対 path で動かす。
        let mut deleting_dirs: BTreeMap<&Path, PathBuf> = BTreeMap::new();
        for d in plan
            .delete
            .iter()
            .filter(|d| d.reason != DeleteReason::StaleTarget)
        {
            if moved_targets.iter().any(|t| d.path.starts_with(t)) {
                // 動かした target dir に含まれる（量はこの行が持つ）。
                deleted.push(entry(d));
                continue;
            }
            let Some(profile) = item_profile.get(d.path.as_path()).copied() else {
                errors.push(format!("no profile for {}", d.path.display()));
                continue;
            };
            let dest = deleting_dirs
                .entry(profile)
                .or_insert_with(|| profile.join(deleting_name()))
                .clone();
            match rename_into(&d.path, profile, &dest) {
                Ok(()) => {
                    if !to_remove.contains(&dest) {
                        to_remove.push(dest);
                    }
                    deleted.push(entry(d));
                }
                Err(e) => {
                    errors.push(format!("rename {}: {e}", d.path.display()));
                    skipped.push(SkippedItem {
                        path: d.path.clone(),
                        reason: "rename_failed".to_string(),
                    });
                }
            }
        }
    }
    // rename が済んだので lock を放す（待たされた cargo はここで進む）。
    drop(held);

    let leftovers = scan.leftovers;
    if apply {
        for path in leftovers.iter().chain(to_remove.iter()) {
            if let Err(e) = remove_any(path) {
                errors.push(format!("remove {}: {e}", path.display()));
            }
        }
    }

    // root ごとの量（apply は実際に rename できた分だけ数える）。
    let mut roots_out: Vec<RootReport> = Vec::new();
    let mut over_cap_unresolved = false;
    for summary in &plan.roots {
        let mut r = RootReport {
            root: summary.root.clone(),
            before_bytes: summary.before_bytes,
            ..RootReport::default()
        };
        for d in deleted.iter().filter(|d| d.root == summary.root) {
            r.deleted_bytes = r.deleted_bytes.saturating_add(d.bytes);
            r.deleted_items += 1;
            match d.reason.as_str() {
                "age" => r.by_reason.age += 1,
                "cap" => r.by_reason.cap += 1,
                _ => r.by_reason.stale_target += 1,
            }
        }
        r.after_bytes = r.before_bytes.saturating_sub(r.deleted_bytes);
        let unresolved = summary.over_cap_unresolved
            || (r.before_bytes > params.max_bytes_per_root
                && r.after_bytes > params.cap_target_bytes());
        over_cap_unresolved |= unresolved;
        roots_out.push(r);
    }

    SweepReport {
        mode,
        roots: roots_out,
        deleted,
        skipped,
        over_cap_unresolved,
        leftovers,
        errors,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// 付記 A2・A3: daemon の保守 executor だけが足す範囲（`celerisctl target sweep` は使わない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepScope {
    /// `<scratch>/targets` を root に足す（scratch が有効なときだけ）。
    pub scratch_targets: bool,
    /// 終端から何秒経った task の作業場所の target を消すか（`0` で無効）。
    pub workspace_target_after_secs: u64,
}

impl Default for SweepScope {
    fn default() -> Self {
        Self {
            scratch_targets: true,
            workspace_target_after_secs: DEFAULT_WORKSPACE_TARGET_AFTER_SECS,
        }
    }
}

/// 付記 A3: 終わった task の作業場所の target の掃除（[`sweep_workspace_targets`]）の既定の猶予（6 時間）。
pub const DEFAULT_WORKSPACE_TARGET_AFTER_SECS: u64 = 6 * 3600;

/// ADR 2026-10-10-local-disk-growth-paths D2: 終端 task の repo 直下 target を 1 つ脇へ寄せた記録。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedRepoTarget {
    pub task_id: task_core::TaskId,
    pub target: PathBuf,
    /// rename 先（`<repo>/.deleting-cargo-target-<ulid>`。dry run では予定の path）。
    pub moved: PathBuf,
    /// rename 前に測った量（`st_blocks × 512`。du は使わない）。
    pub bytes: u64,
}

/// [`repo_target_gc_move_aside`] の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoTargetGc {
    pub moved: Vec<MovedRepoTarget>,
    /// 残した target と理由（`running_run`・`active_descendant`・`grace`・`build_in_progress`・`rename_failed`）。
    pub held: Vec<SkippedItem>,
    /// lock を放した後に `remove_dir_all` するもの（rename 済みの target と前回の残り）。dry run では空。
    pub to_remove: Vec<PathBuf>,
    /// 見つけた前回の残り（`.deleting-cargo-target-*`）。
    pub leftovers: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// ADR 2026-10-10-local-disk-growth-paths D2: checkout を持つ task とその子孫が全て終端になり、最後の終端から
/// `after_secs` 経った task の repo 直下 target を、cargo の lock を全 profile で持ったまま
/// `.deleting-cargo-target-*` へ rename する（`apply = false` は測るだけ）。削除そのものは呼び出し側
/// （[`remove_moved_targets`]。dispatcher の tick では別スレッド）。
pub fn repo_target_gc_move_aside(
    store: &dyn task_core::TaskStore,
    workspace_root: &Path,
    after_secs: u64,
    now: time::OffsetDateTime,
    apply: bool,
) -> RepoTargetGc {
    use task_worker::workspace_targets::{RemoveTarget, remove_target, scan_finished_task_targets};
    let mut gc = RepoTargetGc::default();
    let scan = match scan_finished_task_targets(store, workspace_root, now, after_secs) {
        Ok(scan) => scan,
        Err(e) => {
            gc.errors
                .push(format!("workspace targets: list finished tasks: {e}"));
            return gc;
        }
    };
    for held in scan.held {
        for path in held.targets {
            gc.held.push(SkippedItem {
                path,
                reason: held.reason.to_string(),
            });
        }
    }
    for task in scan.found {
        for target in task.targets {
            match remove_target(&target, &deleting_name()[DELETING_PREFIX.len()..], apply) {
                Ok(RemoveTarget::Locked) => gc.held.push(SkippedItem {
                    path: target,
                    reason: "build_in_progress".to_string(),
                }),
                Ok(RemoveTarget::Moved { moved, bytes }) => {
                    if apply {
                        gc.to_remove.push(moved.clone());
                    }
                    gc.moved.push(MovedRepoTarget {
                        task_id: task.task_id,
                        target,
                        moved,
                        bytes,
                    });
                }
                Err(e) => {
                    gc.errors.push(format!("rename {}: {e}", target.display()));
                    gc.held.push(SkippedItem {
                        path: target,
                        reason: "rename_failed".to_string(),
                    });
                }
            }
        }
        for path in task.leftovers {
            if let Some(_lock) = task_worker::workspace_targets::try_lock_target(&path) {
                gc.leftovers.push(path.clone());
                if apply {
                    gc.to_remove.push(path);
                }
            }
        }
    }
    gc
}

/// [`remove_moved_targets`] の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemovedRepoTargets {
    pub removed: usize,
    /// `probe` の filesystem の `statvfs` 空きの増分（他の書込みも含む。測れなければ `None`）。D5。
    pub freed_statvfs: Option<u64>,
    pub errors: Vec<String>,
}

/// rename 済みの target を消し、前後の `statvfs` 空きの差を返す（`probe` は workspace root）。
pub fn remove_moved_targets(paths: &[PathBuf], probe: &Path) -> RemovedRepoTargets {
    let before = crate::scratch_gc::fs_stats(probe).map(|(_, avail)| avail);
    let mut out = RemovedRepoTargets::default();
    for path in paths {
        match remove_any(path) {
            Ok(()) => out.removed += 1,
            Err(e) => out.errors.push(format!("remove {}: {e}", path.display())),
        }
    }
    let after = crate::scratch_gc::fs_stats(probe).map(|(_, avail)| avail);
    out.freed_statvfs = before
        .zip(after)
        .map(|(before, after)| after.saturating_sub(before));
    out
}

/// 付記 A3: 終端（done・cancelled・failed）になってから `after_secs` 経った task の作業場所に残る cargo の target を
/// 消し、`report` に root = `workspace_root` の 1 行（`by_reason.stale_target` に件数）として足す。
/// ADR 2026-10-10-local-disk-growth-paths D2: 子孫が動いている・猶予内の木は理由付きで `skipped` に残す。
/// build 中（どれかの profile の lock が取れない）は `build_in_progress` で残す。`dry_run` は木を変えない。
pub fn sweep_workspace_targets(
    report: &mut SweepReport,
    store: &dyn task_core::TaskStore,
    workspace_root: &Path,
    after_secs: u64,
    now: time::OffsetDateTime,
) {
    let started = Instant::now();
    let apply = report.mode == TargetSweepMode::Apply;
    let gc = repo_target_gc_move_aside(store, workspace_root, after_secs, now, apply);
    let mut root = RootReport {
        root: workspace_root.to_path_buf(),
        ..RootReport::default()
    };
    for m in gc.moved {
        root.before_bytes = root.before_bytes.saturating_add(m.bytes);
        root.deleted_bytes = root.deleted_bytes.saturating_add(m.bytes);
        root.deleted_items += 1;
        root.by_reason.stale_target += 1;
        report.deleted.push(DeletedItem {
            root: workspace_root.to_path_buf(),
            path: m.target,
            bytes: m.bytes,
            reason: "stale_target".to_string(),
        });
    }
    report.skipped.extend(gc.held);
    report.leftovers.extend(gc.leftovers);
    report.errors.extend(gc.errors);
    report
        .errors
        .extend(remove_moved_targets(&gc.to_remove, workspace_root).errors);
    root.after_bytes = root.before_bytes.saturating_sub(root.deleted_bytes);
    if root.before_bytes > 0 || root.deleted_items > 0 {
        report.roots.push(root);
    }
    report.duration_ms = report
        .duration_ms
        .saturating_add(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
}

#[cfg(test)]
mod tests;
