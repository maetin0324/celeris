//! ADR-0075 D2 / D6（Phase G1）: scratch pool の走査・GC の実行・削除スレッド・測定スレッド・状態の組み立て。
//!
//! 純粋な部分（分類・`plan_gc`・adopt の候補選び）は `task_worker::scratch` にある。ここは I/O を伴う薄い層で、
//! dispatcher の tick の `scratch_gc` phase と `celerisctl scratch {status,gc}` の両方が使う。**LLM は呼ばない。**
//! tick の中でやるのは「lease と DB の状態を読む → `plan_gc` → `.deleting-*` へ rename」まで。`remove_dir_all` と
//! サイズの測定（木を辿る）は別スレッド（同時にそれぞれ 1 本）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use task_ops::daemon::{
    SCRATCH_STATUS_SCHEMA, ScratchCacheStats, ScratchCacheView, ScratchGcRemovedView,
    ScratchGcView, ScratchLegacyView, ScratchOwnerView, ScratchSccacheStats, ScratchSccacheView,
    ScratchStatus,
};
use task_worker::scratch::{
    self, AdoptCandidate, Class, DELETING_PREFIX, GcEntry, GcParams, GcPlan, Lease, Owner, Pool,
    ScratchSettings, StatusLookup,
};

/// 測定スレッドの結果（パス → サイズ・木の最新の mtime・測定時刻）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    pub bytes: u64,
    pub last_write: SystemTime,
    pub measured_at: SystemTime,
}

pub type SizeCache = Arc<Mutex<HashMap<PathBuf, Measured>>>;

/// 走査した owner 1 つ。
#[derive(Debug, Clone)]
pub struct OwnerRow {
    /// owner として読めない野良は `Err(path)`。
    pub owner: Result<Owner, PathBuf>,
    pub lease: Option<Lease>,
    pub lease_mtime: Option<SystemTime>,
    pub class: Class,
    pub reason: String,
    pub has_target: bool,
    /// GC の entry にするパス（owner は `target/`、野良はディレクトリそのもの）。
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LegacyRow {
    pub path: PathBuf,
    pub class: Class,
    pub measured: Option<Measured>,
}

/// 1 回の走査の結果。
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub owners: Vec<OwnerRow>,
    pub legacy: Vec<LegacyRow>,
    pub entries: Vec<GcEntry>,
    /// adopt の候補（P3 で `target/` があるもの）。
    pub candidates: Vec<AdoptCandidate>,
    /// P0〜P2 の owner の repo（seed を残す条件）。
    pub active_repo_keys: BTreeSet<String>,
    /// target を刈った後の lease だけのディレクトリで、記録の保持期限を過ぎたもの（片づける）。
    pub stale_records: Vec<PathBuf>,
}

/// ADR-0075 D7: pool の外の旧い target の根。`build_cache_dir/cargo/*`（直下の実ディレクトリ）と、
/// `<releases_dir>/.cargo-target`（release.sh の symlink の先。`.build` の worktree は対象外）。
pub fn legacy_paths(build_cache_dir: &Path, releases_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let cargo = task_worker::build_cache::cargo_root(build_cache_dir);
    if let Ok(meta) = std::fs::symlink_metadata(&cargo)
        && meta.file_type().is_dir()
        && let Ok(rd) = std::fs::read_dir(&cargo)
    {
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| {
                !e.file_name().to_string_lossy().starts_with('.')
                    && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
            })
            .map(|e| e.path())
            .collect();
        dirs.sort();
        out.extend(dirs);
    }
    if let Some(rel) = releases_dir {
        let link = rel.join(".cargo-target");
        if let Ok(meta) = std::fs::symlink_metadata(&link) {
            let target = if meta.file_type().is_symlink() {
                std::fs::read_link(&link)
                    .ok()
                    .map(|t| if t.is_relative() { rel.join(t) } else { t })
            } else if meta.file_type().is_dir() {
                Some(link.clone())
            } else {
                None
            };
            if let Some(t) = target.filter(|t| t.is_dir()) {
                out.push(t);
            }
        }
    }
    out
}

/// 削除待ち（`.deleting-*`）を探す根（pool の `targets/` と legacy の親）。
pub fn deleting_roots(pool: &Pool, legacy: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
    roots.insert(pool.targets_dir());
    for p in legacy {
        if let Some(parent) = p.parent() {
            roots.insert(parent.to_path_buf());
        }
    }
    roots.into_iter().collect()
}

