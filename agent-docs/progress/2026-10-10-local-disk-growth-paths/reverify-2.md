---
title: "Local disk growth reverify 2 results"
tasks: ["01M4JCT269M0TMKMTJBTN6018K"]
status: complete
updated: 2026-10-10
---
# 全体試験の再検証（reverify-2）

## 実行結果

同一 HEAD `10c254de` で `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` を2回実行した。各ログは run の一時領域 `/local/celeris/data/workspaces/01M4JCT269M0TMKMTJBTN6018K/runs/01M4JCVMNKKJRGVX52ASXC3S7C/tmp/` に保存した。

| 回 | exit | passed / failed | `CELERIS_TEST_SUMMARY` | 失敗名 |
|---|---:|---:|---|---|
| 1 | exit 0 | 5044 / 0（13 skipped） | `CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 175, "nextest_binaries": 165, "doc_binaries": 10, "passed": 5044, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "nextest_secs": 344.3, "doctest_secs": 13.2, "tmp_leftovers": 5, "summary_parsed": true}` | なし（`test-parallel: failed:` と `FAIL [` は0件） |
| 2 | exit 0 | 5044 / 0（13 skipped） | `CELERIS_TEST_SUMMARY {"runner": "nextest", "nextest_version": "0.9.146", "jobs": 8, "binaries": 175, "nextest_binaries": 165, "doc_binaries": 10, "passed": 5044, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "nextest_secs": 123.7, "doctest_secs": 19.5, "tmp_leftovers": 8, "summary_parsed": true}` | なし（`test-parallel: failed:` と `FAIL [` は0件） |

失敗がなかったため、単独再実行および branch 起因か範囲外 flaky かの切り分けは不要だった。最終回（2回目）は exit 0。

## 追加検査

- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0
- `df -h /local`: 300G中189G使用、108G空き、64%。95%未満のため disk_watch 起因の可能性は低い。
- `sh scripts/dev/check-doc-links.sh`: exit 0
- `sh scripts/dev/check-adr-numbers.sh`: exit 0
- `sh scripts/dev/progress-index.sh --check`: exit 0

## 提案

2回とも失敗なし。再現した試験名はなく、追加の flaky 対応提案はない。
