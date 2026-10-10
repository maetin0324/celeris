---
status: done
task: 01M4J03GGHBA4G27054A54SWK0
work_unit: fail-names
updated: 2026-10-10
---

# fail-names: test-parallel.sh が失敗した test 名を出す

## 目的
final review は check の stdout/stderr の末尾しか残さず、`scripts/dev/test-parallel.sh` は nextest 失敗時に
`cargo nextest run failed (exit N)` だけを出していた。そのため「5023 passed / 1 failed」で落ちた test の名前が
分からなかった（2 回続けて）。

## 変更
- `scripts/dev/test-parallel.sh`: nextest が非 0 で終わったとき、exit する直前に `$logdir/nextest.log` から
  結果行（`FAIL [` / `TIMEOUT [` / `SIG*` で始まる行。時間 `[...]` と `(n/m)` を除いた残り）を取り、重複を除いて
  最大 30 行を `test-parallel: failed: <crate> <test 名>` の形で stderr に出す。最終行は従来どおり
  `test-parallel: cargo nextest run failed (exit N)`。`CELERIS_TEST_SUMMARY` の形は変えていない。
- `scripts/dev/tests/test-parallel-fail-names.sh`（新規）: PATH 先頭に偽の `cargo` を置き、偽の nextest.log を
  流して抽出部分だけを確かめる。cargo は走らせない。確かめること: 失敗名 3 件（重複 1 件を除く）が期待どおり出る、
  最終行が `cargo nextest run failed (exit 100)` のまま、失敗名行が最終行より前、`CELERIS_TEST_SUMMARY {` が stdout に出る。

## 証拠
- `bash -n scripts/dev/test-parallel.sh` → exit 0（構文 OK）。
- `sh scripts/dev/tests/test-parallel-fail-names.sh` → `test-parallel-fail-names: all checks passed`、exit 0。
  stderr の実出力:
  ```
  test-parallel: failed: task-worker tests::broken_one
  test-parallel: failed: task-dispatch slow::test
  test-parallel: failed: celeris crash::test
  test-parallel: cargo nextest run failed (exit 100)
  ```
- 全体試験（`bash scripts/dev/test-parallel.sh`）は取り直していない（cargo を走らせる範囲外。この unit の受け入れは
  抽出部分の試験で確かめる）。次の全体試験（integrate-diag 後の reverify-2）で実際の失敗名が出ることを確かめる。

## 未解決事項
- 実 nextest の結果行の書式は nextest 0.9.146 の出力を前提にしている（`FAIL [   0.104s] crate test` の形）。
  実出力で名前が出ない場合は grep の正規表現を直す。
- 落ちた test の出力（panic 文）は nextest.log に残るが、final review の末尾には入らない。名前だけ出す仕様どおり。

## 提案
- final review の check には `2>&1 | tail -n 40` より、失敗名行を含む `grep 'test-parallel: failed:'` の結果を
  残す形が読みやすい（review 側の設定の話なので、この unit では変えない）。
