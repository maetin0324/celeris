//! `TieredStore`: L1（ローカル）→ L2（NFS）→ miss。PUT は L1 に書いてすぐ返し、flusher が L2 へ非同期に書く。

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use task_ops::daemon::{SCRATCH_CACHE_STATS_SCHEMA, ScratchCacheStats};

use crate::bucket::TokenBucket;
use crate::clock::{Clock, SystemClock};
use crate::key;
use crate::l2::{Breaker, DirL2, ExecError, L2Backend, L2Exec};
use crate::rfc3339;

/// MB（10^6 byte。`[scratch.l2] flush_mbps` の単位）。
pub const MB: u64 = 1_000_000;

/// 未 flush の key の追記ログ（L1 の dir の直下。再起動後に積み直す）。
pub const PENDING_LOG: &str = ".pending";

/// `TieredStore` の設定（`celeris cache-server` が `[scratch]` / `[scratch.l2]` から組む）。
#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub l1_dir: PathBuf,
    pub l1_max_bytes: u64,
    /// L1 の使用量がこの割合を超えたら `l1_low_watermark` まで LRU で落とす。
    pub l1_high_watermark: f64,
    pub l1_low_watermark: f64,
    /// L2 の root（`None` なら L2 を使わない）。
    pub l2_dir: Option<PathBuf>,
    pub l2_max_bytes: u64,
    /// flusher の帯域（byte / 秒。0 = 無制限）。
    pub flush_bytes_per_sec: u64,
    /// token bucket に貯められる上限（byte）。
    pub flush_burst_bytes: u64,
    /// 待ち行列の上限（byte）。超えたら古いものから「L2 に書かない」で落とす。
    pub flush_queue_max_bytes: u64,
    /// GET の L2 の読み込みを待つ上限。
    pub l2_get_timeout: Duration,
    /// L2 の I/O スレッドの数と待ち行列の長さ。
    pub l2_threads: usize,
    pub l2_queue: usize,
    /// 連続失敗で切り離す回数とバックオフ。
    pub l2_fail_threshold: u32,
    pub l2_backoff_base: Duration,
    pub l2_backoff_max: Duration,
    /// L2 の GC の間隔と、起動後の最初の走査までの待ち（0 = 背景の GC をしない）。
    pub l2_gc_interval: Duration,
    pub l2_gc_initial_delay: Duration,
    /// GC の目標（`l2_max_bytes × l2_gc_target`）。
    pub l2_gc_target: f64,
    /// L2 hit で mtime を touch する最短の間隔（NFS への書き込みを増やさない。既定 1 日）。
    pub l2_touch_interval: Duration,
    /// zstd の level。
    pub zstd_level: i32,
}

impl StoreConfig {
    /// 既定値（ADR-0075 D5: flush 25 MB/s、L2 GET 500 ms、L2 300 GB、L1 40 GB）。
    pub fn new(l1_dir: impl Into<PathBuf>, l2_dir: Option<PathBuf>) -> Self {
        Self {
            l1_dir: l1_dir.into(),
            l1_max_bytes: 40 * 1024 * 1024 * 1024,
            l1_high_watermark: 0.90,
            l1_low_watermark: 0.70,
            l2_dir,
            l2_max_bytes: 300 * 1024 * 1024 * 1024,
            flush_bytes_per_sec: 25 * MB,
            flush_burst_bytes: 25 * MB,
            flush_queue_max_bytes: 4096 * MB,
            l2_get_timeout: Duration::from_millis(500),
            l2_threads: 4,
            l2_queue: 16,
            l2_fail_threshold: 3,
            l2_backoff_base: Duration::from_secs(5),
            l2_backoff_max: Duration::from_secs(300),
            l2_gc_interval: Duration::from_secs(86_400),
            l2_gc_initial_delay: Duration::from_secs(60),
            l2_gc_target: 0.90,
            l2_touch_interval: Duration::from_secs(86_400),
            zstd_level: 3,
        }
    }
}

/// GET の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    L1(Vec<u8>),
    /// L2 hit（L1 へ promote 済み）。
    L2(Vec<u8>),
    Miss,
}

impl Lookup {
    pub fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Lookup::L1(b) | Lookup::L2(b) => Some(b),
            Lookup::Miss => None,
        }
    }
}

/// `flush_once` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlushOutcome {
    /// 待ち行列が空（または L2 を使わない）。
    Empty,
    /// L2 を切り離し中（バックオフが明けるまで待つ）。
    Detached,
    Written {
        key: String,
        bytes: u64,
    },
    /// L2 に既にあった。
    SkippedExisting {
        key: String,
    },
    /// L1 から消えていた（書けない）。
    Missing {
        key: String,
    },
    /// L2 への書き込みに失敗（待ち行列の先頭に戻した）。
    Failed {
        key: String,
        error: String,
    },
}

