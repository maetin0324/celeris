# 差し戻し修正後の全体検証（final-verify）

- 実行日: 2026-10-11
- 作業 HEAD: `2cc3cb46`（integrate wu/stage-gate）
- 原票ログ（run の artifacts 配下）: `test-parallel.log` `clippy.log` `fmt.log` `web-install.log` `web-typecheck.log` `web-test.log` `web-e2e.log`

## 検査 4 群

### 1. Rust 試験（TMPDIR=/tmp）

- コマンド: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`
- exit: 0
- nextest Summary 原票: `5185 tests run: 5185 passed, 13 skipped`
- doctest: `3 passed`（doc binaries 10 のうち結果行 3 passed）
- 計: passed 5188 / failed 0 / ignored 14（skipped 13 + doctest ignored 1）
- test-parallel 最終行: `CELERIS_TEST_SUMMARY {... "passed": 5188, "failed": 0, "nextest_exit": 0, "doctest_exit": 0 ...}`、`test-parallel: ok`
- 注: 既定 TMPDIR では unix socket path 長超過で browser/credentiald fixture が落ちるため、`TMPDIR=/tmp` で実行した。

### 2. clippy

- コマンド: `cargo clippy --workspace -- -D warnings`
- exit: 0（`Finished dev profile`、警告 0）

### 3. fmt

- コマンド: `cargo fmt --all -- --check`
- exit: 0（差分なし）

### 4. web

- install: `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → exit 0（オフライン取得で通過。取り直しは不要だった）
- typecheck: `corepack pnpm@12.6.0 -C web typecheck` → exit 0（`tsc -b`）
- test: `corepack pnpm@12.6.0 -C web test` → exit 0（Test Files 95 passed / Tests 691 passed）
- e2e: `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/work/wait-reasons.spec.ts e2e/work/task-tree-waits.spec.ts` → exit 0
  - `task tree shows ordered stages, WU dependency keys, and child wait reasons on mobile` 合格
  - `board and task list show runnable and waiting tasks with links on mobile` 合格
  - `2 passed (6.7s)`

## 範囲の確認

- `git diff --quiet "${CELERIS_WU_BASE}" -- crates/ web/` → exit 0（差分ゼロ）
- `git status --short` → 空（作業ツリーは clean。本記録のみ追加）
- 注: `CELERIS_WU_BASE`（1f605ba9）は現 HEAD（2cc3cb46）の祖先ではない。範囲の差分は crates/・web/ ともに空のため、この点は判定に影響しない。

## 結論

- 4 群の検査はすべて exit 0。失敗はない。
- この WorkUnit はコードを変えず、本記録だけを追加する。ADR Status と進捗ファイルの更新は record-final の担当。

## 未解決事項

- なし（この検査の範囲で）。
- 既知の注意: `scripts/dev/test-parallel.sh` の summary parser は今回 `passed` を正しく数えた（5188 = 5185 + doctest 3）が、`passed (N slow)` の数え落としの恐れは前回から未修正。
