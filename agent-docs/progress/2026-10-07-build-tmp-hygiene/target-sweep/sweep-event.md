---
title: sweep-event（Event::TargetSweepRan の追加と種類表の更新）
tasks: [01M4B6W2ZGFP9V4QTC3FJ78XRF]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# sweep-event

- `task-core` の `Event::TargetSweepRan`（mode, roots, skipped, skipped_total, over_cap_unresolved, duration_ms）と
  型 `TargetSweepMode`・`TargetSweepRootReport`・`TargetSweepByReason`・`TargetSweepSkip` を `model.rs` に追加（task-worker に依存しない）
- 追従: `task-api/src/query.rs` の種類名 `target_sweep_ran`、`docs/api/v1/{api-v1,event}.schema.json`（UPDATE_SCHEMA=1）、
  `web/api/generated/*`・`web/api/realtime/event-kinds.ts`、`gui/app/celeris/types.ts`、gui-api.md の種類一覧
- `web/api/realtime/invalidation-map.ts` に `target_sweep_ran: { sets: ["T"] }`（schema 照合試験が要求。`ADR_TABLE` の 35 件には足さない）
- 証拠:
  - `cargo test -p task-core target_sweep` → target_sweep_event_round_trips 1 passed
  - `UPDATE_SCHEMA=1 cargo test -p task-core` 866 passed／`-p task-api` 全 ok（schema 照合を含む）
  - `pnpm -C web exec vitest run` 80 files 全 pass、`cargo clippy -p task-core -p task-api -- -D warnings` 警告なし
- 未実施: dispatcher からの発行（後続の葉）
