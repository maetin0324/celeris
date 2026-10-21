---
title: "[mlsys-main-2 不具合 1/budget] Budget.max_tokens・BudgetKind::Tokens・POST /tasks の max_tokens（欄とパイプ）"
tasks: [01M5C8KF8Y3MB6TT064AYV2F23]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# 欄とパイプの追加（ADR 2026-10-20-mlsys-main2-fixes D1-1）

- `task_core::Budget.max_tokens: Option<u64>`（serde default・None は出さない）、`BudgetKind::Tokens`（`describe_run_end` の腕のみ）。
- `NewTaskSpec.max_tokens`（task-ops）→ `POST /tasks` の body から task の budget へ。
- `Budget` を struct literal で構築する全 crate に `max_tokens: None` を追加。
- schema 再生成（`UPDATE_SCHEMA=1`）、`docs/api/v1/api-v1.schema.json`・`web/api/generated/{schema.json,types.ts}` 更新。
- 範囲外（途中観測・再開抑止・campaign・adapter）は後続 unit。前の run が入れていたそれらの実装はこの unit から外した（base に戻した）。

## 証拠
- `cargo check --workspace --all-targets` exit 0。
- `cargo test -p task-core budget_max_tokens` → 1 passed（serde 往復・省略時 None）。
- `cargo test -p task-api --test operations max_tokens` → 1 passed（POST /tasks の max_tokens が budget に入る）。
- schema 試験（UPDATE_SCHEMA=1）ok、web の `pnpm gen:types` 実行済み。

## 本番への移行
なし（`budget` は task の JSON、欄は省略可。DB migration 不要）。
