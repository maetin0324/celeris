//! ADR-0075 §5 G3 の受け入れ条件 2〜4 の単体テスト（`scratch_cache::tests::*`）。

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::bucket::TokenBucket;
use crate::clock::Clock;
use crate::l2::{DirL2, L2Backend, L2Object};
use crate::store::{self, FlushOutcome, Lookup, MB, StoreConfig, TieredStore};

/// 仮想の時計（`sleep` は時刻を進めるだけ）。
struct FakeClock {
    t: Mutex<Duration>,
    base: SystemTime,
}

impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            t: Mutex::new(Duration::ZERO),
            base: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000),
        })
    }
    fn advance(&self, d: Duration) {
        *self.t.lock().unwrap() += d;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        *self.t.lock().unwrap()
    }
    fn wall(&self) -> SystemTime {
        self.base + self.now()
    }
    fn sleep(&self, d: Duration) {
        self.advance(d);
    }
}

/// メモリの L2（遅延・失敗を差し込める）。
#[derive(Default)]
struct MemL2 {
    objs: Mutex<HashMap<String, (Vec<u8>, SystemTime)>>,
    read_delay: Mutex<Duration>,
    write_delay: Mutex<Duration>,
    fail: AtomicBool,
}

impl MemL2 {
    fn check(&self) -> io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            Err(io::Error::other("injected L2 failure"))
        } else {
            Ok(())
        }
    }
    fn has(&self, k: &str) -> bool {
        self.objs.lock().unwrap().contains_key(k)
    }
}

impl L2Backend for MemL2 {
    fn read(&self, k: &str) -> io::Result<Option<(Vec<u8>, SystemTime)>> {
        std::thread::sleep(*self.read_delay.lock().unwrap());
        self.check()?;
        Ok(self.objs.lock().unwrap().get(k).cloned())
    }
    fn exists(&self, k: &str) -> io::Result<bool> {
        self.check()?;
        Ok(self.has(k))
    }
    fn write(&self, k: &str, data: &[u8]) -> io::Result<bool> {
        std::thread::sleep(*self.write_delay.lock().unwrap());
        self.check()?;
        let mut m = self.objs.lock().unwrap();
        if m.contains_key(k) {
            return Ok(false);
        }
        m.insert(k.to_string(), (data.to_vec(), SystemTime::now()));
        Ok(true)
    }
    fn touch(&self, k: &str, t: SystemTime) -> io::Result<()> {
        if let Some(o) = self.objs.lock().unwrap().get_mut(k) {
            o.1 = t;
        }
        Ok(())
    }
    fn remove(&self, k: &str) -> io::Result<()> {
        self.objs.lock().unwrap().remove(k);
        Ok(())
    }
    fn scan(&self) -> io::Result<Vec<L2Object>> {
        self.check()?;
        Ok(self
            .objs
            .lock()
            .unwrap()
            .iter()
            .map(|(k, (d, t))| L2Object {
                key: k.clone(),
                size: d.len() as u64,
                mtime: *t,
            })
            .collect())
    }
    fn location(&self) -> String {
        "mem".to_string()
    }
}

/// `DirL2` に遅延を足す。
struct SlowDirL2 {
    inner: DirL2,
    write_delay: Duration,
}

impl L2Backend for SlowDirL2 {
    fn read(&self, k: &str) -> io::Result<Option<(Vec<u8>, SystemTime)>> {
        self.inner.read(k)
    }
    fn exists(&self, k: &str) -> io::Result<bool> {
        self.inner.exists(k)
    }
    fn write(&self, k: &str, data: &[u8]) -> io::Result<bool> {
        std::thread::sleep(self.write_delay);
        self.inner.write(k, data)
    }
    fn touch(&self, k: &str, t: SystemTime) -> io::Result<()> {
        self.inner.touch(k, t)
    }
    fn remove(&self, k: &str) -> io::Result<()> {
        self.inner.remove(k)
    }
    fn scan(&self) -> io::Result<Vec<L2Object>> {
        self.inner.scan()
    }
    fn location(&self) -> String {
        self.inner.location()
    }
}

