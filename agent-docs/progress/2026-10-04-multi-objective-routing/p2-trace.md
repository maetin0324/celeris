---
task: multi-objective-routing
unit: p2-trace
phase: 2
status: done-in-branch
date: 2026-10-05
---

# p2-trace: RoutingTraceV1 に除外理由・score 内訳・費用成分・最終 source を足す

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §9 の routing trace を additive に拡張した。
旧 event は新欄が None で読め、新欄は None のとき直列化されない（旧 JSON は再直列化で同じ値に戻る）。

## 実装内容

- `crates/task-core/src/model_router/trace.rs`
  - `ExcludedReason`（`#[serde(tag = "kind")]`）: `constraint{name}`・`quota_exhausted`・`cooldown`・`concurrency`・
    `rate_limit`・`health_down`・`circuit_open`・`disabled`・`unknown_required{field}`・`quality_invalid`・
    `quality_below_min`・`invalid_estimate`・`other{code}`。`from_code` が optimizer/cost の理由コードを写し、
    `primary` は型の順序で最小のものを代表にする（理由の並び順に依らない）。
  - `ScoreTrace { q, c, l, p, wq, wc, wl, wp, unknown, score }`。`ScoreTrace::new(&cost::ScoreBreakdown, &Weights)`。
  - `CandidateTrace` に `config_order`・`excluded_reason`・`score_breakdown`・`cash_usd`・`shadow_usd`・
    `resource_usd`・`effective_usd`（全て Option、`serde(default, skip_serializing_if)`）。`Default` を derive。
    `with_cost(&cost::CostEstimate)` で費用成分を写す。
  - `RoutingTraceV1` に最終の `source_id`・`model`・`account_id`（Option）。
  - `RoutingTraceV1::normalize()`: 候補を（設定順〈無しは最後〉→ deployment id → model id）で並べ、各候補の
    `excluded_reasons` を整列・重複除去し、代表理由を補う。`fallback_order` は選択順位なので並べ替えない。
- `optimizer.rs`: trace に `config_order`・代表の `excluded_reason`・`score_breakdown`（optimizer が計算した Q/C/L/P・
  重み・unknown flag）を埋める。順位と traces の並び（入力順）は変えていない。費用成分と最終 source の埋め込みは
  runtime 段（proxy-select・dispatch-enforce）が行う。
- `crates/task-dispatch/src/dispatcher/provider_select.rs`: 構造体リテラルに `..Default::default()` と新欄の `None` を
  足しただけ（4 行）。新欄を足すと他 crate のリテラルがコンパイルできないため避けられない。
- `docs/api/v1/api-v1.schema.json`・`docs/api/v1/event.schema.json`: `UPDATE_SCHEMA=1` で再生成（schema 試験を通すため）。
  gui/web の生成型の再生成は schema 段に残す。

## 証拠

- `cargo test -p task-core routing_` → 13 passed（`routing_trace_fields_are_additive`・
  `routing_candidate_reasons_serialize_stably`・`routing_old_events_deserialize_without_optimizer` を含む）
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- `bash scripts/dev/test-parallel.sh` → exit 0、Summary「3939 tests run: 3939 passed (1 slow), 12 skipped」

## 未解決事項

- 範囲 check（`-- crates` の差分を model_router/・model_policy.rs 等に限る）は `provider_select.rs` で落ちる。
  `CandidateTrace`/`RoutingTraceV1` の欄追加は他 crate の構造体リテラルを必ず壊すため、最小の追随（4 行）を入れた。
  model_policy.rs は変えていない（`RoutingRecord.optimizer` の型が広がるだけで足りた）。

## 提案

- 範囲 check の allow に `crates/task-dispatch/src/dispatcher/provider_select.rs` を足す（model 段の兄弟の和）。
- runtime 段は trace を作ったあと `normalize()` を呼び、`with_cost` と `source_id/model/account_id` を埋める。