fn has_children_work_units(owner_dir: &Path) -> bool {
    std::fs::read_dir(owner_dir)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("wu-"))
        })
        .unwrap_or(false)
}

/// pool を走査して分類する（lease と DB の状態を読むだけ。木は辿らない）。
pub fn scan(
    settings: &ScratchSettings,
    legacy: &[PathBuf],
    lookup: &dyn StatusLookup,
    has_db: bool,
    sizes: &HashMap<PathBuf, Measured>,
    now: SystemTime,
) -> Scan {
    let pool = settings.pool();
    let mut out = Scan::default();
    for item in pool.list_owner_dirs() {
        match item {
            Ok(owner) => {
                let owner_dir = pool.owner_dir(&owner);
                let lease_path = pool.lease_path(&owner);
                let lease = scratch::read_lease(&lease_path).ok().flatten();
                let target = pool.target_dir(&owner);
                let has_target = target.is_dir();
                let lease_mtime = scratch::mtime(&lease_path);
                if lease.is_none() && !has_target {
                    // WU の入れ物としてだけある `task-<id>/`（Task 単位の lease が無い）など。
                    continue;
                }
                let (class, reason) = if lease.is_none() {
                    // lease の無い owner のディレクトリは野良と同じ扱い（木の最新の mtime で 1h を判定）。
                    let (c, r) =
                        scratch::legacy_class(sizes.get(&target).map(|m| m.last_write), now);
                    if c == Class::Legacy {
                        (Class::Stray, "no lease.json (idle >= 1h)")
                    } else {
                        (c, r)
                    }
                } else {
                    scratch::classify(
                        &owner,
                        lease.as_ref(),
                        lease_mtime.unwrap_or(now),
                        now,
                        settings,
                        lookup,
                        has_db,
                    )
                };
                let repo_key = lease.as_ref().map(|l| l.repo_key.clone());
                if matches!(class, Class::Pinned | Class::Waiting | Class::Retry)
                    && let Some(k) = &repo_key
                {
                    out.active_repo_keys.insert(k.clone());
                }
                if class == Class::Completed && !has_target {
                    let old = lease_mtime
                        .and_then(|t| now.duration_since(t).ok())
                        .is_some_and(|d| d.as_secs() >= scratch::LEASE_RECORD_KEEP_SECS);
                    if old && !has_children_work_units(&owner_dir) {
                        out.stale_records.push(owner_dir.clone());
                    }
                }
                if has_target {
                    let measured = lease
                        .as_ref()
                        .and_then(|l| l.size_bytes)
                        .or_else(|| sizes.get(&target).map(|m| m.bytes));
                    out.entries.push(GcEntry {
                        id: owner.to_string(),
                        path: target.clone(),
                        owner: Some(owner.clone()),
                        repo_key: repo_key.clone(),
                        class,
                        reason: reason.to_string(),
                        last_write: lease_mtime.unwrap_or(now),
                        size_bytes: measured,
                        in_pool: true,
                    });
                    if class == Class::Completed
                        && let Some(k) = &repo_key
                        && let Some(lw) = scratch::last_write(&owner_dir)
                    {
                        out.candidates.push(AdoptCandidate {
                            owner: owner.clone(),
                            repo_key: k.clone(),
                            base_commit: lease.as_ref().and_then(|l| l.base_commit.clone()),
                            last_write: lw,
                            lease_mtime,
                        });
                    }
                }
                out.owners.push(OwnerRow {
                    owner: Ok(owner),
                    lease,
                    lease_mtime,
                    class,
                    reason: reason.to_string(),
                    has_target,
                    path: target,
                });
            }
            Err(path) => {
                let m = sizes.get(&path).copied();
                let (c, r) = scratch::legacy_class(m.map(|m| m.last_write), now);
                let (class, reason) = if c == Class::Legacy {
                    (Class::Stray, "not an owner directory (idle >= 1h)")
                } else {
                    (c, r)
                };
                out.entries.push(GcEntry {
                    id: path.display().to_string(),
                    path: path.clone(),
                    owner: None,
                    repo_key: None,
                    class,
                    reason: reason.to_string(),
                    last_write: m.map(|m| m.last_write).unwrap_or(now),
                    size_bytes: m.map(|m| m.bytes),
                    in_pool: true,
                });
                out.owners.push(OwnerRow {
                    owner: Err(path.clone()),
                    lease: None,
                    lease_mtime: scratch::mtime(&path),
                    class,
                    reason: reason.to_string(),
                    has_target: true,
                    path,
                });
            }
        }
    }
    for path in legacy {
        let m = sizes.get(path).copied();
        let (class, reason) = scratch::legacy_class(m.map(|m| m.last_write), now);
        out.entries.push(GcEntry {
            id: path.display().to_string(),
            path: path.clone(),
            owner: None,
            repo_key: None,
            class,
            reason: reason.to_string(),
            last_write: m.map(|m| m.last_write).unwrap_or(now),
            size_bytes: m.map(|m| m.bytes),
            in_pool: false,
        });
        out.legacy.push(LegacyRow {
            path: path.clone(),
            class,
            measured: m,
        });
    }
    out
}

