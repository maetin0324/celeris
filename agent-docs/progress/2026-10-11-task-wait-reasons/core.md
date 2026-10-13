---
title: 待ちの理由 — 葉 core（task-core の待ち判定の共有と DaemonSnapshot.task_waits）
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# 葉 core: task-core の待ち判定関数を共有し、メモリの見送り理由を DaemonSnapshot に載せる

完了日: 2026-10-11

## したこと
- `crates/task-core/src/wait.rs`（新規）: ADR D2 の `TaskWait`（20 種）・`UnitWait`・`HaltState`・`HumanGateKind`・`CapacityScope`・`BackoffCause`、
  純粋関数 `halt_waits`（D1〜D4）・`db_waits`（D1〜D6）・`resolve_children`（D7 の `child:<key>` 解決）・`unit_waits`（D8/D9）・`plan_waits`（D7〜D11・D13）。
- `ready_tasks_impl` は行ごとに `db_waits_locked`（祖先・deps・親を読んで `wait::db_waits`）が空のものだけ返す。`halted_by_ancestry_locked` は
  `ancestors_locked`（祖先の行を集める）に置き換え、`halted_by_pause_impl` も `halt_waits` を使う。`halted_projects_locked`／`halted_milestones_locked` は id → `HaltState` の表を返す。
- `newly_ready`／`newly_ready_with` は `wait::UnitWaitJudge`（`unit_waits` と同じ判定）で「待ちが空の pending」を返す。`earlier_phases_done` は消した。
- 一覧・木の API 用に `SqliteStore::db_waits_for(&[Task])`（ready_tasks と同じ読み方。終端は空）を足した（api 葉が使う）。
- dispatcher: `resolve_child_dependencies` は `task_core::resolve_children` の `done`・`failed`・`waiting_units` を使う（`plan_waits` の `Children` と同じ関数）。
- dispatcher に `dispatch_waits`（tick ごとに clear）と `note_dispatch_wait` を足し、`dispatch_one`／`dispatch_ready` の見送りで理由を記録:
  全体・CoS の枠（M1）、窓・容量で照合しなかった候補（M9 → `Capacity{global}`。全枠が埋まって早期 return する tick も窓の ready に付ける）、
  provider が選べない（`unroutable` なら `Unroutable`、他は `Capacity{source}`）、cluster 設定なし（`Unroutable`）、cluster の cooldown・並列度・接続不通（`Cluster`）、
  write-set（`WriteSet`）、retry／infra のバックオフ（`Backoff`）、disk critical（`DiskCritical`）。
- `DaemonSnapshot.task_waits: Vec<SnapshotTaskWait{task_id, wait}>`（task_id 順・1 task 1 件）。`snapshot_task_waits` が `dispatch_waits` に
  M7（`awaiting_children` → `DelegatedChildren`、未完の子を store から読む）・M8（`awaiting_human` → `ReviewAwaitingHuman`）を足す。
  組み立て箇所 `crates/task-api/tests/common/mod.rs`・`crates/task-ops/src/inbox/tests.rs` も追従。
- schema 再生成: `UPDATE_SCHEMA=1 cargo test -p task-api --lib`（`docs/api/v1/api-v1.schema.json` に `task_waits`・`TaskWait` 等）。

## 証拠
- `cargo test -p task-core task_wait_` → exit 0、`test result: ok. 8 passed`
- `cargo test -p task-dispatch task_wait_` → exit 0、`test result: ok. 3 passed`
  （`task_wait_full_capacity_and_unroutable_skips_are_in_the_snapshot`・`task_wait_depends_on_and_paused_ancestor_are_not_dispatched`・`task_wait_parent_waiting_on_child_uses_the_shared_resolution`）
- `cargo test -p task-core`（ready_tasks 試験を含む全件）→ exit 0、`908 passed`（+ doc 等 1・3 passed）
- `cargo clippy -p task-core -p task-dispatch -p task-ops --all-targets -- -D warnings` → exit 0、警告なし。`cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/test-parallel.sh -E 'package(task-core) | package(task-dispatch) | package(task-ops) | package(task-api)'` →
  `3037 tests run: 3029 passed, 8 failed`。失敗 8 件はすべて task-api の browser 試験で、この変更と無関係の環境要因:
  `browser_live_cross` 3 件・`browser_waits` 1 件は unix socket の `path must be shorter than SUN_LEN`（run の `$TMPDIR` が長い）で、短い TMPDIR で再実行すると通る。
  `browser_e2e` 4 件は `timed out waiting for fixture readiness`（credential broker の fixture。worker sandbox で完走しない既知の型）。

## 未解決
- ADR D4 の「`DaemonSnapshot.unroutable`・`.awaiting_children`・`.awaiting_human` を消す」はこの葉では行っていない。読み手が gui の
  `daemon.tsx`・`graph.tsx`・`graph-layout.ts`（この葉の範囲外。api 葉の範囲も `gui/app/celeris/types.ts` だけ）で、消すと gui が壊れる。
  `task_waits` には同じ情報（`Unroutable`・`DelegatedChildren`・`ReviewAwaitingHuman`）が入っている。
- `docs/api/v1` の schema は再生成したが、web の生成型（`node web/scripts/gen-types.mjs`）は範囲外（web/）なので api 葉で再生成する。
- D12（人の gate）の `task_ops::wait::gate_waits` と D11 の `open_decision_keys` の導出（events を読む）は task-ops 側で api 葉が組む。
  `plan_waits` はその鍵を引数で受ける。

## 提案
- 旧 3 欄の削除と gui の読み替えは、gui を範囲に持つ葉（web-list か verify の前の 1 葉）に明示的に持たせる。
