---
tasks: [01M4KKRM8B801THCFKB490QPRB]
tested_sha: 0cbfdc2d772b45b81a180a1ad862bbb7dd948c54
status: done
updated: 2026-10-10
---

# 保存済み credential 再利用: 最終 HEAD の隔離設定・通常設定の全体検査（userns-gate）

- 検査対象は `tested_sha: 0cbfdc2d`（作業開始時の HEAD）。実装・試験コードは変えていない。
- 人のコメント（2026-10-10T19:12）の前提「worker・reviewer の sandbox では userns の unshare が許されない」は、この run の sandbox では当てはまらなかった。`unshare -Ur true` が exit 0 で成功したため、隔離設定の試験をこの sandbox で実行できた。release gate での再実行は、別環境での再確認として残す。

## 1. userns の確認

- `unshare -Ur true`: exit 0（user namespace を作成できた）。

## 2. 隔離設定の全体試験（release gate と同じ env）

- コマンド: `TMPDIR=/tmp CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh`
- exit: 0（`test-parallel: ok`）
- nextest 本体の Summary: `5110 tests run: 5110 passed, 13 skipped`（`failed` 0）。`Summary [173.678s]`、`(2 slow)` を含む。
- doctest: 10 binary、exit 0、`3 passed`（doctest の `test result` 行の合計）、`failed` 0、`1 ignored`。
- `CELERIS_TEST_SUMMARY`: `"userns": true`、`"nextest_exit": 0`、`"doctest_exit": 0`、`"failed": 0`。
- 注意（スクリプトの集計バグ）: 同じ行の `"passed": 3` は誤り。`scripts/dev/test-parallel.sh` の nextest 集計（113〜126 行付近）が `passed (2 slow),` の形を読めず、nextest 側の passed を 0 と数えている。実際の passed は上の nextest Summary の `5110`。通常設定（次節）では slow が無いので `passed: 5113` が正しく出る。
- 単独再試験は不要（失敗 0）。

## 3. 通常設定の全体試験・検査（同じ tested_sha）

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0。`Summary [95.696s] 5110 tests run: 5110 passed, 13 skipped`。`CELERIS_TEST_SUMMARY`: `"passed": 5113, "failed": 0, "ignored": 14, "userns": null`、`"tmp_leftovers": 1`（run の TMPDIR に残った 1 entry の警告。試験の失敗ではない）。
- `cargo fmt --all -- --check`: exit 0。差分なし。
- `cargo clippy --workspace -- -D warnings`: exit 0。`Finished dev profile`。
- `corepack pnpm@12.6.0 -C web lint`: exit 0。`Checked 468 files`、warning 4 件（既存の reduced-motion の `!important`）。
- `corepack pnpm@12.6.0 -C web test`: exit 0。`Tests 659 passed (659)`、Node 試験 `pass 93 / fail 0`。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0（`tsc -b`）。

## 未解決事項

- `scripts/dev/test-parallel.sh` の nextest 集計が `passed (N slow)` を読めない（上記 2 節）。gate の判定に使う `failed`・exit code は正しいが、`passed` の数字が隔離設定の run で過小に出る。修正は別の unit で行う。
- 前回 attempt の記録にあった `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login` の初回 1 件失敗（`Target.setDiscoverTargets: cdp_command_failed`）は、今回の隔離設定の全体試験では再現しなかった（`failed: 0`）。

## 提案

- `scripts/dev/test-parallel.sh` の Summary 行の正規表現を、括弧付きの注記（`(2 slow)`）を許す形へ直す。併せて、この集計の試験（`passed` が nextest Summary と一致すること）を足す。