/// statvfs（容量・空き。byte）。
pub fn fs_stats(path: &Path) -> Option<(u64, u64)> {
    let existing = path.ancestors().find(|p| p.exists())?;
    let st = nix::sys::statvfs::statvfs(existing).ok()?;
    let frag = st.fragment_size();
    Some((
        st.blocks().saturating_mul(frag),
        st.blocks_available().saturating_mul(frag),
    ))
}

/// ADR-0075 D1: 実効上限 = min(total_max, filesystem の容量 − pool の外の使用量 − min_free)。
pub fn effective_max(
    total_max: u64,
    fs: Option<(u64, u64)>,
    pool_used: u64,
    min_free_bytes: u64,
) -> u64 {
    match fs {
        Some((total, free)) => {
            let used = total.saturating_sub(free);
            let outside = used.saturating_sub(pool_used);
            total_max.min(total.saturating_sub(outside).saturating_sub(min_free_bytes))
        }
        None => total_max,
    }
}

/// GC の結果（rename できたもの）。
#[derive(Debug, Clone)]
pub struct Executed {
    pub removed: Vec<scratch::GcPick>,
    pub reclaimed_bytes: u64,
}

/// `plan` の選んだものを同じ親の中の `.deleting-*` へ rename する（`.lock` を持って）。journal に 1 行ずつ出す。
pub fn execute(pool: &Pool, plan: &GcPlan, scan: &Scan, emergency: bool) -> Executed {
    let mut removed = Vec::new();
    let mut reclaimed = 0u64;
    if plan.selected.is_empty() && scan.stale_records.is_empty() {
        return Executed {
            removed,
            reclaimed_bytes: 0,
        };
    }
    let _lock = match pool.lock() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(dir = %pool.root().display(), error = %e, "scratch: could not take the pool lock; GC skipped");
            return Executed {
                removed,
                reclaimed_bytes: 0,
            };
        }
    };
    let now = SystemTime::now();
    for pick in &plan.selected {
        let flat = match &pick.owner {
            Some(o) => o.flat(),
            None => pick
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "entry".to_string()),
        };
        // owner の target は `targets/` の直下へ（Task の下の WU も pool の根にまとめる）、legacy は同じ親の中へ。
        let parent = match &pick.owner {
            Some(_) => pool.targets_dir(),
            None => match pick.path.parent() {
                Some(p) => p.to_path_buf(),
                None => continue,
            },
        };
        let trash = parent.join(scratch::deleting_name(&flat, now));
        match std::fs::rename(&pick.path, &trash) {
            Ok(()) => {
                tracing::info!(
                    owner = %pick.id,
                    class = pick.class.label(),
                    seed = pick.seed,
                    estimated_bytes = pick.estimated_bytes,
                    why = pick.why,
                    emergency,
                    pressure = plan.pressure.as_str(),
                    "scratch gc: reclaiming a target"
                );
                reclaimed = reclaimed.saturating_add(pick.estimated_bytes);
                removed.push(pick.clone());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %pick.path.display(), error = %e, "scratch gc: could not move the target aside");
            }
        }
    }
    for dir in &scan.stale_records {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            tracing::debug!(path = %dir.display(), error = %e, "scratch gc: could not remove an old lease record");
        }
    }
    Executed {
        removed,
        reclaimed_bytes: reclaimed,
    }
}

/// `.deleting-*` が 1 つでもあるか。
pub fn has_pending_deletes(roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| {
        std::fs::read_dir(r)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with(DELETING_PREFIX))
            })
            .unwrap_or(false)
    })
}

