---
title: repair-verify-3 — 全体並列でのみ落ちる task-dispatch の merge_base 試験の決定化
status: done
completed: 2026-10-19
tasks: [01M59XT2H8KMZZDBFZYC81WGNR]
---
# repair-verify-3

- `git diff c0510a835040 -- crates/task-dispatch` は空（本 branch は task-dispatch を変えていない既存 flaky）。
- 原因: `merge_base_check_merges_main_deterministically_when_there_is_no_conflict` が `run_until_idle(&mut d, 400)` の tick 回数上限で待っていた。負荷で git subprocess が遅れると、merge 完了前に回数が尽きる。
- 修正（`crates/task-dispatch/src/dispatcher/tests/tick_and_dispatch.rs` のみ）: tick 回数でなく「task が terminal かつ dispatcher が idle」の状態到達で待つ。保険の壁時計上限は 300s。repair WU を作らず Done の主張は不変。本体 src は触っていない。
- 環境メモ: seed 由来の古い test binary に `CARGO_BIN_EXE_celerisctl` の絶対パスが焼き込まれ celerisctl 試験が一斉に NotFound になった。`cargo clean -p celerisctl`（scratch target のみ）で解消。repo 変更ではない。

## 証拠
- `bash scripts/dev/test-parallel.sh` → exit 0、5880 passed / 14 skipped、doc-test ok
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `cargo test -p celeris-campaign --test global_ledger_atomic` → 24 passed
