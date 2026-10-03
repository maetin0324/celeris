---
title: task-core・task-ops・task-dispatch の sandbox 試験計測
tasks: [01M40P9SXYB9K2WCMPH9WDNVZK]
status: done
updated: 2026-10-03
---
# task-core・task-ops・task-dispatch の sandbox 試験計測

## 個別計測

対象 HEAD: `d618ad06c48a`（統合後）。sandbox 内で指定順に foreground 実行し、各 crate の全 cargo 出力と `/usr/bin/time -p` を run artifacts に保存した。

| crate | コマンド | exit | passed / failed / ignored | real | 試験本体 |
|---|---|---:|---|---:|---:|
| task-core | `cargo test -p task-core` | 0 | 688 / 0 / 0（doc 0） | 16.76 s | 14.50 s |
| task-ops | `cargo test -p task-ops` | 0 | 451 / 0 / 1（手動計測用 ignore、doc 0） | 16.38 s | 15.44 s |
| task-dispatch | `cargo test -p task-dispatch` | 0 | 578 / 0 / 0（574 lib + 4 unified_kill、doc 0） | 42.92 s | 40.19 s + 1.69 s |

3 crate とも統合後 HEAD で成功。試験出力に 60 秒超警告はなく、60 秒を超えた個別試験も無い。task-ops の `a_command_that_never_finishes_is_killed` は先行する crate leaf で修正済みで、詳細は [`t-ops.md`](core-ops-dispatch-tests/t-ops.md)。ログは run artifacts の `task-core.log`、`task-ops.log`、`task-dispatch.log`。

個別 leaf の旧 HEAD での修正経緯は [`t-core.md`](core-ops-dispatch-tests/t-core.md)、[`t-ops.md`](core-ops-dispatch-tests/t-ops.md)、[`t-dispatch.md`](core-ops-dispatch-tests/t-dispatch.md) に残す。ws-green の index.json 除去および credentiald broker 子 process の環境分離と workspace 検証結果は [`ws-green.md`](core-ops-dispatch-tests/ws-green.md) を参照。

## 統合後の確認

| コマンド | exit | 結果 |
|---|---:|---|
| `cargo test -p celeris-credentiald --test broker daemon_rejects_worker_secret_retrieval_even_with_valid_lease -- --exact` | 0 | 1 passed。sandbox 固有の環境継承不具合を修正 |
| `cargo test --workspace` | 0 | 126 試験バイナリすべて ok（5m35s）。e2e `cluster_scenarios` の 2 件を含め失敗なし |
| `cargo clippy --workspace -- -D warnings` | 0 | warning なし |

詳細は [`core-ops-dispatch-tests/ws-green.md`](core-ops-dispatch-tests/ws-green.md)。