/// 削除スレッド（同時に 1 本）: `roots` の直下の `.deleting-*` を `remove_dir_all` する。
pub fn spawn_removal(roots: Vec<PathBuf>, busy: Arc<AtomicBool>) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let flag = busy.clone();
    let spawned = std::thread::Builder::new()
        .name("celeris-scratch-rm".to_string())
        .spawn(move || {
            for root in roots {
                let Ok(rd) = std::fs::read_dir(&root) else {
                    continue;
                };
                for e in rd.flatten() {
                    if !e.file_name().to_string_lossy().starts_with(DELETING_PREFIX) {
                        continue;
                    }
                    let path = e.path();
                    let res = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    if let Err(err) = res {
                        tracing::warn!(path = %path.display(), error = %err, "scratch gc: could not remove a target");
                    }
                }
            }
            flag.store(false, Ordering::SeqCst);
        });
    if let Err(e) = spawned {
        busy.store(false, Ordering::SeqCst);
        tracing::warn!(error = %e, "scratch gc: could not spawn the removal thread");
    }
}

/// 次に測るもの（未測定を先に、次に測定が古い順。同順位はパス順）。`(測るパス, 書き戻す lease)`。
pub fn next_to_measure(
    scan: &Scan,
    sizes: &HashMap<PathBuf, Measured>,
) -> Option<(PathBuf, Option<PathBuf>)> {
    let mut items: Vec<(Option<SystemTime>, PathBuf, Option<PathBuf>)> = Vec::new();
    for row in &scan.owners {
        if !row.has_target {
            continue;
        }
        let lease_path = row
            .owner
            .as_ref()
            .ok()
            .and_then(|_| row.path.parent().map(|p| p.join(scratch::LEASE_FILE)))
            .filter(|_| row.lease.is_some());
        items.push((
            sizes.get(&row.path).map(|m| m.measured_at),
            row.path.clone(),
            lease_path,
        ));
    }
    for row in &scan.legacy {
        items.push((
            sizes.get(&row.path).map(|m| m.measured_at),
            row.path.clone(),
            None,
        ));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    items.into_iter().next().map(|(_, p, l)| (p, l))
}

/// 1 つを測る（木を辿る。重い）。結果を cache と lease に書く（lease の mtime は変えない）。
pub fn measure_one(path: &Path, lease: Option<&Path>, sizes: &SizeCache) {
    match scratch::measure_tree(path) {
        Ok((bytes, last_write)) => {
            let now = SystemTime::now();
            if let Ok(mut map) = sizes.lock() {
                map.insert(
                    path.to_path_buf(),
                    Measured {
                        bytes,
                        last_write,
                        measured_at: now,
                    },
                );
            }
            if let Some(lease) = lease
                && let Err(e) = scratch::write_lease_measurement(lease, bytes, now)
            {
                tracing::debug!(lease = %lease.display(), error = %e, "scratch: could not record a measurement");
            }
        }
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "scratch: measurement failed")
        }
    }
}

/// 測定スレッド（同時に 1 本、1 回に 1 つ）。
pub fn spawn_measure(
    path: PathBuf,
    lease: Option<PathBuf>,
    sizes: SizeCache,
    busy: Arc<AtomicBool>,
) {
    if busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let flag = busy.clone();
    let spawned = std::thread::Builder::new()
        .name("celeris-scratch-du".to_string())
        .spawn(move || {
            measure_one(&path, lease.as_deref(), &sizes);
            flag.store(false, Ordering::SeqCst);
        });
    if let Err(e) = spawned {
        busy.store(false, Ordering::SeqCst);
        tracing::warn!(error = %e, "scratch: could not spawn the measurement thread");
    }
}

