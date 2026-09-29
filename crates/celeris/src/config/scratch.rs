//! `[scratch]`（ADR-0075）: ローカルの scratch pool・L2・cache server・sccache・cargo の既定。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::Config;

/// `[scratch]`（ADR-0075 D1〜D3、Phase G1）: ローカルの scratch pool。既定は有効。`enabled = false` で ADR-0066 D1 /
/// F5-fix の `build_cache_dir` の挙動に戻る（1 リリースの間の退路）。`dir` が NFS 上なら起動時に無効化する。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchConfig {
    #[serde(default = "default_scratch_enabled")]
    pub enabled: bool,
    /// 既定は `[workspace] build_cache_dir` の**親の `scratch/`**（本番は `/var/lib/celeris/scratch`）。
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default = "default_scratch_targets_max_gb")]
    pub targets_max_gb: u64,
    #[serde(default = "default_scratch_l1_max_gb")]
    pub l1_max_gb: u64,
    #[serde(default = "default_scratch_total_max_gb")]
    pub total_max_gb: u64,
    #[serde(default = "default_scratch_high_watermark")]
    pub high_watermark: f64,
    #[serde(default = "default_scratch_low_watermark")]
    pub low_watermark: f64,
    #[serde(default = "default_scratch_external_lease_ttl_secs")]
    pub external_lease_ttl_secs: u64,
    #[serde(default = "default_scratch_waiting_keep_secs")]
    pub waiting_keep_secs: u64,
    #[serde(default = "default_scratch_failed_keep_secs")]
    pub failed_keep_secs: u64,
    #[serde(default = "default_scratch_completed_grace_secs")]
    pub completed_grace_secs: u64,
    #[serde(default = "default_scratch_warm_seeds_per_repo")]
    pub warm_seeds_per_repo: usize,
    #[serde(default = "default_scratch_gc_max_per_tick")]
    pub gc_max_per_tick: usize,
    #[serde(default = "default_scratch_enabled")]
    pub adopt: bool,
    #[serde(default = "default_scratch_adopt_max_distance")]
    pub adopt_max_distance: u64,
    #[serde(default = "default_scratch_measure_interval_secs")]
    pub measure_interval_secs: u64,
    /// ADR-0075 D4（Phase G2）: `[scratch.sccache]`。既定は有効（バイナリか server が無ければ自動で配線しない）。
    #[serde(default)]
    pub sccache: ScratchSccacheConfig,
    /// ADR-0075 D4（Phase G2）: `[scratch.cargo]`。
    #[serde(default)]
    pub cargo: ScratchCargoConfig,
    /// ADR-0075 D5 (b)（Phase G3）: `[scratch.l2]`。既定で動く（D7 の N-1 の規則）。
    #[serde(default)]
    pub l2: ScratchL2Config,
    /// ADR-0075 D5 (b)（Phase G3）: `[scratch.cache_server]`。既定で動く。
    #[serde(default)]
    pub cache_server: ScratchCacheServerConfig,
}

/// `[scratch.l2]`（ADR-0075 D5 (b)、Phase G3）: cache server の L2（NFS 上の content-addressed な immutable object）。
/// **既定値だけで動く**（本番 config に足すのは、この節を知る release の昇格後。D7）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchL2Config {
    #[serde(default = "default_scratch_enabled")]
    pub enabled: bool,
    /// 既定 `$CELERIS_STATE_DIR/cache/sccache-l2`（`~/.local/celeris/cache/sccache-l2`、NFS）。`~` は展開する。
    #[serde(default)]
    pub dir: Option<PathBuf>,
    /// L2 の上限（GB。超えたら mtime の古い順に消す）。
    #[serde(default = "default_scratch_l2_max_gb")]
    pub max_gb: u64,
    /// flusher の帯域（MB/s。0 = 無制限）。
    #[serde(default = "default_scratch_l2_flush_mbps")]
    pub flush_mbps: u64,
    /// flush の待ち行列の上限（MB）。
    #[serde(default = "default_scratch_l2_flush_queue_max_mb")]
    pub flush_queue_max_mb: u64,
    /// GET の L2 の読み込みを待つ上限（ms）。
    #[serde(default = "default_scratch_l2_get_timeout_ms")]
    pub get_timeout_ms: u64,
    /// L2 の I/O スレッドの数。
    #[serde(default = "default_scratch_l2_io_threads")]
    pub io_threads: usize,
    /// L2 の GC の間隔（秒）。
    #[serde(default = "default_scratch_l2_gc_interval_secs")]
    pub gc_interval_secs: u64,
}

