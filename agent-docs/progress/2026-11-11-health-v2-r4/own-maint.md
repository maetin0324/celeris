---
title: "健全性v2 R4 own-maint: disk・scratch・target の保守を HostMaintenance へ移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: completed
updated: 2026-11-12
---
# 健全性v2 R4 own-maint: disk・scratch・target の保守を HostMaintenance へ移す

完了日: 2026-11-12

## 概要

- `Dispatcher` の 10 field（`target_sweep`・`disk_watch`・`target_sweep_scope`・`disk_low`・`disk_ready`・`removing_build_caches`・
  `repo_target_gc_at`・`removing_repo_targets`・`scratch`・`task_workspaces`）を `crates/task-dispatch/src/host_maintenance.rs` の
  `HostMaintenance` 1 個（`Dispatcher::maint`）に移した。field は private で、`Dispatcher` の側は method だけを呼ぶ。
  `pub struct Dispatcher` の field は 92 件から 83 件になった。
- `dispatcher/housekeeping.rs` の処理（disk の空きの検査と通知、scratch の semantic GC、seed の後始末、WU の target の掃除、
  中止された worktree の片付け、作業場所の prune、repo target の常設の掃除）と `dispatcher/maintenance.rs` の処理（保守 executor
  `target_sweep`、disk 監視の tick と `critical` の判定）は `impl HostMaintenance` に移し、旧い `impl Dispatcher` の本体は消した。
  `ScratchState`・`free_disk_mb`・`REPO_TARGET_GC_INTERVAL_SECS`・`fallback_sweep_roots` も `host_maintenance.rs` に移した。
- `dispatcher/housekeeping.rs` に残るのは `MaintenanceInputs` と実行中の task の集合を組んで owner を呼ぶ入口（`maintenance_parts`・
  `refresh_disk_ready`・`tick_build_caches`・`cleanup_cancelled_worktrees`・`tasks_in_flight`・`prune_one_workspace`・
  `sweep_repo_targets`・`scratch_active`）と、保守の状態を持たない処理（`verify_smoke_only`・承認と browser の wait の照合・通知 feed・
  検査と reviewer の `CARGO_TARGET_DIR`）だけ。`dispatcher/maintenance.rs` は setter と executor の入口だけ。
- `disk_ready` は ADR の表どおり `HostMaintenance` に置いた。`tick` は drain より前に `refresh_disk_ready` を呼び、他の module
  （`cron_tick`・`review_spawn`・`cos_chat`）は `maint.disk_ready()` で読む。
- 基準は `$CELERIS_WU_BASE`（WU の base）。

## 実装前 brief

1. 根本原因: 保守の状態 10 field が `Dispatcher` の field で、`dispatcher/` のどの module からも書ける（ADR D1・D2）。
2. 責務 owner: `crate::host_maintenance::HostMaintenance`（ADR D1 の表。`target_sweep`・`target_sweep_scope`・`task_workspaces` は
   objective に従って移し、ADR に付記した）。
3. 既存の再利用先: 処理は本体ごと移し、判定は変えない。試験は `dispatcher/tests/{cleanup_and_disk,build_cache,cron_jobs,
   git_workspace,target_env,work_units}.rs` をそのまま使う。注入時計は `Dispatcher::now_utc` の値を `MaintenanceInputs::now` で渡す
   （元の処理が `OffsetDateTime::now_utc()`・`SystemTime::now()`・`Instant::now()` を直に使っていた所はそのまま）。
4. 選択肢: (a) field を struct に包んで `self.maint.scratch` と書く（ADR で不採用の梱包だけ）。(b) 処理ごと owner に移し、owner の外の
   値を引数（`MaintenanceInputs`）で渡す。b を採った。`DispatchConfig` は `dispatcher` module の型なので、owner は値を個別に受ける
   （D2 の 3）。
5. 新しい状態・分岐・fallback・port: 無し。`MaintenanceInputs` は借用の束、`MaintenanceError` は既存の 2 つの失敗（store と scratch
   pool の lock）の名前で、入口が今と同じ `DispatchError` の variant に写す。`WorkspaceCleanup` は既存の match の 5 分岐の名前。
6. 撤去対象: `Dispatcher` の 10 field と初期化、`ScratchState` と `free_disk_mb` の旧い定義、`housekeeping.rs`・`maintenance.rs` の
   `impl Dispatcher` の本体、試験だけが使っていた `Dispatcher::check_disk_space`・`seed_housekeeping`、module map の `housekeeping`
   の旧い行。
7. 不変条件: 実行中の run と判定中の review の作業場所を消さない（試験 `a_cancelled_task_keeps_its_worktree_while_its_run_is_in_flight`
   と `plan_workspace_cleanup_never_touches_a_task_with_a_run_in_flight`）。構造は `structure_guard_host_maintenance` が確かめる。
   ほかの振る舞いは既存の試験（同じ assertion のまま owner を通す）。

## 実行中の run の作業場所を消さない条件

