---
title: task-ops の試験を sandbox で個別に通し所要時間を測る（t-ops）
tasks: [01M40P9SXYB9K2WCMPH9WDNVZK]
status: done
updated: 2026-10-03
---
# task-ops の試験を sandbox で個別に通し所要時間を測る（t-ops）

対象 HEAD: `a16b472ccde9`（ブランチ `celeris-wu/01M40P9SXYB9K2WCMPH9WDNVZK/t-ops`）。run sandbox 内で foreground 実行（24 core）。`CARGO_TARGET_DIR` は Celeris が渡したローカル scratch。

## 結果

| 回 | 時点 | コマンド | exit | passed / failed / ignored | real | user | sys | 試験本体 | 備考 |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 修正前 | `time cargo test -p task-ops` | 0 | 451 / 0 / 0（lib）+ 0 / 0 / 1（tests/feed_measure）+ doc 0 | 69.9 s | 56.6 s | 212.6 s | 32.44 s | build 込み（36.5 s） |
| 2 | 修正前 | 同上 | 0 | 同上 | 34.0 s | 37.2 s | 166.4 s | 32.64 s | build cache 済み |
| 5 | 修正後 | 同上 | 0 | 同上 | 17.6 s | 36.8 s | 119.3 s | 16.19 s | build cache 済み |
| 6 | 修正後 | 同上 | 0 | 同上 | 17.0 s | 33.5 s | 84.7 s | 15.45 s | build cache 済み |

- ignored の 1 件は `tests/feed_measure.rs`（`#[ignore = "manual measurement against a copy of a production database"]`、手動計測用。前からある）。
- 単体で 60 秒を超えた試験: **無し**（修正前・修正後とも。libtest の `has been running for over 60 seconds` も出ていない）。
- 失敗した試験: 無し。

### 試験ごとの所要（`--report-time`、上位）

修正前（`RUSTC_BOOTSTRAP=1 <test binary> -Z unstable-options --report-time`）:

| 秒 | 試験 |
|---|---|
| 30.00 | `changes::tests::a_command_that_never_finishes_is_killed` |
| 8.64 | `comment::tests::the_human_comment_effect_table_holds_for_every_status` |
| 6.53 | `graph::tests::graph_over_node_limit_is_a_validation_error` |
| 5.69 | `edit::tests::terminal_tasks_refuse_edits_but_running_and_reviewing_accept_them` |

修正後: `a_command_that_never_finishes_is_killed` は 0.20 s。最長は `the_human_comment_effect_table_holds_for_every_status`（10.1 s）。

## 修正

`changes::tests::a_command_that_never_finishes_is_killed` が 1 件で壁時計の約半分（30 s）を占めていた。原因は試験でなく本体の不具合: `changes::run` は時間切れで子（`sh`）だけを殺し、孫の `sleep 30` が stdout/stderr のパイプを握ったまま残るので、読み切りスレッドの join が孫の終わり（30 s）まで待っていた。つまり上限 200 ms が効いていない。本番でも `git` の `ssh` や `gh` の子が残れば、`GIT_TIMEOUT` 等の上限を越えて API ハンドラが握られうる。

- `crates/task-ops/src/changes.rs`: 子を `process_group(0)` で自分のプロセスグループに入れ、時間切れでは `kill -KILL -- -<pgid>`（`kill(1)`）でグループごと殺してから `child.kill()`。task-ops に signal の crate を足すと `Cargo.lock` が変わり範囲外になるため、`kill(1)` を使った。
- `crates/task-ops/src/changes/tests.rs`: 同試験に「20 秒未満で返る」の assert を足し、退行を固定した（修正前の実装では 30 s で落ちる）。時間依存の待ちは無く、CPU を焼く手も使っていない。

## main merge の衝突箇所（delivery.rs・delivery/tests.rs）

衝突を解いた merge `44082cab` の両親それぞれにある `fn` が merge 後にすべて残っているかを比べ、欠けは 0 件だった（両側保持は壊れていない）。delivery の試験も全件合格。

## 証拠コマンド

```
( time cargo test -p task-ops 2>&1 ) > artifacts/t-ops-1.log; echo exit=$?    # exit=0（修正前、build 込み）
( time cargo test -p task-ops 2>&1 ) > artifacts/t-ops-2.log; echo exit=$?    # exit=0（修正前）
RUSTC_BOOTSTRAP=1 $CARGO_TARGET_DIR/debug/deps/task_ops-<hash> -Z unstable-options --report-time   # 試験ごとの所要
( time cargo test -p task-ops 2>&1 ) > artifacts/t-ops-5.log; echo exit=$?    # exit=0（修正後）
( time cargo test -p task-ops 2>&1 ) > artifacts/t-ops-6.log; echo exit=$?    # exit=0（修正後）
cargo clippy --workspace --all-targets -- -D warnings                        # exit=0
cargo fmt -p task-ops -- --check                                              # exit=0
```

## 未解決事項

- 子が正常に終わったあとに孫が残ってパイプを握る場合（時間切れでない経路）は、読み切りの join がまだ孫を待つ。今回の試験では起きておらず、直していない。
- sys 時間（1.5〜3.5 分）が real を大きく上回る。内訳は調べていない（失敗・60 秒超は無い）。
- `cargo test --workspace` はこの葉では流していない（計画で crate ごとの計測に分けているため）。

## 提案

- 無し。
