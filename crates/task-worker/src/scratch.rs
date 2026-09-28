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
/// G2 の sccache の local disk cache（G1 では作るだけ・使わない）。
pub const L1_DIR: &str = "sccache-l1";
/// ADR-0075 D5 (b)（Phase G3）: cache server の L1。
pub const CACHE_L1_DIR: &str = "cache-l1";
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
    pub l1_max_bytes: u64,
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
    /// ADR-0075 D4（Phase G2）: `[scratch.sccache]`。
    pub sccache: SccacheSettings,
    /// ADR-0075 D4（Phase G2）: `[scratch.cargo]`。
    pub cargo: CargoTuning,
    /// ADR-0075 D5 (b)（Phase G3）: `[scratch.l2]`。
    pub l2: L2Settings,
    /// ADR-0075 D5 (b)（Phase G3）: `[scratch.cache_server]`。
    pub cache_server: CacheServerSettings,
}

/// ADR-0075 D5 (b)（Phase G3）: `[scratch.l2]` を解決した値（L2 = NFS 上の content-addressed な immutable object）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Settings {
    pub enabled: bool,
    /// 既定 `$CELERIS_STATE_DIR/cache/sccache-l2`（NFS）。
    pub dir: PathBuf,
    pub max_bytes: u64,
    /// flusher の帯域（MB/s = 10^6 byte / 秒。0 = 無制限）。
    pub flush_mbps: u64,
    pub flush_queue_max_mb: u64,
    pub get_timeout_ms: u64,
    pub io_threads: usize,
    pub gc_interval_secs: u64,
}

impl L2Settings {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            dir: PathBuf::new(),
            max_bytes: 300 * GIB,
            flush_mbps: 25,
            flush_queue_max_mb: 4096,
            get_timeout_ms: 500,
            io_threads: 4,
            gc_interval_secs: 86_400,
        }
    }
}

/// ADR-0075 D5 (b)（Phase G3）: `[scratch.cache_server]` を解決した値（`celeris cache-server`、loopback だけ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheServerSettings {
    pub enabled: bool,
    pub port: u16,
    /// DAV の Bearer token（`SCCACHE_WEBDAV_TOKEN`）。既定 `<scratch>/cache-server.token`（cache server が初回に作る）。
    pub token_file: PathBuf,
}

/// cache server の既定の port（D5）。
pub const DEFAULT_CACHE_SERVER_PORT: u16 = 4237;

impl CacheServerSettings {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_CACHE_SERVER_PORT,
            token_file: PathBuf::new(),
        }
    }
}

/// ADR-0075 D4（Phase G2）: `[scratch.sccache]` を解決した値。`enabled` でも `binary` が無い・server が応答しない
/// なら配線しない（`resolve_sccache`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SccacheSettings {
    pub enabled: bool,
    /// 本物の sccache（既定 `$CELERIS_STATE_DIR/tools/sccache/bin/sccache`）。`RUSTC_WRAPPER` にはこれを包む
    /// `<scratch>/bin/sccache` を与える（`wrapper_script`）。
    pub binary: PathBuf,
    /// `SCCACHE_SERVER_PORT`（既定 4236。人が自分で使う sccache の既定 4226 と分ける）。
    pub server_port: u16,
}

/// sccache の server の既定の port（D4）。
pub const DEFAULT_SCCACHE_PORT: u16 = 4236;

impl SccacheSettings {
    /// 配線しない設定（テストと、sccache を使わない構成）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            binary: PathBuf::new(),
            server_port: DEFAULT_SCCACHE_PORT,
        }
    }
}

/// ADR-0075 D4（Phase G2）: `[scratch.cargo]` を解決した値。scratch が有効な経路に常に与える（sccache の有無に依らない）。
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
    /// 既定値（ADR-0075 の表）で `dir` を指す有効な設定。sccache は配線しない（`[scratch.sccache]` の既定は
    /// `celeris::config` が解決する。テストが手元の sccache の server を拾わないため）。
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            enabled: true,
            disabled_reason: None,
            dir: dir.into(),
            targets_max_bytes: 100 * GIB,
            l1_max_bytes: 40 * GIB,
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
            sccache: SccacheSettings::disabled(),
            cargo: CargoTuning::default(),
            l2: L2Settings::disabled(),
            cache_server: CacheServerSettings::disabled(),
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

/// 起動時の検査（D1）: `dir` と `sccache-l1/` が NFS 上なら無効化して理由を返す。`probe` は `is_on_nfs`
/// （テストでは差し替える）。検査できないときは有効のまま（ローカルを仮定しない理由が無い。ログは呼び出し側）。
pub fn apply_nfs_check(
    mut settings: ScratchSettings,
    probe: impl Fn(&Path) -> io::Result<bool>,
) -> ScratchSettings {
    if !settings.enabled {
        return settings;
    }
    for path in [settings.dir.clone(), settings.dir.join(L1_DIR)] {
        if let Ok(true) = probe(&path) {
            settings.enabled = false;
            settings.disabled_reason = Some(format!(
                "scratch dir {} is on NFS; falling back to [workspace] build_cache_dir (ADR-0075 D1)",
                path.display()
            ));
            return settings;
        }
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
    pub fn l1_dir(&self) -> PathBuf {
        self.root.join(L1_DIR)
    }
    /// ADR-0075 D5 (b)（Phase G3）: cache server の L1（G2 の sccache の disk cache `sccache-l1/` とは分ける）。
    pub fn cache_l1_dir(&self) -> PathBuf {
        self.root.join(CACHE_L1_DIR)
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

// ---------------------------------------------------------------------------
// sccache L1（D4、Phase G2）
// ---------------------------------------------------------------------------

/// `<scratch>/bin/`（Celeris が生成する wrapper の置き場。`targets/` の外なので GC の対象にならない）。
pub const WRAPPER_DIR: &str = "bin";
/// wrapper のファイル名。**`sccache` でなければならない**: cc-rs は `RUSTC_WRAPPER` の stem が `sccache` のときだけ
/// C/C++ にも同じ wrapper を使う（G2 の U1 の測定）。
pub const WRAPPER_NAME: &str = "sccache";

/// sccache の配線の状態（`resolve_sccache` の結果）。`scratch status` と起動ログに出す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SccacheState {
    /// 配線する。`wrapper` = `RUSTC_WRAPPER` に与える `<scratch>/bin/sccache`。
    Ready { wrapper: PathBuf },
    /// `[scratch.sccache] enabled = false`、または scratch 自体が無効。
    Disabled { reason: String },
    /// 有効だが使えない（バイナリが無い・server が応答しない・wrapper を書けない）。
    Unavailable { reason: String },
}

impl SccacheState {
    pub fn label(&self) -> &'static str {
        match self {
            SccacheState::Ready { .. } => "ready",
            SccacheState::Disabled { .. } => "disabled",
            SccacheState::Unavailable { .. } => "unavailable",
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            SccacheState::Ready { .. } => None,
            SccacheState::Disabled { reason } | SccacheState::Unavailable { reason } => {
                Some(reason)
            }
        }
    }
    pub fn wrapper(&self) -> Option<&Path> {
        match self {
            SccacheState::Ready { wrapper } => Some(wrapper),
            _ => None,
        }
    }
}

/// `127.0.0.1:<port>` に TCP で繋がるか（sccache の server が居るか）。**sccache の client は呼ばない**
/// （`sccache --show-stats` は server が無いと起こしてしまう。server は run の中から起こさない。D4）。
pub fn server_listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// `path` が実行できる通常のファイルか。
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// sh の単一引用符で包む。
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `<scratch>/bin/sccache` の中身（決定的）。sccache 0.18 は `CARGO_` で始まる env を全て Rust の key に入れるので、
/// owner ごとに違う `CARGO_TARGET_DIR` を rustc の env から外してから本物の sccache を exec する（G2 の U1）。
pub fn wrapper_script(binary: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         # Celeris が生成（ADR-0075 D4、Phase G2）。手で編集しない（次の run で書き直される）。\n\
         # sccache は CARGO_* の env を Rust の cache key に入れる。owner ごとに違う target の場所を外し、\n\
         # owner をまたいで依存 crate の cache を共有する。\n\
         unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR\n\
         exec {} \"$@\"\n",
        sh_quote(&binary.display().to_string())
    )
}