fn key(n: u32) -> String {
    format!("{n:02x}{:062x}", n as u128 * 0x9e37_79b9)
}

/// 圧縮の効かないバイト列（xorshift）。
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn cfg(root: &Path, l2: bool) -> StoreConfig {
    let mut c = StoreConfig::new(root.join("l1"), l2.then(|| root.join("l2")));
    c.l2_gc_interval = Duration::ZERO;
    c
}

fn dir_store(root: &Path, clock: Arc<FakeClock>) -> (TieredStore, Arc<DirL2>) {
    std::fs::create_dir_all(root.join("l2")).unwrap();
    let l2 = Arc::new(DirL2::new(root.join("l2")));
    let s = TieredStore::open_with(cfg(root, true), Some(l2.clone()), clock).unwrap();
    (s, l2)
}

fn put_l2(l2: &dyn L2Backend, k: &str, data: &[u8]) {
    assert!(l2.write(k, &store::compress(data, 3).unwrap()).unwrap());
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 受け入れ条件 2: GET は L1 → L2 → miss の順（同じ key が両方にあれば L1 を返す）。
#[test]
fn get_prefers_l1_then_l2_then_miss() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, l2) = dir_store(tmp.path(), FakeClock::new());
    let (both, only_l2, none) = (key(1), key(2), key(3));
    s.put(&both, b"from-l1").unwrap();
    put_l2(l2.as_ref(), &both, b"from-l2");
    put_l2(l2.as_ref(), &only_l2, b"only-in-l2");

    assert_eq!(s.get(&both).unwrap(), Lookup::L1(b"from-l1".to_vec()));
    assert_eq!(s.get(&only_l2).unwrap(), Lookup::L2(b"only-in-l2".to_vec()));
    assert_eq!(s.get(&none).unwrap(), Lookup::Miss);
    let st = s.stats();
    assert_eq!(
        (st.gets, st.l1_hits, st.l2_hits, st.misses),
        (3, 1, 1, 1),
        "{st:?}"
    );
    assert_eq!(st.l2_state, "ok");
}

/// 受け入れ条件 2: L2 hit は L1 へ promote され、次の GET は L1 から返る。promote した entry は flush しない。
#[test]
fn l2_hit_is_promoted_to_l1() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, l2) = dir_store(tmp.path(), FakeClock::new());
    let k = key(7);
    put_l2(l2.as_ref(), &k, b"object");
    assert_eq!(s.get(&k).unwrap(), Lookup::L2(b"object".to_vec()));
    let l1_file = tmp.path().join("l1").join(&k[..2]).join(&k);
    assert_eq!(std::fs::read(&l1_file).unwrap(), b"object");
    // L2 を消しても L1 から返る。
    l2.remove(&k).unwrap();
    assert_eq!(s.get(&k).unwrap(), Lookup::L1(b"object".to_vec()));
    let st = s.stats();
    assert_eq!((st.l2_hits, st.promotes, st.l1_hits), (1, 1, 1));
    assert_eq!(st.flush_queue_len, 0, "promoted entries are already in L2");
    assert_eq!(st.l1_entries, 1);
}

/// 受け入れ条件 2: PUT は L1 に書いた時点で返り、L2 への書き込みは flusher が後で行う（L2 が 1.5 秒遅くても待たない）。
#[test]
fn put_returns_before_the_l2_write() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("l2")).unwrap();
    let slow = Arc::new(SlowDirL2 {
        inner: DirL2::new(tmp.path().join("l2")),
        write_delay: Duration::from_millis(1500),
    });
    let s = TieredStore::open_with(
        cfg(tmp.path(), true),
        Some(slow.clone()),
        Arc::new(crate::clock::SystemClock::new()),
    )
    .unwrap();
    s.start_background();
    let k = key(9);
    let started = Instant::now();
    s.put(&k, b"compiled output").unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_millis(500), "PUT took {took:?}");
    assert!(!slow.exists(&k).unwrap(), "L2 must not be written yet");
    assert_eq!(s.get(&k).unwrap(), Lookup::L1(b"compiled output".to_vec()));
    wait_until("the flusher to write L2", || slow.exists(&k).unwrap());
    wait_until("the queue to drain", || s.queue_len() == 0);
    let st = s.stats();
    assert_eq!(st.flush_written, 1);
    assert!(st.flush_last_at.is_some());
    s.shutdown();
}

