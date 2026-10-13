---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
title: selector evidence and stage persistence
---
# 根拠表と段階の永続化

## 変更

- schema 0074（`0074_routing_evidence_stage.sql`）で `routing_evidence` と `routing_stage` を追加した。根拠行の主キーは ADR D2 の 5 key fields と evidence_version。`routing_stage` の主キーは genre + harness_id。
- `TaskStore` に版単位の根拠表置換（`replace_routing_evidence`）・genre/harness の最新版取得（`latest_routing_evidence`）・stage 読み書き（`routing_stage` / `set_routing_stage`）を追加した。行のない genre×harness は shadow として読む。
- stage 更新は `routing_stage_changed` event を同一 transaction で追記する。event は genre、harness、遷移元/先、理由、window 値、actor（human|daemon 以外は無効）を持つ。stage 更新値は `RoutingStageChange` にまとめ、clippy の引数数制限を満たす。
- event を task-api の event type vocabulary に追加し、`docs/api/v1/event.schema.json` と `api-v1.schema.json` を再生成、`web/api/generated/{schema.json,types.ts}` を `node web/scripts/gen-types.mjs` で再生成、`web/api/realtime/{event-kinds.ts,invalidation-map.ts}` に `routing_stage_changed`（`sets: ["T","R"]`）を追加した。
- migration の古い DB fixture は selector 表も落として旧版を再現するよう修正し、schema version の期待値を 74 に追従した（`crates/celerisctl/tests/no_migrate.rs` は `SCHEMA_VERSION` 参照なので変更不要）。

## 検査

- 今回の再確認: `cargo test -p task-core --lib routing_evidence -- --nocapture`（1 passed）、`cargo test -p task-api --lib query::tests -- --nocapture`（13 passed）、`cargo test -p task-core --lib event_row_schema_matches_committed`（1 passed）、`cargo test -p celerisctl --test no_migrate`（4 passed）。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と `git diff --check`: 成功。変更は許可範囲内。
- `cargo test -p task-core --lib -- routing_evidence event_row_schema_matches_committed migration_00`: 16 passed / 0 failed（根拠表の版置換・最新版読み・無行 shadow・stage 遷移と event 追記を含む）。
- `cargo check --workspace --all-targets`: 成功（event 追加に伴う task-api の網羅 match を含む）。
- `cargo test -p celerisctl --test no_migrate`: 成功（4 tests。前回 run の記録）。
- `cargo clippy -p task-core -p celerisctl --all-targets -- -D warnings`: 成功（前回 run の記録）。
- `UPDATE_SCHEMA=1 cargo test -p task-core store::tests::event_row_schema_matches_committed -- --exact` / `-p task-api schema::tests::committed_schema_matches_generated`: 成功（前回 run の記録）。
- web の vitest は `web/node_modules` が無く未実行（`invalidation-map.test.ts` の kind 網羅は event-kinds.ts と invalidation-map.ts の両方に追加して整合させた）。

## 未解決

1. 別 task（eval-harness 01M4Q53E9WQEKMAS448GZMWTFB）の旧案にも `routing_evidence` 表と store module がある。現ブランチは migration 0074 と `route_select::EvidenceRow` を用いるため並行実装間で差分・migration の競合が予想される。統合時に先行実装との重複がないか確認し、必要なら統合 unit で一本化する。
2. `latest_routing_evidence` は `MAX(evidence_version)` を文字列比較で取る。評価 runner は manifest sha と集計日を含む版を発行するため、日付が比較の先頭になる形式（または同等の順序保証）を lane-defaults / eval 側と合わせる。
3. 今回も `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` は pnpm store に `@vitest/mocker@5.0.0` が無く失敗した。共通規則に従いネットワーク取得はせず、web typecheck/test は未実行。既存の generated files は commit 済みで、API schema の一致試験は通過した。
