//! ADR 2026-10-07-build-tmp-hygiene D1.1・D1.2・D1.4: `[maintenance.target_sweep]`（共有 cargo target の掃除）。
//! D4.1: `[[maintenance.disk_watch]]`（ディスク使用率の監視の path としきい値）。
//!
//! cron の決定的な保守 executor（雛形の `extra.action = "target_sweep"`）と `celerisctl target sweep` の
//! `--root` 省略時が読む。欄を省略したら `SweepParams::default()` と既定の roots
//! （`<[workspace].build_cache_dir>/cargo` と `/var/tmp/agent-platform-build`）。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use task_worker::target_sweep::SweepParams;

use super::ConfigError;

/// `worktree-target-dir.sh` の `CELERIS_BUILD_CACHE` の既定（D1.1 の既定 root の 2 つ目）。
pub const DEFAULT_DEV_BUILD_CACHE: &str = "/var/tmp/agent-platform-build";

/// `[maintenance]`。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceConfig {
    #[serde(default)]
    pub target_sweep: TargetSweepConfig,
    /// D4.1: 省略すると既定の 3 つ（`/`・`/local`・`/tmp`、80% / 95%）。`disk_watch = []` で監視しない。
    #[serde(default = "default_disk_watch")]
    pub disk_watch: Vec<DiskWatchConfig>,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            target_sweep: TargetSweepConfig::default(),
            disk_watch: default_disk_watch(),
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
}

impl Default for TargetSweepConfig {
    fn default() -> Self {
        Self {
            roots: None,
            max_age_days: default_max_age_days(),
            max_bytes_per_root: default_max_bytes_per_root(),
            target_ratio: default_target_ratio(),
            stale_target_days: default_stale_target_days(),
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
    /// 実際に掃除する roots。書いていなければ `<build_cache_dir>/cargo` と [`DEFAULT_DEV_BUILD_CACHE`]。
    pub fn resolved_roots(&self, build_cache_dir: &Path) -> Vec<PathBuf> {
        match &self.roots {
            Some(roots) => roots.clone(),
            None => vec![
                build_cache_dir.join("cargo"),
                PathBuf::from(DEFAULT_DEV_BUILD_CACHE),
            ],
        }
    }

    /// 計画関数・I/O 層に渡す値（roots は [`Self::resolved_roots`]）。
    pub fn params(&self, build_cache_dir: &Path) -> SweepParams {
        SweepParams {
            roots: self.resolved_roots(build_cache_dir),
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

    /// cron の保守 executor と `celerisctl target sweep`（`--root` 省略時）が使う掃除の roots と上限。
    pub fn target_sweep_params(&self) -> SweepParams {
        self.maintenance
            .target_sweep
            .params(&self.workspace.build_cache_dir)
    }
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
