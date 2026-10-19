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

## 追補（再実行: 全体検査の 46 件失敗の原因）
- 原因: target dir の seed 複製（reflink）由来で、celerisctl の test binary に古い `CARGO_BIN_EXE_celerisctl`（`scratch/seeds/.../.building-gen-*/target/debug/celerisctl`）が焼き込まれており、spawn が ENOENT。コードの不具合ではなく scratch target の汚染。
- 対処: `cargo clean -p celerisctl` 後に `RUSTC_WRAPPER= cargo nextest run --workspace --no-run` で再ビルド（ソース変更なし）。
- 証拠: `cargo fmt --all -- --check` 0 / `cargo clippy --workspace -- -D warnings` 0 / `bash scripts/dev/test-parallel.sh` 0（5880 passed, 0 failed, doctest 含め passed 5883）。
- `git diff c0510a835040 -- crates/task-dispatch` は tests/tick_and_dispatch.rs のみ（前回 run の決定化）。

## 追補2（再々実行）
- check 再実行で再び celerisctl 46 件が ENOENT（scratch seed の古い `CARGO_BIN_EXE_celerisctl` が test binary に焼き込まれた）。`cargo clean -p celerisctl` 後に再実行: fmt 0 / clippy 0 / test-parallel 0（5883 passed, 0 failed）。コード変更なし。
- 提案: seed 複製後に CARGO_BIN_EXE の絶対 path が現 target と食い違う場合の再ビルドを worktree-target-dir/seed 作成側で保証する。