/// L2 の GC の結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GcReport {
    pub scanned: u64,
    pub total_bytes: u64,
    pub removed: u64,
    pub removed_bytes: u64,
}

#[derive(Debug, Clone)]
struct L1Entry {
    size: u64,
    last_use: SystemTime,
    /// ファイルの mtime（L1 の LRU を再起動後も保つため hit で touch する。1 時間に 1 回まで）。
    disk_mtime: SystemTime,
    /// L2 に書いた（または書かないと決めた）。`false` の entry は LRU で落とさない。
    flushed: bool,
}

#[derive(Default)]
struct L1Index {
    map: HashMap<String, L1Entry>,
    bytes: u64,
}

impl L1Index {
    fn remove(&mut self, k: &str) -> Option<L1Entry> {
        let e = self.map.remove(k)?;
        self.bytes = self.bytes.saturating_sub(e.size);
        Some(e)
    }
    fn insert(&mut self, k: String, e: L1Entry) {
        self.bytes = self.bytes.saturating_add(e.size);
        if let Some(old) = self.map.insert(k, e) {
            self.bytes = self.bytes.saturating_sub(old.size);
        }
    }
}

#[derive(Debug, Clone)]
struct Pending {
    key: String,
    size: u64,
    enqueued: Duration,
}

#[derive(Default)]
struct FlushQueue {
    items: VecDeque<Pending>,
    keys: HashSet<String>,
    bytes: u64,
    /// `.pending` の行数（圧縮の判断）。
    log_lines: u64,
}

#[derive(Default)]
struct Counters {
    gets: AtomicU64,
    puts: AtomicU64,
    l1_hits: AtomicU64,
    l2_hits: AtomicU64,
    misses: AtomicU64,
    promotes: AtomicU64,
    l1_evicted: AtomicU64,
    l2_errors: AtomicU64,
    l2_timeouts: AtomicU64,
    l2_corrupt: AtomicU64,
    flush_written: AtomicU64,
    flush_written_bytes: AtomicU64,
    flush_skipped: AtomicU64,
    flush_dropped: AtomicU64,
    gc_removed: AtomicU64,
    gc_removed_bytes: AtomicU64,
}

#[derive(Default)]
struct L2Info {
    bytes: Option<u64>,
    entries: Option<u64>,
    scanned_at: Option<SystemTime>,
    gc_last_at: Option<SystemTime>,
}

struct Inner {
    cfg: StoreConfig,
    clock: Arc<dyn Clock>,
    l2: Option<Arc<dyn L2Backend>>,
    exec: L2Exec,
    l1: Mutex<L1Index>,
    queue: Mutex<FlushQueue>,
    queue_cv: Condvar,
    bucket: Mutex<TokenBucket>,
    breaker: Mutex<Breaker>,
    counters: Counters,
    l2_info: Mutex<L2Info>,
    last_flush_at: Mutex<Option<SystemTime>>,
    /// `.sccache_check`（メモリだけ）。
    check: Mutex<Option<Vec<u8>>>,
    stop: AtomicBool,
    started_at: SystemTime,
    seq: AtomicU64,
}

/// L1 / L2 の階層 cache。`Clone` は同じ store を指す。
#[derive(Clone)]
pub struct TieredStore {
    inner: Arc<Inner>,
    threads: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// PUT されたバイト列を zstd（content checksum 付き）で包む。
pub fn compress(data: &[u8], level: i32) -> io::Result<Vec<u8>> {
    let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(data.len() / 2 + 64), level)?;
    enc.include_checksum(true)?;
    enc.write_all(data)?;
    enc.finish()
}

/// `.zst` を開く（checksum の不一致・切れたファイルは Err）。
pub fn decompress(z: &[u8]) -> io::Result<Vec<u8>> {
    zstd::stream::decode_all(z)
}

impl TieredStore {
    /// `cfg.l2_dir` の `DirL2` と本物の時計で開き、flusher と L2 の GC のスレッドを起こす。
    pub fn open(cfg: StoreConfig) -> io::Result<Self> {
        let l2: Option<Arc<dyn L2Backend>> = match &cfg.l2_dir {
            Some(dir) => {
                // 初回だけ root を作る（以後の書き込みは root を作り直さない。`DirL2::require_root`）。
                if let Err(e) = fs::create_dir_all(dir) {
                    tracing::warn!(dir = %dir.display(), error = %e, "scratch-cache: could not create the L2 root; L2 will be degraded");
                }
                Some(Arc::new(DirL2::new(dir.clone())))
            }
            None => None,
        };
        let store = Self::open_with(cfg, l2, Arc::new(SystemClock::new()))?;
        store.start_background();
        Ok(store)
    }

