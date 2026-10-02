//! ADR-0075 D1〜D3（Phase G1）: ローカルの scratch pool。
//!
//! `<scratch>/targets/<owner>/{lease.json,target/}` の owner とパス、`lease.json` の読み書き、`.lock` の flock、
//! 割り当て（`allocate`。adopt〈rename による引き継ぎ〉を含む）、分類（`classify`）、削除順を決める純粋関数
//! `plan_gc`、adopt の候補選び `choose_adopt` をここに置く。**LLM は呼ばない**（DESIGN 原則 1）。
//! pool の走査・DB の状態の読み出し・削除スレッド・測定スレッドは `task-dispatch` の `scratch_gc` が持つ
//! （dispatcher の tick と `celerisctl scratch` の両方が使う）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use task_core::Status;
use task_core::execution_plan::WorkUnitStatus;

/// `lease.json` の schema 名（ADR-0075 D1）。
pub const LEASE_SCHEMA: &str = "celeris.scratch-lease/1";
/// `scratch status --json` と `GET /api/v1/metrics/scratch` の schema 名（D6）。
pub const STATUS_SCHEMA: &str = "celeris.scratch-status/1";
pub const LEASE_FILE: &str = "lease.json";
/// owner のディレクトリの中の `CARGO_TARGET_DIR`。
pub const TARGET_SUBDIR: &str = "target";
pub const TARGETS_DIR: &str = "targets";
pub const LOCK_FILE: &str = ".lock";
/// GC が rename した削除待ち（`targets/.deleting-<owner-flat>-<nanos>`）。
pub const DELETING_PREFIX: &str = ".deleting-";
/// lease の無いディレクトリ・legacy の target を「使われていない」とみなすまでの秒数（D2 / D7。作りかけと競合しない）。
pub const IDLE_SECS: u64 = 3600;
/// P3 で target を刈った後の lease だけのディレクトリを片づけるまでの秒数（G1 の明確化。記録を 1 週間残す）。
pub const LEASE_RECORD_KEEP_SECS: u64 = 7 * 86400;

/// `CARGO_TARGET_DIR`（`build_cache` と同じ名前）。
pub use crate::build_cache::CARGO_TARGET_DIR_VAR;

// ---------------------------------------------------------------------------
// owner
// ---------------------------------------------------------------------------

/// owner の種類（`lease.json` の `kind`）。
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Task,
    WorkUnit,
    Release,
    Agent,
}

impl OwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerKind::Task => "task",
            OwnerKind::WorkUnit => "work_unit",
            OwnerKind::Release => "release",
            OwnerKind::Agent => "agent",
        }
    }

    /// daemon の DB の状態で生存が決まる owner（task / work_unit）。外部の owner（release / agent）は lease の mtime。
    pub fn is_daemon_owned(self) -> bool {
        matches!(self, OwnerKind::Task | OwnerKind::WorkUnit)
    }
}

/// ADR-0075 D1: `task-<task_id>` / `task-<task_id>/wu-<work_unit_id>` / `release-<sha12>` / `agent-<name>`。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    Task {
        task_id: String,
    },
    WorkUnit {
        task_id: String,
        work_unit_id: String,
    },
    Release {
        sha12: String,
    },
    Agent {
        name: String,
    },
}

fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl Owner {
    pub fn task(task_id: impl Into<String>) -> Self {
        Owner::Task {
            task_id: task_id.into(),
        }
    }

    pub fn work_unit(task_id: impl Into<String>, work_unit_id: impl Into<String>) -> Self {
        Owner::WorkUnit {
            task_id: task_id.into(),
            work_unit_id: work_unit_id.into(),
        }
    }

    /// `task-<id>` / `task-<id>/wu-<id>` / `release-<sha12>` / `agent-<name>` を読む。
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "invalid scratch owner {s:?} (expected task-<id>, task-<id>/wu-<id>, release-<sha12> or agent-<name>)"
            )
        };
        let owner = if let Some(rest) = s.strip_prefix("task-") {
            match rest.split_once('/') {
                None => Owner::task(rest),
                Some((task, wu)) => {
                    let wu = wu.strip_prefix("wu-").ok_or_else(bad)?;
                    Owner::work_unit(task, wu)
                }
            }
        } else if let Some(rest) = s.strip_prefix("release-") {
            Owner::Release {
                sha12: rest.to_string(),
            }
        } else if let Some(rest) = s.strip_prefix("agent-") {
            Owner::Agent {
                name: rest.to_string(),
            }
        } else {
            return Err(bad());
        };
        let ok = match &owner {
            Owner::Task { task_id } => valid_component(task_id),
            Owner::WorkUnit {
                task_id,
                work_unit_id,
            } => valid_component(task_id) && valid_component(work_unit_id),
            Owner::Release { sha12 } => valid_component(sha12),
            Owner::Agent { name } => valid_component(name),
        };
        if ok { Ok(owner) } else { Err(bad()) }
    }

    pub fn kind(&self) -> OwnerKind {
        match self {
            Owner::Task { .. } => OwnerKind::Task,
            Owner::WorkUnit { .. } => OwnerKind::WorkUnit,
            Owner::Release { .. } => OwnerKind::Release,
            Owner::Agent { .. } => OwnerKind::Agent,
        }
    }

    /// `targets/` からの相対パス（WU は Task の下に入れ子）。
    pub fn relative_dir(&self) -> PathBuf {
        match self {
            Owner::Task { task_id } => PathBuf::from(format!("task-{task_id}")),
            Owner::WorkUnit {
                task_id,
                work_unit_id,
            } => PathBuf::from(format!("task-{task_id}")).join(format!("wu-{work_unit_id}")),
            Owner::Release { sha12 } => PathBuf::from(format!("release-{sha12}")),
            Owner::Agent { name } => PathBuf::from(format!("agent-{name}")),
        }
    }

    /// `.deleting-*` の名前に使う平らな形（`/` → `__`）。
    pub fn flat(&self) -> String {
        self.to_string().replace('/', "__")
    }

    /// この owner の Task（task / work_unit のときだけ）。
    pub fn task_id(&self) -> Option<&str> {
        match self {
            Owner::Task { task_id } | Owner::WorkUnit { task_id, .. } => Some(task_id),
            _ => None,
        }
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Owner::Task { task_id } => write!(f, "task-{task_id}"),
            Owner::WorkUnit {
                task_id,
                work_unit_id,
            } => write!(f, "task-{task_id}/wu-{work_unit_id}"),
            Owner::Release { sha12 } => write!(f, "release-{sha12}"),
            Owner::Agent { name } => write!(f, "agent-{name}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 設定（`celeris::config` の `[scratch]` を解決した値）
// ---------------------------------------------------------------------------

/// ADR-0075 D1 / D2 / D3: `[scratch]` を解決した値。`enabled = false`（設定で無効、または NFS 上で無効化）なら
/// dispatcher は ADR-0066 D1 / F5-fix の `build_cache_dir` の挙動に戻る。
#[derive(Debug, Clone, PartialEq)]
pub struct ScratchSettings {
    pub enabled: bool,
    /// 無効化した理由（NFS 上など。設定で `enabled = false` と書いたときは `None`）。
    pub disabled_reason: Option<String>,
    pub dir: PathBuf,
    pub targets_max_bytes: u64,
    pub total_max_bytes: u64,
    pub high_watermark: f64,
    pub low_watermark: f64,
    pub external_lease_ttl_secs: u64,
    pub waiting_keep_secs: u64,
    pub failed_keep_secs: u64,
    pub completed_grace_secs: u64,
    pub warm_seeds_per_repo: usize,
    pub gc_max_per_tick: usize,
    pub adopt: bool,
    pub adopt_max_distance: u64,
    /// 測定スレッドが owner を 1 つ測る間隔（D2。既定 30 秒）。
    pub measure_interval_secs: u64,
    /// ADR-0075 D4（Phase G2）: `[scratch.cargo]`。
    pub cargo: CargoTuning,
}

/// ADR-0075 D4（Phase G2）: `[scratch.cargo]` を解決した値。scratch が有効な経路に常に与える。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoTuning {
    /// `false`（既定）なら `CARGO_INCREMENTAL=0`。`true` なら env を与えない（cargo の既定に任せる）。
    pub incremental: bool,
    /// `CARGO_PROFILE_DEV_DEBUG` の値（既定 `line-tables-only`）。`None` なら与えない。
    pub dev_debug: Option<String>,
}

impl Default for CargoTuning {
    fn default() -> Self {
        Self {
            incremental: false,
            dev_debug: Some(DEFAULT_DEV_DEBUG.to_string()),
        }
    }
}

/// `[scratch.cargo] dev_debug` の既定（D4）。
pub const DEFAULT_DEV_DEBUG: &str = "line-tables-only";

pub const GIB: u64 = 1024 * 1024 * 1024;

impl ScratchSettings {
    /// 既定値（ADR-0075 の表）で `dir` を指す有効な設定。
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            enabled: true,
            disabled_reason: None,
            dir: dir.into(),
            targets_max_bytes: 100 * GIB,
            total_max_bytes: 150 * GIB,
            high_watermark: 0.90,
            low_watermark: 0.70,
            external_lease_ttl_secs: 21_600,
            waiting_keep_secs: 172_800,
            failed_keep_secs: 86_400,
            completed_grace_secs: 600,
            warm_seeds_per_repo: 1,
            gc_max_per_tick: 8,
            adopt: true,
            adopt_max_distance: 200,
            measure_interval_secs: 30,
            cargo: CargoTuning::default(),
        }
    }

    /// 無効（F5-fix の挙動）。テストと、`[scratch]` を持たない構成の既定。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::with_dir(PathBuf::new())
        }
    }

    pub fn pool(&self) -> Pool {
        Pool::new(&self.dir)
    }
}