impl Default for ScratchL2Config {
    fn default() -> Self {
        Self {
            enabled: default_scratch_enabled(),
            dir: None,
            max_gb: default_scratch_l2_max_gb(),
            flush_mbps: default_scratch_l2_flush_mbps(),
            flush_queue_max_mb: default_scratch_l2_flush_queue_max_mb(),
            get_timeout_ms: default_scratch_l2_get_timeout_ms(),
            io_threads: default_scratch_l2_io_threads(),
            gc_interval_secs: default_scratch_l2_gc_interval_secs(),
        }
    }
}

fn default_scratch_l2_max_gb() -> u64 {
    300
}
fn default_scratch_l2_flush_mbps() -> u64 {
    25
}
fn default_scratch_l2_flush_queue_max_mb() -> u64 {
    4096
}
fn default_scratch_l2_get_timeout_ms() -> u64 {
    500
}
fn default_scratch_l2_io_threads() -> usize {
    4
}
fn default_scratch_l2_gc_interval_secs() -> u64 {
    86_400
}

/// `[scratch.cache_server]`（ADR-0075 D5 (b)、Phase G3）: `celeris cache-server`（loopback だけに bind）。既定で動く。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchCacheServerConfig {
    /// `false` なら sccache の server は常に G2 の local disk で動く（`celerisctl scratch env --server`）。
    #[serde(default = "default_scratch_enabled")]
    pub enabled: bool,
    /// `127.0.0.1:<port>`（既定 4237）。
    #[serde(default = "default_scratch_cache_server_port")]
    pub port: u16,
    /// DAV の Bearer token（`SCCACHE_WEBDAV_TOKEN`）。既定 `<scratch>/cache-server.token`（cache server が初回に作る）。
    #[serde(default)]
    pub token_file: Option<PathBuf>,
}

impl Default for ScratchCacheServerConfig {
    fn default() -> Self {
        Self {
            enabled: default_scratch_enabled(),
            port: default_scratch_cache_server_port(),
            token_file: None,
        }
    }
}

fn default_scratch_cache_server_port() -> u16 {
    task_worker::scratch::DEFAULT_CACHE_SERVER_PORT
}

/// `[scratch.l2] dir` の既定（ADR-0075 D1 / D5）。
fn default_l2_dir() -> PathBuf {
    celeris_state_dir().join("cache/sccache-l2")
}

fn celeris_state_dir() -> PathBuf {
    std::env::var_os("CELERIS_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| task_core::home_dir().map(|h| h.join(".local/celeris")))
        .unwrap_or_else(|| PathBuf::from(".local/celeris"))
}

/// `[scratch.sccache]`（ADR-0075 D4、Phase G2）。**既定値だけで動く**（本番 config に足すのは、この節を知る
/// release の昇格後。D7 の N-1 の規則）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchSccacheConfig {
    #[serde(default = "default_scratch_enabled")]
    pub enabled: bool,
    /// `SCCACHE_SERVER_PORT`（既定 4236）。
    #[serde(default = "default_scratch_sccache_port")]
    pub port: u16,
    /// 本物の sccache。既定 `$CELERIS_STATE_DIR/tools/sccache/bin/sccache`（`scripts/scratch/setup-sccache.sh` が置く）。
    /// `~` は展開し、相対ならこの設定ファイル基準。
    #[serde(default)]
    pub binary: Option<PathBuf>,
}

impl Default for ScratchSccacheConfig {
    fn default() -> Self {
        Self {
            enabled: default_scratch_enabled(),
            port: default_scratch_sccache_port(),
            binary: None,
        }
    }
}