/// token bucket の性質: 返された時間だけ待ってから書けば、区間 T に書く量は burst + rate × T を超えない。
#[test]
fn token_bucket_limits_bytes_over_any_interval() {
    let rate = 25 * MB;
    let burst = 5 * MB;
    let mut b = TokenBucket::new(rate, burst, Duration::ZERO);
    let mut now = Duration::ZERO;
    let mut written = 0u64;
    for i in 0..200u64 {
        let n = (i % 7 + 1) * 700_000;
        now += b.take(n, now);
        written += n;
        let limit = burst as f64 + rate as f64 * now.as_secs_f64();
        assert!(
            written as f64 <= limit + 1.0,
            "wrote {written} by {now:?} (limit {limit})"
        );
    }
    // 無制限（0）は待たない。
    let mut free = TokenBucket::new(0, 0, Duration::ZERO);
    assert_eq!(free.take(u64::MAX / 2, Duration::ZERO), Duration::ZERO);
}

/// 受け入れ条件 2: flusher は `flush_mbps`（25 MB/s）を超えて L2 に書かない（仮想の時計で 100 MB を書く）。
#[test]
fn flusher_respects_the_token_bucket() {
    let tmp = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let l2 = Arc::new(MemL2::default());
    let mut c = cfg(tmp.path(), true);
    c.flush_bytes_per_sec = 25 * MB;
    c.flush_burst_bytes = 5 * MB;
    c.zstd_level = 1;
    let s = TieredStore::open_with(c, Some(l2.clone()), clock.clone()).unwrap();
    let n = 40;
    for i in 0..n {
        s.put(&key(i), &noise(2_500_000, i as u64 + 1)).unwrap();
    }
    let t0 = clock.now();
    let out = s.flush_all();
    let elapsed = (clock.now() - t0).as_secs_f64();
    let written: u64 = out
        .iter()
        .map(|o| match o {
            FlushOutcome::Written { bytes, .. } => *bytes,
            _ => 0,
        })
        .sum();
    assert_eq!(out.len(), n as usize + 1, "{out:?}");
    assert!(written >= 100 * MB, "{written}");
    let mbps = (written - 5 * MB) as f64 / MB as f64 / elapsed;
    assert!(
        mbps <= 25.0 + 0.01,
        "flusher wrote {written} bytes in {elapsed:.2} s ({mbps:.2} MB/s beyond the burst)"
    );
    // 絞りすぎていない（上限の 90 % 以上は出る）。
    assert!(mbps >= 22.5, "{mbps:.2} MB/s");
    let st = s.stats();
    assert_eq!(st.flush_written, n as u64);
    assert_eq!(st.flush_queue_len, 0);
    assert_eq!(st.flush_mbps, 25);
}

