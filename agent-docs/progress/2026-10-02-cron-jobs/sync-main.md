---
title: cron-jobs sync-main — main 取り込みと migration 0046 への振り直し
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
status: done
updated: 2026-10-03
---
# cron-jobs sync-main — main 取り込みと migration 0046 への振り直し

merged-main: bbe4a09d3d525fb97c35076d0acd18e5f900eb1c

final review の不合格（main を祖先に含まない・merge-tree で衝突・migration 番号が main の 0041 より小さい）を
直すため、手元の main（fetch はしない）を `git merge --no-ff main` で task branch（base `4fb3c5e7bb96`）へ
取り込んだ。

## 衝突ごとの解き方

| file | 種類 | 解き方 |
|---|---|---|
| `crates/celeris/src/config/mod.rs` | 内容 | 両側を残す（このブランチの `cron: CronConfig` と main の `storage: StorageConfig`）。 |
| `crates/celerisctl/src/commands/worker_tests.rs` | 内容 | 両側を残す（`cron` と `storage` の `Default::default()`）。 |
| `crates/task-core/src/cluster_job/tests.rs` | 内容 | `SCHEMA_VERSION` の期待を 46 に。schema 33 へ戻す SQL から 0038 の列・index の削除を外した（0038 を消したため）。 |
| `crates/task-core/src/store/migrations.rs` | 内容 | main の `MIGRATION_0041` を残し、cron を `MIGRATION_0046` に。`MIGRATION_0038` は削除。`RESERVED_VERSIONS` を `[38, 39, 40, 42, 43, 44, 45]`、`SCHEMA_VERSION = 46`。 |
| `crates/task-core/src/store/tests.rs` | 内容（8 箇所） | 版数の期待を 39 / 41 から 46 に。 |
| `docs/architecture-map.md` | 内容（2 箇所） | main の行（`../agent-docs/adr/` へのリンク）を採り、このブランチの cron の行（実装済みの module に更新）と Knowledge GC 行の ADR-0131 D6 の参照を足した。ADR-0131 へのリンクは移動前の `adr/0131-cron-jobs.md` のまま（移動は docs-move 葉）。 |
| `tests/e2e/tests/phase7_scenarios.rs` | 内容（3 箇所） | main 側を採る（main は ADR-0131 D7 の failed cancel を含み、cancelled の再 cancel 拒否も確かめる。このブランチの版の上位互換）。 |
| `docs/PROGRESS.md` | modify/delete | main は ADR-0128 で `agent-docs/PROGRESS.md` に移し済み（main に `docs/PROGRESS.md` が無いので `sh scripts/dev/progress-transition.sh main` は何もしない）。このブランチが足した 3 節を `agent-docs/progress/2026-10-02-cron-jobs.md` へ移し、`docs/PROGRESS.md` は削除。 |

## 衝突以外の変更

- migration: `0038_work_unit_sessions.sql`（他ブランチの写し）を削除。`0039_cron_jobs.sql` を `0046_cron_jobs.sql`
  へ `git mv`。0042〜0045 は review-sync 系ブランチが使う。
- `crates/task-core/src/cron/store_tests.rs`: 版数 37 へ戻す SQL から 0038 の列削除を外し、通知の表（0041）を落とす形に。期待版数 46。
- `crates/celeris/src/daemon/tick_loop.rs`: 自動 merge の結果 `notify_started_at` が未使用になった（このブランチで
  `knowledge_maint::schedule` の引数から外し、main で `notify::schedule` が `schedule_routes` に替わった）ので削除。
- `docs/progress/phase-cron-jobs.md` の内容を `agent-docs/progress/2026-10-02-cron-jobs.md` に移し、旧 file は削除。
  `docs/progress/cron-jobs-curation-dry-run.md` はこのブランチ（base `4fb3c5e7`）に存在しないため移動対象なし
  （並行の dry-run 葉の成果。統合時に `agent-docs/progress/2026-10-02-cron-jobs/dry-run.md` へ寄せる）。
- `docs/ops/cron-jobs.md` の migration 名・版数を 0046 / 46 に。
- schema: `cargo test -p task-api` が通ったので `UPDATE_SCHEMA=1` の再生成は不要（gui/web の生成物は main の版のまま）。
- ADR-0131 本文の `0039_cron_jobs.sql` の記述と ADR の `agent-docs/adr` への移動は docs-move 葉の仕事なので触れていない。
