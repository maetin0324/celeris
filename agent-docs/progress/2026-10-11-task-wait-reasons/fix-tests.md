---
title: "fix-tests: main 取り込みで入った task-ops 試験の compile 失敗を直し、workspace 試験を通す"
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# fix-tests（final review 失敗の修正）

## 原因

main 取り込み（50e218ab stranded integration の試験）で `crates/task-ops/src/view/tests.rs` に入った 2 つの呼び出しが、api 葉が足した第 4 引数を持っていなかった。

- 109 行: `task_summary(&store, &task, &view_ctx(), now)`
- 127 行: `task_detail(&store, task.id, &view_ctx(), now)`

api 葉で `task_summary`・`task_detail` は `&crate::wait::MemoryWaits` を取るようになった。同じ file の 556 行以降の呼び出しは既にこの形だった。

## 修正

- `crates/task-ops/src/view/tests.rs`: 上の 2 呼び出しに `&crate::wait::MemoryWaits::default()` を足した（同 file の既存の形に合わせた）。挙動は変えていない。
- 他の compile 失敗は無かった（下の証拠）。他の crate は触っていない。

## 証拠

作業ツリーは `repos/agent-platform`、差分は `crates/task-ops/src/view/tests.rs` の 1 file だけ（16 行追加、2 行削除）。

- `cargo nextest run --workspace --no-run` → exit 0、`Finished test profile`。compile 失敗なし。
- `bash scripts/dev/test-parallel.sh`（TMPDIR 既定の run 用 tmp）→ exit 100、`Summary 5180 tests run: 5072 passed, 108 failed, 13 skipped`。失敗 108 件は全部 `timed out waiting for fixture readiness`（browser・credentiald の試験 fixture が unix socket を待つ）。原因は `$TMPDIR` の長さ。`$TMPDIR/credentiald/runtime/celeris-credentiald/control.sock` が 146 バイトで、unix socket の上限 107 を超える。コードの失敗ではない（verify 記録と同じ環境要因）。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、`Summary 5180 tests run: 5180 passed, 13 skipped`。summary JSON `passed 5183, failed 0, ignored 14, nextest_exit 0, doctest_exit 0, tmp_leftovers 0`。試験 fixture だけ短い TMPDIR を使う。build 出力は `CARGO_TARGET_DIR` のまま。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`、警告なし）。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- 既定の TMPDIR（run の tmp）では fixture の socket 試験が 108 件落ちる。run 側の TMPDIR 長を縮めないと、release gate の既定実行は通らない。直すのは run の作業ディレクトリの設計なので、この task の範囲外。人に上げる（進捗 `2026-10-11-task-wait-reasons.md` の未解決を参照）。

## 提案

- celeris の run の TMPDIR を短い path（例: `/local/celeris/data/tmp/<run>`）にするか、試験 fixture が socket を `/tmp` 直下に作るようにする。どちらにするかは人が決める。