impl Default for ScratchSettings {
    fn default() -> Self {
        Self::disabled()
    }
}

/// ADR-0075 D1: path（無ければ存在する一番近い祖先）が NFS 上か（`statfs` の `f_type == NFS_SUPER_MAGIC`）。
pub fn is_on_nfs(path: &Path) -> io::Result<bool> {
    let existing = path
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(|| io::Error::other(format!("{}: no existing ancestor", path.display())))?;
    let st = nix::sys::statfs::statfs(existing).map_err(io::Error::from)?;
    Ok(st.filesystem_type() == nix::sys::statfs::NFS_SUPER_MAGIC)
}

/// 起動時の検査（D1）: `dir` が NFS 上なら無効化して理由を返す。`probe` は `is_on_nfs`
/// （テストでは差し替える）。検査できないときは有効のまま（ローカルを仮定しない理由が無い。ログは呼び出し側）。
pub fn apply_nfs_check(
    mut settings: ScratchSettings,
    probe: impl Fn(&Path) -> io::Result<bool>,
) -> ScratchSettings {
    if !settings.enabled {
        return settings;
    }
    if let Ok(true) = probe(&settings.dir) {
        settings.enabled = false;
        settings.disabled_reason = Some(format!(
            "scratch dir {} is on NFS; falling back to [workspace] build_cache_dir (ADR-0075 D1)",
            settings.dir.display()
        ));
    }
    settings
}

// ---------------------------------------------------------------------------
// pool
// ---------------------------------------------------------------------------

/// `<scratch>`（`[scratch] dir`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    root: PathBuf,
}

/// `.lock` の flock（drop で解放）。lease の作成・adopt・rename-to-delete を直列化する（短時間だけ持つ）。
pub struct PoolLock {
    _file: std::fs::File,
}

impl Pool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn targets_dir(&self) -> PathBuf {
        self.root.join(TARGETS_DIR)
    }
    pub fn owner_dir(&self, owner: &Owner) -> PathBuf {
        self.targets_dir().join(owner.relative_dir())
    }
    /// `CARGO_TARGET_DIR`（`<scratch>/targets/<owner>/target`）。
    pub fn target_dir(&self, owner: &Owner) -> PathBuf {
        self.owner_dir(owner).join(TARGET_SUBDIR)
    }
    pub fn lease_path(&self, owner: &Owner) -> PathBuf {
        self.owner_dir(owner).join(LEASE_FILE)
    }

    /// `.lock` を排他で取る（無ければ作る）。
    pub fn lock(&self) -> io::Result<PoolLock> {
        std::fs::create_dir_all(&self.root)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join(LOCK_FILE))?;
        file.lock()?;
        Ok(PoolLock { _file: file })
    }

    /// `targets/` の下の owner のディレクトリを列挙する（`task-*` の中の `wu-*` も）。`.deleting-*` は除く。
    /// 名前が owner として読めないディレクトリは `Err(path)` で返す（野良）。
    pub fn list_owner_dirs(&self) -> Vec<Result<Owner, PathBuf>> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.targets_dir()) else {
            return out;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(DELETING_PREFIX) || name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !is_dir {
                continue;
            }
            match Owner::parse(&name) {
                Ok(owner) => {
                    if let Owner::Task { task_id } = &owner
                        && let Ok(children) = std::fs::read_dir(entry.path())
                    {
                        let mut wus: Vec<_> = children
                            .flatten()
                            .filter(|c| c.file_type().map(|t| t.is_dir()).unwrap_or(false))
                            .filter_map(|c| {
                                let n = c.file_name().to_string_lossy().into_owned();
                                n.strip_prefix("wu-").map(str::to_string)
                            })
                            .collect();
                        wus.sort();
                        out.push(Ok(owner.clone()));
                        for wu in wus {
                            match Owner::parse(&format!("task-{task_id}/wu-{wu}")) {
                                Ok(o) => out.push(Ok(o)),
                                Err(_) => out.push(Err(entry.path().join(format!("wu-{wu}")))),
                            }
                        }
                        continue;
                    }
                    out.push(Ok(owner));
                }
                Err(_) => out.push(Err(entry.path())),
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// lease.json
// ---------------------------------------------------------------------------

/// `lease.json`（`celeris.scratch-lease/1`）。生存の合図は**ファイルの mtime**（touch）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub schema: String,
    pub owner: String,
    pub kind: OwnerKind,
    pub repo_key: String,
    pub repo_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_key: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_from: Option<String>,
    /// 外部の owner の lease の TTL（`celerisctl scratch lease --ttl`。無ければ `[scratch] external_lease_ttl_secs`）。
    /// G1 の明確化（ADR-0075 D1 の欄に足した）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
}

pub fn rfc3339(t: SystemTime) -> String {
    let odt = time::OffsetDateTime::from(t);
    odt.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| odt.unix_timestamp().to_string())
}

