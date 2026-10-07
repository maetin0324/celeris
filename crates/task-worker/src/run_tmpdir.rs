//! ADR 2026-10-07-build-tmp-hygiene D2: worker run ごとの一時 dir。
//!
//! ローカルの run は起動前に `<workspace>/runs/<run_id>/tmp/`（`0700`）を作り、worker の環境の
//! `TMPDIR`・`TMP`・`TEMP` をそこへ向ける。run の終了（成功・失敗・timeout・中断）で `tmp/` だけを消す
//! （`runs/<run_id>/` の他の file＝ログ・`result.json` は残す）。終端処理の future ごと落とされた場合
//! （cancel・takeover）も [`RunTmpDir`] の `Drop` が消す。daemon が死んで残った `tmp/` は名前
//! （`runs/<run_id>/tmp`）で見つかるので、[`sweep_stale`] が終端 run のものを拾い直せる。

use std::io;
use std::path::{Path, PathBuf};

/// worker に渡す一時 dir の環境変数（D2.1）。
pub const RUN_TMPDIR_VARS: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];

/// `runs/<run_id>/` の下の一時 dir の名前。
pub const RUN_TMPDIR_NAME: &str = "tmp";

/// run の一時 dir の path（作らない）。
pub fn run_tmp_path(workspace: &Path, run_id: &str) -> PathBuf {
    workspace.join("runs").join(run_id).join(RUN_TMPDIR_NAME)
}

/// run の一時 dir。`remove` か `Drop` で中身ごと消える。
#[derive(Debug)]
pub struct RunTmpDir {
    path: PathBuf,
    removed: bool,
}

impl RunTmpDir {
    /// `<workspace>/runs/<run_id>/tmp/` を `0700` で作る。前の attempt の残りがあれば空にしてから作る。
    pub fn create(workspace: &Path, run_id: &str) -> io::Result<Self> {
        let path = run_tmp_path(workspace, run_id);
        remove_if_exists(&path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self {
            path,
            removed: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// worker に重ねる env（`TMPDIR`・`TMP`・`TEMP` = この dir）。
    pub fn env(&self) -> Vec<(String, String)> {
        let value = self.path.display().to_string();
        RUN_TMPDIR_VARS
            .iter()
            .map(|k| ((*k).to_string(), value.clone()))
            .collect()
    }

    /// 中身ごと消す（無ければ成功）。失敗は呼び出し側が記録する（`Drop` ではもう一度試さない）。
    pub fn remove(mut self) -> io::Result<()> {
        self.removed = true;
        remove_if_exists(&self.path)
    }
}

impl Drop for RunTmpDir {
    fn drop(&mut self) {
        if !self.removed
            && let Err(e) = remove_if_exists(&self.path)
        {
            tracing::warn!(path = %self.path.display(), error = %e, "run tmpdir: cleanup on drop failed");
        }
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// D2.2: `<workspace>/runs/*/tmp` のうち `is_live(run_id)` が偽のもの（終端 run の取りこぼし）を消す。
/// 消した path を返す。symlink の `tmp` は辿らない（`remove_dir_all` は link 自体を消すだけ）。
pub fn sweep_stale(workspace: &Path, is_live: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(workspace.join("runs")) else {
        return removed;
    };
    for entry in entries.flatten() {
        let Some(run_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_live(&run_id) {
            continue;
        }
        let tmp = entry.path().join(RUN_TMPDIR_NAME);
        let Ok(meta) = std::fs::symlink_metadata(&tmp) else {
            continue;
        };
        let result = if meta.file_type().is_dir() {
            std::fs::remove_dir_all(&tmp)
        } else {
            std::fs::remove_file(&tmp)
        };
        match result {
            Ok(()) => removed.push(tmp),
            Err(e) => {
                tracing::warn!(path = %tmp.display(), error = %e, "run tmpdir: stale sweep failed")
            }
        }
    }
    removed.sort();
    removed
}

#[cfg(test)]
mod tests;