/// wrapper を置く（中身が同じなら触らない。違えば tmp → rename）。
pub fn ensure_wrapper(pool: &Pool, binary: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let dir = pool.root().join(WRAPPER_DIR);
    let path = dir.join(WRAPPER_NAME);
    let want = wrapper_script(binary);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(want.as_str())
        && is_executable_file(&path)
    {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{WRAPPER_NAME}.tmp-{}-{seq}", std::process::id()));
    std::fs::write(&tmp, want.as_bytes())?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// sccache を配線するかを決める（D4）。`server_up` は `server_listening`（テストでは差し替える）。
/// sccache の server が webdav（G3 の cache server）で動いているなら cache server の `/healthz` も見る
/// （`resolve_sccache_with`）。
pub fn resolve_sccache(
    settings: &ScratchSettings,
    server_up: impl Fn(u16) -> bool,
) -> SccacheState {
    resolve_sccache_with(settings, server_up, cache_server_healthy)
}

/// `resolve_sccache` の本体。`cache_up` は `cache_server_healthy`（テストでは差し替える）。U5: sccache 0.18 は backend の
/// 応答を timeout なしで待つので、webdav で動く sccache の server に対して cache server が応答しなければ（hang を含む）
/// `RUSTC_WRAPPER` を与えない（素の cargo）。
pub fn resolve_sccache_with(
    settings: &ScratchSettings,
    server_up: impl Fn(u16) -> bool,
    cache_up: impl Fn(u16) -> bool,
) -> SccacheState {
    let state = resolve_sccache_l1(settings, server_up);
    if state.wrapper().is_some()
        && read_sccache_mode(&settings.pool()).as_deref() == Some(SCCACHE_MODE_WEBDAV)
        && !cache_up(settings.cache_server.port)
    {
        return SccacheState::Unavailable {
            reason: format!(
                "sccache uses the webdav backend but the cache server on 127.0.0.1:{} does not answer /healthz \
                 (celeris-scratch-cache.service)",
                settings.cache_server.port
            ),
        };
    }
    state
}

fn resolve_sccache_l1(settings: &ScratchSettings, server_up: impl Fn(u16) -> bool) -> SccacheState {
    if !settings.enabled {
        return SccacheState::Disabled {
            reason: "scratch is disabled".to_string(),
        };
    }
    let s = &settings.sccache;
    if !s.enabled {
        return SccacheState::Disabled {
            reason: "[scratch.sccache] enabled = false".to_string(),
        };
    }
    if !is_executable_file(&s.binary) {
        return SccacheState::Unavailable {
            reason: format!(
                "sccache binary {} not found (run scripts/scratch/setup-sccache.sh)",
                s.binary.display()
            ),
        };
    }
    if !server_up(s.server_port) {
        return SccacheState::Unavailable {
            reason: format!(
                "no sccache server on 127.0.0.1:{} (celeris-sccache.service)",
                s.server_port
            ),
        };
    }
    match ensure_wrapper(&settings.pool(), &s.binary) {
        Ok(wrapper) => SccacheState::Ready { wrapper },
        Err(e) => SccacheState::Unavailable {
            reason: format!("could not write the sccache wrapper: {e}"),
        },
    }
}

/// sccache の server に与える env（`celeris-sccache.service` と `celerisctl scratch env --server`）。client の env と
/// 同じ値（D4 の「server は 1 つ、最初に起こした client の env で動く」への備え）。
pub fn sccache_server_env(settings: &ScratchSettings) -> Vec<(String, String)> {
    vec![
        (
            "SCCACHE_DIR".to_string(),
            settings.pool().l1_dir().display().to_string(),
        ),
        (
            "SCCACHE_CACHE_SIZE".to_string(),
            format!("{}G", settings.l1_max_bytes / GIB),
        ),
        (
            "SCCACHE_SERVER_PORT".to_string(),
            settings.sccache.server_port.to_string(),
        ),
        ("SCCACHE_IDLE_TIMEOUT".to_string(), "0".to_string()),
    ]
}

/// sccache 系の env（`RUSTC_WRAPPER` と server の env）。`Ready` でなければ空。
pub fn sccache_env(settings: &ScratchSettings, state: &SccacheState) -> Vec<(String, String)> {
    let Some(wrapper) = state.wrapper() else {
        return Vec::new();
    };
    let mut env = vec![("RUSTC_WRAPPER".to_string(), wrapper.display().to_string())];
    env.extend(sccache_server_env(settings));
    env
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

/// 経路に渡す env の全体（純粋。`state` は `resolve_sccache` の結果）。順序は固定:
/// `CARGO_TARGET_DIR`、`[scratch.cargo]`、sccache 系。
pub fn cargo_env_with(
    settings: &ScratchSettings,
    owner: &Owner,
    state: &SccacheState,
) -> Vec<(String, String)> {
    let mut env = target_env(&settings.pool(), owner);
    env.extend(cargo_tuning_env(&settings.cargo));
    env.extend(sccache_env(settings, state));
    env
}

/// 経路に渡す env（D3 / D4）。dispatcher（run・WU の checks・統合の検査・reviewer の checks）と
/// `celerisctl scratch env` の両方がこれを使う（ADR-0075 D4 の「env は一か所で組む」）。sccache は server が
/// 応答するときだけ（`resolve_sccache`）。
pub fn cargo_env(settings: &ScratchSettings, owner: &Owner) -> Vec<(String, String)> {
    let state = resolve_sccache(settings, server_listening);
    cargo_env_with(settings, owner, &state)
}

// ---------------------------------------------------------------------------
// L2 の cache server（D5 (b)、Phase G3）
// ---------------------------------------------------------------------------

/// sccache の server が起動時に選んだ backend の記録（`<scratch>/bin/sccache-server.mode`）。
pub const SCCACHE_MODE_FILE: &str = "sccache-server.mode";
pub const SCCACHE_MODE_WEBDAV: &str = "webdav";
pub const SCCACHE_MODE_DISK: &str = "disk";
/// `SCCACHE_WEBDAV_KEY_PREFIX`（cache server は path の最後の要素だけを key に使うので、値は表示のため）。
pub const WEBDAV_KEY_PREFIX: &str = "sccache";

/// `http://127.0.0.1:<port>`。
pub fn cache_server_endpoint(settings: &ScratchSettings) -> String {
    format!("http://127.0.0.1:{}", settings.cache_server.port)
}

/// loopback の HTTP/1.1 の GET（`/healthz`・`/stats`）。`(status, body)`。reqwest を持ち込まない最小の実装
/// （cache server は `Content-Length` を付けて返し、`Connection: close` で閉じる）。
pub fn http_get_local(port: u16, path: &str, timeout: Duration) -> Option<(u16, String)> {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = std::net::TcpStream::connect_timeout(&addr, timeout).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    // 1 回の write にまとめる（`write!` は断片ごとに write し、相手が 1 回の read で要求を読み切れないことがある）。
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).ok()?;
    // 読み切り（上限 4 MiB）→ 判定。
    let mut buf = Vec::new();
    let _ = s.take(4 * 1024 * 1024).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let code = head.split_whitespace().nth(1)?.parse().ok()?;
    let length = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.eq_ignore_ascii_case("content-length") {
            v.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    let body = match length {
        Some(n) if n <= body.len() => body[..n].to_string(),
        Some(_) => return None,
        None => body.to_string(),
    };
    Some((code, body))
}

/// cache server の `/healthz` が 200 を返すか（300 ms）。
pub fn cache_server_healthy(port: u16) -> bool {
    matches!(
        http_get_local(port, "/healthz", Duration::from_millis(300)),
        Some((200, _))
    )
}

/// `<scratch>/bin/sccache-server.mode` を読む（`webdav` | `disk`）。
pub fn read_sccache_mode(pool: &Pool) -> Option<String> {
    let text =
        std::fs::read_to_string(pool.root().join(WRAPPER_DIR).join(SCCACHE_MODE_FILE)).ok()?;
    Some(text.split_whitespace().next()?.to_string())
}

/// `<scratch>/bin/sccache-server.mode` を書く（tmp → rename）。
pub fn write_sccache_mode(pool: &Pool, mode: &str) -> io::Result<()> {
    let dir = pool.root().join(WRAPPER_DIR);
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".{SCCACHE_MODE_FILE}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, format!("{mode}\n"))?;
    std::fs::rename(&tmp, dir.join(SCCACHE_MODE_FILE))
}

/// token を読む（無い・空なら `None`）。
pub fn read_token(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// token を読み、無ければ作る（mode 0600、`/dev/urandom` の 32 byte の 16 進）。cache server が起動時に呼ぶ。
pub fn ensure_token(path: &Path) -> io::Result<String> {
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(t) = read_token(path) {
        return Ok(t);
    }
    let mut raw = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut raw)?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(token.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(token)
}

/// sccache の server の backend（`celerisctl scratch env --server` が起動時に選ぶ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SccacheBackend {
    /// G3: Celeris の cache server（`SCCACHE_WEBDAV_*`）。
    Webdav {
        endpoint: String,
        token: Option<String>,
    },
    /// G2: sccache の local disk cache（`SCCACHE_DIR`）。`reason` は webdav にしなかった理由。
    Disk { reason: Option<String> },
}

impl SccacheBackend {
    pub fn mode(&self) -> &'static str {
        match self {
            SccacheBackend::Webdav { .. } => SCCACHE_MODE_WEBDAV,
            SccacheBackend::Disk { .. } => SCCACHE_MODE_DISK,
        }
    }
}

/// U5: sccache 0.18 は起動時の storage check で backend が応答しないと**起動に失敗する**。cache server が有効で
/// `/healthz` が応答するときだけ webdav、でなければ G2 の local disk に戻す。
pub fn choose_sccache_backend(
    settings: &ScratchSettings,
    cache_up: impl Fn(u16) -> bool,
) -> SccacheBackend {
    let cs = &settings.cache_server;
    if !cs.enabled {
        return SccacheBackend::Disk {
            reason: Some("[scratch.cache_server] enabled = false".to_string()),
        };
    }
    if !cache_up(cs.port) {
        return SccacheBackend::Disk {
            reason: Some(format!(
                "no cache server on 127.0.0.1:{} (celeris-scratch-cache.service)",
                cs.port
            )),
        };
    }
    SccacheBackend::Webdav {
        endpoint: cache_server_endpoint(settings),
        token: read_token(&cs.token_file),
    }
}

/// sccache の server に与える env（backend ごと）。disk は G2 の `sccache_server_env` と同じ。webdav は
/// `SCCACHE_DIR` を与えない（remote の backend と disk を併記しない）。
pub fn sccache_server_env_for(
    settings: &ScratchSettings,
    backend: &SccacheBackend,
) -> Vec<(String, String)> {
    match backend {
        SccacheBackend::Disk { .. } => sccache_server_env(settings),
        SccacheBackend::Webdav { endpoint, token } => {
            let mut env = vec![
                ("SCCACHE_WEBDAV_ENDPOINT".to_string(), endpoint.clone()),
                (
                    "SCCACHE_WEBDAV_KEY_PREFIX".to_string(),
                    WEBDAV_KEY_PREFIX.to_string(),
                ),
            ];
            if let Some(t) = token {
                env.push(("SCCACHE_WEBDAV_TOKEN".to_string(), t.clone()));
            }
            env.push((
                "SCCACHE_SERVER_PORT".to_string(),
                settings.sccache.server_port.to_string(),
            ));
            env.push(("SCCACHE_IDLE_TIMEOUT".to_string(), "0".to_string()));
            env
        }
    }
}

// ---------------------------------------------------------------------------
// 分類（D2）
// ---------------------------------------------------------------------------

/// ADR-0075 D2 の分類。`Legacy` は旧 `build_cache_dir/cargo/*` などの pool の外の target、`Stray` は lease の無い
/// （または owner として読めない）ディレクトリ。
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
pub enum Class {
    /// P0 pinned: 絶対に消さない。
    Pinned,
    /// P1 waiting。
    Waiting,
    /// P2 retry。
    Retry,
    /// P3 completed（回収可能）。
    Completed,
    Legacy,
    Stray,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Pinned => "p0",
            Class::Waiting => "p1",
            Class::Retry => "p2",
            Class::Completed => "p3",
            Class::Legacy => "legacy",
            Class::Stray => "stray",
        }
    }
}

