---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: proxy-shadow
status: done
completed: 2026-10-05
---

# Phase 4 proxy-shadow: llm-proxy の非同期 shadow queue と decision shadow

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§7.2・§10 Phase 4 の llm-proxy 部分。

## 実装
- `crates/llm-proxy/src/shadow.rs`（新規）
  - `ShadowQueue`: 実行 shadow の bounded queue。`ShadowPolicy` が `execute = true` で検証を通るときだけ作れる（既定 off は `None`）。`submit` は同期で返り実行は `tokio::spawn`（primary は待たない）。`max_concurrency` を超える分は `max_queue_depth` まで queue、それ以上は `dropped/queue_full`。queue 内で `timeout_ms` を過ぎたものは `dropped/timeout`（detail `expired_in_queue`）、実行中の期限切れは `failed/timeout`（予約は `TimedOut` で確定）。`primary_pressure()` は未開始分を `dropped/primary_pressure`。primary と同じ非分離 resource group（group 一致、group 不明なら同じ source）は `dropped/resource_group_shared`。入口の判定（off・allowlist 外・標本外）は記録せず送信 0。
  - 予約は `ShadowBudget` trait（`reserve`/`settle`）。試験用の偽 `AllowAllBudget`（費用が分かれば常に許可、未知は `unknown_cost`）。共有 DB の接続は proxy-caps 葉。
  - `ShadowExecutor` trait と `RelayShadowExecutor`（openai-compatible のみ、`stream = false` のコピーを 1 回だけ送る）。出力は `output_sha256` と tokens だけを `ShadowRecord` に残し、本文・tool call は解釈も実行もしない。
  - decision shadow: `DecisionPolicy`（既定 `PreferSelfHosted`）と `decision_record`。候補比較（`same_as_primary` / `differs_from_primary` / `no_candidate`）だけを記録し upstream を呼ばない。
  - `ProxyShadow`（decision policy・実行 queue・sink・費用見積もり）。
- `crates/llm-proxy/src/server.rs`: `ProxyState::with_shadow`（既定 `None` で挙動不変）。primary 成功時（非 stream の応答取得、stream の最初の byte 後）に `shadow_after_primary`、候補が一時的に空になった（予約待ち）時に `primary_pressure`。既存 pub struct に欄は足していない（`ProxyState` は private 欄の追加のみ）。
- `crates/llm-proxy/Cargo.toml`: dev-dependency の tokio に `test-util`（`start_paused`）。

## 証拠
- `cargo test -p llm-proxy` → exit 0（lib 56、proxy_fallback 1、proxy_integration 27、proxy_routing_context 2、proxy_shadow 2 すべて pass）。
  - `shadow::tests::routing_shadow_backpressure_timeout_and_no_tool_execution`（pause した時計と gate で queue_full / 実行中 timeout / queue 内期限切れ / primary_pressure / resource_group_shared / unknown_cost / allowlist 外を固定。tool call 出力は hash のみで executor 呼出しは 1 回）
  - `routing_decision_shadow_never_calls_upstream_or_changes_primary`（tests/proxy_shadow.rs。legacy と同じ応答・source、偽上流の chat/probe 数が legacy と同数＝追加 HTTP 0、記録は Decision のみ）
  - `routing_execution_shadow_copies_request_without_delaying_primary`（shadow 上流を止めたまま primary が返る。コピーは stream=false、chat 1 回）
  - 既存の `routing_proxy_fallback_preserves_constraints_and_stream_boundary`・`routing_proxy_legacy_equivalence_tiers_and_fallback`・`routing_reservation_conflict_reselects_once` も pass。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0。
- `bash scripts/dev/test-parallel.sh` → exit 0、4007 tests run: 4007 passed, 12 skipped。

## 未解決事項
- 共有 DB（`SqliteStore::routing_shadow_*`）への `ShadowBudget` 実装と、再起動・2 instance の試験は proxy-caps 葉。
- daemon からの `ProxyShadow` 配線（sink を task event `routing_shadow_recorded` へ、費用見積もり・owner の設定）は daemon-wire 葉。
- legacy catalog は `resource_group_id` を持たないので、server からの job は group 不明（同じ source を非分離とみなす）。

## 提案
- `ShadowBudget::reserve` は同期で SQLite を叩く想定。spawn 済みの task 内で呼ぶので primary は待たないが、DB 実装では `spawn_blocking` を検討する。
