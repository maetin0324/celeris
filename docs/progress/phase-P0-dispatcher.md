# Phase P0: dispatcher.rs の責務分割（ADR-0082）

---
tasks: [01M3QEQPP31ZB29RH6YGFGTAPF]
---

## 完了 2026-09-29（worktree、main 未 merge）

`crates/task-dispatch/src/dispatcher.rs`（test 外出し後 17,224 行）の production を `dispatcher/` の子モジュール 18 本へ移し、
`Dispatcher` を facade にした（2,430 行）。移動だけで、関数の本体・文字列・イベント・公開パス・schema・設定は変えていない。
module map は `dispatcher.rs` の module doc にある。

| 段 | 移したモジュール | dispatcher.rs の行数（後） |
|---|---|---:|
| s1 L1 前半 | `cluster`・`housekeeping`・`snapshot`・`quota_book` | 14,751 |
| s2 L1 後半 | `provider_select`・`workspaces`・`run_context`・`worker_task` | 12,406 |
| s3 L2 | `work_units`・`tree_units`・`child_tasks`・`review_spawn` | 8,878 |
| s4 L3 前半 | `worker_finish`・`planner_flow`・`leases` | 5,499 |
| s5 L3 後半・L4 | `review_verdict`・`phase_integration`・`dispatch_run`、module map | 2,430 |

facade に残したもの: `Dispatcher` struct と private な補助型、公開の設定型、`new` と setter/getter、`tick`（段階の順序はそのまま、
`disk_ready` は drain より前）・`drain_completions`・`dispatch_ready`・`is_idle`、`StoreSink` / `ReviewerSink`（browser ブランチの統合まで）、
`mod` 宣言と明示した `pub use`。

### 証拠（s5）

- `cargo test -p task-dispatch` → 作業前 450 + 4 + 0 passed、作業後 450 + 4 + 0 passed（件数一致）。
- `cargo test --workspace` → exit 0、2,886 passed、0 failed（95 test binary）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt -p task-dispatch -- --check` → exit 0。
- 移動の検査: `0e7627b..HEAD` の差分で、足した行と消した行を空白と `pub(super)` を除いて比べ、違いは mod 宣言・module doc・rustfmt による署名の折り返し 3 か所だけ。
- 本番コードの直接の `OffsetDateTime::now_utc()` は 71 か所のまま。新しい `pub` 項目・trait・glob 再公開は無し。

### 未解決・提案

- 巨大関数（`finish_worker_result`、`dispatch_one`、`on_planner_finished`、`reconcile_tree_units`）の中身の分割は P1（audit §6.1）。
- `StoreSink` / `ReviewerSink` の `sinks` への移動は browser ブランチの統合後。
- `cargo doc -p task-dispatch` の既存の警告 5 件（unresolved link など）は分割前からあり、今回は触っていない。
