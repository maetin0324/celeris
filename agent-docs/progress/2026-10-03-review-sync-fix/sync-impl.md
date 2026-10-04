---
task: review-sync-fix
wu: sync-impl
status: done
completed: 2026-10-03
---
# sync-impl: merge commit を含む branch の review 前同期

- `task_ops::changes::sync_onto_target`: `target..HEAD` に merge commit があれば `git merge --no-ff --no-edit <target_sha>`、無ければ従来の rebase。衝突は `merge --abort` して `Conflict`（戻せなければ `Failed`）。同期後に target が HEAD の祖先であることを検査。
- `SyncOutcome::Rebased` に `method: SyncMethod { Rebase, Merge }` を追加。`task-dispatch/src/dispatcher/review_spawn.rs` は `..` を足しただけ。
- 試験: `sync_onto_target_keeps_merge_history`、`sync_onto_target_linear_branch_still_rebases`、`sync_onto_target_merge_conflict_restores_the_original_head`。
- ADR-0118 末尾に付記（2026-10-03）。merge を選んだ理由（`--rebase-merges` は解決を再適用できず rerere 依存・SHA が変わる）。

## 証拠
- `cargo test -p task-ops` → 480 passed / 0 failed
- `cargo test -p task-dispatch` → 586 + 4 passed / 0 failed
- `cargo clippy --workspace --all-targets -- -D warnings` → pass
- `cargo fmt --all -- --check` → exit 0

## 未解決
- `review_spawn.rs` の衝突メッセージは "rebase onto ..." のまま（merge 経路の衝突でも同じ文言）。
