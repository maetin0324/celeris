//! ADR 2026-11-07-root-done-tick-phase-timing D2: thread ごとの store の接続待ちの累積。
//!
//! 書き込み接続の `Mutex` を取るまでの時間と、読み取りプールから接続を借りるまでの時間を、呼んだ thread の
//! 累積値に足す。呼び出し側は前後の差を取り、「待ち」と「処理（経過時間 − 待ち）」を分けて記録する。
//! SQLite の file lock の `busy_timeout` の待ちは SQL の実行の内側にあるので、ここには入らない。

use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    static WAIT_NS: Cell<u64> = const { Cell::new(0) };
}

/// この thread が store の接続（書き込み接続の `Mutex`・読み取りプール）を取るまでに待った時間の累積。
/// 単調に増える。区間の待ちは前後の差で求める。
pub fn store_lock_wait() -> Duration {
    Duration::from_nanos(WAIT_NS.with(Cell::get))
}

/// `since` から今までを、この thread の待ちに足す。
pub(super) fn add_since(since: Instant) {
    let ns = u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX);
    WAIT_NS.with(|c| c.set(c.get().saturating_add(ns)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoticeStore;
    use crate::SqliteStore;
    use std::sync::Arc;
    use std::sync::mpsc;

    /// 別の thread が書き込み接続を持っている間の書き込みは、待ちが正になる。待っている thread の数
    /// （`writer_waiters`）を見てから lock を放すので、負荷で順序が崩れても結果は変わらない。
    #[test]
    fn writer_mutex_wait_is_counted_for_the_waiting_thread_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(SqliteStore::open(&dir.path().join("t.sqlite3")).expect("open"));
        // 競合が無ければ待ちはほぼ 0（Mutex を取るだけ）。
        let before = store_lock_wait();
        store.feed_cursor_set("probe", "1").expect("set");
        let idle = store_lock_wait() - before;
        assert!(idle < Duration::from_millis(500), "{idle:?}");

        let (held_tx, held_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let holder = {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                let guard = store.lock().expect("lock");
                held_tx.send(()).expect("held");
                go_rx.recv().expect("go");
                // 待つ側が Mutex に並んだのを見てから、さらに少し持ってから放す。
                std::thread::sleep(Duration::from_millis(20));
                drop(guard);
            })
        };
        held_rx.recv().expect("held");
        let waiter = {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                let before = store_lock_wait();
                store.feed_cursor_set("probe", "2").expect("set");
                store_lock_wait() - before
            })
        };
        // waiter が `lock()` に入って待ちを数え始めるまで、上限付きで出来事を待つ。
        let deadline = Instant::now() + Duration::from_secs(60);
        while store.writer_waiters() == 0 {
            assert!(
                Instant::now() < deadline,
                "waiter never queued on the writer mutex"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        go_tx.send(()).expect("go");
        holder.join().expect("holder");
        let waited = waiter.join().expect("waiter");
        assert!(waited >= Duration::from_millis(20), "{waited:?}");
        // 待ったのは waiter の thread で、この thread の累積は増えていない。
        assert!(store_lock_wait() - before < Duration::from_millis(500));
        assert_eq!(store.writer_waiters(), 0);
    }
}
