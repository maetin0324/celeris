//! 一時 DB で起こした daemon の状態の変化を待つ helper。
//!
//! `mod support;`（`script_compare` を含む）を読まない試験も使うので、`#[path]` で直接読む。
//! 読み手: `instance_handoff.rs`・`unpromoted_release.rs`。

use std::time::{Duration, Instant};

/// `check` が真になるまで（最大 `limit`）25 ms ごとに見る。真になれば true、期限を過ぎれば false。
pub async fn wait_until(limit: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
