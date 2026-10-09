---
date: 2026-10-09
task: launcher 方式の browser 自動ログイン（credential 注入）の解放
wu: merge-ledger-fix2
status: done
---

# merge-ledger-fix2: origin/ops/ledger-fix2 の取り込みとレビュー

## 取り込み

- `git merge --no-ff origin/ops/ledger-fix2`（merge commit `ee9a20fb`）。base は main `0ba92ea5`。
- 取り込んだ commit: `3d2ea2f5`（cleanup close の prompt 失敗の再試行）、`679cab36`（controller_kill 試験の helper stdout 分離）。
- 差分: `crates/task-worker/src/browser.rs`、`browser_tests.rs`、`tests/browser_runtime_isolated.rs`（3 files, +64 −2）。

## レビュー所見

1. 再試行は有限（`CLOSE_RETRIES = 2`、最大 3 試行）。待ちは `CLOSE_RETRY_DELAY = 500ms`。無限にならない。
2. 再試行は「素早く失敗した close」だけ。timeout（50 秒）と spawn 失敗は `None` として即 `false`。stuck な session を再試行で隠さない。
3. 失敗は隠れない。`close_with` の戻り値は呼び出し元で使われる（`browser.rs` の credential segment 経路で `closed` が偽なら auth interval を解放せず、通常経路では `guard.armed` を保つ）。
4. 追加された非試験コードに `unwrap()` は無い（新規試験の `unwrap` は試験のみで許容）。
   - 注: `browser.rs` の 1588/1593 行の `unwrap()` は main（`0ba92ea5`）から既にあり、この取り込みでは増えていない。本 WU の範囲外のため触っていない。
5. 試験 `browser_runtime_isolated.rs` の `stdout(Stdio::null())`: helper の libtest 出力が親の `test … ok` 行を割るのを防ぐ。コメントどおり、marker は file で渡しているため stdout を捨てても判定は変わらない。
6. 新規試験 `browser::tests::cleanup_close_retries_a_prompt_failure` は、1 回目だけ失敗する fake CLI で「2 回目で成功・試行回数 2」と「再試行 0 なら失敗のまま」の両方を確かめる。

所見の結論: 問題は見つからず、差分は直さずに取り込んだ。

## 検証

- 条件 0: `git merge-base --is-ancestor 679cab36 HEAD` → exit 0（ancestor OK）。
- `cargo test -p task-worker --lib`（TMPDIR は run 既定の 93 文字）: 932 passed / 30 failed。失敗 30 件はすべて `bind: path must be shorter than SUN_LEN`（unix socket path 上限）で、環境起因。取り込みの差分とは無関係。
- `TMPDIR=/local/celeris/data/scratch/tw-short cargo test -p task-worker --lib`（試験専用の短い TMPDIR）: exit 0、**962 passed, 0 failed, 4 ignored**。`browser::tests::cleanup_close_retries_a_prompt_failure ... ok` を含む。
- `cargo clippy -p task-worker --all-targets -- -D warnings`: exit 0。
- 未実施: `bash scripts/dev/test-parallel.sh`（workspace 全体）。この WU の範囲は task-worker lib と clippy のみ。全体検査は integrate 段で行う。

## 未解決・提案

- run の TMPDIR（93 文字）では unix socket 試験が落ちる。全体検査（test-parallel.sh）の TMPDIR 扱いは別 WU で決める（既存の記録どおり）。
- `browser.rs` 1588/1593 の `unwrap()` は既存の負債。別件として扱うかは人が決める。