/// ADR-0075 D3: commit の距離（`git rev-list --count --left-right a...b` の和）。git が無い・失敗は `None`。
/// run の開始時（既に git を使う経路）にだけ呼ぶ。tick では呼ばない。
pub fn commit_distance(repo: &Path, a: &str, b: &str) -> Option<u64> {
    if a == b {
        return Some(0);
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-list", "--count", "--left-right", &format!("{a}...{b}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut sum = 0u64;
    for part in text.split_whitespace() {
        sum = sum.saturating_add(part.parse::<u64>().ok()?);
    }
    Some(sum)
}

/// `scratch status` / metrics / スナップショットの状態を組む。
#[allow(clippy::too_many_arguments)]
pub fn build_status(
    settings: &ScratchSettings,
    scan: &Scan,
    plan: &GcPlan,
    fs: Option<(u64, u64)>,
    min_free_bytes: u64,
    sizes: &HashMap<PathBuf, Measured>,
    last_gc: Option<ScratchGcView>,
    now: SystemTime,
) -> ScratchStatus {
    let mut owners: Vec<ScratchOwnerView> = scan
        .owners
        .iter()
        .map(|row| {
            let (owner, kind) = match &row.owner {
                Ok(o) => (o.to_string(), o.kind().as_str().to_string()),
                Err(p) => (p.display().to_string(), "stray".to_string()),
            };
            let seed = plan.seeds.contains(&owner);
            let class = if seed {
                "seed".to_string()
            } else {
                row.class.label().to_string()
            };
            let lease = row.lease.as_ref();
            ScratchOwnerView {
                estimated_bytes: plan.estimated.get(&owner).copied().unwrap_or(0),
                size_bytes: lease
                    .and_then(|l| l.size_bytes)
                    .or_else(|| sizes.get(&row.path).map(|m| m.bytes)),
                measured_at: lease.and_then(|l| l.measured_at.clone()).or_else(|| {
                    sizes
                        .get(&row.path)
                        .map(|m| scratch::rfc3339(m.measured_at))
                }),
                owner,
                kind,
                class,
                reason: row.reason.clone(),
                has_target: row.has_target,
                lease_mtime: row.lease_mtime.map(scratch::rfc3339),
                repo_key: lease.map(|l| l.repo_key.clone()),
                base_commit: lease
                    .and_then(|l| l.base_commit.as_ref())
                    .map(|c| c.chars().take(12).collect()),
                adopted_from: lease.and_then(|l| l.adopted_from.clone()),
                work_unit_key: lease.and_then(|l| l.work_unit_key.clone()),
            }
        })
        .collect();
    owners.sort_by(|a, b| a.owner.cmp(&b.owner));
    let legacy = scan
        .legacy
        .iter()
        .map(|l| ScratchLegacyView {
            path: l.path.display().to_string(),
            class: l.class.label().to_string(),
            size_bytes: l.measured.map(|m| m.bytes),
            last_write: l.measured.map(|m| scratch::rfc3339(m.last_write)),
        })
        .collect();
    ScratchStatus {
        schema: SCRATCH_STATUS_SCHEMA.to_string(),
        enabled: settings.enabled,
        disabled_reason: settings.disabled_reason.clone(),
        dir: settings.dir.display().to_string(),
        observed_at: scratch::rfc3339(now),
        fs_total_bytes: fs.map(|f| f.0),
        fs_free_bytes: fs.map(|f| f.1),
        targets_bytes: plan.used_bytes,
        pinned_bytes: plan.pinned_bytes,
        targets_max_bytes: settings.targets_max_bytes,
        total_max_bytes: settings.total_max_bytes,
        effective_max_bytes: effective_max(
            settings.total_max_bytes,
            fs,
            plan.used_bytes,
            min_free_bytes,
        ),
        high_watermark: settings.high_watermark,
        low_watermark: settings.low_watermark,
        pressure: plan.pressure.as_str().to_string(),
        owners,
        legacy,
        last_gc,
        sccache: None,
        cache: None,
    }
}

/// scratch を無効にした構成の状態（理由だけ）。
pub fn disabled_status(settings: &ScratchSettings, now: SystemTime) -> ScratchStatus {
    ScratchStatus {
        schema: SCRATCH_STATUS_SCHEMA.to_string(),
        enabled: false,
        disabled_reason: settings.disabled_reason.clone(),
        dir: settings.dir.display().to_string(),
        observed_at: scratch::rfc3339(now),
        fs_total_bytes: None,
        fs_free_bytes: None,
        targets_bytes: 0,
        pinned_bytes: 0,
        targets_max_bytes: settings.targets_max_bytes,
        total_max_bytes: settings.total_max_bytes,
        effective_max_bytes: settings.total_max_bytes,
        high_watermark: settings.high_watermark,
        low_watermark: settings.low_watermark,
        pressure: "none".to_string(),
        owners: Vec::new(),
        legacy: Vec::new(),
        last_gc: None,
        sccache: None,
        cache: None,
    }
}

// ---------------------------------------------------------------------------
// sccache L1（ADR-0075 D4 / D6、Phase G2）
// ---------------------------------------------------------------------------

/// `ScratchStatus.sccache`（統計なし。統計は `query_sccache_stats` で足す）。
pub fn sccache_view(
    settings: &ScratchSettings,
    state: &scratch::SccacheState,
) -> ScratchSccacheView {
    ScratchSccacheView {
        state: state.label().to_string(),
        reason: state.reason().map(str::to_string),
        binary: settings.sccache.binary.display().to_string(),
        port: settings.sccache.server_port,
        dir: settings.pool().l1_dir().display().to_string(),
        max_bytes: settings.l1_max_bytes,
        stats: None,
    }
}

/// `sccache --show-stats --stats-format=json`（0.18）の要約。`cache_hits.counts` は言語ごと（`Rust` / `C/C++` / …）。
pub fn parse_sccache_stats(json: &str) -> Option<ScratchSccacheStats> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let stats = v.get("stats")?;
    let counts = |key: &str| -> (u64, u64) {
        let Some(map) = stats
            .get(key)
            .and_then(|c| c.get("counts"))
            .and_then(|c| c.as_object())
        else {
            return (0, 0);
        };
        let total = map.values().filter_map(|n| n.as_u64()).sum();
        let rust = map.get("Rust").and_then(|n| n.as_u64()).unwrap_or(0);
        (total, rust)
    };
    let (hits, rust_hits) = counts("cache_hits");
    let (misses, rust_misses) = counts("cache_misses");
    Some(ScratchSccacheStats {
        compile_requests: stats
            .get("compile_requests")
            .and_then(|n| n.as_u64())
            .unwrap_or(0),
        hits,
        misses,
        rust_hits,
        rust_misses,
        cache_size_bytes: v.get("cache_size").and_then(|n| n.as_u64()),
    })
}

