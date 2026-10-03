---
title: task-core・task-ops・task-dispatch の sandbox 試験計測
tasks: [01M40P9SXYB9K2WCMPH9WDNVZK]
status: done
updated: 2026-10-03
---
# task-core・task-ops・task-dispatch の sandbox 試験計測

## 個別計測

| crate | コマンド | exit | passed / failed / ignored | 計測 |
|---|---|---:|---:|---|
| task-core | `time cargo test -p task-core` | 0 | 688 / 0 / 0 + doc tests 0 | 44.8 s build 込み、16.2 s cache 済み。試験本体 12.97–13.25 s |
| task-ops | `time cargo test -p task-ops` | 0 | 451 / 0 / 1（手動測定用 ignore）+ doc 0 | 修正前 69.9 s build 込み・34.0 s cache 済み、修正後 17.6 s・17.0 s。試験本体 15.45–16.19 s |
| task-dispatch | `time cargo test -p task-dispatch` | 0 | 574 / 0 / 0 + unified_kill 4 / 0 / 0 + doc 0 | 138.5 s build 込み、38.5 s cache 済み。試験本体 35.43–35.91 s + unified_kill 1.20 s |

個別結果と各試験の詳細は [`core-ops-dispatch-tests/`](core-ops-dispatch-tests/) の `t-core.md`、`t-ops.md`、`t-dispatch.md` にある。task-ops は `a_command_that_never_finishes_is_killed` の子孫 process が pipe を保持して timeout 後も待つ不具合を修正し、30 s から約 0.2 s に短縮した。task-dispatch 最長試験は約 22 s で、60 秒超の単一試験は無かった。

## 統合後の確認

| コマンド | exit | 結果 |
|---|---:|---|
| `cargo test -p celeris-credentiald --test broker daemon_rejects_worker_secret_retrieval_even_with_valid_lease -- --exact` | 0 | 1 passed。sandbox 固有の環境継承不具合を修正 |
| `cargo test --workspace` | 0 | 126 試験バイナリすべて ok（5m35s）。e2e `cluster_scenarios` の 2 件を含め失敗なし |
| `cargo clippy --workspace -- -D warnings` | 0 | warning なし |

詳細は [`core-ops-dispatch-tests/ws-green.md`](core-ops-dispatch-tests/ws-green.md)。
