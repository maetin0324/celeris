---
title: browser 適合台帳 ledger-fix の統合と本番 loopback 例外の固定
tasks: [01M4F7Y6S890EPC7ZK3YC6YX3A]
status: done
updated: 2026-10-09
---
# browser 適合台帳 ledger-fix の統合と本番 loopback 例外の固定

`ops/ledger-fix` の `bf757bffa734d3d01c1703562fb8b81b13df1e1a` を no-ff merge した。merge-base からのレビューでは、生成器の action socket 提供と origin 形式、isolated runtime binaries の build、fallback fixture の loopback origin と GET click が ADR の決定に沿っていた。`TEST_LOOPBACK_ALLOW` と daemon helper は unit-test 用 `cfg(test)` に閉じており、ignored の実 harness fallback 試験は fixture port のみを設定する。検査しやすくするため helper と同じ値を返す `#[doc(hidden)]` 読み出し関数を加え、通常の integration test から non-test library build を検査する。

## 実行結果

- `git fetch origin ops/ledger-fix`、`git merge --no-ff --no-edit FETCH_HEAD` → 完了。`git merge-base --is-ancestor bf757bff HEAD` → exit 0。
- 通常の `python3 -m unittest discover -s scripts/tests -p 'test_browser_conformance*.py'` は長い run `TMPDIR` 由来の Unix socket path 超過で 1 test が error。`TMPDIR=/tmp python3 -m unittest discover -s scripts/tests -p 'test_browser_conformance*.py'` → exit 0（4 tests）。実装不具合ではなく短い一時ディレクトリで回避した。
- `TMPDIR=/tmp cargo test -p task-worker --test browser_loopback_prod_build && TMPDIR=/tmp cargo test -p task-worker --lib browser` → exit 0。統合試験 1 passed。browser filter は 163 passed、0 failed、3 ignored。ignored の実 browser harness は `CELERIS_USERNS_TESTS` を有効にしない通常実行では skip。
- `TMPDIR=/tmp cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。

## 変更

- `crates/task-worker/src/browser.rs`: daemon runtime と同一の cfg 分岐を返す hidden accessor を追加。`TEST_LOOPBACK_ALLOW` 自体は引き続き `#[cfg(test)]`。
- `crates/task-worker/tests/browser_loopback_prod_build.rs`: integration test が non-test library build の accessor を呼び、daemon runtime に渡る loopback 許可が空であることを固定。
