---
status: done
completed: 2026-10-21
---
# run-wait: changes.rs run() の固定 sleep poll を終了待ちへ

- ADR 2026-10-21-task-list-opt-in-integrations D3 のとおり、wait thread + `mpsc::recv_timeout(timeout)` にした。時間切れは `kill_group` + pid kill → wait 結果を回収して `timed_out = true`。`sleep` の poll は無い（`grep sleep crates/task-ops/src/changes.rs` は 0 件）。
- 試験（changes/tests.rs）: 即終了は timeout 前に返る / timeout 超過は kill・エラー（既存）/ 1 MiB の stdout・stderr で詰まらない。
- 証拠: `cargo test -p task-ops --lib changes::tests::a_command` → 3 passed。`cargo clippy -p task-ops -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 本番への移行
config 変更なし。
