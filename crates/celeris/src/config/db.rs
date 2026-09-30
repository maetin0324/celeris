//! `db`（ADR-0064 D1）: 文字列でも `[db]` テーブルでも書ける DB の設定。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// ADR-0045 D2: `~/.local/celeris/celeris.sqlite3`（Phase 57 までは設定ファイル基準の旧い名前だった）。
fn default_db() -> PathBuf {
    PathBuf::from("~/.local/celeris/celeris.sqlite3")
}

/// ADR-0064 D1: `db` キー。**文字列**（`db = "<path>"`。従来どおり、互換）か、**テーブル**
/// （`[db]` に `path` と `busy_timeout_ms` / `checkpoint_interval_secs` / `backup_dir` /
/// `backup_interval_secs` / `backup_keep` を書く）のどちらでも受け付ける。状態ディレクトリ
/// （`~/.local/celeris`）は `/home` のままで、DB ファイルだけローカルディスクに置けるようにする
/// のが狙い（本番で観測した I/O 遅延。`docs/adr/0064-db-local-disk-and-store-resilience.md`）。
#[derive(Debug, Clone, PartialEq)]
pub struct DbConfig {
    pub path: PathBuf,
    /// `PRAGMA busy_timeout`（既定 5000 ms。本番では 15000 ms を勧める）。
    pub busy_timeout_ms: u64,
    /// 背景チェックポイント（`PRAGMA wal_checkpoint(PASSIVE)`）の間隔（既定 30 秒）。
    pub checkpoint_interval_secs: u64,
    /// 定期バックアップの置き場所。`None`（既定）ならバックアップしない。相対パスは設定ファイル基準
    /// （`Config::load` が絶対化する）。
    pub backup_dir: Option<PathBuf>,
    /// 定期バックアップの間隔（既定 3600 秒）。
    pub backup_interval_secs: u64,
    /// 残す世代数（既定 48）。
    pub backup_keep: usize,
}

impl DbConfig {
    pub fn busy_timeout(&self) -> Duration {
        Duration::from_millis(self.busy_timeout_ms)
    }

    pub fn checkpoint_interval(&self) -> Duration {
        Duration::from_secs(self.checkpoint_interval_secs)
    }

    pub fn backup_interval(&self) -> Duration {
        Duration::from_secs(self.backup_interval_secs)
    }
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            path: default_db(),
            busy_timeout_ms: default_db_busy_timeout_ms(),
            checkpoint_interval_secs: default_db_checkpoint_interval_secs(),
            backup_dir: None,
            backup_interval_secs: default_db_backup_interval_secs(),
            backup_keep: default_db_backup_keep(),
        }
    }
}

fn default_db_busy_timeout_ms() -> u64 {
    5000
}
fn default_db_checkpoint_interval_secs() -> u64 {
    30
}
fn default_db_backup_interval_secs() -> u64 {
    3600
}
fn default_db_backup_keep() -> usize {
    48
}

impl<'de> Deserialize<'de> for DbConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Path(PathBuf),
            Table(Table),
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Table {
            #[serde(default = "default_db")]
            path: PathBuf,
            #[serde(default = "default_db_busy_timeout_ms")]
            busy_timeout_ms: u64,
            #[serde(default = "default_db_checkpoint_interval_secs")]
            checkpoint_interval_secs: u64,
            #[serde(default)]
            backup_dir: Option<PathBuf>,
            #[serde(default = "default_db_backup_interval_secs")]
            backup_interval_secs: u64,
            #[serde(default = "default_db_backup_keep")]
            backup_keep: usize,
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Path(path) => DbConfig {
                path,
                ..DbConfig::default()
            },
            Repr::Table(t) => DbConfig {
                path: t.path,
                busy_timeout_ms: t.busy_timeout_ms,
                checkpoint_interval_secs: t.checkpoint_interval_secs,
                backup_dir: t.backup_dir,
                backup_interval_secs: t.backup_interval_secs,
                backup_keep: t.backup_keep,
            },
        })
    }
}

impl DbConfig {
    /// ADR-0045 D2 / ADR-0064 D1: `db` の既定は `~/.local/celeris/celeris.sqlite3`。`~` を展開して
    /// から、それでも相対なら従来どおり設定ファイルのディレクトリ基準にする。`[db] backup_dir` も
    /// 同じ規則（省略時は触らない）。
    pub(super) fn resolve_paths(&mut self, base: &Path) {
        self.path = task_core::expand_home(&self.path, task_core::home_dir().as_deref());
        if self.path.is_relative() {
            self.path = base.join(&self.path);
        }
        if let Some(backup_dir) = &self.backup_dir {
            let expanded = task_core::expand_home(backup_dir, task_core::home_dir().as_deref());
            self.backup_dir = Some(if expanded.is_relative() {
                base.join(&expanded)
            } else {
                expanded
            });
        }
    }
}
