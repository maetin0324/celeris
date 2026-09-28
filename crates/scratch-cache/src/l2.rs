//! L2（NFS）: content-addressed な immutable object `<root>/<k0k1>/<key>.zst`、その I/O を閉じ込める上限付きの
//! スレッド（`L2Exec`）、連続失敗で L2 を切り離す `Breaker`。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::key;

/// L2 の 1 object（GC の走査の結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2Object {
    pub key: String,
    pub size: u64,
    pub mtime: SystemTime,
}

/// L2 の置き場。本物は `DirL2`（NFS のディレクトリ）。テストは遅い・壊れた L2 に差し替える。
pub trait L2Backend: Send + Sync {
    /// object の中身（`.zst` のまま）と mtime。無ければ `None`。**L2 そのものが見えない**（root が無い）ときは Err。
    fn read(&self, key: &str) -> io::Result<Option<(Vec<u8>, SystemTime)>>;
    fn exists(&self, key: &str) -> io::Result<bool>;
    /// tmp → write → fsync → close → rename。既にあれば書かない（`Ok(false)`）。
    fn write(&self, key: &str, data: &[u8]) -> io::Result<bool>;
    fn touch(&self, key: &str, t: SystemTime) -> io::Result<()>;
    fn remove(&self, key: &str) -> io::Result<()>;
    /// 全 object（GC）。古い書きかけの tmp も片づける。
    fn scan(&self) -> io::Result<Vec<L2Object>>;
    /// 表示用の場所。
    fn location(&self) -> String;
}

/// 書きかけの tmp をこれより古ければ走査で消す。
const STALE_TMP: Duration = Duration::from_secs(3600);

/// NFS 上のディレクトリの L2。
pub struct DirL2 {
    root: PathBuf,
    seq: AtomicU64,
}

impl DirL2 {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            seq: AtomicU64::new(0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn object_path(&self, k: &str) -> PathBuf {
        self.root.join(key::shard(k)).join(format!("{k}.zst"))
    }

    /// root が見えなければ Err（NFS が外れた・dir を消された。ここで root を作り直すと mount point の下の
    /// ローカルに書いてしまうので作らない）。
    fn require_root(&self) -> io::Result<()> {
        if self.root.is_dir() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("L2 root {} is not a directory", self.root.display()),
            ))
        }
    }
}

impl L2Backend for DirL2 {
    fn read(&self, k: &str) -> io::Result<Option<(Vec<u8>, SystemTime)>> {
        let path = self.object_path(k);
        let mut f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.require_root()?;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let mtime = f.metadata()?.modified()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(Some((buf, mtime)))
    }

    fn exists(&self, k: &str) -> io::Result<bool> {
        match fs::metadata(self.object_path(k)) {
            Ok(m) => Ok(m.is_file()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.require_root()?;
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    fn write(&self, k: &str, data: &[u8]) -> io::Result<bool> {
        let path = self.object_path(k);
        if path.is_file() {
            return Ok(false);
        }
        self.require_root()?;
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("object path has no parent"))?;
        fs::create_dir_all(dir)?;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{k}.zst.tmp-{}-{nanos}-{seq}", std::process::id()));
        let res = (|| {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
            drop(f);
            // 同じ key の同時書き込みは後勝ち。どちらの中身も同じ入力に対する有効な出力（sccache の key は入力の hash）。
            fs::rename(&tmp, &path)
        })();
        if let Err(e) = res {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(true)
    }

    fn touch(&self, k: &str, t: SystemTime) -> io::Result<()> {
        let f = File::options().write(true).open(self.object_path(k))?;
        f.set_modified(t)
    }

    fn remove(&self, k: &str) -> io::Result<()> {
        match fs::remove_file(self.object_path(k)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn scan(&self) -> io::Result<Vec<L2Object>> {
        self.require_root()?;
        let now = SystemTime::now();
        let mut out = Vec::new();
        for shard in fs::read_dir(&self.root)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for ent in fs::read_dir(shard.path())? {
                let ent = ent?;
                let name = ent.file_name();
                let Some(name) = name.to_str() else { continue };
                let Ok(meta) = ent.metadata() else { continue };
                let mtime = meta.modified().unwrap_or(now);
                if name.starts_with('.') {
                    if name.contains(".zst.tmp-")
                        && now.duration_since(mtime).unwrap_or_default() > STALE_TMP
                    {
                        let _ = fs::remove_file(ent.path());
                    }
                    continue;
                }
                let Some(k) = name.strip_suffix(".zst") else {
                    continue;
                };
                if !key::valid_key(k) || !meta.is_file() {
                    continue;
                }
                out.push(L2Object {
                    key: k.to_string(),
                    size: meta.len(),
                    mtime,
                });
            }
        }
        Ok(out)
    }

    fn location(&self) -> String {
        self.root.display().to_string()
    }
}

// ---------------------------------------------------------------------------
// L2 の I/O を閉じ込める上限付きのスレッド
// ---------------------------------------------------------------------------

type Job = Box<dyn FnOnce() + Send + 'static>;

/// `L2Exec::run` の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// 全スレッドが塞がっていて待ち行列も満杯（NFS が D state で止まっているときなど）。
    Busy,
    /// 時間内に終わらなかった（スレッドはそのまま走り続けるが、呼び出し側は待たない）。
    Timeout,
}

/// 固定数のスレッドと上限付きの待ち行列。NFS の hard mount でスレッドが D state になっても、呼び出し側は
/// タイムアウトで戻り、塞がったスレッドの数以上は増えない。
pub struct L2Exec {
    tx: Mutex<Option<SyncSender<Job>>>,
}

impl L2Exec {
    pub fn new(threads: usize, queue: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Job>(queue.max(1));
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads.max(1) {
            let rx: Arc<Mutex<Receiver<Job>>> = rx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("scratch-cache-l2-{i}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let Ok(guard) = rx.lock() else { return };
                            match guard.recv() {
                                Ok(job) => job,
                                Err(_) => return,
                            }
                        };
                        job();
                    }
                });
        }
        Self {
            tx: Mutex::new(Some(tx)),
        }
    }

