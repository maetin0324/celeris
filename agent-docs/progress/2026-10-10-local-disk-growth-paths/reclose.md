---
title: "Local disk growth reclose (v4 統合後 HEAD の全体検査)"
tasks: ["01M4JCT269M0TMKMTJBTN6018K"]
status: complete
updated: 2026-10-10
---
# 全体試験・clippy・fmt の取り直し（reclose）

対象 HEAD: `033b776f`（`integrate wu/sizing-impl (phase reprune)`）。reprune 段（prune-workspace・sizing-adr・sizing-impl）を取り込んだ後の状態。試験 file・本体コードは変更していない。

## 実行結果

| 検査 | コマンド | exit | 結果 |
|---|---|---:|---|
| 全体試験 | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | nextest 5047 passed / 0 failed / 14 ignored、doctest exit 0、`tmp_leftovers: 0` |
| clippy | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`（警告 0） |
| fmt | `cargo fmt --all -- --check` | 0 | 差分なし |

`CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 175, "nextest_binaries": 165, "doc_binaries": 10, "passed": 5047, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "nextest_secs": 227.1, "doctest_secs": 10.1, "tmp_leftovers": 0, "summary_parsed": true}`

`test-parallel: failed:` 行と `FAIL [` 行は 0 件。失敗名はない。

## 文書検査（3 本）

| コマンド | exit |
|---|---:|
| `sh scripts/dev/check-doc-links.sh` | 0 |
| `sh scripts/dev/check-adr-numbers.sh` | 0 |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0（`check-doc-layout: ok`） |

補足: `sh scripts/dev/progress-index.sh --check` も exit 0。`check-doc-layout.sh` は tsv 引数を要るため、引数なしの実行は usage を出して exit 1 になる（引数の付け忘れで、検査の失敗ではない）。

## 注記

- 同じ 033b776f で走らせた試験の passed 数は、前回の reverify-2（10c254de、5044 passed）より 3 多い。reprune 段で `scratch_shared_` 系の試験 3 件を足したため。
- 検査時の `df -h /local` は 300G 中 201G 使用（68%）。disk_watch の 95% 未満。
- 失敗を再現させるための試験の繰り返しや負荷試験はしていない（CPU 負荷の規則に従う）。

## 結論

受け入れ条件 0（全体試験の passed/failed 数と clippy・fmt の結果を本文に記録）と 2（文書検査 3 本）を満たす。
