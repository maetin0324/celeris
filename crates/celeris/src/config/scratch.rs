//! `[scratch]`（ADR-0075）: ローカルの scratch pool と cargo の既定。

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
    /// ADR-0129 (3): `dir` を置く mount point（例 `/local`）。書けば起動時に mount を確かめ、mount されていなければ
    /// `dir` を従来の場所（`dir` を書かないときの既定）へ戻す。
    #[serde(default)]
    pub mount: Option<PathBuf>,
    /// ADR-0129 (4): 新しい task・WU の target を repo の seed から reflink で作る（既定 true。seed が無ければ空から）。
    #[serde(default = "default_scratch_enabled")]
    pub seed_reflink: bool,
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
    /// 廃止済みの節。古い config を読めるようにするためだけに保持し、値は使わない。
    #[serde(default)]
    sccache: Option<toml::Table>,
    /// ADR-0075 D4（Phase G2）: `[scratch.cargo]`。
    #[serde(default)]
    pub cargo: ScratchCargoConfig,
    #[serde(default)]
    l2: Option<toml::Table>,
    #[serde(default)]
    cache_server: Option<toml::Table>,
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

impl Default for ScratchConfig {
    fn default() -> Self {
        Self {
            enabled: default_scratch_enabled(),
            dir: None,
            mount: None,
            seed_reflink: default_scratch_enabled(),
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
            sccache: None,
            cargo: ScratchCargoConfig::default(),
            l2: None,
            cache_server: None,
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
            None => self.default_scratch_dir(),
        }
    }

    /// `[scratch] dir` を書かないときの場所（`build_cache_dir` の親の `scratch/`）。ADR-0129 (3) の `mount` が
    /// mount されていないときもここへ戻る。
    pub fn default_scratch_dir(&self) -> PathBuf {
        self.workspace
            .build_cache_dir
            .parent()
            .map(|p| p.join("scratch"))
            .unwrap_or_else(|| self.workspace.build_cache_dir.join("scratch"))
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
            cargo: task_worker::scratch::CargoTuning {
                incremental: c.cargo.incremental,
                dev_debug: Some(c.cargo.dev_debug.clone()).filter(|v| !v.is_empty()),
            },
            seed_reflink: c.seed_reflink,
            mount: c.mount.clone(),
            dir_fallback_reason: None,
        }
    }

    /// ADR-0075 D1: 起動時の検査つき。`dir` が NFS 上なら `enabled = false` と理由
    /// （dispatcher が起動ログに出す）。
    /// ADR-0129 (3): `mount` を書いていれば、mount されていないとき `dir` を従来の場所へ戻す（NFS の検査の前）。
    pub fn scratch_settings(&self) -> task_worker::scratch::ScratchSettings {
        task_worker::scratch::apply_nfs_check(
            task_worker::scratch::apply_mount_check(
                self.scratch_settings_unchecked(),
                &self.default_scratch_dir(),
                task_worker::scratch::is_mount_point,
            ),
            task_worker::scratch::is_on_nfs,
        )
    }
}

impl ScratchConfig {
    /// 警告すべき旧節を返す。節の中のキーは互換性のため検証しない。
    pub(super) fn deprecated_sections(&self) -> Vec<&'static str> {
        [
            (self.sccache.is_some(), "scratch.sccache"),
            (self.l2.is_some(), "scratch.l2"),
            (self.cache_server.is_some(), "scratch.cache_server"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect()
    }

    /// ADR-0075 D7: `[scratch] dir`（書いたときだけ。既定は `scratch_dir()` が build_cache_dir の親から組む）。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        let resolve = |p: &PathBuf| {
            let expanded = task_core::expand_home(p, task_core::home_dir().as_deref());
            if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            }
        };
        self.dir = self.dir.as_ref().map(resolve);
        self.mount = self.mount.as_ref().map(resolve);
    }
}
