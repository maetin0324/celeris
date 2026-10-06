---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: daemon-wire
status: done-in-branch
completed: 2026-10-05
---

# p3-daemon-wire: celeris の registry・proxy event sink・Phase 3 設定の配線

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §5・§6・§10 Phase 3 のうち、daemon の配線分。

## 実装

- `crates/celeris/src/daemon/run.rs`: `InMemoryRoutingContextRegistry` を 1 つ作り、`Dispatcher::set_routing_context_registry` と
  `build_llm_proxy_state` の両方に同じ `Arc` を渡す。
- `crates/celeris/src/daemon/services.rs`: `TaskProxyEventSink`（`llm_proxy::routing_context::ProxyEventSink` の実装）。
  - `record` は blocking pool で `append_proxy_event` を呼ぶ（HTTP 応答を待たせない）。
  - `append_proxy_event` は run の所属を `runs` 索引（`run_index_get`）で照合し、task に属さない run は拒否する（推定で結ばない）。
    features と request の `request_id`/`decision_id` が食い違えば拒否。同じ `(request_id, decision_id)` の
    `RoutingRequestDecided` が既にあれば何も足さない（冪等）。無ければ `RoutingFeaturesRecorded` と `RoutingRequestDecided` を追記。
  - `parent_decision` は run の `RoutingDecided` の `optimizer.decision_id` を返す。
  - standalone 要求（context header 無し）は proxy 側が sink を呼ばないので proxy log のみ。
- 設定 `crates/celeris/src/config/model_routing.rs`: `[model_routing] context_safety_margin`（任意）と
  `[model_routing.escalation] quality_failures_per_lane` / `max_total_attempts`（任意、既定は `EscalationThresholds::default()` = ADR-0069 の 2 / 4。0 はエラー）。
  `RoutingRuntime` → `DispatchRoutingSettings` に載せ、reload で原子的に差し替わる。
  `ModelRoutingConfig` と `EscalationEntry` は未知の欄を無視する（`deny_unknown_fields` を外した。Objective の「未知の欄は無視」に合わせた）。
- `crates/task-dispatch`: `DispatchRoutingSettings` に `context_safety_margin`・`escalation` を追加。retry lane の
  `EscalationThresholds` は設定値を基に org ceiling を重ねる（`dispatch_run.rs`）。context 登録時に margin を上書きし provenance に
  `model_routing.context_safety_margin` を残す（`routing_context.rs`）。
- `docs/ops/model-routing-migration.md` §8: 設定例と reload で効く旨（`config/celeris.example.toml` は葉の範囲外なので変えない）。

## privacy 制約と context_transport

`constraint_exclusions`（`routing_enforce.rs`）は制約が既定でない enforce run で proxy 経由行を `context_transport_unsupported` で
不適格にする。試験 `constraints_exclude_proxy_routes_and_unknowns_with_reasons` と `tests/routing_enforce.rs` の enforce 試験が固定している。
daemon 側は `dispatch_settings()` が `constraints` を既定のまま渡す（Phase 2 の設定は server 全体の組織制約を受け付けない）ので、
現状の本番配線では組織 privacy 制約は入らない。

## 証拠

- `cargo nextest run -p celeris routing` → 10 passed（`routing_phase3_escalation_defaults_and_validation`、
  `routing_runtime_is_wired_to_the_dispatcher_and_reload_is_atomic`（reload で閾値 3/5 が dispatcher に届く）、
  `routing_tests::one_registry_is_shared_by_the_dispatcher_and_the_proxy`、`routing_tests::proxy_sink_appends_only_correlated_requests_once`）
- `cargo nextest run -p celeris -p task-dispatch` → 1089 passed
- `bash scripts/dev/test-parallel.sh` → 3988 passed, 12 skipped, test-parallel: ok
- `cargo clippy --workspace -- -D warnings` → exit 0、`cargo fmt --all --check` → exit 0
- 再走（check 不合格の修正）: `services.rs` の `start_llm_proxy` を test module の前へ移した（clippy items_after_test_module）。`cargo clippy -p celeris --all-targets -- -D warnings && cargo fmt --all -- --check` → exit 0、`cargo nextest run -p celeris` → 402 passed、範囲 check → exit 0

## 未解決

- 組織 privacy 制約の設定経路（server/org 単位の `Constraints`）は未実装。入れたら daemon 経路の試験を足す。
- `ProxyState::in_flight_contexts` を見て run 終了後に ref を外す処理は dispatcher 側で未確認（registry は TTL で失効する）。
- `[model_routing.retry]` と同じく、proxy 側の設定は起動時のみ。escalation・margin は reload で効く。

## 提案

- close 葉の ADR 付記に「ModelRoutingConfig は未知欄を無視（以前は拒否）」を書く。
