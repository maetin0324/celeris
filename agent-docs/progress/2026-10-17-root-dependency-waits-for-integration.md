---
tasks: [01M54YWKAZMZYQTHQRMT14RSBY]
status: done
completed: 2026-10-17
---
# root 間 depends_on は依存先の取り込みまで待つ

ADR: [2026-10-17-root-dependency-waits-for-integration](../adr/2026-10-17-root-dependency-waits-for-integration.md)

## 実装
- `task-core/wait.rs`: `TaskWait::DependsOnIntegration { task_id, cause, detail }` と `IntegrationWaitCause`。
- `task-ops/dep_integration.rs`: 依存先 branch の既定 branch 祖先判定（git のみ）と見送り・保留・破棄の理由。
- `task-dispatch/dispatcher.rs`: `dispatch_ready` が候補ごとに判定し、待ちを snapshot の `task_waits` に載せる。祖先と分かった依存先は記憶。
- `task-ops/inbox.rs`: `DeliverySkipped` 項目の summary に、待っている依存元の task id を足す。
- schema・web 生成型・`web/features/tasks/wait-reason.tsx`・`docs/api/v1/api.md` を更新。

## 証拠
- `cargo test -p task-dispatch dep_integration`: 3 件合格（未取り込みで待つ→merge 後に dispatch、delivery_skipped の理由、木の子は対象外）。
- `cargo test -p task-ops delivery_skip_attention_names_waiting_dependents`: 合格。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0（5701 passed / 0 failed）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。

## 未解決・提案
- web の tsc は node_modules が無く未実行（型は gen-types の出力と手書きの switch のみ）。
- 本番への移行は不要（DB・config 変更なし）。