// ---------------------------------------------------------------------------
// L2 の cache server（ADR-0075 D5 (b) / D6、Phase G3）
// ---------------------------------------------------------------------------

/// cache server の `/stats`（500 ms。応答が無い・読めなければ `None`）。
pub fn query_cache_stats(settings: &ScratchSettings) -> Option<ScratchCacheStats> {
    let (code, body) = scratch::http_get_local(
        settings.cache_server.port,
        "/stats",
        std::time::Duration::from_millis(500),
    )?;
    if code != 200 {
        return None;
    }
    serde_json::from_str(&body).ok()
}

/// `ScratchStatus.cache`。`fetch` なら `/stats` を問い合わせる（応答すれば `ready`）。
pub fn cache_view(settings: &ScratchSettings, fetch: bool) -> ScratchCacheView {
    let endpoint = scratch::cache_server_endpoint(settings);
    let sccache_mode = scratch::read_sccache_mode(&settings.pool());
    if !settings.enabled || !settings.cache_server.enabled {
        return ScratchCacheView {
            state: "disabled".to_string(),
            reason: Some(if settings.enabled {
                "[scratch.cache_server] enabled = false".to_string()
            } else {
                "scratch is disabled".to_string()
            }),
            endpoint,
            sccache_mode,
            stats: None,
        };
    }
    let stats = if fetch {
        query_cache_stats(settings)
    } else {
        None
    };
    let (state, reason) = match (&stats, fetch) {
        (Some(_), _) => ("ready", None),
        (None, true) => (
            "unavailable",
            Some(format!(
                "no cache server on 127.0.0.1:{} (celeris-scratch-cache.service)",
                settings.cache_server.port
            )),
        ),
        (None, false) => ("unavailable", Some("not queried".to_string())),
    };
    ScratchCacheView {
        state: state.to_string(),
        reason,
        endpoint,
        sccache_mode,
        stats,
    }
}

/// server に統計を問い合わせる（`Ready` のときだけ。直前にもう一度 port を確かめる: sccache の client は server が
/// 無いと起こしてしまうので、万一の起動に備えて server と同じ env も渡す）。
pub fn query_sccache_stats(
    settings: &ScratchSettings,
    state: &scratch::SccacheState,
) -> Option<ScratchSccacheStats> {
    state.wrapper()?;
    if !scratch::server_listening(settings.sccache.server_port) {
        return None;
    }
    let out = std::process::Command::new(&settings.sccache.binary)
        .args(["--show-stats", "--stats-format=json"])
        .envs(scratch::sccache_server_env(settings))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_sccache_stats(&String::from_utf8_lossy(&out.stdout))
}

/// GC の 1 回を記録用の view にする。
pub fn gc_view(
    plan: &GcPlan,
    executed: &Executed,
    emergency: bool,
    now: SystemTime,
) -> ScratchGcView {
    ScratchGcView {
        at: scratch::rfc3339(now),
        pressure: plan.pressure.as_str().to_string(),
        emergency,
        removed: executed
            .removed
            .iter()
            .map(|p| ScratchGcRemovedView {
                id: p.id.clone(),
                class: if p.seed {
                    "seed".to_string()
                } else {
                    p.class.label().to_string()
                },
                estimated_bytes: p.estimated_bytes,
                why: p.why.to_string(),
            })
            .collect(),
        reclaimed_bytes: executed.reclaimed_bytes,
    }
}

