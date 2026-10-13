---
title: 待ちの理由 — 葉 api（一覧・詳細・木の waits と schema・生成型）
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# 葉 api: task-ops / task-api で待ちの理由を出す

完了日: 2026-10-11

## したこと
- task-core: `TaskStore::db_waits_for`（trait。`SqliteStore` の実体は `db_waits_for_impl`）。
- task-ops `wait.rs`（新規）: `MemoryWaits::from_snapshot`（`last_tick_at` から `tick_ms × 10` 以内の snapshot だけ。古ければ空）、
  `task_waits`（DB `db_waits_for` → 計画 `task_core::plan_waits`〈子の事実・未回答の `needed_before: self` の決定 key〉→ 人の gate `gate_waits`〈blocked の task だけ events を読む〉→ メモリ）、
  `is_runnable`（`ready && waits 空`）。
- `TaskSummary`・`TaskDetail`: `waits`・`runnable`。`GET /tasks?run_state=runnable|waiting`（waits を組んでから絞り、cursor は `rs:<位置>`。不明値は 400）。
  `GET /tasks/{id}/execution`（`TaskExecutionView`）にも `waits`。
- `TaskTreeNode`: `waits`・`stages`（`TreeStageView{key, order, state: done|current|pending, integrate_key}`）。
  `TreeUnitView`: `seq`・`depends_on`・`waiting_on`（`task_core::unit_waits`）・`waiting_stage`。unit は seq 順。
- 消した旧欄（ADR D6）: `TaskSummary.pending_children`、`ExecutionPhase::AwaitingChildren`・`ExecutionView.awaiting_children`・`AwaitedChildView`・
  `TaskExecutionView.awaiting_children`、`TreeNodePhase::{AwaitingChildren, HeldOnDecision, BlockedInfra}`、`view.rs::awaits_only_children`。
  参照の追従: celerisctl（`task_detail` の引数）、web `execution-panel.tsx` のラベルと e2e fixture、gui `tree.ts`（`treeNodeHold` は `waits` の
  `human_decision`／`units_blocked(infra)` から）・`task-execution.ts`・test fixture（JSON と TS）。
- schema・生成型: `UPDATE_SCHEMA=1 cargo test -p task-api --lib`、`node web/scripts/gen-types.mjs`、`node gui/scripts/gen-types.mjs`（`gui/app/celeris/types.ts`）。追跡 API 文書 `docs/api/v1/gui-api.md` と `gui/docs/celeris-api-v1.md` の旧 `pending_children`／Execution の `awaiting_children`／旧 TreeNodePhase の説明も更新。

## 証拠
- `cargo test -p task-api --test task_wait_api` → 6 passed（一覧の子待ち・depends_on 待ち・paused・snapshot の枠待ち／run_state／古い snapshot／詳細／木の WU 待ち key と段の順／子節点の waits）
- `cargo test -p task-ops --lib task_wait_api` → 2 passed
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh -E 'package(task-core) | package(task-ops) | package(task-api) | package(task-dispatch) | package(celerisctl) | package(celeris-mcp)'` → exit 0、`3257 tests run: 3257 passed, 3 skipped`
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `git diff --check`・`sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0（今回の追加差分は API 文書 2 件のみ）
- `node web/scripts/gen-types.mjs --check`・`node gui/scripts/gen-types.mjs --check` → exit 0
- web: `pnpm typecheck` exit 0、`vitest run` → 92 files / 676 passed
- gui: `pnpm typecheck` exit 0、`vitest run` → 1342 中 browser-phase2 の 1 件だけ失敗（run の長い TMPDIR で unix socket が作れない。`TMPDIR=/tmp` で 19/19 passed）

## 未解決
- `DaemonSnapshot.unroutable`・`.awaiting_children`・`.awaiting_human` の削除（ADR D4）は未了。読み手が gui `daemon.tsx`・`graph.tsx`・`graph-layout.ts`・web `daemon-screen.tsx` で、
  この葉の範囲（task-ops/task-api と生成型）を越える。`task_waits` に同じ情報がある。
- D12 の `gate_waits` は task-ops にあるが、dispatcher の `human_gate_hold`（段の `review: human`）はまだ共有していない（StageReview は出していない）。
- 旧 GUI（gui/）の木は子待ちのラベルを出さなくなった（`phase` から外したため）。web の木は web-tree 葉が `waits` で出す。

## 提案
- 旧 3 欄の削除と gui・web daemon 画面の読み替えは web-list か verify の前の 1 葉に明示的に持たせる。