pub fn read_lease(path: &Path) -> io::Result<Option<Lease>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// tmp → rename で書く（mtime は今になる＝touch を兼ねる）。
pub fn write_lease(path: &Path, lease: &Lease) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("lease path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".lease.json.tmp-{}", std::process::id()));
    let body = serde_json::to_vec_pretty(lease).map_err(io::Error::other)?;
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

/// 測定結果（`size_bytes` / `measured_at`）を書く。**mtime は元に戻す**（mtime は外部の owner の生存の合図で、
/// 測定で延命させない。ADR-0075 D2 の明確化）。
pub fn write_lease_measurement(path: &Path, size_bytes: u64, at: SystemTime) -> io::Result<()> {
    let Some(mut lease) = read_lease(path)? else {
        return Ok(());
    };
    let mtime = std::fs::metadata(path)?.modified()?;
    lease.size_bytes = Some(size_bytes);
    lease.measured_at = Some(rfc3339(at));
    write_lease(path, &lease)?;
    set_mtime(path, mtime)
}

pub fn set_mtime(path: &Path, t: SystemTime) -> io::Result<()> {
    let file = std::fs::OpenOptions::new().write(true).open(path)?;
    file.set_times(std::fs::FileTimes::new().set_modified(t))
}

pub fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
}

/// 候補の target の最終書き込み（D3）: `lease.json` の mtime と、`target/` とその直下の mtime の最も新しいもの。
pub fn last_write(owner_dir: &Path) -> Option<SystemTime> {
    let mut latest = mtime(&owner_dir.join(LEASE_FILE));
    let target = owner_dir.join(TARGET_SUBDIR);
    let mut bump = |t: Option<SystemTime>| {
        if let Some(t) = t {
            latest = Some(latest.map_or(t, |l| l.max(t)));
        }
    };
    bump(mtime(&target));
    if let Ok(entries) = std::fs::read_dir(&target) {
        for e in entries.flatten() {
            bump(e.metadata().ok().and_then(|m| m.modified().ok()));
        }
    }
    latest
}

/// 引き継ぐ側の worktree の checkout 時刻（`<worktree>/.git` の mtime。`git worktree add` は `.git` を書いてから
/// ファイルを checkout するので、checkout されたソースは全てこれ以降の mtime になる）。
pub fn checkout_time(worktree_dir: &Path) -> Option<SystemTime> {
    mtime(&worktree_dir.join(".git"))
}

// ---------------------------------------------------------------------------
// 割り当て（D3）
// ---------------------------------------------------------------------------

/// adopt の候補（GC の直近の走査で P3 / seed に分類された owner の target）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptCandidate {
    pub owner: Owner,
    pub repo_key: String,
    pub base_commit: Option<String>,
    pub last_write: SystemTime,
    /// 走査したときの `lease.json` の mtime。割り当て時に変わっていたら（再び使われ始めた）候補から外す。
    pub lease_mtime: Option<SystemTime>,
}