/// 1 回の GC（走査 → 計画 → rename）。dispatcher の tick と `celerisctl scratch gc` が使う。`dry_run` なら rename しない。
pub struct GcRun {
    pub scan: Scan,
    pub plan: GcPlan,
    pub fs: Option<(u64, u64)>,
    pub executed: Option<Executed>,
}

#[allow(clippy::too_many_arguments)]
pub fn run_gc(
    settings: &ScratchSettings,
    legacy: &[PathBuf],
    lookup: &dyn StatusLookup,
    has_db: bool,
    sizes: &HashMap<PathBuf, Measured>,
    min_free_bytes: u64,
    emergency: bool,
    extra_active_repo_keys: &BTreeSet<String>,
    dry_run: bool,
) -> GcRun {
    let now = SystemTime::now();
    let scan = scan(settings, legacy, lookup, has_db, sizes, now);
    let fs = fs_stats(settings.pool().root());
    let mut active = scan.active_repo_keys.clone();
    active.extend(extra_active_repo_keys.iter().cloned());
    let params = GcParams::from_settings(
        settings,
        now,
        min_free_bytes,
        fs.map(|f| f.1),
        emergency,
        active,
    );
    let plan = scratch::plan_gc(&scan.entries, &params);
    let executed = (!dry_run).then(|| execute(&settings.pool(), &plan, &scan, emergency));
    GcRun {
        scan,
        plan,
        fs,
        executed,
    }
}

/// P0 の owner の一覧（ディスク不足の通知の本文に足す）。
pub fn pinned_summary(scan: &Scan, plan: &GcPlan) -> String {
    let mut by: BTreeMap<String, u64> = BTreeMap::new();
    for e in scan
        .entries
        .iter()
        .filter(|e| e.in_pool && e.class == Class::Pinned)
    {
        by.insert(
            e.id.clone(),
            plan.estimated.get(&e.id).copied().unwrap_or(0),
        );
    }
    let gb = |b: u64| b as f64 / scratch::GIB as f64;
    let list: Vec<String> = by
        .iter()
        .map(|(k, v)| format!("{k} {:.1} GB", gb(*v)))
        .collect();
    format!(
        "scratch pool: pinned {:.1} GB（{}）",
        gb(plan.pinned_bytes),
        if list.is_empty() {
            "なし".to_string()
        } else {
            list.join(", ")
        }
    )
}

