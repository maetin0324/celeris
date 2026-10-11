---
tasks: [01M4MAZNNYT4JVRYKCVZ1TF1VF]
---
# API and CLI stranded integration surface

## Status

未完了。開始時の変更は `query/tests.rs` の既存型 serialization 試験と前回記録だけだった。`task-ops::stranded` に discard/hold/reopen/merge の操作はあるが、この WorkUnit の差分範囲は `task-api` と `celerisctl` が中心で、task list/detail は `task-ops::view::{TaskSummary,TaskDetail}`、受信箱の `AttentionItem::Unintegrated` も task-ops が組み立てる。状態と promotion を list/detail/inbox に一貫して載せるにはそれらの view 型変更が必要となる。

API handler の操作に必要な repository path/default branch/release directory を安全に得る wiring も未特定。既存の `changes` の統合経路は `task_integrations`（ADR-0043）で、今回の `task_integration_state` による git ancestry 判定・状態遷移とは別経路である。従って、この状態で表面だけ追加すると要求された意味の merge と誤認される危険があり、未実装のまま停止した。

## 確認

- `task-ops/src/stranded.rs`: 状態遷移と既存 integration request への merge route を確認。
- `task-ops/src/inbox.rs`, `human_inbox.rs`: Unintegrated item の生成と kind を確認。
- `task-ops/src/view.rs`: TaskSummary/TaskDetail の所有位置を確認。
- `celerisctl/src/main.rs`, `commands/cos_ops.rs`: API request は run credential 前提の別系統で、通常の integration CLI とは異なることを確認。
- `cargo test -p task-api -p celerisctl -- stranded_`: 前回は exit 0 だが対象試験が 0 件（test result の passed count が 0）。

## 未解決

- API の一覧・詳細・inbox status/promotion 欄と、操作/list endpoints。
- celerisctl integration list/merge/discard/hold/reopen と試験。
- schema/generated types と acceptance checks。
