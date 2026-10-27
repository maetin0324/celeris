//! ADR 2026-10-07-build-tmp-hygiene D1.1・D1.2・D1.4: `[maintenance.target_sweep]`（共有 cargo target の掃除）。
//! D4.1: `[[maintenance.disk_watch]]`（ディスク使用率の監視の path としきい値）。
//!
//! cron の決定的な保守 executor（雛形の `extra.action = "target_sweep"`）と `celerisctl target sweep` の
//! `--root` 省略時が読む。欄を省略したら `SweepParams::default()` と既定の roots
//! （`<[workspace].build_cache_dir>/cargo` と `<[scratch] dir>/dev-targets`。ADR 2026-10-14-eval-trial-scratch-target D3）。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_worker::target_sweep::SweepParams;

use super::ConfigError;

/// `[maintenance]`。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceConfig {
    #[serde(default)]
    pub target_sweep: TargetSweepConfig,
    /// D4.1: 省略すると既定の 3 つ（`/`・`/local`・`/tmp`、80% / 95%）。`disk_watch = []` で監視しない。
    #[serde(default = "default_disk_watch")]
    pub disk_watch: Vec<DiskWatchConfig>,
    /// ADR 2026-10-27-project-stagnation-inbox D4: `[maintenance.stagnation]`（案件単位の停滞の閾値）。
    #[serde(default)]
    pub stagnation: StagnationConfig,
}

/// `[maintenance.stagnation]`。閾値は秒、`0` でその種類を止める。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagnationConfig {
    #[serde(default = "default_project_idle_secs")]
    pub project_idle_secs: u64,
    #[serde(default = "default_human_wait_secs")]
    pub human_wait_secs: u64,
    #[serde(default = "default_release_unpromoted_secs")]
    pub release_unpromoted_secs: u64,
    #[serde(default = "default_task_unscheduled_secs")]
    pub task_unscheduled_secs: u64,
    /// 判定の間隔（秒、10 以上）。
    #[serde(default = "default_stagnation_scan_interval_secs")]
    pub scan_interval_secs: u64,
}

impl Default for StagnationConfig {
    fn default() -> Self {
        Self {
            project_idle_secs: default_project_idle_secs(),
            human_wait_secs: default_human_wait_secs(),
            release_unpromoted_secs: default_release_unpromoted_secs(),
            task_unscheduled_secs: default_task_unscheduled_secs(),
            scan_interval_secs: default_stagnation_scan_interval_secs(),
        }
    }
}

fn default_project_idle_secs() -> u64 {
    task_ops::stagnation::DEFAULT_PROJECT_IDLE_SECS
}

fn default_human_wait_secs() -> u64 {
    task_ops::stagnation::DEFAULT_HUMAN_WAIT_SECS
}

fn default_release_unpromoted_secs() -> u64 {
    task_ops::stagnation::DEFAULT_RELEASE_UNPROMOTED_SECS
}

fn default_task_unscheduled_secs() -> u64 {
    task_ops::stagnation::DEFAULT_TASK_UNSCHEDULED_SECS
}

fn default_stagnation_scan_interval_secs() -> u64 {
    task_ops::stagnation::DEFAULT_SCAN_INTERVAL_SECS
}

/// `scan_interval_secs` の下限（tick ごとに store を読み直さない）。
const MIN_STAGNATION_SCAN_INTERVAL_SECS: u64 = 10;

impl StagnationConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if self.scan_interval_secs < MIN_STAGNATION_SCAN_INTERVAL_SECS {
            return Err(ConfigError::Invalid(format!(
                "[maintenance.stagnation] scan_interval_secs must be >= {MIN_STAGNATION_SCAN_INTERVAL_SECS} (got {})",
                self.scan_interval_secs
            )));
        }
        Ok(())
    }

    pub fn policy(&self) -> task_ops::stagnation::StagnationPolicy {
        task_ops::stagnation::StagnationPolicy {
            project_idle_secs: self.project_idle_secs,
            human_wait_secs: self.human_wait_secs,
            release_unpromoted_secs: self.release_unpromoted_secs,
            task_unscheduled_secs: self.task_unscheduled_secs,
        }
    }
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            target_sweep: TargetSweepConfig::default(),
            disk_watch: default_disk_watch(),
            stagnation: StagnationConfig::default(),
        }
    }
}

