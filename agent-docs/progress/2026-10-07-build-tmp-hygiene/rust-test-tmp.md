---
title: 葉 rust-test-tmp — Rust 試験・test-parallel.sh の一時 dir 後片付け
tasks: [01M4B60FG863X7G1P5YY5GQ50N]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 rust-test-tmp

- `scripts/dev/check-test-tmp-leftovers.sh` 新設: 空 dir を TMPDIR にして `cargo test <args>`、残りがあれば表示して exit 1。
- browser 試験の `record_dir`（task-worker `browser_tests.rs` の `celeris-browser-unit-<pid>`、task-dispatch `browser_fallback.rs` の
  `celeris-browser-dispatch-unit-<pid>`）を `std::env::temp_dir()` 直下から、試験の tempdir 配下へ移した（drop で消える）。
- `test-parallel.sh`: `TMPDIR=$logdir/tmp` を試験に渡し、既存 trap で丸ごと削除。残数を `tmp_leftovers` に出す。
- 証拠: `for c in task-core celerisctl celeris task-worker; do sh scripts/dev/check-test-tmp-leftovers.sh -p $c; done` は全て exit 0
  （no leftovers）。`cargo clippy --workspace -- -D warnings` exit 0。
## 未解決
- `-p task-dispatch` は cos 添付 dir（読み取り専用 file）が TMPDIR に残る（今回の範囲外）。
- `clippy --all-targets` は task-dispatch の `undeclared_artifacts/tests.rs:146` の useless vec! で落ちる（既存）。
