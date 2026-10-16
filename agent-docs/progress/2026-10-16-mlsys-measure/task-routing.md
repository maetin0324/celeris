---
title: "task 単位の routing 設定の固定欄（routing_override: mode・下段・根拠表・online 更新）"
tasks: [01M51QR9CCRP89GWXZDR4XB3S6]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# task-routing（WU task-routing）

設計: `agent-docs/adr/2026-10-16-mlsys-measure-task-routing.md`（D1〜D7）。

## 実装済み
- task-core `route_select/task_override.rs`: `RoutingOverride {mode, lower_policy?, evidence_table_id?, online_update=true}`・`validate`・`resolve_routing`（task 欄 > config）・`ResolvedRouting`・`ModeSource`。`TaskRouting.routing_override`（tasks.json 内。migration 不要）。`RoutingMode` に JsonSchema。
- store: `TaskStore::routing_evidence_at(genre, harness, version)`（指定版の根拠表）。
- dispatcher: `Dispatcher::routing_for(task)`。lane 層（`apply_lane_assist`）は task ごとの mode。`RouteContext.evidence_version` で根拠表の版を固定。欄のある task の `RouteDecision.routing_resolved` に解決結果。決定ログ `routing_mode` にも書く。
- task-ops: `record_routing_outcomes` は `online_update=false` の task で 0 件（根拠を動かさない）。`POST /tasks`（`NewTaskSpec.routing_override`）で受理・検査（未知 mode・欄・下段版は 4xx）。CoS 経路は書かない。
- celeris daemon 分類段 `ClassifyTick::run`: task ごとに mode を解決（fixed 固定 task は分類しない、DA 固定 task は config fixed でも分類）。
- schema 再生成: `docs/api/v1/{api-v1,event}.schema.json`・`docs/protocol/worker-protocol.schema.json`・`web/api/generated/{schema.json,types.ts}`（旧 `gui/` には生成型が無い）。

## mock 検証済み（決定的試験。実 API・本番 DB なし）
- `task_fixed_beats_config_decision_assisted_and_no_assist_runs`・`task_decision_assisted_beats_config_fixed_and_assist_runs`・`without_the_task_field_the_config_mode_decides_and_no_resolution_is_written`（task-dispatch `dispatcher/tests/lane_assist.rs`）
- `task_routing_override_beats_the_config_mode`（celeris `daemon/classify_tick.rs`）
- `online_update_false_leaves_the_evidence_untouched`・`unknown_lower_policy_is_rejected_at_create`（task-ops `routing_outcome/tests.rs`）
- `create_accepts_and_pins_routing_override`（task-api `tests/project_plan.rs`）
- `task_override::tests`（task-core）、`routing_evidence` の固定版読み（task-core store）

## 実経路検証済み
- なし（本番 daemon には入れていない）。

## 未実施
- campaign の B/C アダプタからの投入（後続 unit adapter-bc が `routing_override` を付けて POST する）。
- celerisctl add のフラグ（不要なら作らない）。

## 証拠
- `bash scripts/dev/test-parallel.sh` → exit 0、5389 tests run: 5389 passed, 12 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-adr-numbers.sh` → ok、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok

## 提案
- `cargo clippy --workspace --all-targets` は `crates/celeris/src/daemon/jev_verifier.rs` の `items_after_test_module` で落ちる（既存。gate は `--all-targets` なしなので通る）。