/// DB の状態の読み出し（dispatcher は store、`celerisctl` は読み取り専用の store か「DB 無し」）。
pub trait StatusLookup {
    fn task_status(&self, task_id: &str) -> Option<Status>;
    fn work_unit_status(&self, work_unit_id: &str) -> Option<WorkUnitStatus>;
}

/// DB を持たない呼び出し（DB が無い・開けない）。daemon 由来の owner は「DB に行が無い」＝ P3 にはせず、
/// **P0 扱い**にする（見えないものを消さない）。
pub struct NoDb;
impl StatusLookup for NoDb {
    fn task_status(&self, _: &str) -> Option<Status> {
        None
    }
    fn work_unit_status(&self, _: &str) -> Option<WorkUnitStatus> {
        None
    }
}

/// `dyn TaskStore` による lookup。
pub struct StoreLookup<'a>(pub &'a dyn task_core::store::TaskStore);
impl StatusLookup for StoreLookup<'_> {
    fn task_status(&self, task_id: &str) -> Option<Status> {
        let id: task_core::TaskId = task_id.parse().ok()?;
        self.0.get(id).ok().flatten().map(|t| t.status)
    }
    fn work_unit_status(&self, work_unit_id: &str) -> Option<WorkUnitStatus> {
        self.0
            .work_unit_get(work_unit_id)
            .ok()
            .flatten()
            .map(|w| w.status)
    }
}

fn age(now: SystemTime, t: SystemTime) -> u64 {
    now.duration_since(t).map(|d| d.as_secs()).unwrap_or(0)
}

/// ADR-0075 D2 の表。`lease_mtime` は `lease.json` の mtime（lease が無ければディレクトリの mtime）。
/// `has_db = false` のとき（`NoDb`）、daemon 由来の owner で DB に行が見つからないものは P0（消さない）。
pub fn classify(
    owner: &Owner,
    lease: Option<&Lease>,
    lease_mtime: SystemTime,
    now: SystemTime,
    s: &ScratchSettings,
    lookup: &dyn StatusLookup,
    has_db: bool,
) -> (Class, &'static str) {
    let a = age(now, lease_mtime);
    if lease.is_none() {
        return if a >= IDLE_SECS {
            (Class::Stray, "no lease.json (idle >= 1h)")
        } else {
            (Class::Pinned, "no lease.json yet (being created)")
        };
    }
    let waiting = |why_keep: &'static str, why_expired: &'static str| {
        if a < s.waiting_keep_secs {
            (Class::Waiting, why_keep)
        } else {
            (Class::Completed, why_expired)
        }
    };
    let retry = || {
        if a < s.failed_keep_secs {
            (Class::Retry, "task failed (retry possible)")
        } else {
            (Class::Completed, "task failed (retry window passed)")
        }
    };
    match owner {
        Owner::Task { task_id } => match lookup.task_status(task_id) {
            Some(Status::Running) => (Class::Pinned, "task running"),
            Some(Status::Reviewing) => (Class::Pinned, "task reviewing"),
            Some(Status::Draft | Status::Ready | Status::Blocked) => {
                waiting("task waiting", "task waiting (keep window passed)")
            }
            Some(Status::Failed) => retry(),
            Some(Status::Done) => (Class::Completed, "task done"),
            Some(Status::Cancelled) => (Class::Completed, "task cancelled"),
            None if has_db => (Class::Completed, "task not in db"),
            None => (Class::Pinned, "no db (unknown task)"),
        },
        Owner::WorkUnit {
            task_id,
            work_unit_id,
        } => {
            let task = lookup.task_status(task_id);
            let wu = lookup.work_unit_status(work_unit_id);
            match (task, wu) {
                (None, _) | (_, None) if !has_db => (Class::Pinned, "no db (unknown work unit)"),
                (None, _) => (Class::Completed, "task not in db"),
                (_, None) => (Class::Completed, "work unit not in db"),
                (Some(Status::Failed), Some(_)) => retry(),
                (Some(t), Some(_)) if t.is_terminal() => (Class::Completed, "task finished"),
                (Some(_), Some(w)) => match w {
                    WorkUnitStatus::Pending
                    | WorkUnitStatus::Ready
                    | WorkUnitStatus::Running
                    | WorkUnitStatus::NeedsContinuation => (Class::Pinned, "work unit in progress"),
                    WorkUnitStatus::Blocked | WorkUnitStatus::Failed => waiting(
                        "work unit failed/blocked (replan may retry)",
                        "work unit failed/blocked (keep window passed)",
                    ),
                    WorkUnitStatus::Done => (Class::Completed, "work unit done"),
                    WorkUnitStatus::Superseded => (Class::Completed, "work unit superseded"),
                    WorkUnitStatus::Cancelled => (Class::Completed, "work unit cancelled"),
                },
            }
        }
        Owner::Release { .. } | Owner::Agent { .. } => {
            if lease.and_then(|l| l.released_at.as_ref()).is_some() {
                (Class::Completed, "lease released")
            } else if a
                >= lease
                    .and_then(|l| l.ttl_secs)
                    .unwrap_or(s.external_lease_ttl_secs)
            {
                (Class::Completed, "lease expired")
            } else {
                (Class::Pinned, "lease live")
            }
        }
    }
}

/// ADR-0075 D7: pool の外の旧い target（`build_cache_dir/cargo/*`、`release-build/.cargo-target`）の分類。
/// 最終書き込み（測定スレッドが木を辿って得た最新の mtime）が 1 時間以上前のときだけ回収できる（今動いている
/// 実装エージェントが手で指している target を途中で消さない）。未測定は消さない。
pub fn legacy_class(
    measured_last_write: Option<SystemTime>,
    now: SystemTime,
) -> (Class, &'static str) {
    match measured_last_write {
        None => (Class::Pinned, "legacy: not measured yet"),
        Some(t) if age(now, t) >= IDLE_SECS => (Class::Legacy, "legacy: idle >= 1h"),
        Some(_) => (Class::Pinned, "legacy: written within 1h"),
    }
}

