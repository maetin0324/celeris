---
title: task 一覧 API の遅さ: task_integration_list_impl を 1 本の SELECT に（WU store-select）
updated: 2026-10-21
tasks: [01M5EDRV4B2K5NYPMF9EZ0YN9A]
status: done
completed: 2026-10-21
---
# WU store-select

ADR: `agent-docs/adr/2026-10-21-task-list-opt-in-integrations.md` D2。

## 実施
- `crates/task-core/src/store/task_integration_state.rs` の `task_integration_list_impl` を、id 一覧 → 1 行ずつ get の N+1 から、全列を取る 1 本の SELECT に変えた。並び順（updated_at DESC）・エラー表現は従来どおり。
- 試験 `task_integration_list_is_one_read_and_keeps_order_and_content`: 7 行で順序・内容が期待どおり、`lock_counts()` の差分が 1（N に比例しない）、state 絞り込みも確認。

## 証拠
- `cargo test -p task-core task_integration` → 2 passed。
- `cargo clippy -p task-core -- -D warnings`、`cargo fmt --all -- --check` は下記の結果（done 報告参照）。

## 本番への移行
config・DB schema の変更なし。

## 未解決・提案
なし。
