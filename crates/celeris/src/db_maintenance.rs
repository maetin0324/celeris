//! ADR-0064 D3 / D5（Phase 110a）: DB の背景メンテナンス。
//!
//! - **背景チェックポイント（D5）**: デーモンの各接続は `background_checkpoint = true`（`wal_autocheckpoint = 0`）
//!   で開くので、`PRAGMA wal_checkpoint` は自動では走らない。ここが `checkpoint_interval_secs` ごとに
//!   専用の接続で `PASSIVE` チェックポイントを打つ（fsync がリクエストや tick の中に落ちないように）。
//! - **定期バックアップ（D3）**: `[db] backup_dir` があれば `backup_interval_secs` ごとに
//!   `VACUUM INTO` で一貫した WAL スナップショットを作り、`backup_keep` 世代だけ残す。
//!
//! どちらも `tokio::spawn` の背景タスクで、専用の同期接続を `spawn_blocking` の中で開く（tick も
//! API のリクエストも待たせない）。失敗は WARN で継続する（デーモンは止めない）。

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use time::OffsetDateTime;

/// `background_checkpoint` の接続に合わせる WAL の目安サイズ（64 MB。ADR-0064 D5）。
/// `task_core::store` の同名の定数と値を揃えている（`configure_pragmas` の `journal_size_limit`）。
const JOURNAL_SIZE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// 動いている背景タスク。`stop` で graceful に止める（`RunningApi` 等と同じ流儀）。
pub struct RunningDbMaintenance {
    stop: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
    name: &'static str,
    cancel: Option<Arc<AtomicBool>>,
}

impl RunningDbMaintenance {
    /// 同じ流儀の背景タスク（`chat_gc` 等）が止め方を共有する。
    pub(crate) fn from_task(
        stop: tokio::sync::oneshot::Sender<()>,
        handle: tokio::task::JoinHandle<()>,
        name: &'static str,
    ) -> Self {
        Self {
            stop,
            handle,
            name,
            cancel: None,
        }
    }

    pub async fn stop(self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        let _ = self.stop.send(());
        match tokio::time::timeout(Duration::from_secs(5), self.handle).await {
            Ok(Ok(())) => tracing::info!(task = self.name, "db maintenance task stopped"),
            Ok(Err(e)) => {
                tracing::error!(task = self.name, error = %e, "db maintenance task panicked")
            }
            Err(_) => tracing::warn!(
                task = self.name,
                "db maintenance task did not stop within 5s"
            ),
        }
    }
}

/// ADR-0064 D5: `checkpoint_interval` ごとに `PRAGMA wal_checkpoint(PASSIVE)` を打つ背景タスクを起こす。
pub fn spawn_checkpoint_task(
    db_path: PathBuf,
    checkpoint_interval: Duration,
    busy_timeout: Duration,
) -> RunningDbMaintenance {
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(checkpoint_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await; // 起動直後の 1 回は間隔を置く（最初の tick は即座に返るため）。
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = ticker.tick() => {
                    let path = db_path.clone();
                    let result = tokio::task::spawn_blocking(move || checkpoint_once(&path, busy_timeout)).await;
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::warn!(error = %e, "background checkpoint failed; will retry next interval"),
                        Err(e) => tracing::warn!(error = %e, "background checkpoint task panicked; will retry next interval"),
                    }
                }
            }
        }
    });
    RunningDbMaintenance {
        stop: stop_tx,
        handle,
        name: "checkpoint",
        cancel: None,
    }
}

fn checkpoint_once(db_path: &Path, busy_timeout: Duration) -> Result<(), rusqlite::Error> {
    let conn = rusqlite::Connection::open(db_path)?;
    conn.busy_timeout(busy_timeout)?;
    run_checkpoint_pragma(&conn, "PASSIVE")?;
    // ADR-0064 D5: WAL が `journal_size_limit`（64 MB）を超えていたら TRUNCATE も試みる（PASSIVE は
    // 読み取り中の接続があると縮められないことがあるため、これも best-effort）。
    if wal_file_len(db_path).is_some_and(|len| len > JOURNAL_SIZE_LIMIT_BYTES) {
        run_checkpoint_pragma(&conn, "TRUNCATE")?;
    }
    Ok(())
}

fn run_checkpoint_pragma(conn: &rusqlite::Connection, mode: &str) -> Result<(), rusqlite::Error> {
    conn.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |_row| Ok(()))?;
    Ok(())
}

fn wal_file_len(db_path: &Path) -> Option<u64> {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    std::fs::metadata(PathBuf::from(name)).ok().map(|m| m.len())
}