// ---------------------------------------------------------------------------
// plan_gc（D2）
// ---------------------------------------------------------------------------

/// `plan_gc` の 1 行（owner の `target/`、legacy のディレクトリ、野良のディレクトリ）。
#[derive(Debug, Clone, PartialEq)]
pub struct GcEntry {
    /// 表示と同順位の決定に使う（owner の文字列、legacy / 野良はパス）。
    pub id: String,
    /// rename して消すディレクトリ。
    pub path: PathBuf,
    pub owner: Option<Owner>,
    pub repo_key: Option<String>,
    pub class: Class,
    pub reason: String,
    /// LRU の基準（lease の mtime。legacy は測定した最終書き込み）。
    pub last_write: SystemTime,
    /// 測定したサイズ（未測定は `None`）。
    pub size_bytes: Option<u64>,
    /// pool（`targets/`）の中か（legacy は pool の外）。
    pub in_pool: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pressure {
    None,
    /// `targets` が `targets_max × high_watermark` を超えた。
    HighWatermark,
    /// filesystem の空きが `min_free_disk_mb × 2` を下回った。
    LowDisk,
    /// 空きが `min_free_disk_mb` を下回った（dispatch の直前の緊急 GC）。
    Emergency,
}

impl Pressure {
    pub fn as_str(self) -> &'static str {
        match self {
            Pressure::None => "none",
            Pressure::HighWatermark => "high_watermark",
            Pressure::LowDisk => "low_disk",
            Pressure::Emergency => "emergency",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcParams {
    pub now: SystemTime,
    pub targets_max_bytes: u64,
    pub high_watermark: f64,
    pub low_watermark: f64,
    pub min_free_bytes: u64,
    /// filesystem の空き（statvfs。不明なら `None` で空きの条件は見ない）。
    pub fs_free_bytes: Option<u64>,
    /// 緊急モード（空き < `min_free_disk_mb`）。目標 = 空き `min_free × 3`。
    pub emergency: bool,
    pub completed_grace_secs: u64,
    pub warm_seeds_per_repo: usize,
    pub max_per_tick: usize,
    /// その repo を使う非終端の owner / Task がある repo_key（seed を残す条件）。
    pub active_repo_keys: BTreeSet<String>,
}

impl GcParams {
    pub fn from_settings(
        s: &ScratchSettings,
        now: SystemTime,
        min_free_bytes: u64,
        fs_free_bytes: Option<u64>,
        emergency: bool,
        active_repo_keys: BTreeSet<String>,
    ) -> Self {
        Self {
            now,
            targets_max_bytes: s.targets_max_bytes,
            high_watermark: s.high_watermark,
            low_watermark: s.low_watermark,
            min_free_bytes,
            fs_free_bytes,
            emergency,
            completed_grace_secs: s.completed_grace_secs,
            warm_seeds_per_repo: s.warm_seeds_per_repo,
            max_per_tick: s.gc_max_per_tick,
            active_repo_keys,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcPick {
    pub id: String,
    pub path: PathBuf,
    pub owner: Option<Owner>,
    pub class: Class,
    pub seed: bool,
    pub estimated_bytes: u64,
    /// `immediate`（watermark に関係なく即回収）か `pressure`（目標に届くまで）。
    pub why: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcPlan {
    pub pressure: Pressure,
    /// pool（`targets/`）の推定使用量。
    pub used_bytes: u64,
    pub pinned_bytes: u64,
    /// 目標に届くまでに pool から減らす量（watermark）。
    pub pool_need_bytes: u64,
    /// 目標に届くまでに filesystem で空ける量（空き）。
    pub disk_need_bytes: u64,
    pub selected: Vec<GcPick>,
    /// seed に選ばれた entry の id。
    pub seeds: BTreeSet<String>,
    /// entry ごとの推定サイズ（未測定は同じ repo の最大値）。
    pub estimated: BTreeMap<String, u64>,
}

/// 未測定の entry の推定（D2: 0 とみなさず同じ repo の最大値。repo に測定済みが無ければ全体の最大値）。
fn estimates(entries: &[GcEntry]) -> BTreeMap<String, u64> {
    let mut repo_max: BTreeMap<&str, u64> = BTreeMap::new();
    let mut all_max = 0u64;
    for e in entries {
        if let Some(sz) = e.size_bytes {
            all_max = all_max.max(sz);
            if let Some(k) = &e.repo_key {
                let m = repo_max.entry(k.as_str()).or_insert(0);
                *m = (*m).max(sz);
            }
        }
    }
    entries
        .iter()
        .map(|e| {
            let v = e.size_bytes.unwrap_or_else(|| {
                e.repo_key
                    .as_deref()
                    .and_then(|k| repo_max.get(k).copied())
                    .unwrap_or(all_max)
            });
            (e.id.clone(), v)
        })
        .collect()
}

/// ADR-0075 D2: 削除順を決める純粋関数。**P0 は決して選ばない**。① legacy / 野良 → ② P3（LRU）→ ③ seed（古い順）→
/// ④ P2（古い順）→ ⑤ P1（古い順）。同順位は id の文字列順。watermark / 空きの目標に届いたら止まる。目標が無くても
/// legacy・野良、P3 の WU・release（seed を除く）、猶予の過ぎた P3 の Task は即回収する。1 回に最大 `max_per_tick` 件。
pub fn plan_gc(entries: &[GcEntry], p: &GcParams) -> GcPlan {
    let estimated = estimates(entries);
    let est = |e: &GcEntry| estimated.get(&e.id).copied().unwrap_or(0);
    let used: u64 = entries.iter().filter(|e| e.in_pool).map(est).sum();
    let pinned: u64 = entries
        .iter()
        .filter(|e| e.in_pool && e.class == Class::Pinned)
        .map(est)
        .sum();

    // seed: P3 の owner のうち、repo ごとに最新の `warm_seeds_per_repo` 個（その repo が使われているときだけ）。
    let mut seeds = BTreeSet::new();
    if p.warm_seeds_per_repo > 0 {
        let mut by_repo: BTreeMap<&str, Vec<&GcEntry>> = BTreeMap::new();
        for e in entries
            .iter()
            .filter(|e| e.class == Class::Completed && e.owner.is_some() && e.in_pool)
        {
            if let Some(k) = e.repo_key.as_deref()
                && p.active_repo_keys.contains(k)
            {
                by_repo.entry(k).or_default().push(e);
            }
        }
        for list in by_repo.values_mut() {
            list.sort_by(|a, b| {
                b.last_write
                    .cmp(&a.last_write)
                    .then_with(|| a.id.cmp(&b.id))
            });
            for e in list.iter().take(p.warm_seeds_per_repo) {
                seeds.insert(e.id.clone());
            }
        }
    }

    let low_limit = (p.targets_max_bytes as f64 * p.low_watermark) as u64;
    let high_limit = (p.targets_max_bytes as f64 * p.high_watermark) as u64;
    let mut pressure = Pressure::None;
    let mut pool_need = 0u64;
    if used > high_limit {
        pressure = Pressure::HighWatermark;
        pool_need = used - low_limit;
    }
    let mut disk_need = 0u64;
    if let Some(free) = p.fs_free_bytes {
        let goal = p.min_free_bytes.saturating_mul(3);
        if p.emergency || free < p.min_free_bytes {
            pressure = Pressure::Emergency;
            disk_need = goal.saturating_sub(free);
        } else if free < p.min_free_bytes.saturating_mul(2) {
            if pressure == Pressure::None {
                pressure = Pressure::LowDisk;
            }
            disk_need = goal.saturating_sub(free);
        }
    } else if p.emergency {
        pressure = Pressure::Emergency;
    }

    let rank = |e: &GcEntry| -> Option<u8> {
        match e.class {
            Class::Pinned => None,
            Class::Legacy | Class::Stray => Some(1),
            Class::Completed if seeds.contains(&e.id) => Some(3),
            Class::Completed => Some(2),
            Class::Retry => Some(4),
            Class::Waiting => Some(5),
        }
    };
    let mut ordered: Vec<(u8, &GcEntry)> = entries
        .iter()
        .filter_map(|e| rank(e).map(|r| (r, e)))
        .collect();
    ordered.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then_with(|| a.last_write.cmp(&b.last_write))
            .then_with(|| a.id.cmp(&b.id))
    });
    let immediate = |r: u8, e: &GcEntry| -> bool {
        match r {
            1 => true,
            2 => match &e.owner {
                Some(Owner::WorkUnit { .. } | Owner::Release { .. }) => true,
                Some(Owner::Task { .. }) => age(p.now, e.last_write) >= p.completed_grace_secs,
                _ => false,
            },
            _ => false,
        }
    };

    let mut selected = Vec::new();
    let (mut pool_freed, mut disk_freed) = (0u64, 0u64);
    let pick = |e: &GcEntry, r: u8, why: &'static str| GcPick {
        id: e.id.clone(),
        path: e.path.clone(),
        owner: e.owner.clone(),
        class: e.class,
        seed: r == 3,
        estimated_bytes: est(e),
        why,
    };
    let mut done_ids = BTreeSet::new();
    for (r, e) in &ordered {
        if selected.len() >= p.max_per_tick {
            break;
        }
        if immediate(*r, e) {
            let sz = est(e);
            if e.in_pool {
                pool_freed += sz;
            }
            disk_freed += sz;
            selected.push(pick(e, *r, "immediate"));
            done_ids.insert(e.id.clone());
        }
    }
    for (r, e) in &ordered {
        if selected.len() >= p.max_per_tick {
            break;
        }
        if pool_freed >= pool_need && disk_freed >= disk_need {
            break;
        }
        if done_ids.contains(&e.id) {
            continue;
        }
        let sz = est(e);
        if e.in_pool {
            pool_freed += sz;
        }
        disk_freed += sz;
        selected.push(pick(e, *r, "pressure"));
    }
    GcPlan {
        pressure,
        used_bytes: used,
        pinned_bytes: pinned,
        pool_need_bytes: pool_need,
        disk_need_bytes: disk_need,
        selected,
        seeds,
        estimated,
    }
}

/// `.deleting-<flat>-<nanos>` の名前（同じ親の中へ rename する。filesystem をまたがない）。
pub fn deleting_name(flat: &str, now: SystemTime) -> String {
    let nanos = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos();
    format!("{DELETING_PREFIX}{flat}-{nanos}")
}

/// `path` 以下の実ファイルの使用量（`st_blocks × 512`）と最新の mtime。symlink の先は辿らない。
/// 重い（大きな target では数秒〜）ので tick の中では呼ばない（測定スレッドだけ）。
pub fn measure_tree(path: &Path) -> io::Result<(u64, SystemTime)> {
    use std::os::unix::fs::MetadataExt;
    let mut total = 0u64;
    let mut latest = UNIX_EPOCH;
    let mut pending = vec![path.to_path_buf()];
    while let Some(p) = pending.pop() {
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        total = total.saturating_add(meta.blocks().saturating_mul(512));
        if let Ok(t) = meta.modified() {
            latest = latest.max(t);
        }
        if meta.file_type().is_dir()
            && let Ok(rd) = std::fs::read_dir(&p)
        {
            for e in rd.flatten() {
                pending.push(e.path());
            }
        }
    }
    Ok((total, latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const T0: u64 = 1_700_000_000;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn entry(id: &str, class: Class, last: u64, size: Option<u64>, repo: &str) -> GcEntry {
        let owner = Owner::parse(id).ok();
        GcEntry {
            id: id.to_string(),
            path: PathBuf::from(format!("/scratch/targets/{id}/target")),
            owner,
            repo_key: Some(repo.to_string()),
            class,
            reason: String::new(),
            last_write: at(last),
            size_bytes: size,
            in_pool: true,
        }
    }

    fn params(max: u64) -> GcParams {
        GcParams {
            now: at(T0 + 100_000),
            targets_max_bytes: max,
            high_watermark: 0.9,
            low_watermark: 0.7,
            min_free_bytes: 0,
            fs_free_bytes: None,
            emergency: false,
            completed_grace_secs: 600,
            warm_seeds_per_repo: 1,
            max_per_tick: 100,
            active_repo_keys: BTreeSet::new(),
        }
    }

    fn ids(plan: &GcPlan) -> Vec<&str> {
        plan.selected.iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn owner_paths_nest_work_units_under_their_task() {
        let pool = Pool::new("/var/lib/celeris/scratch");
        let task = Owner::task("01TASK");
        let wu = Owner::work_unit("01TASK", "01WU");
        assert_eq!(
            pool.target_dir(&task),
            PathBuf::from("/var/lib/celeris/scratch/targets/task-01TASK/target")
        );
        assert_eq!(
            pool.target_dir(&wu),
            PathBuf::from("/var/lib/celeris/scratch/targets/task-01TASK/wu-01WU/target")
        );
        assert!(pool.owner_dir(&wu).starts_with(pool.owner_dir(&task)));
        assert_eq!(wu.to_string(), "task-01TASK/wu-01WU");
        assert_eq!(Owner::parse("task-01TASK/wu-01WU").unwrap(), wu);
        assert_eq!(
            Owner::parse("release-0123456789ab").unwrap().kind(),
            OwnerKind::Release
        );
        assert_eq!(
            Owner::parse("agent-agent-a5caa712b0867e383").unwrap(),
            Owner::Agent {
                name: "agent-a5caa712b0867e383".into()
            }
        );
        assert_eq!(wu.flat(), "task-01TASK__wu-01WU");
        for bad in [
            "",
            "x-1",
            "task-",
            "task-a/b",
            "release-../x",
            "agent-.hidden",
            "agent-a/b",
        ] {
            assert!(Owner::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            target_env(&pool, &wu),
            vec![(
                "CARGO_TARGET_DIR".to_string(),
                "/var/lib/celeris/scratch/targets/task-01TASK/wu-01WU/target".to_string()
            )]
        );
    }

    #[test]
    fn plan_gc_never_selects_pinned_owners() {
        let entries = vec![
            entry("task-A", Class::Pinned, T0, Some(90), "r"),
            entry("task-B", Class::Pinned, T0, Some(90), "r"),
            entry("task-C", Class::Waiting, T0, Some(1), "r"),
        ];
        // watermark も空きも目標に届かない・緊急でも P0 は選ばない。
        let mut p = params(100);
        p.emergency = true;
        p.fs_free_bytes = Some(0);
        p.min_free_bytes = 1_000_000;
        let plan = plan_gc(&entries, &p);
        assert_eq!(plan.pressure, Pressure::Emergency);
        assert_eq!(ids(&plan), vec!["task-C"]);
        assert_eq!(plan.pinned_bytes, 180);
    }

    #[test]
    fn plan_gc_follows_the_semantic_order_and_stops_at_low_watermark() {
        // max 100、used 100 > 90 → low 70 まで 30 減らす。各 10。
        let mut legacy = entry("/cache/cargo/old", Class::Legacy, T0 + 50, Some(10), "r");
        legacy.owner = None;
        legacy.in_pool = false;
        let entries = vec![
            entry("task-P1", Class::Waiting, T0, Some(10), "r"),
            entry("task-P2", Class::Retry, T0, Some(10), "r"),
            entry("agent-old", Class::Completed, T0 + 10, Some(10), "r"),
            entry("agent-new", Class::Completed, T0 + 20, Some(10), "r"),
            entry("task-Pin", Class::Pinned, T0, Some(40), "r"),
            entry("stray-x", Class::Stray, T0 + 40, Some(10), "r"),
            entry("agent-seed", Class::Completed, T0 + 30, Some(10), "r"),
            legacy,
        ];
        let mut p = params(100);
        p.active_repo_keys.insert("r".into());
        let plan = plan_gc(&entries, &p);
        assert_eq!(plan.pressure, Pressure::HighWatermark);
        assert_eq!(plan.used_bytes, 100);
        assert_eq!(plan.pool_need_bytes, 30);
        // ① stray / legacy（即回収、legacy は pool の外なので pool の目標には数えない）→ ② P3 LRU → 止まる。
        assert_eq!(
            ids(&plan),
            vec!["stray-x", "/cache/cargo/old", "agent-old", "agent-new"]
        );
        // 目標を大きくすると seed → P2 → P1 の順に続く（P0 は選ばない）。
        let mut p = params(10);
        p.active_repo_keys.insert("r".into());
        let plan = plan_gc(&entries, &p);
        assert_eq!(
            ids(&plan),
            vec![
                "stray-x",
                "/cache/cargo/old",
                "agent-old",
                "agent-new",
                "agent-seed",
                "task-P2",
                "task-P1"
            ]
        );
        assert!(
            plan.selected
                .iter()
                .find(|s| s.id == "agent-seed")
                .unwrap()
                .seed
        );
        // 目標が無ければ即回収の分だけ（agent の P3 と seed・P2・P1 は残る）。
        let mut p = params(1_000_000);
        p.active_repo_keys.insert("r".into());
        let plan = plan_gc(&entries, &p);
        assert_eq!(plan.pressure, Pressure::None);
        assert_eq!(ids(&plan), vec!["stray-x", "/cache/cargo/old"]);
        // 1 回の件数の上限。
        let mut p = params(10);
        p.max_per_tick = 2;
        assert_eq!(ids(&plan_gc(&entries, &p)).len(), 2);
    }

    #[test]
    fn plan_gc_reclaims_finished_work_units_and_releases_immediately_but_task_after_grace() {
        let now = T0 + 100_000;
        let entries = vec![
            entry("task-T/wu-W", Class::Completed, now - 5, Some(10), "r"),
            entry("release-abc", Class::Completed, now - 5, Some(10), "q"),
            entry("task-Fresh", Class::Completed, now - 5, Some(10), "q"),
            entry("task-Old", Class::Completed, now - 601, Some(10), "q"),
            entry("task-W/wu-X", Class::Waiting, now - 5, Some(10), "r"),
        ];
        let plan = plan_gc(&entries, &params(1_000_000));
        assert_eq!(ids(&plan), vec!["task-Old", "release-abc", "task-T/wu-W"]);
    }

    #[test]
    fn plan_gc_keeps_one_warm_seed_per_repo() {
        let now = T0 + 100_000;
        let entries = vec![
            entry("task-T/wu-a", Class::Completed, now - 30, Some(10), "r"),
            entry("task-T/wu-b", Class::Completed, now - 10, Some(10), "r"),
            entry("task-T/wu-c", Class::Completed, now - 20, Some(10), "r"),
            entry(
                "task-U/wu-d",
                Class::Completed,
                now - 10,
                Some(10),
                "unused",
            ),
            entry("task-Live", Class::Pinned, now, Some(10), "r"),
        ];
        let mut p = params(1_000_000);
        p.active_repo_keys.insert("r".into());
        let plan = plan_gc(&entries, &p);
        assert_eq!(plan.seeds, BTreeSet::from(["task-T/wu-b".to_string()]));
        // seed（最新の wu-b）は即回収しない。使われていない repo の seed は作らない。
        assert_eq!(
            ids(&plan),
            vec!["task-T/wu-a", "task-T/wu-c", "task-U/wu-d"]
        );
    }

    #[test]
    fn plan_gc_treats_unmeasured_owners_as_the_repo_maximum() {
        let entries = vec![
            entry("task-A", Class::Pinned, T0, Some(50), "r"),
            entry("task-B", Class::Completed, T0, None, "r"),
            entry("agent-c", Class::Completed, T0, None, "other"),
            entry("agent-d", Class::Completed, T0, Some(5), "other"),
        ];
        let plan = plan_gc(&entries, &params(1000));
        assert_eq!(plan.estimated["task-B"], 50);
        assert_eq!(plan.estimated["agent-c"], 5);
        assert_eq!(plan.used_bytes, 110);
        // 未測定を 0 とみなすと超えない上限でも、repo の最大値で数えれば high watermark を超える。
        let plan = plan_gc(&entries, &params(120));
        assert_eq!(plan.pressure, Pressure::HighWatermark);
    }

    struct Db {
        tasks: HashMap<String, Status>,
        wus: HashMap<String, WorkUnitStatus>,
    }
    impl StatusLookup for Db {
        fn task_status(&self, id: &str) -> Option<Status> {
            self.tasks.get(id).copied()
        }
        fn work_unit_status(&self, id: &str) -> Option<WorkUnitStatus> {
            self.wus.get(id).copied()
        }
    }

    fn lease_for(owner: &Owner) -> Lease {
        Lease {
            schema: LEASE_SCHEMA.into(),
            owner: owner.to_string(),
            kind: owner.kind(),
            repo_key: "r".into(),
            repo_path: "/repo".into(),
            base_commit: None,
            work_unit_key: None,
            created_at: rfc3339(at(T0)),
            released_at: None,
            size_bytes: None,
            measured_at: None,
            adopted_from: None,
            ttl_secs: None,
        }
    }

    #[test]
    fn classify_follows_the_adr_table() {
        let s = ScratchSettings::with_dir("/s");
        let db = Db {
            tasks: HashMap::from([
                ("R".to_string(), Status::Running),
                ("V".to_string(), Status::Reviewing),
                ("B".to_string(), Status::Blocked),
                ("F".to_string(), Status::Failed),
                ("D".to_string(), Status::Done),
            ]),
            wus: HashMap::from([
                ("w-run".to_string(), WorkUnitStatus::Running),
                ("w-pend".to_string(), WorkUnitStatus::Pending),
                ("w-fail".to_string(), WorkUnitStatus::Failed),
                ("w-block".to_string(), WorkUnitStatus::Blocked),
                ("w-done".to_string(), WorkUnitStatus::Done),
                ("w-sup".to_string(), WorkUnitStatus::Superseded),
            ]),
        };
        let now = at(T0 + 10);
        let c = |o: &str| {
            let owner = Owner::parse(o).unwrap();
            classify(&owner, Some(&lease_for(&owner)), at(T0), now, &s, &db, true).0
        };
        assert_eq!(c("task-R"), Class::Pinned);
        assert_eq!(c("task-V"), Class::Pinned);
        assert_eq!(c("task-B"), Class::Waiting);
        assert_eq!(c("task-F"), Class::Retry);
        assert_eq!(c("task-D"), Class::Completed);
        assert_eq!(c("task-Gone"), Class::Completed);
        assert_eq!(c("task-R/wu-w-run"), Class::Pinned);
        assert_eq!(c("task-R/wu-w-pend"), Class::Pinned);
        assert_eq!(c("task-R/wu-w-fail"), Class::Waiting);
        assert_eq!(c("task-R/wu-w-block"), Class::Waiting);
        assert_eq!(c("task-R/wu-w-done"), Class::Completed);
        assert_eq!(c("task-R/wu-w-sup"), Class::Completed);
        assert_eq!(c("task-D/wu-w-run"), Class::Completed);
        // 保持期限を過ぎた P1 / P2 は P3。
        let late = at(T0 + 172_800 + 1);
        let owner = Owner::task("B");
        assert_eq!(
            classify(
                &owner,
                Some(&lease_for(&owner)),
                at(T0),
                late,
                &s,
                &db,
                true
            )
            .0,
            Class::Completed
        );
        let owner = Owner::task("F");
        assert_eq!(
            classify(
                &owner,
                Some(&lease_for(&owner)),
                at(T0),
                at(T0 + 86_401),
                &s,
                &db,
                true
            )
            .0,
            Class::Completed
        );
        // DB が無い（celerisctl で DB を開けない）ときは daemon 由来の owner を消さない。
        let owner = Owner::task("Gone");
        assert_eq!(
            classify(
                &owner,
                Some(&lease_for(&owner)),
                at(T0),
                now,
                &s,
                &NoDb,
                false
            )
            .0,
            Class::Pinned
        );
        // lease の無いディレクトリ: 1h 未満は作りかけ（P0）、以後は野良。
        assert_eq!(
            classify(&owner, None, at(T0), now, &s, &db, true).0,
            Class::Pinned
        );
        assert_eq!(
            classify(&owner, None, at(T0), at(T0 + 3600), &s, &db, true).0,
            Class::Stray
        );
    }

    #[test]
    fn external_lease_expires_after_ttl() {
        let s = ScratchSettings::with_dir("/s");
        let owner = Owner::parse("release-0123456789ab").unwrap();
        let mut lease = lease_for(&owner);
        let c = |lease: &Lease, now: u64| {
            classify(&owner, Some(lease), at(T0), at(now), &s, &NoDb, false).0
        };
        assert_eq!(c(&lease, T0 + 21_599), Class::Pinned);
        assert_eq!(c(&lease, T0 + 21_600), Class::Completed);
        // lease ごとの TTL（`--ttl`）が既定より優先。
        lease.ttl_secs = Some(60);
        assert_eq!(c(&lease, T0 + 59), Class::Pinned);
        assert_eq!(c(&lease, T0 + 60), Class::Completed);
        lease.ttl_secs = None;
        lease.released_at = Some(rfc3339(at(T0 + 1)));
        assert_eq!(c(&lease, T0 + 2), Class::Completed);
    }

    #[test]
    fn lease_touch_release_round_trip_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = Pool::new(tmp.path());
        let owner = Owner::parse("agent-x").unwrap();
        let none = |_: &AdoptCandidate| None;
        let req = AllocateRequest {
            owner: &owner,
            repo_path: Path::new("/repo/agent-platform"),
            base_commit: Some("abc".into()),
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &none,
            adopt: true,
            max_distance: 200,
        };
        let a = allocate(&pool, &req).unwrap();
        assert!(a.created);
        assert_eq!(a.target_dir, pool.target_dir(&owner));
        assert!(a.target_dir.is_dir());
        let lease_path = pool.lease_path(&owner);
        set_mtime(&lease_path, at(T0)).unwrap();
        assert!(touch(&pool, &owner).unwrap());
        assert!(mtime(&lease_path).unwrap() > at(T0));
        assert!(release(&pool, &owner).unwrap());
        assert!(
            read_lease(&lease_path)
                .unwrap()
                .unwrap()
                .released_at
                .is_some()
        );
        // 再び lease を取ると released_at が消える。
        let a = allocate(&pool, &req).unwrap();
        assert!(!a.created);
        assert!(
            read_lease(&lease_path)
                .unwrap()
                .unwrap()
                .released_at
                .is_none()
        );
        // 測定は mtime を変えない。
        set_mtime(&lease_path, at(T0)).unwrap();
        write_lease_measurement(&lease_path, 42, at(T0 + 5)).unwrap();
        assert_eq!(mtime(&lease_path).unwrap(), at(T0));
        assert_eq!(
            read_lease(&lease_path).unwrap().unwrap().size_bytes,
            Some(42)
        );
        assert!(!touch(&pool, &Owner::parse("agent-none").unwrap()).unwrap());
    }

    fn cand(owner: &str, last: u64, base: Option<&str>) -> AdoptCandidate {
        AdoptCandidate {
            owner: Owner::parse(owner).unwrap(),
            repo_key: "r".into(),
            base_commit: base.map(str::to_string),
            last_write: at(last),
            lease_mtime: None,
        }
    }

    #[test]
    fn adopt_requires_the_target_to_predate_the_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = Pool::new(tmp.path());
        let repo = Path::new("/repo/agent-platform");
        let key = crate::build_cache::repo_cache_key(repo);
        let none = |_: &AdoptCandidate| None;
        // 候補（P3 の task-Old）: target の中に目印のファイル、最終書き込みを T0 に固定。
        let old = Owner::task("Old");
        let alloc = |owner: &Owner, checkout: Option<SystemTime>, cands: &[AdoptCandidate]| {
            allocate(
                &pool,
                &AllocateRequest {
                    owner,
                    repo_path: repo,
                    base_commit: None,
                    work_unit_key: None,
                    checkout,
                    candidates: cands,
                    distance: &none,
                    adopt: true,
                    max_distance: 200,
                },
            )
            .unwrap()
        };
        alloc(&old, None, &[]);
        std::fs::write(pool.target_dir(&old).join("marker"), "x").unwrap();
        for p in [
            pool.target_dir(&old).join("marker"),
            pool.target_dir(&old),
            pool.lease_path(&old),
        ] {
            set_mtime_any(&p, at(T0));
        }
        let candidate = AdoptCandidate {
            owner: old.clone(),
            repo_key: key.clone(),
            base_commit: None,
            last_write: at(T0),
            lease_mtime: mtime(&pool.lease_path(&old)),
        };
        // checkout が候補の最終書き込みより前 → adopt しない（空から）。
        let a = alloc(
            &Owner::task("Early"),
            Some(at(T0 - 1)),
            std::slice::from_ref(&candidate),
        );
        assert_eq!(a.adopted_from, None);
        assert!(!a.target_dir.join("marker").exists());
        assert!(pool.target_dir(&old).join("marker").exists());
        // 同時刻も不可（「前」であること）。
        let a = alloc(
            &Owner::task("Same"),
            Some(at(T0)),
            std::slice::from_ref(&candidate),
        );
        assert_eq!(a.adopted_from, None);
        // checkout が後 → rename で引き継ぐ。
        let a = alloc(
            &Owner::task("Late"),
            Some(at(T0 + 1)),
            std::slice::from_ref(&candidate),
        );
        assert_eq!(a.adopted_from.as_deref(), Some("task-Old"));
        assert!(a.target_dir.join("marker").exists());
        assert!(!pool.target_dir(&old).exists());
        let lease = read_lease(&pool.lease_path(&Owner::task("Late")))
            .unwrap()
            .unwrap();
        assert_eq!(lease.adopted_from.as_deref(), Some("task-Old"));
    }

    fn set_mtime_any(path: &Path, t: SystemTime) {
        let f = std::fs::File::open(path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(t))
            .unwrap();
    }

    #[test]
    fn adopt_skips_a_candidate_that_was_leased_again_after_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = Pool::new(tmp.path());
        let repo = Path::new("/repo/x");
        let key = crate::build_cache::repo_cache_key(repo);
        let none = |_: &AdoptCandidate| None;
        let old = Owner::task("Old");
        fn mk<'a>(
            owner: &'a Owner,
            repo: &'a Path,
            none: &'a dyn Fn(&AdoptCandidate) -> Option<u64>,
        ) -> AllocateRequest<'a> {
            AllocateRequest {
                owner,
                repo_path: repo,
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: none,
                adopt: true,
                max_distance: 200,
            }
        }
        allocate(&pool, &mk(&old, repo, &none)).unwrap();
        for p in [pool.target_dir(&old), pool.lease_path(&old)] {
            set_mtime_any(&p, at(T0));
        }
        let candidate = AdoptCandidate {
            owner: old.clone(),
            repo_key: key,
            base_commit: None,
            last_write: at(T0),
            // 走査のときの mtime と違う → 再び使われ始めた。
            lease_mtime: Some(at(T0 - 100)),
        };
        let new = Owner::task("New");
        let cands = [candidate];
        let mut r = mk(&new, repo, &none);
        r.checkout = Some(at(T0 + 10));
        r.candidates = &cands;
        assert_eq!(allocate(&pool, &r).unwrap().adopted_from, None);
        assert!(pool.target_dir(&old).exists());
    }

    #[test]
    fn adopt_prefers_the_nearest_commit_within_the_limit() {
        let cands = vec![
            cand("task-far", T0 + 30, Some("far")),
            cand("task-near", T0 + 10, Some("near")),
            cand("task-mid", T0 + 20, Some("mid")),
            cand("task-late", T0 + 1000, Some("near")),
        ];
        let dist = |c: &AdoptCandidate| match c.base_commit.as_deref() {
            Some("near") => Some(3),
            Some("mid") => Some(50),
            Some("far") => Some(500),
            _ => None,
        };
        let checkout = at(T0 + 100);
        // task-late は checkout より後なので安全条件で外れる。
        let got = choose_adopt(&cands, "r", checkout, &dist, 200).unwrap();
        assert_eq!(got.owner.to_string(), "task-near");
        // 上限を 2 にすると範囲内が無い → 最も新しい安全な候補。
        let got = choose_adopt(&cands, "r", checkout, &dist, 2).unwrap();
        assert_eq!(got.owner.to_string(), "task-far");
        // 別の repo は選ばない。
        assert!(choose_adopt(&cands, "other", checkout, &dist, 200).is_none());
    }

    #[test]
    fn nfs_check_disables_scratch_with_a_reason() {
        let s = ScratchSettings::with_dir("/mnt/nfs/scratch");
        let got = apply_nfs_check(s.clone(), |_| Ok(true));
        assert!(!got.enabled);
        assert!(got.disabled_reason.unwrap().contains("NFS"));
        let got = apply_nfs_check(s.clone(), |p| Ok(p.ends_with(L1_DIR)));
        assert!(!got.enabled);
        let got = apply_nfs_check(s.clone(), |_| Ok(false));
        assert!(got.enabled);
        // 本物の検査は一時ディレクトリ（ローカル）では NFS ではない。
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_on_nfs(&tmp.path().join("missing/child")).unwrap());
    }

    #[test]
    fn list_owner_dirs_finds_tasks_work_units_and_strays() {
        let tmp = tempfile::tempdir().unwrap();
        let pool = Pool::new(tmp.path());
        let t = pool.targets_dir();
        for d in [
            "task-A/wu-W1",
            "task-A/target",
            "release-abc",
            "agent-x",
            "junk",
            ".deleting-task-B-1",
        ] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        let got = pool.list_owner_dirs();
        let owners: Vec<String> = got
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|o| o.to_string())
            .collect();
        assert_eq!(
            owners,
            vec!["agent-x", "release-abc", "task-A", "task-A/wu-W1"]
        );
        let strays: Vec<&PathBuf> = got.iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(strays, vec![&t.join("junk")]);
    }

