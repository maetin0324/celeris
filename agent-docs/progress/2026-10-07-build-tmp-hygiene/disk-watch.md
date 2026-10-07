---
title: 葉 disk-watch — /・/local・/tmp の使用率がしきい値を超えたら通知・受信箱に出す（ADR 2026-10-07-build-tmp-hygiene D4）
tasks: [01M4BDG2M9EMS6GY526M36JNKX]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 disk-watch

## 実装（D4）

- **状態の表**: `crates/task-core/migrations/0058_disk_watch_state.sql`（`SCHEMA_VERSION` 57 → 58。既存試験の版数の assert も 58 に更新）。
  `crates/task-core/src/disk_watch.rs`: `DiskLevel`（ok / warn / critical / unavailable）・`DiskWatchState`・
  `DiskWatchStore`（`disk_watch_states` は読み取り接続、`disk_watch_apply` は状態の upsert と通知の記録を 1 transaction）。
  `TaskStore` の supertrait に追加。
- **通知の種類**: `NoticeKind::Disk`（`"disk"`、`crates/task-core/src/feed.rs`）。group_key `disk:<path>`。
- **受信箱の種類**: `InboxKind::DiskFull`（`"disk_full"`、`crates/task-ops/src/human_inbox.rs`）。`inbox::inbox` が
  `critical` の行を `Inbox.disk_full`（serde / schema は skip）に載せ、id は `disk_full-<slug>`（`disk_full_item_id`）。
  answer は `409 native_action_required`（`crates/task-api/src/inbox_notifications.rs`）。
- **判定と probe**: `crates/task-dispatch/src/disk_watch.rs`。`DiskProbe` trait（本物は `StatvfsProbe`）、
  使用率 `(blocks − bfree) / (blocks − bfree + bavail)`、`next_level`（以上で上げる・しきい値 −5 未満で下げる）、
  `evaluate`（純関数）、`run_disk_watch`、`DiskWatchRunner`（60 秒間隔。注入時計）。LLM は呼ばない。
- **tick の段**: `crates/task-dispatch/src/dispatcher.rs` の `prune_one_workspace` の後に `tick_disk_watch`
  （`accepting_new_work` のときだけ）。`Dispatcher::set_disk_watch` / `set_disk_watch_with_probe`（`dispatcher/maintenance.rs`）。
- **D1.5 の通知**: 保守 executor の target sweep が `over_cap_unresolved` なら `disk:target_sweep`（`target_sweep_notice`）。
- **config**: `[[maintenance.disk_watch]]`（`crates/celeris/src/config/maintenance.rs`、既定 `/`・`/local`・`/tmp`、80 / 95）、
  `Config::disk_watch_entries()` を bootstrap と reload（admin）が渡す。`config/celeris.example.toml` に既定値どおりの 3 件。
- **生成物**: `docs/api/v1/api-v1.schema.json`（`InboxKind` に `disk_full`、`NoticeKind` に `disk`）、`gui/app/celeris/types.ts`、
  `web/api/generated/{schema.json,types.ts}`。Event の種類は増やしていないので `event-kinds.ts`・`invalidation-map.ts` は変更なし。
- **web**: `web/features/inbox/inbox-model.ts`（`disk_full` の表示名・専用画面の種類・行き先 `/daemon`）、
  `web/features/notifications/notice-view.ts`（`disk` = 「ディスク」warning）。
- **文書**: ADR の付記（実装の具体）と状態、`docs/architecture-map.md` に 1 行。

## 試験（`disk_watch_`、13 件）

- task-dispatch `disk_watch/tests.rs`（10）: used_pct_excludes_root_reserve / notifies_once_on_crossing_warn（しきい値超え・重複抑止）/
  renotifies_warn_once_per_24h / critical_goes_to_inbox（受信箱段・通知なし）/ inbox_item_clears_when_below_critical（解除）/
  hysteresis_suppresses_flapping / unavailable_path_recorded_once / skips_writes_for_small_changes / runner_measures_every_60s /
  target_sweep_over_cap_notice
- task-dispatch `dispatcher/tests/cleanup_and_disk.rs`（1）: disk_watch_tick_phase_notifies_and_opens_inbox_without_stopping_runs
  （注入 probe・注入時計で tick を通し、run を止めないことも確かめる）
- celeris `config/maintenance_tests.rs`（2）: disk_watch_config_defaults_and_overrides / disk_watch_config_rejects_invalid_entries
- 偽の `DiskProbe` と引数の時計だけを使い、実 filesystem・sleep・負荷は使わない（ADR-0125）。

## 証拠

| コマンド | 結果 |
|---|---|
| `cargo test --workspace disk_watch_`（acceptance 0 の形） | exit 0、`disk_watch_` の ok 13 件 |
| `cargo clippy --workspace -- -D warnings` | exit 0、警告なし |
| `bash scripts/dev/test-parallel.sh` | exit 0。nextest 4710 passed / 0 failed / 13 skipped（binaries 160）、doctest exit 0、tmp_leftovers 0 |
| `UPDATE_SCHEMA=1 cargo test -p {task-core,task-api,task-worker} --lib` | schema は api-v1.schema.json だけ変化（enum 2 値の追加） |
| `pnpm -C gui gen:types`・`pnpm -C web gen:types`・`pnpm -C web typecheck`・`pnpm -C gui typecheck` | exit 0 |
| `pnpm -C web exec vitest run features/inbox features/notifications api` | 140 passed |

## 未解決事項

- 本番での有効化は daemon の次の release から（既定で `/`・`/local`・`/tmp` を監視する）。本番の設定・daemon には触っていない。
  監視を止めたいときは `~/.config/celeris` の設定に `[maintenance]` `disk_watch = []` を書いて reload（人の操作）。
- GUI（gui/）の受信箱は旧来の `GET /inbox` を使っており、`disk_full` は web（新 SPA）の受信箱にだけ出る。
- 存在しない path は tracing の warn と状態の行（`unavailable`）だけで、人への通知は出さない（ADR D4.2 の「1 回だけ記録」の解釈）。

## 提案

- ADR-0133 の D1 対応表・D3.1 の種類一覧（9 種・14 種）に `disk`・`disk_full` を書き足す（今は build-tmp-hygiene の付記にだけある）。
