---
title: task-dispatch の試験を sandbox で個別に通し所要時間を測る（t-dispatch）
tasks: [01M40P9SXYB9K2WCMPH9WDNVZK]
status: done
updated: 2026-10-03
---
# task-dispatch の試験を sandbox で個別に通し所要時間を測る（t-dispatch）

対象 HEAD: `9189ccdb04d2`（ブランチ `celeris-wu/01M40P9SXYB9K2WCMPH9WDNVZK/t-dispatch`）。run sandbox 内で foreground 実行（24 core）。`CARGO_TARGET_DIR` は Celeris が渡したローカル scratch。

## 結果

| 回 | コマンド | exit | passed / failed / ignored | real | user | sys | 試験本体 | 備考 |
|---|---|---|---|---|---|---|---|---|
| 1 | `time cargo test -p task-dispatch` | 0 | 574 / 0 / 0（lib）+ 4 / 0 / 0（tests/unified_kill）+ doc 0 / 0 / 0 | 138.5 s | 106.9 s | 276.8 s | 35.43 s + 1.20 s | build 込み（132 crate を compile、build 1 分 41 秒） |
| 2 | 同上 | 0 | 同上 | 38.5 s | 69.4 s | 272.3 s | 35.91 s + 1.20 s | build cache 済み（0.80 s） |

- 合計 578 passed / 0 failed / 0 ignored。
- 単体で 60 秒を超えた試験: **無し**（libtest の `has been running for over 60 seconds` 警告も出ていない）。
- 失敗した試験: 無し。

### 試験ごとの所要（`--report-time`、lib の上位）

`RUSTC_BOOTSTRAP=1 <task_dispatch test binary> -Z unstable-options --report-time`（exit 0、574 passed、35.74 s）:

| 秒 | 試験 |
|---|---|
| 22.10 | `dispatcher::tests::tree::child_failure_fails_the_unit_and_the_stage_does_not_complete` |
| 13.46 | `dispatcher::tests::tree_approval::liveness_flags_only_unexplained_stalls` |
| 9.46 | `dispatcher::tests::tree_gate::plan_limits_raise_limit_decisions_and_hold_only_the_excess` |
| 6.19 | `dispatcher::tests::tree::child_waits_for_dependencies_and_decisions` |
| 5.48 | `dispatcher::tests::tree_decisions::limit_raise_answer_resumes_subtree` |
| 5.25 | `dispatcher::tests::tree_replan::child_replans_are_bounded_by_tree_and_node_limits` |

最長の 22 s は `run_until_idle(&mut d, 800)`（tick ごとに実時間 20 ms の sleep）が idle にならず上限 800 tick まで回るため（子が失敗したまま段が完了しないことを確かめる試験なので、idle に達しないのは想定どおり）。60 秒の閾値には遠く、flaky の兆候も無い。

## 修正

無し（失敗・60 秒超の試験が無いため crates/task-dispatch/ は変更していない）。修正前後の比較は不要。

## 証拠コマンド

```
( time cargo test -p task-dispatch 2>&1 ) > artifacts/t-dispatch-1.log; echo exit=$?   # exit=0
( time cargo test -p task-dispatch 2>&1 ) > artifacts/t-dispatch-2.log; echo exit=$?   # exit=0
grep -E '^test result|over 60|^real' artifacts/t-dispatch-*.log
RUSTC_BOOTSTRAP=1 $CARGO_TARGET_DIR/debug/deps/task_dispatch-<hash> -Z unstable-options --report-time
```

## 未解決事項

- 無し。sys 時間（約 4.5 分）は real を大きく上回るが、内訳は調べていない。

## 提案

- `child_failure_fails_the_unit_and_the_stage_does_not_complete` は `run_until_idle` の上限 800 tick × 20 ms を使い切る。`tokio::time::pause` 下で回すか、子と unit の Failed を待ってから数 tick だけ追加で回す形にすれば 22 s → 1 s 前後に縮む見込み（試験本体の約 6 割）。60 秒超ではないので今回は手を入れていない。