    #[test]
    fn legacy_build_cache_entries_are_reclaimed_only_when_idle() {
        let now = at(T0 + 10_000);
        assert_eq!(legacy_class(None, now).0, Class::Pinned);
        assert_eq!(
            legacy_class(Some(at(T0 + 10_000 - 3599)), now).0,
            Class::Pinned
        );
        assert_eq!(
            legacy_class(Some(at(T0 + 10_000 - 3600)), now).0,
            Class::Legacy
        );
        // legacy は pool の外なので pinned の量にも pool の使用量にも数えない。
        let mut busy = entry(
            "/cache/cargo/agent-platform-g1",
            Class::Pinned,
            T0,
            Some(99),
            "r",
        );
        busy.owner = None;
        busy.in_pool = false;
        let mut idle = entry("/cache/cargo/old", Class::Legacy, T0, Some(5), "r");
        idle.owner = None;
        idle.in_pool = false;
        let plan = plan_gc(&[busy, idle], &params(1_000_000));
        assert_eq!(ids(&plan), vec!["/cache/cargo/old"]);
        assert_eq!(plan.used_bytes, 0);
        assert_eq!(plan.pinned_bytes, 0);
    }

    #[test]
    fn measure_tree_counts_blocks_and_latest_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        std::fs::write(tmp.path().join("a/b/f"), vec![0u8; 10_000]).unwrap();
        let (size, latest) = measure_tree(tmp.path()).unwrap();
        assert!(size >= 10_000, "{size}");
        assert!(latest > at(T0 - 1_000_000_000));
        assert_eq!(measure_tree(&tmp.path().join("missing")).unwrap().0, 0);
    }