    fn submit(&self, job: Job) -> bool {
        let Ok(guard) = self.tx.lock() else {
            return false;
        };
        match guard.as_ref() {
            Some(tx) => tx.try_send(job).is_ok(),
            None => false,
        }
    }

    /// `f` を L2 のスレッドで実行し、`timeout` まで待つ。
    pub fn run<T: Send + 'static>(
        &self,
        timeout: Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ExecError> {
        let (rtx, rrx) = mpsc::sync_channel(1);
        if !self.submit(Box::new(move || {
            let _ = rtx.send(f());
        })) {
            return Err(ExecError::Busy);
        }
        rrx.recv_timeout(timeout).map_err(|_| ExecError::Timeout)
    }

    /// 結果を待たずに L2 のスレッドで実行する（touch・壊れた object の削除）。塞がっていれば捨てる。
    pub fn spawn(&self, f: impl FnOnce() + Send + 'static) -> bool {
        self.submit(Box::new(f))
    }

    /// スレッドを止める（待ち行列の残りを実行し終えたら抜ける）。
    pub fn close(&self) {
        if let Ok(mut guard) = self.tx.lock() {
            guard.take();
        }
    }
}

// ---------------------------------------------------------------------------
// 連続失敗で L2 を切り離す（ADR-0066 D3 と同じ指数バックオフ）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Breaker {
    threshold: u32,
    base: Duration,
    max: Duration,
    failures: u32,
    backoff: Duration,
    retry_at: Option<Duration>,
    degraded_since: Option<SystemTime>,
    last_error: Option<String>,
}

impl Breaker {
    pub fn new(threshold: u32, base: Duration, max: Duration) -> Self {
        Self {
            threshold: threshold.max(1),
            base,
            max: max.max(base),
            failures: 0,
            backoff: base,
            retry_at: None,
            degraded_since: None,
            last_error: None,
        }
    }

    /// L2 を試してよいか（切り離し中でバックオフが明けていなければ `false`。明けたら 1 回試す）。
    pub fn allow(&self, now: Duration) -> bool {
        self.retry_at.is_none_or(|t| now >= t)
    }

    pub fn on_success(&mut self) {
        if self.degraded_since.is_some() {
            tracing::info!("scratch-cache: L2 is reachable again; re-attached");
        }
        self.failures = 0;
        self.backoff = self.base;
        self.retry_at = None;
        self.degraded_since = None;
    }

    pub fn on_failure(&mut self, now: Duration, wall: SystemTime, err: String) {
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(err);
        if self.failures >= self.threshold {
            if self.degraded_since.is_none() {
                tracing::warn!(
                    failures = self.failures,
                    error = self.last_error.as_deref().unwrap_or(""),
                    "scratch-cache: L2 detached after consecutive failures; serving from L1 only"
                );
            }
            self.retry_at = Some(now + self.backoff);
            self.backoff = (self.backoff * 2).min(self.max);
            self.degraded_since.get_or_insert(wall);
        }
    }

    pub fn degraded_since(&self) -> Option<SystemTime> {
        self.degraded_since
    }

    /// 次に試す時刻（単調時計）。
    pub fn retry_at(&self) -> Option<Duration> {
        self.retry_at
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}
