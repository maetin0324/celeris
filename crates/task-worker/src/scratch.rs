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
// 継いだ sccache 系の env を外す（G3-fix1）
// ---------------------------------------------------------------------------

/// 経路の子プロセスに与える env の全体（G3-fix1）。`set` は `cargo_env` と同じ（重ねる値）、`remove` は
/// 親（daemon・テストの process・人の shell）から継いだものを**外す** key（`Command::env_remove`）。
/// sccache を配線しないときに継いだ `RUSTC_WRAPPER` が run に漏れると、Celeris が Celeris を build する run
/// （ADR-0040 の self-dogfood）の中で配線の判定が親の値に上書きされる。`remove` と `set` は交わらない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoEnv {
    pub set: Vec<(String, String)>,
    pub remove: Vec<String>,
}

impl CargoEnv {
    /// `set` だけ（`remove` 無し。legacy の `CARGO_TARGET_DIR` など、sccache の判定をしない経路）。
    pub fn set_only(set: Vec<(String, String)>) -> Self {
        Self {
            set,
            remove: Vec::new(),
        }
    }

    /// `set` から `remove` を組む（`sccache_env_removals`。親の process の env を見る）。
    pub fn from_set(set: Vec<(String, String)>) -> Self {
        let remove = sccache_env_removals(&set);
        Self { set, remove }
    }

    /// `env_remove` を持たない経路（`WorkerAdapter::with_env_removed` が `None` のアダプタ）への代替: `remove` に
    /// ある `RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` を空の値で上書きした `set`。cargo は空の値を「未設定」と
    /// 扱う（cargo 1.98.1 で確かめた。G3-fix1）。`SCCACHE_*` は空にしない（sccache は空の値を「設定あり」と
    /// 読みうる。wrapper が無ければ cargo は sccache を起こさない）。
    pub fn set_with_empty_wrappers(&self) -> Vec<(String, String)> {
        let mut set = self.set.clone();
        for key in RUSTC_WRAPPER_VARS {
            if self.remove.iter().any(|k| k == key) {
                set.push((key.to_string(), String::new()));
            }
        }
        set
    }
}

/// cargo が rustc を包む env（G3-fix1）。sccache を配線しないときは両方、配線するときは `RUSTC_WORKSPACE_WRAPPER`
/// だけを外す（`RUSTC_WRAPPER` は Celeris の wrapper）。
pub const RUSTC_WRAPPER_VARS: [&str; 2] = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"];
/// sccache が読む env の接頭辞。
pub const SCCACHE_VAR_PREFIX: &str = "SCCACHE_";
/// 親の env に無くても `remove` に常に入れる `SCCACHE_*`（Celeris が与えうる key。結果を親の env に依らせない）。
pub const KNOWN_SCCACHE_VARS: [&str; 7] = [
    "SCCACHE_DIR",
    "SCCACHE_CACHE_SIZE",
    "SCCACHE_SERVER_PORT",
    "SCCACHE_IDLE_TIMEOUT",
    "SCCACHE_WEBDAV_ENDPOINT",
    "SCCACHE_WEBDAV_KEY_PREFIX",
    "SCCACHE_WEBDAV_TOKEN",
];

/// sccache の配線に関わる env か（`RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` / `SCCACHE_*`）。
pub fn is_sccache_family(key: &str) -> bool {
    RUSTC_WRAPPER_VARS.contains(&key) || key.starts_with(SCCACHE_VAR_PREFIX)
}

/// 外す key（純粋。G3-fix1）: sccache の族（`RUSTC_WRAPPER_VARS`、`KNOWN_SCCACHE_VARS`、`inherited` に居る `SCCACHE_*`）
/// のうち `set` が与えないもの。並びは辞書順・重複なし。shell の名前にならない key は入れない（`unset` に出すため）。
pub fn sccache_env_removals_with(
    set: &[(String, String)],
    inherited: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut keys: Vec<String> = RUSTC_WRAPPER_VARS
        .iter()
        .chain(KNOWN_SCCACHE_VARS.iter())
        .map(|k| k.to_string())
        .chain(inherited.into_iter().filter(|k| {
            k.starts_with(SCCACHE_VAR_PREFIX)
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }))
        .filter(|k| !set.iter().any(|(s, _)| s == k))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// `sccache_env_removals_with` に今の process の env を渡す。
pub fn sccache_env_removals(set: &[(String, String)]) -> Vec<String> {
    sccache_env_removals_with(
        set,
        std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()),
    )
}

/// `cargo_env_with` に `remove` を足したもの（dispatcher の run・checks が使う）。
pub fn cargo_child_env_with(
    settings: &ScratchSettings,
    owner: &Owner,
    state: &SccacheState,
) -> CargoEnv {
    CargoEnv::from_set(cargo_env_with(settings, owner, state))
}

/// `cargo_env` に `remove` を足したもの（dispatcher と `celerisctl scratch env` が使う）。
pub fn cargo_child_env(settings: &ScratchSettings, owner: &Owner) -> CargoEnv {
    CargoEnv::from_set(cargo_env(settings, owner))
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

// Lease と cargo 環境の管理から GC の分類・回収計画を分離する。
mod gc;
pub use gc::*;

#[cfg(test)]
mod tests;