fn default_scratch_sccache_port() -> u16 {
    task_worker::scratch::DEFAULT_SCCACHE_PORT
}

/// `[scratch.cargo]`（ADR-0075 D4、Phase G2）: scratch を使う経路の cargo の既定。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchCargoConfig {
    /// `false`（既定）なら `CARGO_INCREMENTAL=0` を与える。
    #[serde(default)]
    pub incremental: bool,
    /// `CARGO_PROFILE_DEV_DEBUG`（既定 `"line-tables-only"`）。`""` なら与えない。
    #[serde(default = "default_scratch_dev_debug")]
    pub dev_debug: String,
}

impl Default for ScratchCargoConfig {
    fn default() -> Self {
        Self {
            incremental: false,
            dev_debug: default_scratch_dev_debug(),
        }
    }
}

fn default_scratch_dev_debug() -> String {
    task_worker::scratch::DEFAULT_DEV_DEBUG.to_string()
}

/// `[scratch.sccache] binary` の既定（ADR-0075 D4）。
fn default_sccache_binary() -> PathBuf {
    std::env::var_os("CELERIS_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| task_core::home_dir().map(|h| h.join(".local/celeris")))
        .unwrap_or_else(|| PathBuf::from(".local/celeris"))
        .join("tools/sccache/bin/sccache")
}

impl Default for ScratchConfig {
    fn default() -> Self {
        Self {
            enabled: default_scratch_enabled(),
            dir: None,
            targets_max_gb: default_scratch_targets_max_gb(),
            l1_max_gb: default_scratch_l1_max_gb(),
            total_max_gb: default_scratch_total_max_gb(),
            high_watermark: default_scratch_high_watermark(),
            low_watermark: default_scratch_low_watermark(),
            external_lease_ttl_secs: default_scratch_external_lease_ttl_secs(),
            waiting_keep_secs: default_scratch_waiting_keep_secs(),
            failed_keep_secs: default_scratch_failed_keep_secs(),
            completed_grace_secs: default_scratch_completed_grace_secs(),
            warm_seeds_per_repo: default_scratch_warm_seeds_per_repo(),
            gc_max_per_tick: default_scratch_gc_max_per_tick(),
            adopt: default_scratch_enabled(),
            adopt_max_distance: default_scratch_adopt_max_distance(),
            measure_interval_secs: default_scratch_measure_interval_secs(),
            sccache: ScratchSccacheConfig::default(),
            cargo: ScratchCargoConfig::default(),
            l2: ScratchL2Config::default(),
            cache_server: ScratchCacheServerConfig::default(),
        }
    }
}

fn default_scratch_enabled() -> bool {
    true
}
fn default_scratch_targets_max_gb() -> u64 {
    100
}
fn default_scratch_l1_max_gb() -> u64 {
    40
}
fn default_scratch_total_max_gb() -> u64 {
    150
}
fn default_scratch_high_watermark() -> f64 {
    0.90
}
fn default_scratch_low_watermark() -> f64 {
    0.70
}
fn default_scratch_external_lease_ttl_secs() -> u64 {
    21_600
}
fn default_scratch_waiting_keep_secs() -> u64 {
    172_800
}
fn default_scratch_failed_keep_secs() -> u64 {
    86_400
}
fn default_scratch_completed_grace_secs() -> u64 {
    600
}
fn default_scratch_warm_seeds_per_repo() -> usize {
    1
}
fn default_scratch_gc_max_per_tick() -> usize {
    8
}
fn default_scratch_adopt_max_distance() -> u64 {
    200
}
fn default_scratch_measure_interval_secs() -> u64 {
    30
}

impl Config {
    /// ADR-0075 D7: `[scratch] dir`。書いていなければ `[workspace] build_cache_dir` の親の `scratch/`。
    pub fn scratch_dir(&self) -> PathBuf {
        match &self.scratch.dir {
            Some(dir) => dir.clone(),
            None => self
                .workspace
                .build_cache_dir
                .parent()
                .map(|p| p.join("scratch"))
                .unwrap_or_else(|| self.workspace.build_cache_dir.join("scratch")),
        }
    }

    /// ADR-0075 D1: `[scratch]` を解決した値（NFS の検査をしない。テストと `scratch_settings` の下請け）。
    pub fn scratch_settings_unchecked(&self) -> task_worker::scratch::ScratchSettings {
        let c = &self.scratch;
        let gib = task_worker::scratch::GIB;
        task_worker::scratch::ScratchSettings {
            enabled: c.enabled,
            disabled_reason: None,
            dir: self.scratch_dir(),
            targets_max_bytes: c.targets_max_gb.saturating_mul(gib),
            l1_max_bytes: c.l1_max_gb.saturating_mul(gib),
            total_max_bytes: c.total_max_gb.saturating_mul(gib),
            high_watermark: c.high_watermark,
            low_watermark: c.low_watermark,
            external_lease_ttl_secs: c.external_lease_ttl_secs,
            waiting_keep_secs: c.waiting_keep_secs,
            failed_keep_secs: c.failed_keep_secs,
            completed_grace_secs: c.completed_grace_secs,
            warm_seeds_per_repo: c.warm_seeds_per_repo,
            gc_max_per_tick: c.gc_max_per_tick,
            adopt: c.adopt,
            adopt_max_distance: c.adopt_max_distance,
            measure_interval_secs: c.measure_interval_secs,
            sccache: task_worker::scratch::SccacheSettings {
                enabled: c.sccache.enabled,
                binary: c
                    .sccache
                    .binary
                    .clone()
                    .unwrap_or_else(default_sccache_binary),
                server_port: c.sccache.port,
            },
            cargo: task_worker::scratch::CargoTuning {
                incremental: c.cargo.incremental,
                dev_debug: Some(c.cargo.dev_debug.clone()).filter(|v| !v.is_empty()),
            },
            l2: task_worker::scratch::L2Settings {
                enabled: c.l2.enabled,
                dir: c
                    .l2
                    .dir
                    .as_deref()
                    .map(|d| task_core::expand_home(d, task_core::home_dir().as_deref()))
                    .unwrap_or_else(default_l2_dir),
                max_bytes: c.l2.max_gb.saturating_mul(gib),
                flush_mbps: c.l2.flush_mbps,
                flush_queue_max_mb: c.l2.flush_queue_max_mb,
                get_timeout_ms: c.l2.get_timeout_ms,
                io_threads: c.l2.io_threads,
                gc_interval_secs: c.l2.gc_interval_secs,
            },
            cache_server: task_worker::scratch::CacheServerSettings {
                enabled: c.cache_server.enabled,
                port: c.cache_server.port,
                token_file: c
                    .cache_server
                    .token_file
                    .as_deref()
                    .map(|d| task_core::expand_home(d, task_core::home_dir().as_deref()))
                    .unwrap_or_else(|| self.scratch_dir().join("cache-server.token")),
            },
        }
    }

    /// ADR-0075 D1: 起動時の検査つき。`dir` か `sccache-l1/` が NFS 上なら `enabled = false` と理由
    /// （dispatcher が起動ログに出す）。
    pub fn scratch_settings(&self) -> task_worker::scratch::ScratchSettings {
        task_worker::scratch::apply_nfs_check(
            self.scratch_settings_unchecked(),
            task_worker::scratch::is_on_nfs,
        )
    }
}

impl ScratchConfig {
    /// ADR-0075 D7: `[scratch] dir`（書いたときだけ。既定は `scratch_dir()` が build_cache_dir の親から組む）。
    /// ADR-0075 D4（Phase G2）: `[scratch.sccache] binary`（書いたときだけ）。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        if let Some(dir) = &self.dir {
            let expanded = task_core::expand_home(dir, task_core::home_dir().as_deref());
            self.dir = Some(if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            });
        }
        if let Some(bin) = &self.sccache.binary {
            let expanded = task_core::expand_home(bin, task_core::home_dir().as_deref());
            self.sccache.binary = Some(if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            });
        }
    }
}