/// ADR-0075 D3: 安全条件（候補の最終書き込み < checkout 時刻）を満たす同じ repo の候補のうち、commit の距離が
/// `max_distance` 以内で最小のもの（同じ距離なら新しいもの）、無ければ最も新しいもの。`distance` は呼び出し側
/// （git を使う経路）が計算する（`None` = 不明）。
pub fn choose_adopt<'a>(
    candidates: &'a [AdoptCandidate],
    repo_key: &str,
    checkout: SystemTime,
    distance: &dyn Fn(&AdoptCandidate) -> Option<u64>,
    max_distance: u64,
) -> Option<&'a AdoptCandidate> {
    let safe: Vec<&AdoptCandidate> = candidates
        .iter()
        .filter(|c| c.repo_key == repo_key && c.last_write < checkout)
        .collect();
    let newest = |a: &&AdoptCandidate, b: &&AdoptCandidate| {
        a.last_write
            .cmp(&b.last_write)
            .then_with(|| b.owner.cmp(&a.owner))
    };
    let near = safe
        .iter()
        .filter_map(|c| distance(c).filter(|d| *d <= max_distance).map(|d| (d, *c)))
        .min_by(|(da, a), (db, b)| da.cmp(db).then_with(|| newest(b, a)));
    match near {
        Some((_, c)) => Some(c),
        None => safe.into_iter().max_by(newest),
    }
}

/// `allocate` の入力。
pub struct AllocateRequest<'a> {
    pub owner: &'a Owner,
    pub repo_path: &'a Path,
    pub base_commit: Option<String>,
    pub work_unit_key: Option<String>,
    /// 引き継ぐ側の worktree の checkout 時刻。`None` なら adopt しない。
    pub checkout: Option<SystemTime>,
    pub candidates: &'a [AdoptCandidate],
    pub distance: &'a dyn Fn(&AdoptCandidate) -> Option<u64>,
    pub adopt: bool,
    pub max_distance: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    pub target_dir: PathBuf,
    pub adopted_from: Option<String>,
    /// lease を新しく作った（既にあったなら false）。
    pub created: bool,
}

/// ADR-0075 D3: owner の lease を作る（あれば touch し `released_at` を消す）。`target/` がまだ無ければ adopt を試み、
/// 無理なら空の `target/` を作る。`.lock` を持って行う。
pub fn allocate(pool: &Pool, req: &AllocateRequest<'_>) -> io::Result<Allocation> {
    let _lock = pool.lock()?;
    let owner_dir = pool.owner_dir(req.owner);
    let lease_path = owner_dir.join(LEASE_FILE);
    let target = owner_dir.join(TARGET_SUBDIR);
    std::fs::create_dir_all(&owner_dir)?;
    let repo_key = crate::build_cache::repo_cache_key(req.repo_path);
    let existing = read_lease(&lease_path).ok().flatten();
    let created = existing.is_none();
    let mut lease = existing.unwrap_or_else(|| Lease {
        schema: LEASE_SCHEMA.to_string(),
        owner: req.owner.to_string(),
        kind: req.owner.kind(),
        repo_key: repo_key.clone(),
        repo_path: req.repo_path.display().to_string(),
        base_commit: None,
        work_unit_key: None,
        created_at: rfc3339(SystemTime::now()),
        released_at: None,
        size_bytes: None,
        measured_at: None,
        adopted_from: None,
        ttl_secs: None,
    });
    lease.released_at = None;
    if req.base_commit.is_some() {
        lease.base_commit = req.base_commit.clone();
    }
    if req.work_unit_key.is_some() {
        lease.work_unit_key = req.work_unit_key.clone();
    }
    let mut adopted_from = None;
    if !target.exists()
        && req.adopt
        && let Some(checkout) = req.checkout
    {
        // 走査の後に再び使われ始めた候補（lease の mtime が変わった）・消えた候補は外す。
        let live: Vec<AdoptCandidate> = req
            .candidates
            .iter()
            .filter(|c| &c.owner != req.owner)
            .filter(|c| mtime(&pool.lease_path(&c.owner)) == c.lease_mtime)
            .filter(|c| pool.target_dir(&c.owner).is_dir())
            .filter_map(|c| {
                // 最終書き込みは今の値で見直す（走査の後に書かれていたら安全条件を満たさないかもしれない）。
                let lw = last_write(&pool.owner_dir(&c.owner))?;
                Some(AdoptCandidate {
                    last_write: lw,
                    ..c.clone()
                })
            })
            .collect();
        if let Some(c) = choose_adopt(&live, &repo_key, checkout, req.distance, req.max_distance) {
            let from = pool.target_dir(&c.owner);
            match std::fs::rename(&from, &target) {
                Ok(()) => {
                    adopted_from = Some(c.owner.to_string());
                    lease.adopted_from = adopted_from.clone();
                    lease.size_bytes = None;
                    lease.measured_at = None;
                }
                Err(e) => {
                    tracing::warn!(from = %from.display(), to = %target.display(), error = %e, "scratch: adopt failed; starting from an empty target");
                }
            }
        }
    }
    std::fs::create_dir_all(&target)?;
    write_lease(&lease_path, &lease)?;
    Ok(Allocation {
        target_dir: target,
        adopted_from,
        created,
    })
}