- 旧い `cleanup_cancelled_worktrees` は `self.running_for_task(id) > 0 || self.reviewing.contains_key(&id)` を見て飛ばしていた。
  今は入口の `Dispatcher::tasks_in_flight` が `running` の鍵の task と `reviewing` の鍵を集め、`HostMaintenance::cleanup_cancelled_worktrees`
  に渡す。owner は集合に入っている task の store を読まず（旧いのと同じ順序）、純粋関数 `plan_workspace_cleanup(in_flight, status)` が
  片付け方を決める。
- scratch の GC（`scratch_gc`）・WU の target の掃除・repo target の掃除・prune は、元から run の表を見ず、lease の file・WU と task の
  状態・target の flock で走っている run を守っている。この経路は変えていない。

## 試験の変更

- 足した試験 `structure_guard_host_maintenance`（`host_maintenance.rs`）: 不変条件は ADR D2 の 1〜3・7（field が private、
  `#[cfg(test)]` の field が無い、組み立て役の型を受けない、`dispatcher` module を使わない、field は 10 件）。既存の試験は振る舞いだけを
  見るので、field を `pub` に戻しても通る。
- 足した試験 `plan_workspace_cleanup_never_touches_a_task_with_a_run_in_flight`（`host_maintenance.rs`）: 走っている task は状態に
  依らず触らず、中止は消し、終端は残す。既存の cancel の試験は走っていない task だけを見る。
- 足した試験 `a_cancelled_task_keeps_its_worktree_while_its_run_is_in_flight`（`dispatcher/tests/git_workspace.rs`）: 終わらない run を
  dispatch し、worktree ができるのを待ってから task を中止し、入口 `cleanup_cancelled_worktrees` を呼んでも worktree と記録が残る。
  入口が `running` を渡し忘れても既存の試験は通るが、この試験は落ちる（下の証拠）。最初の版は後半で生きている run の worktree を
  消していて、test-affected の負荷の下で worker の準備と競合して 1 回落ちた。後半を外し、走っていない中止の削除は既存の
  `cancelling_a_task_removes_its_worktree_and_branch` と純粋関数の試験に任せた。
- 既存の試験は assertion を変えていない。`d.disk_low`・`d.scratch.view`・`free_disk_mb` の直接参照を owner の method
  （`#[cfg(test)]` の `disk_low()`、`scratch_view()`）と `crate::host_maintenance::free_disk_mb` に置き換え、`d.check_disk_space()`・
  `d.seed_housekeeping(..)` は試験の file 内の helper が `maintenance_parts` から owner を呼ぶ形にした。
- `host_maintenance.rs` の `target_sweep_fallback_roots_use_scratch_dev_targets_not_var_tmp` は `maintenance.rs` の試験をそのまま移した。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-dispatch -E 'test(/structure_guard|plan_workspace_cleanup|a_cancelled_task_keeps|cancelling_a_task|disk|seed_housekeeping|target_sweep|prune|repo_target|scratch|build_cache|dirty_worktree|emergency|cron_jobs|git_workspace|cleanup_and_disk|target_env|snapshot|cos_chat_launch/)'`:
  117 passed（試験の修正の前）。修正の後に `test(/a_cancelled_task_keeps|cancelling_a_task_removes|plan_workspace_cleanup|structure_guard_host/)`: 4 passed。
- guard を外した確認: 入口が `in_flight` の代わりに空の集合を渡すように一時的に書き換えると、
  `a_cancelled_task_keeps_its_worktree_while_its_run_is_in_flight` が「run が走っている間は中止でも worktree を消さない」で落ちた
  （1 failed）。元に戻した。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `python3 scripts/dev/check-architecture-map.py`: OK（996 件）。
- 計画の check の形: `pub struct Dispatcher` の本体に 10 field の名前の行が無い（grep exit 1）、`maint:` が 1 行、
  `host_maintenance.rs` に `impl|&mut|& Dispatcher` が無い、範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）の path はすべて
  task-dispatch・地図・ADR・この進捗の中。
- `bash scripts/dev/test-affected.sh`（1 回目、試験の修正の前の HEAD）: exit 100。nextest 3742 件中 3741 passed・1 failed
  （足した試験の後半。上の「試験の変更」）。repo_health_gate: all checks passed。2 回目の結果は下の追記。

## commit（件名）

- phase 1: disk・scratch・target の保守の状態と処理を HostMaintenance へ移す
- phase 1: HostMaintenance の地図・ADR の付記を足す
- phase 1: 実行中の run の worktree を消さない試験を決定的にする
- phase 1: own-maint の進捗を書く

## 未解決事項

- `health_inventory`・`script_compare` は ADR の表では host-maintenance だが、この unit の objective に無いので `Dispatcher` に残した。
- `task_workspaces` は ADR の表では `ExecutionCoordination`。objective に従って `HostMaintenance` に置いた（ADR に付記）。
  ExecutionCoordination を作る unit が移すかを決める。
- celeris の配線（`set_target_sweep`・`set_target_sweep_scope`・`set_disk_watch`）は `Dispatcher` の setter のまま。setter は
  `HostMaintenance` に渡すだけの入口。

## 提案

- 無し。
