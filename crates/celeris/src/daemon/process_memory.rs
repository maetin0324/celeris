//! P4-F（第 4 フェーズ）: daemon の process の memory の内訳を [`PROCESS_MEMORY_LOG_INTERVAL`] ごとに 1 行の log
//! （`process_memory`）に出す（ADR `2026-11-05-phase4-p4f-daemon-soak`）。
//!
//! 本番の RSS の増加（2026-11-04、182→589 MB／26 分）の原因を外から切り分けるための観測値で、DB には書かない。
//! 読む値は次のとおり（どれも読むだけで、process の状態を変えない）。
//! - `/proc/self/status` の `VmRSS`・`RssAnon`・`RssFile`・`Threads`。
//! - SQLite の process 全体の memory（`sqlite3_status64`）。page cache の bytes（`PAGECACHE_OVERFLOW`。
//!   `SQLITE_CONFIG_PAGECACHE` を使わないので page cache は全部ここに入る）は常に数える。`MEMORY_USED`・その最大・
//!   malloc の回数は memstatus が有効なときだけ数え、bundled の SQLite の既定（無効）では 0 になる（P4-F の実測）。
//! - glibc の malloc の統計（`mallinfo2`: arena の bytes・mmap の bytes・使用中・空き）。
//! - `[script]` があれば、生きた generation ごとの V8 heap（[`celeris_script::GenerationRegistry::heap_stats`]）の合計。
//!
//! 外部の driver（`scripts/dev/daemon-soak/daemon_soak.py`）はこの行を daemon の log から読み、`/proc/<pid>` の値と
//! DB の行数と合わせて時系列にする。

use std::time::Duration;

use celeris_script::GenerationRegistry;

/// `process_memory` の log 行の間隔（1 日 1,440 行）。
pub(crate) const PROCESS_MEMORY_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// `/proc/self/status` から読む値（kB。`Threads` は数）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcStatus {
    pub rss_kb: u64,
    pub rss_anon_kb: u64,
    pub rss_file_kb: u64,
    pub threads: u64,
}

/// `/proc/<pid>/status` の本文から値を読む。無い行は 0。
pub(crate) fn parse_proc_status(text: &str) -> ProcStatus {
    let mut s = ProcStatus::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(value)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        match key {
            "VmRSS:" => s.rss_kb = value,
            "RssAnon:" => s.rss_anon_kb = value,
            "RssFile:" => s.rss_file_kb = value,
            "Threads:" => s.threads = value,
            _ => {}
        }
    }
    s
}

/// SQLite の process 全体の memory（bytes。`malloc_count` は数）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SqliteMemory {
    pub memory_used: i64,
    pub memory_highwater: i64,
    pub pagecache_bytes: i64,
    pub malloc_count: i64,
}

fn sqlite_status(op: i32) -> (i64, i64) {
    let mut current: i64 = 0;
    let mut highwater: i64 = 0;
    // SAFETY: `sqlite3_status64` は process 全体の統計を読むだけで、渡すのは生きた局所変数への pointer。
    // reset の引数は 0（最大値を消さない）。bundled の SQLite は threadsafe で、どの thread から呼んでもよい。
    let rc = unsafe { rusqlite::ffi::sqlite3_status64(op, &mut current, &mut highwater, 0) };
    if rc == rusqlite::ffi::SQLITE_OK {
        (current, highwater)
    } else {
        (0, 0)
    }
}

pub(crate) fn sqlite_memory() -> SqliteMemory {
    let (memory_used, memory_highwater) = sqlite_status(rusqlite::ffi::SQLITE_STATUS_MEMORY_USED);
    let (pagecache_bytes, _) = sqlite_status(rusqlite::ffi::SQLITE_STATUS_PAGECACHE_OVERFLOW);
    let (malloc_count, _) = sqlite_status(rusqlite::ffi::SQLITE_STATUS_MALLOC_COUNT);
    SqliteMemory {
        memory_used,
        memory_highwater,
        pagecache_bytes,
        malloc_count,
    }
}

/// glibc の malloc の統計（bytes）。glibc でない target では読まない。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MallocStats {
    /// `brk`・arena で確保した bytes（main と他の arena の合計）。
    pub arena: u64,
    /// mmap で直接確保した bytes。
    pub mmap: u64,
    /// 使用中の bytes。
    pub in_use: u64,
    /// arena の中の空きの bytes（RSS に残り得る断片）。
    pub free: u64,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn malloc_stats() -> Option<MallocStats> {
    // SAFETY: `mallinfo2` は引数を取らず、値で構造体を返す（glibc 2.33 以降）。arena の lock は glibc が取る。
    let m = unsafe { libc::mallinfo2() };
    Some(MallocStats {
        arena: m.arena as u64,
        mmap: m.hblkhd as u64,
        in_use: m.uordblks as u64,
        free: m.fordblks as u64,
    })
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub(crate) fn malloc_stats() -> Option<MallocStats> {
    None
}

/// 生きた generation の V8 heap の合計。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct V8Totals {
    pub generations: u64,
    pub used_heap: u64,
    pub total_heap: u64,
    pub external: u64,
    pub malloced: u64,
}