/// `[[maintenance.disk_watch]]` の 1 つ（path としきい値の組。しきい値は使用率 %）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskWatchConfig {
    pub path: PathBuf,
    #[serde(default = "default_warn_pct")]
    pub warn_pct: f64,
    #[serde(default = "default_critical_pct")]
    pub critical_pct: f64,
}

fn default_warn_pct() -> f64 {
    task_dispatch::disk_watch::DEFAULT_WARN_PCT
}

fn default_critical_pct() -> f64 {
    task_dispatch::disk_watch::DEFAULT_CRITICAL_PCT
}

fn default_disk_watch() -> Vec<DiskWatchConfig> {
    task_dispatch::disk_watch::DiskWatchEntry::defaults()
        .into_iter()
        .map(|e| DiskWatchConfig {
            path: e.path,
            warn_pct: e.warn_pct,
            critical_pct: e.critical_pct,
        })
        .collect()
}

impl MaintenanceConfig {
    /// `~` を展開する（相対 path は `validate` で拒否する。監視の path は host の mount 点）。
    pub(super) fn resolve_disk_watch_paths(&mut self) {
        let home = task_core::home_dir();
        for w in &mut self.disk_watch {
            w.path = task_core::expand_home(&w.path, home.as_deref());
        }
    }

    pub(super) fn validate_disk_watch(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for w in &self.disk_watch {
            let invalid = |msg: &str| {
                ConfigError::Invalid(format!(
                    "[[maintenance.disk_watch]] {}: {msg}",
                    w.path.display()
                ))
            };
            if !w.path.is_absolute() {
                return Err(invalid("path must be absolute"));
            }
            if !(w.warn_pct > 0.0 && w.warn_pct < w.critical_pct && w.critical_pct <= 100.0) {
                return Err(invalid(
                    "thresholds must satisfy 0 < warn_pct < critical_pct <= 100",
                ));
            }
            if !seen.insert(w.path.clone()) {
                return Err(invalid("duplicate path"));
            }
        }
        Ok(())
    }
}

/// `[maintenance.target_sweep]`。`roots` を省略すると既定の 2 つ（[`TargetSweepConfig::resolved_roots`]）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSweepConfig {
    #[serde(default)]
    pub roots: Option<Vec<PathBuf>>,
    #[serde(default = "default_max_age_days")]
    pub max_age_days: u64,
    #[serde(default = "default_max_bytes_per_root")]
    pub max_bytes_per_root: u64,
    #[serde(default = "default_target_ratio")]
    pub target_ratio: f64,
    #[serde(default = "default_stale_target_days")]
    pub stale_target_days: u64,
    /// 付記 A2（2026-10-09）: scratch pool の `<scratch>/targets`（run の `CARGO_TARGET_DIR`）も daemon の掃除の
    /// root に足す（`release-build` は除く）。既定 true。scratch が無効なら足さない。
    #[serde(default = "default_true")]
    pub scratch_targets: bool,
    /// 付記 A3（2026-10-09）: 終端になってからこの時間（時）経った task の作業場所の cargo target を消す。
    /// 既定 6。`0` で無効。
    #[serde(default = "default_workspace_target_after_hours")]
    pub workspace_target_after_hours: u64,
}

fn default_true() -> bool {
    true
}

fn default_workspace_target_after_hours() -> u64 {
    task_dispatch::target_sweep::DEFAULT_WORKSPACE_TARGET_AFTER_SECS / 3600
}

impl Default for TargetSweepConfig {
    fn default() -> Self {
        Self {
            roots: None,
            max_age_days: default_max_age_days(),
            max_bytes_per_root: default_max_bytes_per_root(),
            target_ratio: default_target_ratio(),
            stale_target_days: default_stale_target_days(),
            scratch_targets: true,
            workspace_target_after_hours: default_workspace_target_after_hours(),
        }
    }
}

