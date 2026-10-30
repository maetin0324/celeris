//! process 全体の SQLite 設定（agent-docs/adr/2026-10-29-sqlite-memstatus-off.md）。
//!
//! bundled の SQLite は既定で memory 統計（`SQLITE_CONFIG_MEMSTATUS`）を取り、`sqlite3_malloc` の
//! たびに process で 1 つの mutex を取る。1 process の中で多数の接続を並行に使う（`cargo test` の
//! 試験 thread 群、daemon の読み取り pool）と、この mutex の futex 待ちが kernel 時間の大半になる。
//! この repo は memory 統計（`sqlite3_memory_used` 等）を使わないので、接続を開く前に切る。
//!
//! `sqlite3_config` は `sqlite3_initialize` の前（最初の接続の前）にしか効かず、他の SQLite 呼び出しと
//! 並行に呼んではいけない。そこで、thread が 1 本しかない load 時（`.init_array`）に 1 回だけ呼ぶ。

use std::os::raw::c_int;

extern "C" fn disable_memstatus() {
    // SAFETY: load 時（main と試験 thread の前）に 1 回だけ呼ばれ、他の SQLite 呼び出しと並行しない。
    // 初期化後に呼ばれても SQLITE_MISUSE を返すだけで状態は変わらない。
    let off: c_int = 0;
    unsafe {
        rusqlite::ffi::sqlite3_config(rusqlite::ffi::SQLITE_CONFIG_MEMSTATUS, off);
    }
}

#[used]
#[unsafe(link_section = ".init_array")]
static DISABLE_MEMSTATUS: extern "C" fn() = disable_memstatus;

/// store の open から呼び、この module の object が link から落ちないようにする（rlib の中で
/// 誰も参照しない object は linker が拾わず、`.init_array` の登録も消える）。
pub(crate) fn keep_linked() {
    std::hint::black_box(&DISABLE_MEMSTATUS);
}

#[cfg(test)]
mod tests {
    #[test]
    fn memstatus_is_off_before_the_first_connection() {
        super::keep_linked();
        let conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (randomblob(4096));")
            .expect("write");
        // memory 統計が止まっていれば、確保しても使用量は 0 のまま。
        let used = unsafe { rusqlite::ffi::sqlite3_memory_used() };
        assert_eq!(used, 0);
    }
}
