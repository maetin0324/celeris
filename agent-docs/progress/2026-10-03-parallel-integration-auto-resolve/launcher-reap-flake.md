---
title: browser launcher の shutdown_reaps_all_sessions 断続失敗の原因と修正
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# browser launcher の shutdown_reaps_all_sessions 断続失敗の原因と修正

- 原因（launcher 本体の競合）: `ServerHandle::shutdown` は accept・lease の thread しか join せず、接続 thread を待たなかった。shutdown が接続を閉じると接続 thread が自分の session を `close_conn_sessions` で引き取って teardown する。shutdown 側の `sessions.drain()` は空になってすぐ戻り、process は止まったが記録（`*.json`）の削除は接続 thread が後で行う。この間に数えると記録が 1 つ残る。
- 修正（`crates/task-worker/src/browser_launcher/server.rs`）: shutdown は accept と lease の thread を先に止めてから全接続を閉じ、接続 thread が回収を終えるまで待つ（`connections` が 0 になるまで。終了は `conn_exit` Condvar で知らせる）。その後に残りの session を回収する。`try_clone` できない接続は受けない（shutdown で閉じられず、終了待ちが idle 時間まで延びるため）。fail closed は弱めていない（回収は増えるだけ）。
- 再現試験: `shutdown_waits_for_inflight_connection_teardown`。試験専用フック（`TestHook::TeardownBeforeRemove` / `ShutdownWaitConns`、`#[cfg(test)]`）で接続 thread を「process 停止後・記録削除前」に止めて shutdown を呼ぶ。出来事だけで順序を決め、sleep は使わない。修正前は `left: 1` で落ち、修正後は通る。
- 証拠: `cargo test -p task-worker --lib browser_launcher` 21 passed（5 回連続）、shutdown 系 2 件を 20 回連続 ok、`cargo clippy -p task-worker --all-targets -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 運用: launcher は本番の権限分離の部品。この修正を本番に反映するには host の `celeris-browser-launcher` binary の入れ替え（人の作業）が要る。