/// 測定の間隔の既定（テスト用に分けておく）。
pub fn measure_interval(settings: &ScratchSettings) -> Duration {
    Duration::from_secs(settings.measure_interval_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_worker::scratch::{AllocateRequest, NoDb};

    #[test]
    fn effective_max_shrinks_with_usage_outside_the_pool() {
        let g = scratch::GIB;
        // 252G の fs、空き 91G、pool 20G → 外 141G → 252-141-5 = 106G。
        assert_eq!(
            effective_max(150 * g, Some((252 * g, 91 * g)), 20 * g, 5 * g),
            106 * g
        );
        assert_eq!(effective_max(150 * g, None, 0, 0), 150 * g);
        assert_eq!(
            effective_max(150 * g, Some((1000 * g, 900 * g)), 0, 0),
            150 * g
        );
    }

    #[test]
    fn gc_dry_run_lists_without_removing_and_execute_renames() {
        let tmp = tempfile::tempdir().unwrap();
        let s = ScratchSettings::with_dir(tmp.path().join("scratch"));
        let pool = s.pool();
        let owner = Owner::parse("release-0123456789ab").unwrap();
        let none = |_: &AdoptCandidate| None;
        scratch::allocate(
            &pool,
            &AllocateRequest {
                owner: &owner,
                repo_path: Path::new("/repo/x"),
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: &none,
                adopt: false,
                max_distance: 0,
            },
        )
        .unwrap();
        scratch::release(&pool, &owner).unwrap();
        let run = run_gc(
            &s,
            &[],
            &NoDb,
            false,
            &HashMap::new(),
            0,
            false,
            &BTreeSet::new(),
            true,
        );
        assert_eq!(run.plan.selected.len(), 1);
        assert!(run.executed.is_none());
        assert!(pool.target_dir(&owner).exists());
        let run = run_gc(
            &s,
            &[],
            &NoDb,
            false,
            &HashMap::new(),
            0,
            false,
            &BTreeSet::new(),
            false,
        );
        assert_eq!(run.executed.unwrap().removed.len(), 1);
        assert!(!pool.target_dir(&owner).exists());
        // lease は「刈った」記録として残る。
        assert!(pool.lease_path(&owner).exists());
        let roots = deleting_roots(&pool, &[]);
        assert!(has_pending_deletes(&roots));
        let busy = Arc::new(AtomicBool::new(false));
        spawn_removal(roots.clone(), busy.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while (busy.load(Ordering::SeqCst) || has_pending_deletes(&roots))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!has_pending_deletes(&roots));
    }

    #[test]
    fn legacy_paths_list_build_cache_entries_and_the_release_target() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("build-cache");
        std::fs::create_dir_all(cache.join("cargo/agent-platform-abc")).unwrap();
        std::fs::create_dir_all(cache.join("cargo/.deleting-x")).unwrap();
        std::fs::write(cache.join("cargo/file"), "x").unwrap();
        let releases = tmp.path().join("releases");
        let real = tmp.path().join("release-build/.cargo-target");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&releases).unwrap();
        std::os::unix::fs::symlink(&real, releases.join(".cargo-target")).unwrap();
        let got = legacy_paths(&cache, Some(&releases));
        assert_eq!(got, vec![cache.join("cargo/agent-platform-abc"), real]);
    }

    #[test]
    fn measurement_is_cached_and_written_to_the_lease_without_touching_it() {
        let tmp = tempfile::tempdir().unwrap();
        let s = ScratchSettings::with_dir(tmp.path());
        let pool = s.pool();
        let owner = Owner::parse("agent-x").unwrap();
        let none = |_: &AdoptCandidate| None;
        let a = scratch::allocate(
            &pool,
            &AllocateRequest {
                owner: &owner,
                repo_path: Path::new("/repo/x"),
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: &none,
                adopt: false,
                max_distance: 0,
            },
        )
        .unwrap();
        std::fs::write(a.target_dir.join("blob"), vec![1u8; 8192]).unwrap();
        let sizes: SizeCache = Arc::new(Mutex::new(HashMap::new()));
        let scan1 = scan(&s, &[], &NoDb, false, &HashMap::new(), SystemTime::now());
        let (path, lease) = next_to_measure(&scan1, &HashMap::new()).unwrap();
        assert_eq!(path, a.target_dir);
        let before = scratch::mtime(&pool.lease_path(&owner));
        measure_one(&path, lease.as_deref(), &sizes);
        assert!(sizes.lock().unwrap()[&path].bytes >= 8192);
        let l = scratch::read_lease(&pool.lease_path(&owner))
            .unwrap()
            .unwrap();
        assert!(l.size_bytes.unwrap() >= 8192);
        assert_eq!(scratch::mtime(&pool.lease_path(&owner)), before);
    }

    /// ADR-0075 D6（Phase G2）: `sccache --show-stats --stats-format=json`（0.18 の形）の要約。
    #[test]
    fn sccache_stats_summary_reads_the_json_shape_of_0_18() {
        let json = r#"{"stats":{"compile_requests":624,"requests_executed":576,
            "cache_hits":{"counts":{"Rust":163,"C/C++":261,"Assembler":121},"adv_counts":{}},
            "cache_misses":{"counts":{"Rust":29},"adv_counts":{}},"multi_level":null},
            "cache_location":"Local disk: \"/x\"","cache_size":1073741824,"max_cache_size":21474836480,"version":"0.18.0"}"#;
        let s = parse_sccache_stats(json).unwrap();
        assert_eq!(
            s,
            ScratchSccacheStats {
                compile_requests: 624,
                hits: 545,
                misses: 29,
                rust_hits: 163,
                rust_misses: 29,
                cache_size_bytes: Some(1_073_741_824),
            }
        );
        // 起動直後（counts が空、cache_size が null）。
        let s = parse_sccache_stats(
            r#"{"stats":{"compile_requests":0,"cache_hits":{"counts":{}},"cache_misses":{"counts":{}}},"cache_size":null}"#,
        )
        .unwrap();
        assert_eq!((s.hits, s.misses, s.cache_size_bytes), (0, 0, None));
        assert!(parse_sccache_stats("not json").is_none());
    }
}
