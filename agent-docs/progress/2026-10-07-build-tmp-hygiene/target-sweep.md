---
title: 共有 cargo target の定期掃除（古さ・上限・build 中は消さない）
tasks: [01M4B6W2ZGFP9V4QTC3FJ78XRF]
status: done
updated: 2026-10-07
completed: 2026-10-07
---

# target-sweep（ADR 2026-10-07-build-tmp-hygiene D1）

## 検証（統合後 HEAD 7bdb0232）

| コマンド | 結果 |
|---|---|
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4687 passed / 0 failed / 14 ignored（binaries 160）、doctest exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0、警告なし |
| `cargo nextest run --workspace -E 'test(target_sweep_)'` | 26 tests run: 26 passed |

`target_sweep_` 試験の所在（26 件）:
- task-worker `target_sweep/tests.rs`（9）: params_default_values / deletes_items_older_than_max_age / trims_oldest_until_80pct_of_cap / cap_is_not_applied_under_cap / skips_locked_profile_in_plan / groups_deps_by_key / stale_target_dir_whole / rejects_outside_root_and_symlink / plan_serializes_to_json
- task-dispatch `target_sweep/tests.rs`（6）: skips_profile_with_held_cargo_lock（実 flock）/ dry_run_deletes_nothing / apply_removes_old_items_and_reports_bytes / apply_reports_real_block_bytes / never_touches_live_release_lease / cleans_leftover_deleting_dirs
- task-dispatch `dispatcher/tests/cron_jobs.rs`（2）: cron_fires_deterministic_executor_and_records_event / cron_rejects_unknown_action
- celerisctl `commands/target_sweep_tests.rs`（3）: cli_dry_run_json_deletes_nothing / cli_apply_with_root / cli_default_roots
- celeris `config/maintenance_tests.rs`（4）・`config/cron_tests.rs`（1）: config 既定/上書き/不正値/example toml、cron seed の検証
- task-core `model.rs`（1）: target_sweep_event_round_trips

## 葉の記録（索引）

- [sweep-event](target-sweep/sweep-event.md) — event `TargetSweepRan`、schema・web/gui 種類表
- [sweep-plan](target-sweep/sweep-plan.md) — 純粋な計画関数
- [sweep-io](target-sweep/sweep-io.md) — I/O 層
- [sweep-cli](target-sweep/sweep-cli.md) — `celerisctl target sweep`、release.sh
- [sweep-cron](target-sweep/sweep-cron.md) — cron executor、`[maintenance.target_sweep]`

## ADR D1 と実装・試験の対応

| ADR | 実装位置 | 試験 |
|---|---|---|
| D1.1 roots・項目種別 | `crates/task-dispatch/src/target_sweep.rs`（走査）、`crates/celeris/src/config/maintenance.rs`（roots 既定） | dispatch: apply_removes_old_items…、never_touches_live_release_lease。worker: groups_deps_by_key。config: maintenance_tests |
| D1.2-1 古さ | `task_worker::target_sweep::plan` | deletes_items_older_than_max_age |
| D1.2-2 上限 | 同 plan | trims_oldest_until_80pct_of_cap、cap_is_not_applied_under_cap |
| D1.2-3 放置 target dir | 同 plan | stale_target_dir_whole |
| D1.2-4 build 中は消さない・rename→削除 | `task_dispatch::target_sweep`（`.cargo-lock` 非 blocking flock、`.deleting-*`） | dispatch: skips_profile_with_held_cargo_lock、cleans_leftover_deleting_dirs。worker: skips_locked_profile_in_plan |
| D1.2-5 over_cap_unresolved | plan の出力欄（D4 の `disk` 通知は disk-watch 葉の範囲） | worker: plan_serializes_to_json（欄の出力） |
| D1.2-6 symlink・root 外 | plan の入力検証 | rejects_outside_root_and_symlink |
| D1.3 純粋/I/O 分離 | `task_worker::target_sweep` / `task_dispatch::target_sweep` | worker 9 件（時計・大きさ注入）、dispatch 6 件 |
| D1.4 cron | `crates/task-dispatch/src/dispatcher/maintenance.rs`、`config/celeris.example.toml` の seed `target-sweep`（enabled=false） | cron_fires_deterministic_executor…、cron_rejects_unknown_action、config cron_tests |
| D1.4 手動・release.sh | `crates/celerisctl/src/commands/target_sweep.rs`、`scripts/selfdeploy/release.sh:564` | cli_* 3 件 |
| D1.5 記録 | event `TargetSweepRan`（task-core model.rs、task-api query.rs） | target_sweep_event_round_trips、cron の記録試験 |

## 未解決事項

- **有効化は人**: cron seed は `enabled = false`。人が先に `celerisctl target sweep --dry-run` で計画を確かめ、問題なければ `celerisctl cron resume target-sweep` で有効化する（本番設定・daemon は この run では触っていない）。
- `release.sh` の sweep は次回 release から効く（既存 release の release.sh は旧版）。
- `celerisctl target sweep` の `--root` 省略時に config の roots を使う配線（`Config::load(..)?.target_sweep_params()`）が未完の可能性がある。sweep-cron の記録参照。`cli_default_roots` は既定 roots 前提。
- sweep-io: 走査と lock の間に cargo が使った項目が古い時刻のまま消えうる（cargo が作り直すだけ）。`remove_dir_all` は同期実行。
- `over_cap_unresolved` の D4 `disk` 通知は disk-watch 葉で。

## 提案

- ADR D1.1 の使用時刻の記述に「dir は mtime のみ（走査の read_dir が atime を進めるため）」を付記する（親の close-out）。ADR の状態行は変えていない。
- `TargetSweepRan` に失敗理由（`errors_head`）を足し、failed の理由を GUI で読めるようにする。
- cron 側の削除を別スレッドに逃がす（rename までは lock 保持のまま）。
- plan に root 内の項目以外の量を渡し、上限判定を root の実使用量に揃える。
