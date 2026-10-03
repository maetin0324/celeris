---
task: review-sync-fix
wu: sync-tests
status: done
completed: 2026-10-03
---
# sync-tests: 並列 2 task の無衝突完了と merge 履歴の解決保持の試験、review_verdict の expect 除去

- `crates/task-dispatch/src/dispatcher/tests/target_sync.rs`（既存 module。`tests/mod.rs` は不変）に 2 試験を追加。
  - `target_sync_two_tasks_disjoint_both_land`: 同じ base から A（`a.txt`）と B（`b.txt`）の worktree を切る。A が review → Done → main を reviewed SHA へ ff（配送と同じ ff-only）。その後 B が review に来ると前進した main へ同期され、reviewed SHA が main（= A）を祖先に持ち、`ReviewTargetSynced` の target/reviewed/merge candidate が一致、IntegrationRepair（event・WU）なしで Done。B を ff で入れた main に `a.txt` と `b.txt` の両方がある。
  - `target_sync_merge_history_keeps_resolutions`: task branch に WU branch を `--no-ff` で取り込み README.md の衝突を手で解いた merge commit を作る。main が別ファイル（`main.txt`）で進んだ後、素の rebase なら衝突が再発することを別 worktree で確かめたうえで review 前同期を走らせる。merge commit が reviewed SHA の祖先に残り、target も祖先、README.md は解決済みの内容、`IntegrationRepairScheduled` も WU も無く、review を経て Done。
  - 時間依存は既存の `run_until_idle`（tick を回して idle という出来事を待つ）だけで、sleep による判定はない。
- `crates/task-dispatch/src/dispatcher/review_verdict.rs`: `work_units.last().expect("repair row")` を let-else に置き換え、行が無ければ `StoreError::Invalid` を返す。ファイル内の `expect(` は 0 件。

## 証拠
- `cargo test -p task-dispatch --lib target_sync` → 20 passed / 0 failed（新しい 2 件を含む）
- `cargo test --workspace` → exit 0、3763 passed / 0 failed
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all --check` → exit 0
- `git diff --quiet 21ba1b45 -- crates/task-dispatch/src/dispatcher/tests/mod.rs` → exit 0（不変）

## 未解決
- 試験 1 は B が review に来る前に B を store に入れない（tick の `recover_reviews` が A の着地前に B を review しないため）。worktree と commit は A の着地前に作っている。
- dispatcher は main へ着地させない（配送は crates/celeris）。試験では `git merge --ff-only <reviewed>` で着地を模した。

## 提案
- なし