/// 受け入れ条件 2: L2 の書き込みは tmp → fsync → rename。同じ key の同時書き込みでも壊れず、tmp を残さず、
/// 既にある object は書き直さない。
#[test]
fn l2_write_is_atomic_and_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("l2");
    std::fs::create_dir_all(&root).unwrap();
    let k = key(3);
    let data = noise(3_000_000, 42);
    let zst = store::compress(&data, 3).unwrap();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (root, k, zst) = (root.clone(), k.clone(), zst.clone());
            std::thread::spawn(move || DirL2::new(root).write(&k, &zst).unwrap())
        })
        .collect();
    let wrote: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(wrote.iter().any(|w| *w), "{wrote:?}");
    let l2 = DirL2::new(&root);
    let (got, mtime) = l2.read(&k).unwrap().unwrap();
    assert_eq!(store::decompress(&got).unwrap(), data);
    let names: Vec<String> = std::fs::read_dir(root.join(&k[..2]))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec![format!("{k}.zst")], "no tmp files are left");
    // immutable: 2 回目は書かない。
    std::thread::sleep(Duration::from_millis(20));
    assert!(!l2.write(&k, b"something else").unwrap());
    let (again, mtime2) = l2.read(&k).unwrap().unwrap();
    assert_eq!(again, got);
    assert_eq!(mtime, mtime2);
    // store 側: 同じ key の PUT を重ねても待ち行列は 1 つ。
    let (s, _) = dir_store(tmp.path(), FakeClock::new());
    let k2 = key(4);
    s.put(&k2, b"x").unwrap();
    s.put(&k2, b"x").unwrap();
    assert_eq!(s.queue_len(), 1);
}

/// 受け入れ条件 3: 壊れた `.zst`（checksum 不一致・途中で切れた）は miss 扱いで消し、L1 へ promote しない。
#[test]
fn corrupt_l2_entry_is_discarded() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, l2) = dir_store(tmp.path(), FakeClock::new());
    let data = noise(200_000, 5);
    let (flipped, truncated) = (key(10), key(11));
    put_l2(l2.as_ref(), &flipped, &data);
    put_l2(l2.as_ref(), &truncated, &data);
    let p = l2.object_path(&flipped);
    let mut bytes = std::fs::read(&p).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&p, &bytes).unwrap();
    let p2 = l2.object_path(&truncated);
    let bytes2 = std::fs::read(&p2).unwrap();
    std::fs::write(&p2, &bytes2[..bytes2.len() - 7]).unwrap();

    assert_eq!(s.get(&flipped).unwrap(), Lookup::Miss);
    assert_eq!(s.get(&truncated).unwrap(), Lookup::Miss);
    let st = s.stats();
    assert_eq!((st.l2_corrupt, st.l2_hits, st.misses), (2, 0, 2), "{st:?}");
    assert_eq!(st.l1_entries, 0, "corrupt objects are not promoted");
    wait_until("the corrupt objects to be removed", || {
        !p.exists() && !p2.exists()
    });
    // 壊れた object は L2 の障害ではない（切り離さない）。
    assert_eq!(st.l2_state, "ok");
}

