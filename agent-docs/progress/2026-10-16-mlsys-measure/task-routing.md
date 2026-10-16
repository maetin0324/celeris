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
- task-core `route_select/task_override.rs`: `RoutingOverride {mode, lower_policy?, evidence_table_id?, online_update=true}`・`validate`・`resolve_routing`（task 欄 > config）・`ResolvedRouting`・`ModeSource`。`RoutingMode` に JsonSchema。
- task-core store: migration 0084 `task_routing_overrides`（全ブランチ走査で 0084 が空きを確認）、`SqliteStore::set_task_routing_override_tx`（作成 tx 内で 1 行、2 回目は PK で失敗）、`TaskStore::task_routing_override`。`TaskStore::routing_evidence_at(genre, harness, version)`（指定版の根拠表）。
- task-core event `routing_override_pinned`（追記専用。作成時の固定そのもの）。`feedback::pending_run_outcomes` はこの event が `online_update=false` なら 0 件（根拠を動かさない）。
- dispatcher: `task_routing_override(store, task_id)`・`Dispatcher::routing_for(task)`。lane 層（`apply_lane_assist`）は task ごとの mode。`RouteContext.evidence_version` で根拠表の版を固定。欄のある task の `RouteDecision.routing_resolved` に解決結果。決定ログ `routing_mode` にも書く。
- task-api: `POST /tasks`（`NewTaskBody.routing_override`、監査付き `task.create` も同じ関数）で受理・検査（未知 mode・欄・下段版は 400）、行と event を task 作成と同じ transaction で書く。`EVENT_TYPES` に `routing_override_pinned`。
- celeris daemon 分類段 `ClassifyTick::run`: task ごとに mode を解決（fixed 固定 task は分類しない、DA 固定 task は config fixed でも分類）。
- task-ops・celerisctl は変えない（範囲 check。CoS の `create_task` action・`celerisctl add` は欄を持たない）。
- schema 再生成: `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema`（`docs/api/v1/{api-v1,event}.schema.json`・`docs/protocol/worker-protocol.schema.json`）＋ `node web/scripts/gen-types.mjs`（`web/api/generated/{schema.json,types.ts}`）。`gui/` には生成型が無い。

## mock 検証済み（決定的試験。実 API・本番 DB なし）
- `task_fixed_beats_config_decision_assisted_and_no_assist_runs`・`task_decision_assisted_beats_config_fixed_and_assist_runs`・`without_the_task_field_the_config_mode_decides_and_no_resolution_is_written`（task-dispatch `dispatcher/tests/lane_assist.rs`）
- `task_routing_override_beats_the_config_mode`（celeris `daemon/classify_tick.rs`）
- `online_update_false_leaves_the_evidence_untouched`（task-core `model_router/feedback/tests.rs`）
- `create_accepts_and_pins_routing_override`（task-api `tests/project_plan.rs`: 行・event・不正値 4xx）
- `routing_override_round_trips_and_absent_is_none`（task-core `store/routing_overrides.rs`）、`task_override::tests`、`routing_evidence` の固定版読み

## 実経路検証済み
- なし（本番 daemon には入れていない）。

## 未実施
- campaign の B/C アダプタからの投入（後続 unit adapter-bc が `routing_override` を付けて POST する）。
- celerisctl add のフラグ（不要なら作らない）。

## 証拠（2026-10-16、side table 化の後）
- `bash scripts/dev/test-parallel.sh` → exit 0、5389 tests run: 5389 passed, 12 skipped
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `bash scripts/dev/check-adr-numbers.sh` → ok、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → ok
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の差分 path を許可パターンで除外）→ 範囲外 0 件（初回は task-ops・celerisctl に触れて落ちたので side table 方式へ変えた）

## 提案
- `cargo clippy --workspace --all-targets` は `crates/celeris/src/daemon/jev_verifier.rs` の `items_after_test_module` で落ちる（既存。gate は `--all-targets` なしなので通る）。