#[derive(Debug, thiserror::Error)]
enum BackupError {
    #[error("backup_dir {0} does not exist (create it first, e.g. with sudo)")]
    MissingDir(PathBuf),
    #[error("backup interrupted")]
    Cancelled,
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// ADR-0064 D3: `backup_interval` ごとに `backup_dir` へ `celeris-<unix_ts>.sqlite3` を書き、
/// `keep` 世代だけ残す背景タスクを起こす。
pub fn spawn_backup_task(
    db_path: PathBuf,
    backup_dir: PathBuf,
    backup_interval: Duration,
    keep: usize,
    busy_timeout: Duration,
) -> RunningDbMaintenance {
    let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(backup_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut stop_rx => break,
                _ = ticker.tick() => {
                    let src = db_path.clone();
                    let dir = backup_dir.clone();
                    let cancelled = Arc::clone(&worker_cancel);
                    let result = tokio::task::spawn_blocking(move || backup_once(&src, &dir, keep, busy_timeout, cancelled)).await;
                    match result {
                        Ok(Ok(dest)) => tracing::info!(backup = %dest.display(), "wrote a periodic db backup"),
                        Ok(Err(_)) if worker_cancel.load(Ordering::Relaxed) => {}
                        Ok(Err(e)) => tracing::warn!(error = %e, "periodic db backup failed; will retry next interval"),
                        Err(e) => tracing::warn!(error = %e, "periodic db backup task panicked; will retry next interval"),
                    }
                }
            }
        }
    });
    RunningDbMaintenance {
        stop: stop_tx,
        handle,
        name: "backup",
        cancel: Some(cancel),
    }
}

/// ADR-0064 D3: 1 回分のバックアップ。書いたファイルのパスを返す。
fn backup_once(
    db_path: &Path,
    backup_dir: &Path,
    keep: usize,
    busy_timeout: Duration,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, BackupError> {
    // D2 の relocate-db.sh と同じ規律: ディレクトリは作らない（人が sudo で作る前提）。
    if !backup_dir.is_dir() {
        return Err(BackupError::MissingDir(backup_dir.to_path_buf()));
    }
    cleanup_incomplete_backups(backup_dir)?;
    let now = OffsetDateTime::now_utc();
    let ts = now.unix_timestamp();
    let dest = backup_dir.join(format!("celeris-{ts}.sqlite3"));
    // Separate simultaneous daemon instances must never write the same staging database.
    let partial = backup_dir.join(format!(
        "celeris-{ts}-{}-{}.sqlite3.partial",
        std::process::id(),
        now.unix_timestamp_nanos()
    ));
    // The old incremental backup can restart on every write from another connection. VACUUM INTO
    // holds one WAL read snapshot while writers continue, and its progress handler lets shutdown
    // interrupt the copy. Only the atomic rename makes a backup visible to retention.
    let result = (|| -> Result<(), BackupError> {
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.busy_timeout(busy_timeout)?;
        let progress_cancel = Arc::clone(&cancel);
        conn.progress_handler(1000, Some(move || progress_cancel.load(Ordering::Relaxed)));
        if cancel.load(Ordering::Relaxed) {
            return Err(BackupError::Cancelled);
        }
        conn.execute("VACUUM INTO ?1", [partial.to_string_lossy().as_ref()])?;
        if cancel.load(Ordering::Relaxed) {
            return Err(BackupError::Cancelled);
        }
        std::fs::rename(&partial, &dest)?;
        Ok(())
    })();
    if result.is_err() {
        remove_backup_files(&partial);
    }
    result?;
    prune_backups(backup_dir, keep)?;
    Ok(dest)
}

fn remove_backup_files(path: &Path) {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

/// Remove interrupted staging files and legacy backup-API files with rollback journals.
fn cleanup_incomplete_backups(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A second daemon may be writing in the same directory. Stale files from a prior
        // interval are old enough to distinguish from its active staging file.
        let stale = path
            .metadata()
            .and_then(|m| m.modified())
            .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
            .is_ok_and(|age| age >= Duration::from_secs(300));
        if !stale {
            continue;
        }
        if name.starts_with("celeris-") && name.ends_with(".sqlite3.partial") {
            remove_backup_files(&path);
        } else if name.starts_with("celeris-")
            && ["-journal", "-wal", "-shm"]
                .iter()
                .any(|suffix| name.ends_with(&format!(".sqlite3.partial{suffix}")))
        {
            let base = name
                .split_once(".sqlite3.partial")
                .map(|(stem, _)| dir.join(format!("{stem}.sqlite3.partial")))
                .expect("suffix checked above");
            remove_backup_files(&base);
        } else if name.starts_with("celeris-") && name.ends_with(".sqlite3-journal") {
            let db = PathBuf::from(path.to_string_lossy().trim_end_matches("-journal"));
            remove_backup_files(&db);
        }
    }
    Ok(())
}

/// `backup_dir` の `celeris-*.sqlite3` を新しい順に見て、`keep` を超えた古い世代を消す。
fn prune_backups(backup_dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(backup_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| is_backup_file_name(p))
        .collect();
    // ファイル名が `celeris-<unix_ts>.sqlite3`（固定書式の 10 進数）なので、文字列の昇順が時系列の昇順になる。
    files.sort();
    let excess = files.len().saturating_sub(keep);
    for old in files.into_iter().take(excess) {
        // 消せなくても他の世代の削除は続ける（NFS の一時的な失敗等で全体を止めない）。
        if let Err(e) = std::fs::remove_file(&old) {
            tracing::warn!(path = %old.display(), error = %e, "could not remove an old db backup");
        }
    }
    Ok(())
}

fn is_backup_file_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("celeris-") && n.ends_with(".sqlite3"))
}

#[cfg(test)]
#[path = "db_maintenance/tests.rs"]
mod tests;
