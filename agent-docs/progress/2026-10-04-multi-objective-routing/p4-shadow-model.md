---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: shadow-model
status: done
completed: 2026-10-05
---

# Phase 4 shadow-model: task-core の shadow 型・event・UTC 日次の共有予約 store（0049）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§10 Phase 4 の task-core 部分。
詳細は同 ADR の「付記（2026-10-05、Phase 4 shadow-model unit の実装済み範囲）」。

## 変更
- `crates/task-core/src/model_router/shadow.rs`（+ `shadow/tests.rs`）: `ShadowPolicy`（既定 off、validate、admit、安定 hash 標本化）、`ShadowAllowlist`、`ShadowRecord`（kind/status/reason、出力は SHA-256 と tokens のみ）、予約の入出力型、`utc_day`。
- `crates/task-core/src/model.rs`: `Event::RoutingShadowRecorded`（wire `routing_shadow_recorded`、追記専用。既存 event は不変）。
- `crates/task-core/migrations/0049_routing_shadow_budget.sql` と `store/routing_shadow.rs`（+ `routing_shadow_tests.rs`）: BEGIN IMMEDIATE の予約・確定・日次消費。SCHEMA_VERSION 48→49。
- 0049 の空き確認: 着手時に全 ref（1268 本）の `crates/task-core/migrations/` を走査し 0049 は無かった。
- 旧版 DB を作る既存試験（cluster_job・cron の巻き戻し SQL、`SCHEMA_VERSION, 48` の固定、delivery の版数列）を 49 に追従。
- `docs/api/v1/event.schema.json`・`api-v1.schema.json` 再生成。

## 証拠
- `cargo nextest run --no-fail-fast -p task-core -E 'test(routing_shadow_opt_in_caps_survive_restart_and_handoff)'` → 1 passed
- `cargo nextest run --no-fail-fast -p task-core` → 745 tests run: 745 passed
- `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、4001 tests run: 4001 passed, 12 skipped

## 後続への引継ぎ
- `crates/task-api/src/query.rs`（`event_type_name` の網羅 match と `EVENT_TYPES`）と `crates/task-api/src/query/tests.rs`（件数 assert）は event 追加に伴う必須の追従。再計画で範囲 check の allow に両 file が追加され、`CELERIS_WU_BASE` からの全差分の範囲 check は exit 0。
- ADR §6 の表は `routing_shadow_evaluated` と書くが、Objective の `routing_shadow_recorded` を採った（ADR 付記に記録）。
- gui/web の生成型（`gui/app/celeris/types.ts` など）は未更新（gui/web unit の担当）。

## 提案
- Event に variant を足す葉の範囲 check には、最初から `crates/task-api/src/query`（`EVENT_TYPES` と `event_type_name`）を許可する。