    /// 任意の L2 と時計で開く（背景のスレッドは起こさない。テストは `flush_once` / `gc_l2` を直接呼ぶ）。
    pub fn open_with(
        cfg: StoreConfig,
        l2: Option<Arc<dyn L2Backend>>,
        clock: Arc<dyn Clock>,
    ) -> io::Result<Self> {
        fs::create_dir_all(&cfg.l1_dir)?;
        let now = clock.now();
        let inner = Inner {
            exec: L2Exec::new(cfg.l2_threads, cfg.l2_queue),
            bucket: Mutex::new(TokenBucket::new(
                cfg.flush_bytes_per_sec,
                cfg.flush_burst_bytes,
                now,
            )),
            breaker: Mutex::new(Breaker::new(
                cfg.l2_fail_threshold,
                cfg.l2_backoff_base,
                cfg.l2_backoff_max,
            )),
            started_at: clock.wall(),
            cfg,
            clock,
            l2,
            l1: Mutex::new(L1Index::default()),
            queue: Mutex::new(FlushQueue::default()),
            queue_cv: Condvar::new(),
            counters: Counters::default(),
            l2_info: Mutex::new(L2Info::default()),
            last_flush_at: Mutex::new(None),
            check: Mutex::new(None),
            stop: AtomicBool::new(false),
            seq: AtomicU64::new(0),
        };
        let store = Self {
            inner: Arc::new(inner),
            threads: Arc::new(Mutex::new(Vec::new())),
        };
        store.load()?;
        Ok(store)
    }

    fn cfg(&self) -> &StoreConfig {
        &self.inner.cfg
    }

    fn l1_path(&self, k: &str) -> PathBuf {
        self.cfg().l1_dir.join(key::shard(k)).join(k)
    }

    fn pending_path(&self) -> PathBuf {
        self.cfg().l1_dir.join(PENDING_LOG)
    }

