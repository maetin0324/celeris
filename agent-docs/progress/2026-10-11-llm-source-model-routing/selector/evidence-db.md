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

- `cargo test -p task-core --lib -- routing_evidence event_row_schema_matches_committed migration_00`: 16 passed / 0 failed（根拠表の版置換・最新版読み・無行 shadow・stage 遷移と event 追記を含む）。
- `cargo check --workspace --all-targets`: 成功（event 追加に伴う task-api の網羅 match を含む）。
- `cargo test -p celerisctl --test no_migrate`: 成功（4 tests。前回 run の記録）。
- `cargo clippy -p task-core -p celerisctl --all-targets -- -D warnings`: 成功（前回 run の記録）。
- `UPDATE_SCHEMA=1 cargo test -p task-core store::tests::event_row_schema_matches_committed -- --exact` / `-p task-api schema::tests::committed_schema_matches_generated`: 成功（前回 run の記録）。
- web の vitest は `web/node_modules` が無く未実行（`invalidation-map.test.ts` の kind 網羅は event-kinds.ts と invalidation-map.ts の両方に追加して整合させた）。

## 未解決

1. **範囲 check が通らない（plan_issue で報告）**。`Event` に `routing_stage_changed` を足すと次も必ず変わるが、unit の範囲 allowlist（`crates/task-core/`・`crates/celerisctl/tests/`・`docs/protocol/`・`agent-docs/(progress|adr)/`）に入っていない:
   - `crates/task-api/src/query.rs`・`crates/task-api/src/query/tests.rs`（`EVENT_TYPES` の網羅列と `event_type_name` の網羅 match。直さないと workspace がコンパイルできない）
   - `docs/api/v1/event.schema.json`・`docs/api/v1/api-v1.schema.json`（task-core の `event_row_schema_matches_committed` が一致を要求。event schema の実体は `docs/protocol/` ではなく `docs/api/v1/`）
   - `web/api/generated/schema.json`・`web/api/generated/types.ts`（`docs/api/v1/api-v1.schema.json` の写し）
   - `web/api/realtime/event-kinds.ts`・`web/api/realtime/invalidation-map.ts`（`invalidation-map.test.ts` が schema の全 kind を両方に要求）
2. **別 task（eval-harness 01M4Q53E9WQEKMAS448GZMWTFB）と同じ表・同じ module path を作っている**。あの branch には `crates/task-core/src/routing_evidence.rs`（`EvidenceKey`/`Interval`/`EvidenceRow`）と `crates/task-core/src/store/routing_evidence.rs`、migration `0071_routing_evidence.sql`・`0072_eval_trials.sql`・`0073_routing_evidence_bootstrap.sql` があり、`routing_evidence` 表は ADR D2 の同じ列で作られている（あちらは `low_confidence` あり・`wall_median_s`/`usd_list_median` が NULL 可）。まだ main には入っていない。両方が main に入ると `CREATE TABLE routing_evidence` の重複（0071 と 0074）と `store/routing_evidence.rs` の内容衝突が起きる。この unit の範囲では解けない（selector 側の型は policy-core が `route_select/evidence.rs` に作った）。統合作業か人の決定が要る。
3. `latest_routing_evidence` は `MAX(evidence_version)` を文字列比較で取る。evidence_version が辞書順にならない形式（`v2` と `v10`）だと最新版を誤る。評価 runner が書く版の形式（時刻やゼロ埋め序号）を lane-defaults / eval 側と合わせる。