/// 受け入れ条件 3: L2 が読めない（dir を消す・権限を落とす・遅い）とき、GET は L1 だけで応答し続け、連続失敗で
/// L2 を切り離し（degraded）、バックオフの後に復帰する。
#[test]
fn l2_unavailable_degrades_to_l1_only() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let (s, l2) = dir_store(tmp.path(), clock.clone());
    let (local, remote) = (key(20), key(21));
    s.put(&local, b"local").unwrap();
    assert!(matches!(s.flush_once(), FlushOutcome::Written { .. }));
    put_l2(l2.as_ref(), &remote, b"remote");

    // (1) L2 の dir を消す: miss になり、3 回で切り離す。L1 は応答し続ける。
    let root = tmp.path().join("l2");
    let moved = tmp.path().join("l2-moved");
    std::fs::rename(&root, &moved).unwrap();
    for i in 0..3 {
        assert_eq!(s.get(&key(30 + i)).unwrap(), Lookup::Miss);
    }
    let st = s.stats();
    assert_eq!(st.l2_state, "degraded", "{st:?}");
    assert!(st.l2_errors >= 3);
    assert!(st.l2_degraded_since.is_some() && st.l2_retry_at.is_some());
    assert!(st.l2_last_error.as_deref().unwrap_or("").contains("L2"));
    assert_eq!(s.get(&local).unwrap(), Lookup::L1(b"local".to_vec()));
    // 切り離し中は L2 を見ない（戻しても、バックオフが明けるまでは miss）。PUT は L1 に入り、flusher は待つ。
    std::fs::rename(&moved, &root).unwrap();
    assert_eq!(s.get(&remote).unwrap(), Lookup::Miss);
    let queued = key(40);
    s.put(&queued, b"queued").unwrap();
    assert_eq!(s.flush_once(), FlushOutcome::Detached);
    assert_eq!(s.get(&queued).unwrap(), Lookup::L1(b"queued".to_vec()));

    // バックオフ（5 s）が明けたら 1 回試し、成功すれば復帰する。
    clock.advance(Duration::from_secs(6));
    assert_eq!(s.get(&remote).unwrap(), Lookup::L2(b"remote".to_vec()));
    assert_eq!(s.stats().l2_state, "ok");
    assert!(matches!(s.flush_once(), FlushOutcome::Written { .. }));

    // (2) 権限を落とす（EACCES）: 同じく切り離す。
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let denied = std::fs::read_dir(&root).is_err();
    for i in 0..3 {
        assert_eq!(s.get(&key(50 + i)).unwrap(), Lookup::Miss);
    }
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    if denied {
        // root で走らせたときは権限で落ちないので確かめない。
        assert_eq!(s.stats().l2_state, "degraded");
        clock.advance(Duration::from_secs(20));
        assert_eq!(s.get(&remote).unwrap(), Lookup::L1(b"remote".to_vec()));
        assert_eq!(s.get(&key(60)).unwrap(), Lookup::Miss);
        assert_eq!(s.stats().l2_state, "ok");
    }

    // (3) 遅い L2: GET は `l2_get_timeout` で打ち切って miss を返す（L2 の遅れを待たない）。
    let slow = Arc::new(MemL2::default());
    *slow.read_delay.lock().unwrap() = Duration::from_secs(2);
    let mut c = cfg(&tmp.path().join("slow"), true);
    c.l2_get_timeout = Duration::from_millis(100);
    let s2 = TieredStore::open_with(c, Some(slow.clone()), clock.clone()).unwrap();
    s2.put(&local, b"local").unwrap();
    let started = Instant::now();
    assert_eq!(s2.get(&key(70)).unwrap(), Lookup::Miss);
    assert!(started.elapsed() < Duration::from_millis(1000));
    assert_eq!(s2.get(&local).unwrap(), Lookup::L1(b"local".to_vec()));
    assert_eq!(s2.stats().l2_timeouts, 1);
}

/// 受け入れ条件（L2 の GC）: `l2_max_bytes` を超えたら mtime の古い順に消す。hit で touch された object は残る。
#[test]
fn l2_gc_evicts_lru() {
    let tmp = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    std::fs::create_dir_all(tmp.path().join("l2")).unwrap();
    let l2 = Arc::new(DirL2::new(tmp.path().join("l2")));
    let base = SystemTime::now() - Duration::from_secs(30 * 86_400);
    let mut size = 0;
    for i in 0..10u32 {
        let z = store::compress(&noise(10_000, i as u64 + 9), 3).unwrap();
        size = size.max(z.len() as u64);
        assert!(l2.write(&key(i), &z).unwrap());
        l2.touch(&key(i), base + Duration::from_secs(3600 * i as u64))
            .unwrap();
    }
    let mut c = cfg(tmp.path(), true);
    c.l2_max_bytes = 6 * size;
    c.l2_gc_target = 0.9;
    c.l2_touch_interval = Duration::ZERO;
    let s = TieredStore::open_with(c, Some(l2.clone()), clock.clone()).unwrap();
    // 最古の object を hit させる（mtime が今に touch される）。
    assert!(matches!(s.get(&key(0)).unwrap(), Lookup::L2(_)));
    wait_until("the hit to touch the object", || {
        std::fs::metadata(l2.object_path(&key(0)))
            .and_then(|m| m.modified())
            .is_ok_and(|t| t > base + Duration::from_secs(86_400))
    });
    let r = s.gc_l2().unwrap();
    assert_eq!(r.scanned, 10);
    assert_eq!(r.removed, 5, "{r:?}");
    let left: Vec<u32> = (0..10).filter(|i| l2.exists(&key(*i)).unwrap()).collect();
    assert_eq!(left, vec![0, 6, 7, 8, 9]);
    let st = s.stats();
    assert_eq!(st.l2_entries, Some(5));
    assert!(st.l2_bytes.unwrap() <= 6 * size);
    assert_eq!(st.l2_gc_removed, 5);
    assert!(st.l2_gc_last_at.is_some());
    // 上限の内側なら消さない。
    assert_eq!(s.gc_l2().unwrap().removed, 0);
}

