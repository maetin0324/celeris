//! `[storage]`（ADR-0136）: hot データの正本を置く mount（本番は `/local`）の起動前検査。
//!
//! hot な path（DB・workspaces・build cache・scratch・containers・memory・releases・新規 backup）は
//! それぞれ既存の key（`[db].path`・`workspace_root`・`[workspace].build_cache_dir`・`[scratch].dir`・
//! `[containers].build_dir`・`[memory].dir`・`[selfdeploy].releases_dir`・`[db].backup_dir`）で変える。
//! この節はそれらの置き場が**本当に mount されているか**だけを見る。`hot_mount` を書かなければ
//! 何も検査しない（既定は従来どおり）。書いたのに mount されていなければ、root fs に同名の
//! ディレクトリを作る前に（DB を開く・dir を作るより前に）理由を出して起動を止める。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{Config, ConfigError};

/// ```toml
/// [storage]
/// hot_mount = "/local"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// ADR-0136: hot データの正本を置く mount point（絶対 path）。`None`（既定）なら検査しない。
    #[serde(default)]
    pub hot_mount: Option<PathBuf>,
}

impl StorageConfig {
    pub(super) fn resolve_paths(&mut self) {
        if let Some(mount) = &self.hot_mount {
            self.hot_mount = Some(task_core::expand_home(
                mount,
                task_core::home_dir().as_deref(),
            ));
        }
    }

    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        if let Some(mount) = &self.hot_mount
            && (!mount.is_absolute() || mount == Path::new("/"))
        {
            return Err(ConfigError::Invalid(format!(
                "[storage] hot_mount must be an absolute path other than / (got {})",
                mount.display()
            )));
        }
        Ok(())
    }
}

/// `/proc/self/mountinfo` の内容に、`mount_point` **ちょうど**の mount の行があるか。
/// 祖先（`/` など）の mount に含まれるだけなら `false`（rootfs 上の同名 dir を mount と取り違えない）。
/// mount point の空白などは mountinfo では 8 進 escape（`\040`）なので、比べる前に戻す。
pub fn is_mount_point(mountinfo: &str, mount_point: &Path) -> bool {
    mountinfo.lines().any(|line| {
        let Some((before, _)) = line.split_once(" - ") else {
            return false;
        };
        before
            .split_whitespace()
            .nth(4)
            .is_some_and(|field| Path::new(&unescape_mountinfo(field)) == mount_point)
    })
}

fn unescape_mountinfo(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|d| bytes[i] == b'\\' && d.iter().all(|b| (b'0'..=b'7').contains(b)))
            .map(|d| d.iter().fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0')))
            .and_then(|v| u8::try_from(v).ok());
        match octal {
            Some(value) => {
                out.push(value);
                i += 4;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Config {
    /// ADR-0136 の hot な path（名前と解決後の値）。`[memory]`・`[db].backup_dir` は書いたときだけ。
    /// `Config::load` の後（絶対化済み）に呼ぶ。
    pub fn hot_paths(&self) -> Vec<(&'static str, PathBuf)> {
        let mut paths = vec![
            (
                "[db].path",
                self.db
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.db.path.clone()),
            ),
            ("workspace_root", self.workspace_root.clone()),
            (
                "[workspace].build_cache_dir",
                self.workspace.build_cache_dir.clone(),
            ),
            ("[scratch].dir", self.scratch_dir()),
            ("[containers].build_dir", self.containers.build_dir.clone()),
            (
                "[selfdeploy].releases_dir",
                self.selfdeploy.releases_dir.clone(),
            ),
        ];
        if let Some(memory) = &self.memory {
            paths.push(("[memory].dir", memory.dir.clone()));
        }
        if let Some(backup_dir) = &self.db.backup_dir {
            paths.push(("[db].backup_dir", backup_dir.clone()));
        }
        paths
    }

    /// ADR-0136: `[storage] hot_mount` が mount されていることを確かめる（`mountinfo` は
    /// `/proc/self/mountinfo` の内容。`None` は読めなかった）。未設定なら常に `Ok`。
    /// mount されていなければ、その下に置くはずの path を挙げて `Err`（dir は作らない）。
    pub fn check_hot_mount(&self, mountinfo: Option<&str>) -> Result<(), ConfigError> {
        let Some(mount) = &self.storage.hot_mount else {
            return Ok(());
        };
        if mountinfo.is_some_and(|text| is_mount_point(text, mount)) {
            return Ok(());
        }
        let under: Vec<String> = self
            .hot_paths()
            .into_iter()
            .filter(|(_, path)| path.starts_with(mount))
            .map(|(key, path)| format!("{key} = {}", path.display()))
            .collect();
        let why = if mountinfo.is_none() {
            "cannot read /proc/self/mountinfo to confirm it is mounted"
        } else {
            "it is not a mount point (only a directory on a parent filesystem, or missing)"
        };
        Err(ConfigError::Invalid(format!(
            "[storage] hot_mount {}: {why}; refusing to start so that hot data ({}) is not created \
             on the root filesystem (ADR-0136). Mount it, or remove [storage] hot_mount and point \
             the paths elsewhere",
            mount.display(),
            if under.is_empty() {
                "none configured under it".to_string()
            } else {
                under.join(", ")
            }
        )))
    }
}
