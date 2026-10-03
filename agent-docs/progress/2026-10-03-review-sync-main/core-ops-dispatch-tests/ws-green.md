---
title: workspace 統合 gate の sandbox 結果（ws-green）
tasks: [01M40P9SXYB9K2WCMPH9WDNVZK]
status: done
updated: 2026-10-03
---
# workspace 統合 gate の sandbox 結果（ws-green）

## 原因と修正

- repo 根の `index.json` は `1ad55a95`（task-dispatch 個別試験）の実行時刻 `2026-10-03T11:10:55.915301062Z` で追加された生成物だったため削除した。`run_context` は `task_ops::knowledge::ensure_index(root)` に明示的な knowledge root を渡している。cwd への書き込みは見つからず、dispatcher 試験からの直接生成を示す証拠も無い。既知の knowledge index writer は `crates/task-ops/src/knowledge.rs` の `write_index(root, ...)` である。現状では dispatcher harness/tooling の実行副産物と判断する。
- `crates/celeris-credentiald/tests/broker.rs` の `daemon_rejects_worker_secret_retrieval_even_with_valid_lease` が起動する 3 つの子 process（`serve` daemon、`python3` 直結 worker、`bridge`）すべてに `.env_clear()` を追加し、試験用の一時 `HOME`/`XDG_RUNTIME_DIR`（と `python3` には `PATH` のみ）を渡した。worker sandbox が継がせる `CELERIS_CREDENTIALD_DATA_DIR` などを子が読むことを防ぐ。`initialize_key` の assert は維持し、失敗時に `code` を表示する。修正文面は兄弟 commit `b466b3c1` と diff 単位で一致（前回 run は serve 子の hunk のみ適用で終えており、python3 worker と bridge の hunk が未適用だったため本 run で追加した）。

## 検証結果

| コマンド | exit | 結果 |
|---|---:|---|
| `cargo test -p celeris-credentiald --test broker daemon_rejects_worker_secret_retrieval_even_with_valid_lease -- --exact` | 0 | 1 passed、0 failed、11 filtered out |
| `cargo test --workspace` | 0 | 全 126 試験バイナリ ok（5m35s）。`e2e` crate `tests/cluster_scenarios.rs` 6 件も含め全 passed（前回 run は hunk 未適用のまま観測した db_guard user namespace probe の `Operation not permitted` 失敗が今回は再現せず、本 run の全 env_clear 適用後は安定して green） |
| `cargo clippy --workspace -- -D warnings` | 0 | warning なし |
| `git diff --check` | 0 | whitespace error なし |

対象 3 crate の個別試験結果は [`core-ops-dispatch-tests.md`](../core-ops-dispatch-tests.md) と各 `t-core.md` / `t-ops.md` / `t-dispatch.md` に記録済みで、すべて exit 0。本 run で `cargo test --workspace` と `cargo clippy --workspace -- -D warnings` がどちらも exit 0 になったことを確認し、未解決事項なし。

## 証拠コマンド

```sh
git show 1ad55a95 -- index.json
git show b466b3c1 -- crates/celeris-credentiald/tests/broker.rs
cargo test -p celeris-credentiald --test broker daemon_rejects_worker_secret_retrieval_even_with_valid_lease -- --exact
cargo test --workspace
cargo clippy --workspace -- -D warnings
git diff --check
```