    /// テスト用の sccache のバイナリ（実行ビットつきの空の script。呼ばれない）。
    fn fake_sccache(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("tools/sccache/bin/sccache");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn sccache_settings(dir: &Path, binary: PathBuf) -> ScratchSettings {
        ScratchSettings {
            l1_max_bytes: 40 * GIB,
            sccache: SccacheSettings {
                enabled: true,
                binary,
                server_port: 4236,
            },
            ..ScratchSettings::with_dir(dir.join("scratch"))
        }
    }

    /// ADR-0075 D4 / G2 受け入れ条件 2: server が応答するとき、env は `CARGO_TARGET_DIR`・`[scratch.cargo]`・sccache 系を
    /// 固定の順で全部持ち、何度組んでも同じ。wrapper は `<scratch>/bin/sccache`（cc-rs が認める名前）で、
    /// `CARGO_TARGET_DIR` を外して本物の sccache を exec する。
    #[test]
    fn sccache_env_is_complete_and_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_sccache(tmp.path());
        let settings = sccache_settings(tmp.path(), bin.clone());
        let owner = Owner::work_unit("01TASK", "01WU");
        let up = |p: u16| p == 4236;
        let state = resolve_sccache(&settings, up);
        let root = tmp.path().join("scratch");
        let wrapper = root.join("bin/sccache");
        assert_eq!(
            state,
            SccacheState::Ready {
                wrapper: wrapper.clone()
            }
        );
        let env = cargo_env_with(&settings, &owner, &state);
        let s = |p: PathBuf| p.display().to_string();
        assert_eq!(
            env,
            vec![
                (
                    "CARGO_TARGET_DIR".to_string(),
                    s(root.join("targets/task-01TASK/wu-01WU/target"))
                ),
                ("CARGO_INCREMENTAL".to_string(), "0".to_string()),
                (
                    "CARGO_PROFILE_DEV_DEBUG".to_string(),
                    "line-tables-only".to_string()
                ),
                ("RUSTC_WRAPPER".to_string(), s(wrapper.clone())),
                ("SCCACHE_DIR".to_string(), s(root.join("sccache-l1"))),
                ("SCCACHE_CACHE_SIZE".to_string(), "40G".to_string()),
                ("SCCACHE_SERVER_PORT".to_string(), "4236".to_string()),
                ("SCCACHE_IDLE_TIMEOUT".to_string(), "0".to_string()),
            ]
        );
        // 何度組んでも同じ（wrapper は書き直さない）。
        let before = mtime(&wrapper);
        let again = cargo_env_with(&settings, &owner, &resolve_sccache(&settings, up));
        assert_eq!(env, again);
        assert_eq!(mtime(&wrapper), before);
        let script = std::fs::read_to_string(&wrapper).unwrap();
        assert!(script.starts_with("#!/bin/sh\n"), "{script}");
        assert!(
            script.contains("unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR"),
            "{script}"
        );
        assert!(
            script.contains(&format!("exec '{}' \"$@\"", bin.display())),
            "{script}"
        );
        // server の env は client の sccache 系と同じ値（RUSTC_WRAPPER を除く）。
        assert_eq!(sccache_server_env(&settings), env[4..].to_vec());
        // [scratch.cargo] を変えれば与えない。
        let tuned = ScratchSettings {
            cargo: CargoTuning {
                incremental: true,
                dev_debug: None,
            },
            ..settings.clone()
        };
        let env = cargo_env_with(&tuned, &owner, &state);
        assert!(
            !env.iter()
                .any(|(k, _)| k.starts_with("CARGO_INCREMENTAL") || k == "CARGO_PROFILE_DEV_DEBUG")
        );
        // 実際に wrapper を通すと CARGO_TARGET_DIR が消えている（本物の sccache の代わりに env を出す script）。
        let echo = tmp.path().join("echo-env");
        std::fs::write(
            &echo,
            "#!/bin/sh\necho \"T=${CARGO_TARGET_DIR-unset} A=$1\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&echo, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let w = ensure_wrapper(&settings.pool(), &echo).unwrap();
        let out = std::process::Command::new(&w)
            .arg("rustc")
            .env("CARGO_TARGET_DIR", "/somewhere")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "T=unset A=rustc\n");
    }

