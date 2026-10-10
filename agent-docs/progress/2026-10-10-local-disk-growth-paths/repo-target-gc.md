---
title: "/local disk growth paths: repo 直下 target の終端掃除（D2）"
tasks: [01M4HX54MZ1P6ZYZEB6ABHD6BV]
status: done
updated: 2026-10-10
---

# repo-target-gc: 終端 task の repo 直下 target を消す（ADR 2026-10-10-local-disk-growth-paths D2）

## 完了

- ADR が挙げた漏れを 3 つ直した。
  1. **木の判定**: `task_worker::workspace_targets::tree_hold` が checkout を持つ task と子孫（`parent_id` の鎖）を辿り、
     running の run（`running_run`）・終端でない子孫（`active_descendant`）・最後の終端から猶予内（`grace`）なら残す。
     `scan_finished_task_targets`（`found` / `held`）と既存 `finished_task_targets`、`workspace_prune::find_prune_candidates`
     （target/・node_modules 等を刈る別経路）が同じ判定を使う。親が終端でも子孫が親の checkout で走っていれば消さない。
  2. **cron 依存（既定値）**: cron の `target_sweep` が seed されていない本番では repo 直下 target が消えなかった。
     dispatcher の tick に `sweep_repo_targets`（`dispatcher/housekeeping.rs`）を足し、cron に依らず 600 秒ごと
     （`REPO_TARGET_GC_INTERVAL_SECS`、注入時計）に `[maintenance.target_sweep] workspace_target_after_hours`（既定 6 時間）で
     掃除する。tick 内は全 profile の `.cargo-lock` を持った rename まで、削除は別スレッド（1 本）。`--mode verify` では走らない。
     消した target ごとに `workspace_pruned` event を積む。
  3. **猶予の基準**: 猶予は親の終端時刻ではなく、木の最後の終端（子孫の `updated_at` の最大）から数える。
- 共通の I/O 層 `task_dispatch::target_sweep::repo_target_gc_move_aside`（rename まで・dry run 可）と
  `remove_moved_targets`（削除と `statvfs` 空きの前後差）を足し、cron の `sweep_workspace_targets` もこれを使う。
  `held` は report の `skipped` に理由付きで載る。
- 回収量: target ごとは `st_blocks × 512`（`tree_bytes`、du は使わない）、削除全体は workspace root の `statvfs` 空き差分
  （`statvfs_freed`。他の書込みも含む）を log に出す（D5）。
- cargo の lock 中（どれかの profile の `.cargo-lock` が `LOCK_EX|LOCK_NB` で取れない）は `build_in_progress` で残す（既存の規則）。

## 試験（接頭辞 `repo_target_gc_`）

| crate | 試験 | 固定すること |
|---|---|---|
| task-worker | `repo_target_gc_finished_task_target_is_removed` | 終端の木の target を消す（ソースは残る、量は st_blocks） |
| task-worker | `repo_target_gc_running_descendant_keeps_the_parent_target` | 稼働中の孫がいれば親の target を残す（workspace prune も）、最近終端の子孫は `grace` |
| task-worker | `repo_target_gc_cargo_lock_keeps_the_target` | cargo lock 中は残す |
| task-dispatch | `repo_target_gc_removes_the_target_of_a_finished_task` | done・cancelled の target を rename→削除、dry run は不変、statvfs 差分が取れる |
| task-dispatch | `repo_target_gc_keeps_the_target_while_a_descendant_runs` | 稼働中の子 → `active_descendant`、終端直後 → `grace`、注入時計で 7 時間後に消える |
| task-dispatch | `repo_target_gc_keeps_the_target_while_cargo_holds_the_lock` | lock 中は `build_in_progress`、放せば消える |
| task-dispatch | `repo_target_gc_tick_removes_a_finished_task_target_without_cron` | cron 無しで tick が消して `workspace_pruned` を積む |

全て一時 dir と in-memory DB だけで、時計は注入（`now` 引数）。CPU 負荷はかけない。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 対象試験 | `cargo nextest run -p task-worker -p task-dispatch -E 'test(/repo_target_gc_/)'` | 7 passed、exit 0 |
| 関連試験 | `cargo nextest run -p task-worker -p task-dispatch -E 'test(/repo_target_gc_\|workspace_target\|workspace_prune\|target_sweep\|tick_prunes/)'` | 41 passed |
| 2 crate 全体 | `TMPDIR=/tmp cargo nextest run -p task-dispatch -p task-worker --no-fail-fast` | 2032 passed、9 skipped（run の長い TMPDIR では launcher_run の Unix socket 試験が SUN_LEN で落ちる既知事象） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| fmt | `cargo fmt --all` | 差分適用済み |

workspace 全体の `bash scripts/dev/test-parallel.sh` は close-out 葉で取る。

## 未解決事項

- 既存の `01M4F3V643…` のような、cron 無効の本番に残った repo 直下 target は、本ブランチの release 後に daemon の tick が
  6 時間猶予の後に消す（人の手作業は不要）。本番 host には触れていない。
- 消した量を event に載せる欄は無い（`WorkspacePruned` は path だけ）。量は log（`blocks_bytes`・`statvfs_freed`）と cron の
  `target_sweep_ran` report に出る。API schema を変える葉で欄を足すかは提案。
- 配送前の候補記録（D2 の「配送前は候補を記録して安全性を検査」）は、終端判定と lock 保護で代える。done の木でも
  配送が未完了なら target を消すだけでソースは残るので、配送には影響しない。

## 提案

- `WorkspacePruned` に `bytes`（st_blocks）を足すと GUI から回収量が見える（API schema 再生成が要る）。