/// 受け入れ条件 4: L1 の上限で LRU に落とすが、未 flush の entry は落とさない。
#[test]
fn l1_eviction_keeps_unflushed_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    let l2 = Arc::new(MemL2::default());
    let mut c = cfg(tmp.path(), true);
    c.l1_max_bytes = 10_000;
    let s = TieredStore::open_with(c, Some(l2.clone()), clock.clone()).unwrap();
    for i in 0..20 {
        clock.advance(Duration::from_secs(1));
        s.put(&key(i), &[i as u8; 1000]).unwrap();
    }
    let st = s.stats();
    assert_eq!(st.l1_entries, 20, "unflushed entries stay: {st:?}");
    assert_eq!(st.l1_evicted, 0);
    s.flush_all();
    assert_eq!(s.evict_if_needed(), 13);
    let st = s.stats();
    assert!(st.l1_bytes <= 7_000, "{st:?}");
    // 古いものから落ち、新しいものが残る。L2 からは引ける。
    assert_eq!(s.get(&key(19)).unwrap(), Lookup::L1(vec![19; 1000]));
    assert_eq!(s.get(&key(0)).unwrap(), Lookup::L2(vec![0; 1000]));
}

/// 受け入れ条件 4: 未 flush の key は `.pending` から再起動後に積み直す。
#[test]
fn pending_log_is_replayed_after_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let l2 = Arc::new(MemL2::default());
    {
        let s = TieredStore::open_with(cfg(tmp.path(), true), Some(l2.clone()), FakeClock::new())
            .unwrap();
        for i in 0..3 {
            s.put(&key(i), b"unflushed").unwrap();
        }
        assert!(matches!(s.flush_once(), FlushOutcome::Written { .. }));
        s.shutdown();
    }
    let s =
        TieredStore::open_with(cfg(tmp.path(), true), Some(l2.clone()), FakeClock::new()).unwrap();
    assert_eq!(s.queue_len(), 2);
    assert_eq!(s.stats().l1_entries, 3);
    s.flush_all();
    assert!((0..3).all(|i| l2.has(&key(i))));
    // 空になったら `.pending` も空。
    let log = std::fs::read_to_string(tmp.path().join("l1").join(store::PENDING_LOG)).unwrap();
    assert_eq!(log, "");
}

/// `[0-9A-Za-z_-]` 以外を含む key は拒否する（path traversal を作らない）。
#[test]
fn invalid_keys_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _) = dir_store(tmp.path(), FakeClock::new());
    for bad in ["", "../etc", "a/b", "a.b", ".sccache_check"] {
        assert!(s.get(bad).is_err(), "{bad}");
        assert!(s.put(bad, b"x").is_err(), "{bad}");
    }
    assert!(s.get(&"a".repeat(201)).is_err());
}

/// L2 を使わない構成では PUT は flush を積まず、L1 の LRU の対象になる。
#[test]
fn l1_only_store_never_queues() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = cfg(tmp.path(), false);
    c.l1_max_bytes = 1000;
    let s = TieredStore::open_with(c, None, FakeClock::new()).unwrap();
    for i in 0..5 {
        s.put(&key(i), &[0; 400]).unwrap();
    }
    let st = s.stats();
    assert_eq!(st.l2_state, "disabled");
    assert_eq!(st.flush_queue_len, 0);
    assert!(st.l1_bytes <= 900, "{st:?}");
    assert_eq!(s.flush_once(), FlushOutcome::Empty);
}