pub(crate) fn v8_totals(stats: &[(String, celeris_script::HeapStats)]) -> V8Totals {
    stats.iter().fold(V8Totals::default(), |mut t, (_, h)| {
        t.generations += 1;
        t.used_heap += h.used_heap_size;
        t.total_heap += h.total_heap_size;
        t.external += h.external_memory;
        t.malloced += h.malloced_memory;
        t
    })
}

/// 1 回分の値を読んで log 行を出す。
pub(crate) async fn log_once(registry: Option<&GenerationRegistry>) {
    let status = std::fs::read_to_string("/proc/self/status")
        .map(|t| parse_proc_status(&t))
        .unwrap_or_default();
    let sqlite = sqlite_memory();
    let malloc = malloc_stats().unwrap_or_default();
    let v8 = match registry {
        Some(r) => v8_totals(&r.heap_stats().await),
        None => V8Totals::default(),
    };
    tracing::info!(
        rss_kb = status.rss_kb,
        rss_anon_kb = status.rss_anon_kb,
        rss_file_kb = status.rss_file_kb,
        threads = status.threads,
        sqlite_memory_used = sqlite.memory_used,
        sqlite_memory_highwater = sqlite.memory_highwater,
        sqlite_pagecache_bytes = sqlite.pagecache_bytes,
        sqlite_malloc_count = sqlite.malloc_count,
        malloc_arena = malloc.arena,
        malloc_mmap = malloc.mmap,
        malloc_in_use = malloc.in_use,
        malloc_free = malloc.free,
        v8_generations = v8.generations,
        v8_used_heap = v8.used_heap,
        v8_total_heap = v8.total_heap,
        v8_external = v8.external,
        v8_malloced = v8.malloced,
        "process_memory"
    );
}

/// daemon の起動から止まるまで、起動直後と [`PROCESS_MEMORY_LOG_INTERVAL`] ごとに 1 行出す。tick を待たせない
/// （別の task で、V8 の統計は actor が空いたときに返る）。止めるときは handle を abort する。
pub(crate) fn spawn(registry: Option<GenerationRegistry>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PROCESS_MEMORY_LOG_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            log_once(registry.as_ref()).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_memory_parses_proc_status_fields() {
        let text = "Name:\tceleris\nVmRSS:\t  588668 kB\nRssAnon:\t  520700 kB\nRssFile:\t   67968 kB\n\
                    RssShmem:\t       0 kB\nThreads:\t38\n";
        assert_eq!(
            parse_proc_status(text),
            ProcStatus {
                rss_kb: 588_668,
                rss_anon_kb: 520_700,
                rss_file_kb: 67_968,
                threads: 38,
            }
        );
        assert_eq!(
            parse_proc_status("garbage\nVmRSS: x kB\n"),
            ProcStatus::default()
        );
    }

    #[test]
    fn process_memory_reads_this_process() {
        let own = std::fs::read_to_string("/proc/self/status").expect("proc status");
        let s = parse_proc_status(&own);
        assert!(s.rss_kb > 0 && s.threads > 0, "{s:?}");
        // 接続を開いて page を書くと page cache の bytes が 0 より大きくなる（接続を閉じる前に読む）。
        let db = rusqlite::Connection::open_in_memory().expect("db");
        db.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .expect("sql");
        assert!(sqlite_memory().pagecache_bytes > 0);
        drop(db);
        let m = malloc_stats().expect("glibc");
        assert!(m.in_use > 0, "{m:?}");
    }

    #[test]
    fn process_memory_sums_v8_generations() {
        let h = |used, total| celeris_script::HeapStats {
            used_heap_size: used,
            total_heap_size: total,
            total_physical_size: 0,
            heap_size_limit: 0,
            external_memory: 1,
            malloced_memory: 2,
            number_of_native_contexts: 1,
            generation: 0,
        };
        let t = v8_totals(&[("a".into(), h(10, 20)), ("b".into(), h(5, 8))]);
        assert_eq!(
            t,
            V8Totals {
                generations: 2,
                used_heap: 15,
                total_heap: 28,
                external: 2,
                malloced: 4,
            }
        );
    }
}