    /// ADR-0075 D4 / G2 受け入れ条件 2: バイナリが無い・server が応答しない・`enabled = false`・scratch が無効のときは
    /// sccache 系を与えない（`CARGO_TARGET_DIR` と `[scratch.cargo]` は残る）。
    /// Phase G3: `/healthz` と `/stats` を読む最小の HTTP クライアント（`Content-Length` で切る。閉じた port・
    /// 200 以外は unhealthy）。
    #[test]
    fn cache_server_health_reads_a_minimal_http_response() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut c) = conn else { continue };
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                // 要求の終わり（空行）まで読み切ってから答える（高負荷で要求が分かれて届いても RST にしない）。
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let req = String::from_utf8_lossy(&req).to_string();
                let resp: &[u8] = if req.starts_with("GET /healthz ") {
                    b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nok\ntrailing"
                } else {
                    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n"
                };
                let _ = c.write_all(resp);
            }
        });
        assert_eq!(
            http_get_local(port, "/healthz", Duration::from_secs(2)),
            Some((200, "ok\n".to_string()))
        );
        assert!(cache_server_healthy(port));
        assert_eq!(
            http_get_local(port, "/stats", Duration::from_secs(2)).map(|r| r.0),
            Some(404)
        );
        assert!(!cache_server_healthy(1));
        // token は 0600 で作り、2 回目は同じ値を返す。
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub/cache-server.token");
        let t = ensure_token(&path).unwrap();
        assert_eq!(t.len(), 64);
        assert_eq!(ensure_token(&path).unwrap(), t);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn sccache_env_is_omitted_without_binary_or_server() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_sccache(tmp.path());
        let owner = Owner::task("01TASK");
        let keys =
            |env: &[(String, String)]| env.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        let plain = vec![
            "CARGO_TARGET_DIR".to_string(),
            "CARGO_INCREMENTAL".to_string(),
            "CARGO_PROFILE_DEV_DEBUG".to_string(),
        ];
        // バイナリが無い。
        let settings = sccache_settings(tmp.path(), tmp.path().join("missing/sccache"));
        let state = resolve_sccache(&settings, |_| true);
        assert_eq!(state.label(), "unavailable");
        assert!(state.reason().unwrap().contains("not found"), "{state:?}");
        assert_eq!(keys(&cargo_env_with(&settings, &owner, &state)), plain);
        // server が応答しない。
        let settings = sccache_settings(tmp.path(), bin.clone());
        let state = resolve_sccache(&settings, |_| false);
        assert_eq!(state.label(), "unavailable");
        assert!(
            state.reason().unwrap().contains("127.0.0.1:4236"),
            "{state:?}"
        );
        assert_eq!(keys(&cargo_env_with(&settings, &owner, &state)), plain);
        // `[scratch.sccache] enabled = false`。
        let mut off = sccache_settings(tmp.path(), bin.clone());
        off.sccache.enabled = false;
        let state = resolve_sccache(&off, |_| true);
        assert_eq!(state.label(), "disabled");
        assert_eq!(keys(&cargo_env_with(&off, &owner, &state)), plain);
        // scratch が無効。
        let mut off = sccache_settings(tmp.path(), bin);
        off.enabled = false;
        assert_eq!(resolve_sccache(&off, |_| true).label(), "disabled");
        // wrapper は Ready のときだけ書く。
        assert!(!tmp.path().join("scratch/bin/sccache").exists());
        // 本物の probe（loopback だけ）。閉じた port には port 1（特権 port。テストが bind できないので他のテストと
        // 競合しない）を使う。
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(server_listening(l.local_addr().unwrap().port()));
        assert!(!server_listening(1));
    }
}