/// 外部の owner の lease（`celerisctl scratch lease`）。adopt は呼び出し側が候補を渡したときだけ。
pub fn touch(pool: &Pool, owner: &Owner) -> io::Result<bool> {
    let _lock = pool.lock()?;
    let path = pool.lease_path(owner);
    if !path.exists() {
        return Ok(false);
    }
    set_mtime(&path, SystemTime::now())?;
    Ok(true)
}

/// 外部の lease の TTL を書く（`celerisctl scratch lease --ttl`）。
pub fn set_ttl(pool: &Pool, owner: &Owner, ttl_secs: Option<u64>) -> io::Result<bool> {
    let _lock = pool.lock()?;
    let path = pool.lease_path(owner);
    let Some(mut lease) = read_lease(&path)? else {
        return Ok(false);
    };
    lease.ttl_secs = ttl_secs;
    write_lease(&path, &lease)?;
    Ok(true)
}

/// `released_at` を書き、P3 に落とす。
pub fn release(pool: &Pool, owner: &Owner) -> io::Result<bool> {
    let _lock = pool.lock()?;
    let path = pool.lease_path(owner);
    let Some(mut lease) = read_lease(&path)? else {
        return Ok(false);
    };
    lease.released_at = Some(rfc3339(SystemTime::now()));
    write_lease(&path, &lease)?;
    Ok(true)
}

/// `CARGO_TARGET_DIR` だけ（G1 の値。`cargo_env` の先頭）。
pub fn target_env(pool: &Pool, owner: &Owner) -> Vec<(String, String)> {
    vec![(
        CARGO_TARGET_DIR_VAR.to_string(),
        pool.target_dir(owner).display().to_string(),
    )]
}

/// `[scratch.cargo]` の env（`CARGO_INCREMENTAL=0`、`CARGO_PROFILE_DEV_DEBUG`）。
pub fn cargo_tuning_env(tuning: &CargoTuning) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if !tuning.incremental {
        env.push(("CARGO_INCREMENTAL".to_string(), "0".to_string()));
    }
    if let Some(v) = tuning.dev_debug.as_deref().filter(|v| !v.is_empty()) {
        env.push(("CARGO_PROFILE_DEV_DEBUG".to_string(), v.to_string()));
    }
    env
}

/// 経路に渡す env（D3 / D4）。dispatcher（run・WU の checks・統合の検査・reviewer の checks）と
/// `celerisctl scratch env` の両方がこれを使う（ADR-0075 D4 の「env は一か所で組む」）。順序は固定:
/// `CARGO_TARGET_DIR`、`[scratch.cargo]`。ADR-0129 (1): `RUSTC_WRAPPER` / `SCCACHE_*` は足さない
/// （compiler wrapper は host の cargo 設定に任せる）。
pub fn cargo_env(settings: &ScratchSettings, owner: &Owner) -> Vec<(String, String)> {
    let mut env = target_env(&settings.pool(), owner);
    env.extend(cargo_tuning_env(&settings.cargo));
    env
}

/// 経路の子プロセスに重ねる env（`set` = `cargo_env` と同じ値）。ADR-0129 (1): 親（daemon・人の shell）から
/// 継いだ env は外さない（`remove` の欄は持たない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoEnv {
    pub set: Vec<(String, String)>,
}

impl CargoEnv {
    pub fn set_only(set: Vec<(String, String)>) -> Self {
        Self { set }
    }
}

/// `cargo_env` を `CargoEnv` に包んだもの（dispatcher と `celerisctl scratch env` が使う）。
pub fn cargo_child_env(settings: &ScratchSettings, owner: &Owner) -> CargoEnv {
    CargoEnv::set_only(cargo_env(settings, owner))
}

// Lease と cargo 環境の管理から GC の分類・回収計画を分離する。
mod gc;
pub use gc::*;

#[cfg(test)]
mod tests;