    /// 起動時: L1 を 1 回走査して索引を作り、`.pending` の key を積み直す。
    fn load(&self) -> io::Result<()> {
        let dir = self.cfg().l1_dir.clone();
        let mut idx = L1Index::default();
        for shard in fs::read_dir(&dir)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for ent in fs::read_dir(shard.path())? {
                let ent = ent?;
                let name = ent.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with('.') {
                    // 書きかけの tmp（前回のプロセスの残り）。
                    let _ = fs::remove_file(ent.path());
                    continue;
                }
                if !key::valid_key(name) {
                    continue;
                }
                let Ok(meta) = ent.metadata() else { continue };
                if !meta.is_file() {
                    continue;
                }
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                idx.insert(
                    name.to_string(),
                    L1Entry {
                        size: meta.len(),
                        last_use: mtime,
                        disk_mtime: mtime,
                        flushed: true,
                    },
                );
            }
        }
        let mut q = FlushQueue::default();
        if self.inner.l2.is_some() {
            let now = self.inner.clock.now();
            // `<key>` = 積んだ、`-<key>` = 済んだ。最後の記録が「積んだ」の key だけを、最初に積んだ順に戻す。
            let mut order: Vec<String> = Vec::new();
            let mut live: HashSet<String> = HashSet::new();
            if let Ok(f) = File::open(self.pending_path()) {
                for line in io::BufReader::new(f).lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if let Some(done) = line.strip_prefix('-') {
                        live.remove(done);
                    } else if key::valid_key(line) && live.insert(line.to_string()) {
                        order.push(line.to_string());
                    }
                }
            }
            for k in order {
                if !live.remove(&k) {
                    continue;
                }
                if let Some(e) = idx.map.get_mut(&k) {
                    e.flushed = false;
                    q.bytes += e.size;
                    q.keys.insert(k.clone());
                    q.items.push_back(Pending {
                        key: k,
                        size: e.size,
                        enqueued: now,
                    });
                }
            }
            if !q.items.is_empty() {
                tracing::info!(
                    entries = q.items.len(),
                    bytes = q.bytes,
                    "scratch-cache: re-queued unflushed L1 entries from the pending log"
                );
            }
        }
        *lock(&self.inner.l1) = idx;
        *lock(&self.inner.queue) = q;
        {
            let mut q = lock(&self.inner.queue);
            self.rewrite_pending(&mut q)?;
        }
        self.evict_if_needed();
        Ok(())
    }

    /// `.pending` を待ち行列の中身で書き直す（呼び出し側が queue の lock を持つ）。
    fn rewrite_pending(&self, q: &mut FlushQueue) -> io::Result<()> {
        let path = self.pending_path();
        if q.items.is_empty() {
            if q.log_lines > 0 || path.exists() {
                File::create(&path)?;
            }
            q.log_lines = 0;
            return Ok(());
        }
        let tmp = self.cfg().l1_dir.join(format!("{PENDING_LOG}.tmp"));
        {
            let mut f = io::BufWriter::new(File::create(&tmp)?);
            for p in &q.items {
                writeln!(f, "{}", p.key)?;
            }
            f.flush()?;
        }
        fs::rename(&tmp, &path)?;
        q.log_lines = q.items.len() as u64;
        Ok(())
    }

    /// 背景のスレッド（flusher 1 本、L2 の GC 1 本）を起こす。
    pub fn start_background(&self) {
        let mut threads = lock(&self.threads);
        if self.inner.l2.is_none() {
            return;
        }
        let me = self.clone();
        if let Ok(h) = std::thread::Builder::new()
            .name("scratch-cache-flusher".into())
            .spawn(move || me.flusher_loop())
        {
            threads.push(h);
        }
        if !self.cfg().l2_gc_interval.is_zero() {
            let me = self.clone();
            if let Ok(h) = std::thread::Builder::new()
                .name("scratch-cache-l2-gc".into())
                .spawn(move || me.gc_loop())
            {
                threads.push(h);
            }
        }
    }

    /// 背景のスレッドを止める（未 flush の key は `.pending` に残り、次の起動で積み直す）。
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.queue_cv.notify_all();
        let handles: Vec<_> = lock(&self.threads).drain(..).collect();
        for h in handles {
            let _ = h.join();
        }
        self.inner.exec.close();
    }

    fn stopped(&self) -> bool {
        self.inner.stop.load(Ordering::SeqCst)
    }

    // -----------------------------------------------------------------------
    // GET / HEAD / PUT
    // -----------------------------------------------------------------------

    /// L1 → L2 → miss。L2 hit は L1 へ promote する。L2 が遅い・読めないときは miss（L1 だけで応答し続ける）。
    pub fn get(&self, k: &str) -> io::Result<Lookup> {
        if !key::valid_key(k) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid key"));
        }
        let c = &self.inner.counters;
        c.gets.fetch_add(1, Ordering::Relaxed);
        if let Some(bytes) = self.l1_read(k) {
            c.l1_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Lookup::L1(bytes));
        }
        if let Some(bytes) = self.l2_read(k) {
            c.l2_hits.fetch_add(1, Ordering::Relaxed);
            match self.l1_write(k, &bytes, true) {
                Ok(()) => {
                    c.promotes.fetch_add(1, Ordering::Relaxed);
                    self.evict_if_needed();
                }
                Err(e) => {
                    tracing::warn!(key = k, error = %e, "scratch-cache: could not promote an L2 hit to L1")
                }
            }
            return Ok(Lookup::L2(bytes));
        }
        c.misses.fetch_add(1, Ordering::Relaxed);
        Ok(Lookup::Miss)
    }

    /// 在るか（L1 の索引、無ければ L2 の stat。stats は数えない）。
    pub fn contains(&self, k: &str) -> bool {
        if !key::valid_key(k) {
            return false;
        }
        if lock(&self.inner.l1).map.contains_key(k) {
            return true;
        }
        let Some(l2) = self.inner.l2.clone() else {
            return false;
        };
        if !lock(&self.inner.breaker).allow(self.inner.clock.now()) {
            return false;
        }
        let owned = k.to_string();
        matches!(
            self.inner
                .exec
                .run(self.cfg().l2_get_timeout, move || l2.exists(&owned)),
            Ok(Ok(true))
        )
    }

    /// L1 に書いてすぐ返す。L2 へは flusher が書く。
    pub fn put(&self, k: &str, data: &[u8]) -> io::Result<()> {
        if !key::valid_key(k) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid key"));
        }
        self.inner.counters.puts.fetch_add(1, Ordering::Relaxed);
        let already = {
            let idx = lock(&self.inner.l1);
            idx.map.get(k).map(|e| e.size == data.len() as u64)
        };
        if already == Some(true) && self.l1_path(k).is_file() {
            // content-addressed なので同じ key は同じ中身。書き直さない（待ち行列にあればそのまま）。
            return Ok(());
        }
        let l2 = self.inner.l2.is_some();
        self.l1_write(k, data, !l2)?;
        if l2 {
            self.enqueue(k, data.len() as u64);
        }
        self.evict_if_needed();
        Ok(())
    }

    /// `.sccache_check`（sccache の storage check。L2 に流さない）。
    pub fn check_object(&self) -> Option<Vec<u8>> {
        lock(&self.inner.check).clone()
    }

    pub fn set_check_object(&self, data: Vec<u8>) {
        *lock(&self.inner.check) = Some(data);
    }

    fn l1_read(&self, k: &str) -> Option<Vec<u8>> {
        if !lock(&self.inner.l1).map.contains_key(k) {
            return None;
        }
        let path = self.l1_path(k);
        match fs::read(&path) {
            Ok(bytes) => {
                let now = self.inner.clock.wall();
                let mut touch = false;
                if let Some(e) = lock(&self.inner.l1).map.get_mut(k) {
                    e.last_use = now;
                    if now.duration_since(e.disk_mtime).unwrap_or_default()
                        > Duration::from_secs(3600)
                    {
                        e.disk_mtime = now;
                        touch = true;
                    }
                }
                if touch && let Ok(f) = File::options().write(true).open(&path) {
                    let _ = f.set_modified(now);
                }
                Some(bytes)
            }
            Err(_) => {
                // 索引にあってファイルが無い（手で消された）。
                lock(&self.inner.l1).remove(k);
                None
            }
        }
    }

    fn l1_write(&self, k: &str, data: &[u8], flushed: bool) -> io::Result<()> {
        let path = self.l1_path(k);
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("L1 path has no parent"))?;
        fs::create_dir_all(dir)?;
        let seq = self.inner.seq.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".tmp-{k}-{}-{seq}", std::process::id()));
        if let Err(e) = fs::write(&tmp, data).and_then(|()| fs::rename(&tmp, &path)) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let now = self.inner.clock.wall();
        let mut idx = lock(&self.inner.l1);
        let flushed = flushed || idx.map.get(k).is_some_and(|e| e.flushed);
        idx.insert(
            k.to_string(),
            L1Entry {
                size: data.len() as u64,
                last_use: now,
                disk_mtime: fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .unwrap_or(now),
                flushed,
            },
        );
        Ok(())
    }

    fn l2_read(&self, k: &str) -> Option<Vec<u8>> {
        let l2 = self.inner.l2.clone()?;
        let c = &self.inner.counters;
        let now = self.inner.clock.now();
        if !lock(&self.inner.breaker).allow(now) {
            return None;
        }
        let owned = k.to_string();
        let reader = l2.clone();
        let res = self
            .inner
            .exec
            .run(self.cfg().l2_get_timeout, move || reader.read(&owned));
        let (zst, mtime) = match res {
            Ok(Ok(Some(v))) => {
                lock(&self.inner.breaker).on_success();
                v
            }
            Ok(Ok(None)) => {
                lock(&self.inner.breaker).on_success();
                return None;
            }
            Ok(Err(e)) => {
                c.l2_errors.fetch_add(1, Ordering::Relaxed);
                self.l2_failure(format!("L2 read {k}: {e}"));
                return None;
            }
            Err(ExecError::Timeout) => {
                c.l2_timeouts.fetch_add(1, Ordering::Relaxed);
                self.l2_failure(format!(
                    "L2 read {k} timed out after {} ms",
                    self.cfg().l2_get_timeout.as_millis()
                ));
                return None;
            }
            Err(ExecError::Busy) => {
                c.l2_timeouts.fetch_add(1, Ordering::Relaxed);
                self.l2_failure("L2 I/O threads are all busy".to_string());
                return None;
            }
        };
        match decompress(&zst) {
            Ok(bytes) => {
                let wall = self.inner.clock.wall();
                if wall.duration_since(mtime).unwrap_or_default() >= self.cfg().l2_touch_interval {
                    let owned = k.to_string();
                    let toucher = l2.clone();
                    self.inner.exec.spawn(move || {
                        let _ = toucher.touch(&owned, wall);
                    });
                }
                Some(bytes)
            }
            Err(e) => {
                // 切れた・壊れた object。miss 扱いにして消す（L1 へ promote しない）。
                c.l2_corrupt.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(key = k, error = %e, "scratch-cache: corrupt L2 object discarded");
                let owned = k.to_string();
                self.inner.exec.spawn(move || {
                    let _ = l2.remove(&owned);
                });
                None
            }
        }
    }

    fn l2_failure(&self, err: String) {
        let now = self.inner.clock.now();
        let wall = self.inner.clock.wall();
        lock(&self.inner.breaker).on_failure(now, wall, err);
    }

    // -----------------------------------------------------------------------
    // L1 の LRU
    // -----------------------------------------------------------------------

    /// `l1_max × high` を超えたら `l1_max × low` まで LRU で落とす。**未 flush の entry は落とさない**。
    pub fn evict_if_needed(&self) -> u64 {
        let cfg = self.cfg();
        let high = (cfg.l1_max_bytes as f64 * cfg.l1_high_watermark) as u64;
        let low = (cfg.l1_max_bytes as f64 * cfg.l1_low_watermark) as u64;
        let mut idx = lock(&self.inner.l1);
        if idx.bytes <= high {
            return 0;
        }
        let mut victims: Vec<(SystemTime, String)> = idx
            .map
            .iter()
            .filter(|(_, e)| e.flushed)
            .map(|(k, e)| (e.last_use, k.clone()))
            .collect();
        victims.sort();
        let mut n = 0;
        for (_, k) in victims {
            if idx.bytes <= low {
                break;
            }
            idx.remove(&k);
            let _ = fs::remove_file(self.l1_path(&k));
            n += 1;
        }
        drop(idx);
        self.inner
            .counters
            .l1_evicted
            .fetch_add(n, Ordering::Relaxed);
        n
    }

    // -----------------------------------------------------------------------
    // flusher
    // -----------------------------------------------------------------------

    fn enqueue(&self, k: &str, size: u64) {
        let mut q = lock(&self.inner.queue);
        if q.keys.contains(k) {
            return;
        }
        // 追記ログ（再起動後に積み直す）。書けなくても待ち行列には積む。
        if self.append_pending(k) {
            q.log_lines += 1;
        }
        q.keys.insert(k.to_string());
        q.bytes += size;
        q.items.push_back(Pending {
            key: k.to_string(),
            size,
            enqueued: self.inner.clock.now(),
        });
        // 上限を超えたら古いものから「L2 に書かない」で落とす（L1 の LRU の対象に戻す）。
        let mut dropped = Vec::new();
        while q.bytes > self.cfg().flush_queue_max_bytes && q.items.len() > 1 {
            let Some(p) = q.items.pop_front() else { break };
            q.bytes = q.bytes.saturating_sub(p.size);
            q.keys.remove(&p.key);
            dropped.push(p.key);
        }
        if q.log_lines > 4 * q.items.len() as u64 + 1024 {
            let _ = self.rewrite_pending(&mut q);
        }
        drop(q);
        if !dropped.is_empty() {
            tracing::warn!(
                dropped = dropped.len(),
                "scratch-cache: flush queue overflow; oldest entries will not be written to L2"
            );
            self.inner
                .counters
                .flush_dropped
                .fetch_add(dropped.len() as u64, Ordering::Relaxed);
            self.mark_flushed(&dropped);
        }
        self.inner.queue_cv.notify_one();
    }

    /// `.pending` に 1 行足す（`<key>` = 積んだ、`-<key>` = 済んだ）。呼び出し側が queue の lock を持つ。
    fn append_pending(&self, line: &str) -> bool {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.pending_path())
            .and_then(|mut f| writeln!(f, "{line}"))
            .is_ok()
    }

    fn mark_flushed(&self, keys: &[String]) {
        let mut idx = lock(&self.inner.l1);
        for k in keys {
            if let Some(e) = idx.map.get_mut(k) {
                e.flushed = true;
            }
        }
    }

    /// 待ち行列の先頭を 1 つ L2 へ書く（token bucket の不足分は `Clock::sleep` で待つ）。
    pub fn flush_once(&self) -> FlushOutcome {
        let Some(l2) = self.inner.l2.clone() else {
            return FlushOutcome::Empty;
        };
        if !lock(&self.inner.breaker).allow(self.inner.clock.now()) {
            return FlushOutcome::Detached;
        }
        let Some(p) = lock(&self.inner.queue).items.front().cloned() else {
            return FlushOutcome::Empty;
        };
        let finish = |me: &Self, outcome: FlushOutcome| -> FlushOutcome {
            let mut q = lock(&me.inner.queue);
            if q.items.front().is_some_and(|f| f.key == p.key) {
                q.items.pop_front();
            } else if let Some(i) = q.items.iter().position(|f| f.key == p.key) {
                q.items.remove(i);
            }
            q.keys.remove(&p.key);
            q.bytes = q.bytes.saturating_sub(p.size);
            if q.items.is_empty() {
                if q.log_lines > 0 {
                    let _ = me.rewrite_pending(&mut q);
                }
            } else if me.append_pending(&format!("-{}", p.key)) {
                // 済んだ key の取り消し（再起動後に積み直さない）。
                q.log_lines += 1;
            }
            drop(q);
            me.mark_flushed(std::slice::from_ref(&p.key));
            outcome
        };
        let data = match fs::read(self.l1_path(&p.key)) {
            Ok(d) => d,
            Err(_) => {
                lock(&self.inner.l1).remove(&p.key);
                return finish(self, FlushOutcome::Missing { key: p.key.clone() });
            }
        };
        match l2.exists(&p.key) {
            Ok(true) => {
                self.inner
                    .counters
                    .flush_skipped
                    .fetch_add(1, Ordering::Relaxed);
                lock(&self.inner.breaker).on_success();
                return finish(self, FlushOutcome::SkippedExisting { key: p.key.clone() });
            }
            Ok(false) => {}
            Err(e) => return self.flush_failed(&p, format!("L2 stat {}: {e}", p.key)),
        }
        let zst = match compress(&data, self.cfg().zstd_level) {
            Ok(z) => z,
            Err(e) => return self.flush_failed(&p, format!("zstd {}: {e}", p.key)),
        };
        let wait = lock(&self.inner.bucket).take(zst.len() as u64, self.inner.clock.now());
        self.inner.clock.sleep(wait);
        match l2.write(&p.key, &zst) {
            Ok(written) => {
                lock(&self.inner.breaker).on_success();
                let c = &self.inner.counters;
                if written {
                    c.flush_written.fetch_add(1, Ordering::Relaxed);
                    c.flush_written_bytes
                        .fetch_add(zst.len() as u64, Ordering::Relaxed);
                    let mut info = lock(&self.inner.l2_info);
                    if let Some(b) = info.bytes.as_mut() {
                        *b += zst.len() as u64;
                    }
                    if let Some(n) = info.entries.as_mut() {
                        *n += 1;
                    }
                } else {
                    c.flush_skipped.fetch_add(1, Ordering::Relaxed);
                }
                *lock(&self.inner.last_flush_at) = Some(self.inner.clock.wall());
                finish(
                    self,
                    FlushOutcome::Written {
                        key: p.key.clone(),
                        bytes: zst.len() as u64,
                    },
                )
            }
            Err(e) => self.flush_failed(&p, format!("L2 write {}: {e}", p.key)),
        }
    }

    fn flush_failed(&self, p: &Pending, error: String) -> FlushOutcome {
        self.inner
            .counters
            .l2_errors
            .fetch_add(1, Ordering::Relaxed);
        self.l2_failure(error.clone());
        FlushOutcome::Failed {
            key: p.key.clone(),
            error,
        }
    }

    /// 待ち行列が空になるか、失敗・切り離しで止まるまで書く（テストと shutdown 前の手動 flush）。
    pub fn flush_all(&self) -> Vec<FlushOutcome> {
        let mut out = Vec::new();
        loop {
            let o = self.flush_once();
            match o {
                FlushOutcome::Empty | FlushOutcome::Detached | FlushOutcome::Failed { .. } => {
                    out.push(o);
                    return out;
                }
                _ => out.push(o),
            }
        }
    }

    fn flusher_loop(&self) {
        while !self.stopped() {
            match self.flush_once() {
                FlushOutcome::Written { .. }
                | FlushOutcome::SkippedExisting { .. }
                | FlushOutcome::Missing { .. } => {}
                FlushOutcome::Empty => {
                    let q = lock(&self.inner.queue);
                    if q.items.is_empty() && !self.stopped() {
                        let _ = self.inner.queue_cv.wait_timeout(q, Duration::from_secs(1));
                    }
                }
                FlushOutcome::Detached | FlushOutcome::Failed { .. } => {
                    let q = lock(&self.inner.queue);
                    if !self.stopped() {
                        let _ = self
                            .inner
                            .queue_cv
                            .wait_timeout(q, Duration::from_millis(500));
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // L2 の GC
    // -----------------------------------------------------------------------

    /// L2 を走査し、`l2_max_bytes` を超えていれば mtime の古い順に `l2_max_bytes × l2_gc_target` まで消す。
    pub fn gc_l2(&self) -> io::Result<GcReport> {
        let Some(l2) = self.inner.l2.clone() else {
            return Ok(GcReport::default());
        };
        let mut objs = match l2.scan() {
            Ok(o) => o,
            Err(e) => {
                self.l2_failure(format!("L2 scan: {e}"));
                return Err(e);
            }
        };
        let mut report = GcReport {
            scanned: objs.len() as u64,
            total_bytes: objs.iter().map(|o| o.size).sum(),
            ..GcReport::default()
        };
        let max = self.cfg().l2_max_bytes;
        let mut total = report.total_bytes;
        let mut entries = report.scanned;
        if total > max {
            let target = (max as f64 * self.cfg().l2_gc_target) as u64;
            objs.sort_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.key.cmp(&b.key)));
            for o in &objs {
                if total <= target || self.stopped() {
                    break;
                }
                // immutable なので、消している最中の別プロセスの読み込みは開いた fd で完結する。
                if l2.remove(&o.key).is_ok() {
                    total = total.saturating_sub(o.size);
                    entries = entries.saturating_sub(1);
                    report.removed += 1;
                    report.removed_bytes += o.size;
                }
            }
            tracing::info!(
                removed = report.removed,
                removed_bytes = report.removed_bytes,
                total_bytes = total,
                max_bytes = max,
                "scratch-cache: L2 GC evicted the least recently used objects"
            );
        }
        let c = &self.inner.counters;
        c.gc_removed.fetch_add(report.removed, Ordering::Relaxed);
        c.gc_removed_bytes
            .fetch_add(report.removed_bytes, Ordering::Relaxed);
        let now = self.inner.clock.wall();
        let mut info = lock(&self.inner.l2_info);
        info.bytes = Some(total);
        info.entries = Some(entries);
        info.scanned_at = Some(now);
        info.gc_last_at = Some(now);
        Ok(report)
    }

    fn gc_loop(&self) {
        let mut wait = self.cfg().l2_gc_initial_delay;
        loop {
            let deadline = std::time::Instant::now() + wait;
            while std::time::Instant::now() < deadline {
                if self.stopped() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200).min(wait));
            }
            if self.stopped() {
                return;
            }
            if lock(&self.inner.breaker).allow(self.inner.clock.now())
                && let Err(e) = self.gc_l2()
            {
                tracing::warn!(error = %e, "scratch-cache: L2 GC failed");
            }
            wait = self.cfg().l2_gc_interval;
        }
    }

    // -----------------------------------------------------------------------
    // stats
    // -----------------------------------------------------------------------

    pub fn stats(&self) -> ScratchCacheStats {
        let c = &self.inner.counters;
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let cfg = self.cfg();
        let (l1_bytes, l1_entries) = {
            let idx = lock(&self.inner.l1);
            (idx.bytes, idx.map.len() as u64)
        };
        let now = self.inner.clock.now();
        let wall = self.inner.clock.wall();
        let (queue_len, queue_bytes, oldest) = {
            let q = lock(&self.inner.queue);
            (
                q.items.len() as u64,
                q.bytes,
                q.items
                    .front()
                    .map(|p| now.saturating_sub(p.enqueued).as_secs()),
            )
        };
        let (degraded_since, retry_at, last_error) = {
            let b = lock(&self.inner.breaker);
            let retry = b.retry_at().filter(|t| *t > now).map(|t| wall + (t - now));
            (
                b.degraded_since(),
                retry,
                b.last_error().map(str::to_string),
            )
        };
        let info = lock(&self.inner.l2_info);
        let l2_state = if self.inner.l2.is_none() {
            "disabled"
        } else if degraded_since.is_some() {
            "degraded"
        } else {
            "ok"
        };
        ScratchCacheStats {
            schema: SCRATCH_CACHE_STATS_SCHEMA.to_string(),
            started_at: rfc3339(self.inner.started_at),
            observed_at: rfc3339(wall),
            gets: get(&c.gets),
            puts: get(&c.puts),
            l1_hits: get(&c.l1_hits),
            l2_hits: get(&c.l2_hits),
            misses: get(&c.misses),
            promotes: get(&c.promotes),
            l1_dir: cfg.l1_dir.display().to_string(),
            l1_bytes,
            l1_entries,
            l1_max_bytes: cfg.l1_max_bytes,
            l1_evicted: get(&c.l1_evicted),
            l2_enabled: self.inner.l2.is_some(),
            l2_dir: self.inner.l2.as_ref().map(|l| l.location()),
            l2_state: l2_state.to_string(),
            l2_bytes: info.bytes,
            l2_entries: info.entries,
            l2_scanned_at: info.scanned_at.map(rfc3339),
            l2_max_bytes: cfg.l2_max_bytes,
            l2_errors: get(&c.l2_errors),
            l2_timeouts: get(&c.l2_timeouts),
            l2_corrupt: get(&c.l2_corrupt),
            l2_degraded_since: degraded_since.map(rfc3339),
            l2_retry_at: retry_at.map(rfc3339),
            l2_last_error: last_error,
            l2_gc_last_at: info.gc_last_at.map(rfc3339),
            l2_gc_removed: get(&c.gc_removed),
            l2_gc_removed_bytes: get(&c.gc_removed_bytes),
            flush_queue_len: queue_len,
            flush_queue_bytes: queue_bytes,
            flush_oldest_age_secs: oldest,
            flush_last_at: lock(&self.inner.last_flush_at).map(rfc3339),
            flush_written: get(&c.flush_written),
            flush_written_bytes: get(&c.flush_written_bytes),
            flush_skipped_existing: get(&c.flush_skipped),
            flush_dropped: get(&c.flush_dropped),
            flush_mbps: cfg.flush_bytes_per_sec / MB,
        }
    }

    /// L1 の dir（テストと表示）。
    pub fn l1_dir(&self) -> &Path {
        &self.cfg().l1_dir
    }

    /// 待ち行列の長さ。
    pub fn queue_len(&self) -> usize {
        lock(&self.inner.queue).items.len()
    }
}