fn default_max_age_days() -> u64 {
    SweepParams::default().max_age_days
}

fn default_max_bytes_per_root() -> u64 {
    SweepParams::default().max_bytes_per_root
}

fn default_target_ratio() -> f64 {
    SweepParams::default().target_ratio
}

fn default_stale_target_days() -> u64 {
    SweepParams::default().stale_target_days
}

impl TargetSweepConfig {
    /// 実際に掃除する roots。書いていなければ [`task_worker::target_sweep::default_roots`]
    /// （`<build_cache_dir>/cargo` と `<scratch_dir>/dev-targets`）。
    pub fn resolved_roots(&self, build_cache_dir: &Path, scratch_dir: &Path) -> Vec<PathBuf> {
        match &self.roots {
            Some(roots) => roots.clone(),
            None => task_worker::target_sweep::default_roots(build_cache_dir, scratch_dir),
        }
    }

    /// 計画関数・I/O 層に渡す値（roots は [`Self::resolved_roots`]）。
    pub fn params(&self, build_cache_dir: &Path, scratch_dir: &Path) -> SweepParams {
        SweepParams {
            roots: self.resolved_roots(build_cache_dir, scratch_dir),
            max_age_days: self.max_age_days,
            max_bytes_per_root: self.max_bytes_per_root,
            target_ratio: self.target_ratio,
            stale_target_days: self.stale_target_days,
        }
    }

    /// `~` を展開し、相対 path は設定ファイル基準にする。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        if let Some(roots) = &mut self.roots {
            let home = task_core::home_dir();
            for root in roots.iter_mut() {
                *root = task_core::expand_home(root, home.as_deref());
                if root.is_relative() {
                    *root = base.join(&*root);
                }
            }
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |msg: &str| ConfigError::Invalid(format!("[maintenance.target_sweep] {msg}"));
        if self.max_age_days == 0 || self.stale_target_days == 0 {
            return Err(invalid(
                "max_age_days and stale_target_days must be at least 1",
            ));
        }
        if self.max_bytes_per_root == 0 {
            return Err(invalid("max_bytes_per_root must be greater than 0"));
        }
        if !(self.target_ratio > 0.0 && self.target_ratio <= 1.0) {
            return Err(invalid("target_ratio must be in (0, 1]"));
        }
        if let Some(roots) = &self.roots
            && roots
                .iter()
                .any(|r| r.as_os_str().is_empty() || r == Path::new("/"))
        {
            return Err(invalid("roots must not contain an empty path or \"/\""));
        }
        Ok(())
    }
}

impl super::Config {
    /// D4: dispatcher の tick の段 `disk_watch` に渡す path としきい値。
    pub fn disk_watch_entries(&self) -> Vec<task_dispatch::disk_watch::DiskWatchEntry> {
        self.maintenance
            .disk_watch
            .iter()
            .map(|w| task_dispatch::disk_watch::DiskWatchEntry {
                path: w.path.clone(),
                warn_pct: w.warn_pct,
                critical_pct: w.critical_pct,
            })
            .collect()
    }

    /// 付記 A2・A3: daemon の保守 executor だけが使う範囲（scratch の root・終わった task の作業場所）。
    pub fn target_sweep_scope(&self) -> task_dispatch::target_sweep::SweepScope {
        let t = &self.maintenance.target_sweep;
        task_dispatch::target_sweep::SweepScope {
            scratch_targets: t.scratch_targets,
            workspace_target_after_secs: t.workspace_target_after_hours.saturating_mul(3600),
        }
    }

    /// cron の保守 executor と `celerisctl target sweep`（`--root` 省略時）が使う掃除の roots と上限。
    pub fn target_sweep_params(&self) -> SweepParams {
        self.maintenance
            .target_sweep
            .params(&self.workspace.build_cache_dir, &self.scratch_dir())
    }
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
