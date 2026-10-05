---
task: 01M44ZK8GADYD9PVAYBY73F6YC
unit: api
status: done
completed: 2026-10-05
---
# Phase 2: sources API の鮮度・費用成分と routing audit（p2-api）

## 変更

- `crates/task-api/src/types.rs`: `LlmSourceView.deployments: Vec<LlmSourceStateView>`（空なら欄ごと省く。旧欄はそのまま）。`LlmSourceStateView` は `freshness{observed_at, age_secs, expires_at, stale}`・`reachability`・`latency_ms`・`quota_remaining`（既知窓の最小）・`quota_reset_at`（最も早い reset）・`pressure`・`unknown[]`・`cost{billed{cash_usd}, opportunity{shadow_usd, resource_usd}, effective_usd, assumptions}`。未知は `null`（0 で埋めない）で、名前を `unknown` に残す。請求と機会費用は別の object。
- `crates/task-api/src/llm_sources.rs`: 純粋関数 `source_state_view(&SourceState, Option<&CostEstimate>, concurrency_limit, now)`（時刻は注入）。
- `crates/task-ops/src/routing_audit.rs`: `task_routing_audit_with_requests(store, log, task_id)` と純粋な `routing_audit_with_requests`。stage `proxy` の `routing_decided` を run の子 trace（`RequestRoutingAudit`）にし、dispatch の trace を上書きしない。子は proxy log の相関欄（0048）で request id と decision id が一致したときだけ `log` を結ぶ。結べないもの（`request_id_missing`・`request_log_missing`・`request_log_mismatch`）は `audit_incomplete: true` と `incomplete_reasons`。run_id の分からない要求は `unbound_requests`（`run_unknown`。推定で結ばない）。dispatch の trace が `request_id` を指すのに子が無い run も `request_log_missing`。Phase 2 の trace の無い旧 run は `requests`・`audit_incomplete` が None。`RunRoutingAudit` は旧 `RoutingAudit` を flatten するので旧 JSON 欄の位置は変わらない。旧 `task_routing_audit` は残した（task-dispatch の試験が使う）。
- `crates/task-api/src/routing.rs`: `GET /tasks/{id}/routing` が上を使う。`TaskRoutingView.runs` は `RunRoutingAudit`、`unbound_requests` を追加（空なら省く）。
- `RoutingDecided` の event 型・`EVENT_TYPES` は変えていない。
- `crates/celeris/src/daemon/api.rs`: struct literal に `deployments: vec![]` のみ（実状態の配線は未了）。
- `docs/api/v1/api-v1.schema.json`: `UPDATE_SCHEMA=1 cargo test -p task-api --lib committed_schema` で再生成（gui/web の生成型は schema unit）。

## 証拠

- `cargo test -p task-api routing_source_api_reports_freshness_and_cost_components`（tests/routing_source_state.rs）: pass。
- `cargo test -p task-ops routing_audit_marks_missing_request_as_incomplete`: pass（routing_audit 3 件 pass）。
- `cargo test -p task-api -p task-ops`: 957 passed, 0 failed（`routing_catalog_redacts_secrets_and_keeps_legacy_fields`・`routing_is_empty_without_runs_and_404_for_unknown_tasks` を含む）。
- `bash scripts/dev/test-parallel.sh`: exit 0、Summary「3965 tests run: 3965 passed (1 slow), 12 skipped」。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-api -p task-ops --all-targets -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- 範囲 check（merge-base HEAD celeris/01M44ZK8GADYD9PVAYBY73F6YC 基点、crates は celeris/llm-proxy/task-dispatch/task-api/task-ops のみ）: exit 0。

## 未解決事項

- daemon の `LlmSourcesAdapter` はまだ `deployments` を空で返す。proxy の SourceState と CostEstimate を `source_state_view` に渡す配線は config-api の統合（または close）で行う。
- proxy の要求単位の trace を task events に append する sink（ADR §172）は未実装。sink ができると子 trace が API に出る。現状の run は dispatch の trace だけで、`requests: []`・`audit_incomplete: false`。

## 提案

- sink は proxy trace を `RoutingDecided{run_id, record.optimizer.stage="proxy"}` として append する（本 unit の audit はこの形を読む）。
