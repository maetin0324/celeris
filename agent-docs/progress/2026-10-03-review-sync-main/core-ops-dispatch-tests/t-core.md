---
title: task-core の試験を sandbox で個別に通し所要時間を測る（t-core）
tasks: [01M40P9SXYB9K2WCMPH9WDNVZK]
status: done
updated: 2026-10-03
---
# task-core の試験を sandbox で個別に通し所要時間を測る（t-core）

対象 HEAD: `32efeff10e04`（ブランチ `celeris-wu/01M40P9SXYB9K2WCMPH9WDNVZK/t-core`）。run sandbox 内で foreground 実行。`CARGO_TARGET_DIR` は Celeris が渡したローカル scratch。

## 結果

| 回 | コマンド | exit | passed / failed / ignored | real | user | sys | 備考 |
|---|---|---|---|---|---|---|---|
| 1 | `time cargo test -p task-core` | 0 | 688 / 0 / 0（unittests）+ doc-tests 0 / 0 / 0 | 44.8 s | 67.7 s | 127.0 s | build 込み（69 crate を compile、build 29.6 s） |
| 2 | `time cargo test -p task-core` | 0 | 688 / 0 / 0（unittests）+ doc-tests 0 / 0 / 0 | 16.2 s | 32.7 s | 123.7 s | build cache 済み（0.64 s）、試験本体 13.25 s |

- 試験本体の所要（libtest の `finished in`）: 12.97 s（1 回目）/ 13.25 s（2 回目）。
- 単体で 60 秒を超えた試験: **無し**（試験 binary 全体が 13 秒で終わり、libtest の `has been running for over 60 seconds` 警告も出ていない）。
- 失敗した試験: 無し。

## 修正

無し（失敗・60 秒超の試験が無いため、crates/task-core/ は変更していない）。修正前後の比較は不要。

## 証拠コマンド

```
( time cargo test -p task-core 2>&1 ) > artifacts/t-core-1.log; echo exit=$?   # exit=0
( time cargo test -p task-core 2>&1 ) > artifacts/t-core-2.log; echo exit=$?   # exit=0
grep -E '^test result|over 60' artifacts/t-core-*.log
```

## 未解決事項

- 無し。sys 時間（約 2 分）は real を大きく上回るが、内訳は調べていない（失敗や 60 秒超の遅延は無い）。

## 提案

- 無し。
