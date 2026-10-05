---
task: 01M44ZK8GADYD9PVAYBY73F6YC
unit: config
status: done
completed: 2026-10-05
---
# p2-config: `[model_routing]` の state・cost・retry 設定と enforce の検証

## やったこと

- `crates/celeris/src/config/model_routing.rs`: `ModelRoutingConfig` に optional 欄を追加（全部既定 off か unknown）。
  - `[model_routing.estimator] kind` — enforce は `kind = "heuristic"` の明示（opt-in）だけ受け入れる。
    heuristic 以外の kind は mode を問わず検証エラー。opt-in の無い enforce は従来どおり「Phase 2」を含むエラー。
  - `enforce_routes` — `standalone` / `server`（サーバ全体の制約で足りる経路）だけ。他（`task`・`org` 等）・空は検証エラー。未設定は両方。
  - `observation_ttl_seconds` — 未設定は `FreshnessPolicy::default()`（300 秒）。
  - `[model_routing.subscription_windows.<window_id>] reserve_value_usd` — 未設定は窓の reserve unknown（shadow も unknown）。
  - `[[model_routing.resource_groups]] id, concurrency_limit, usd_per_gpu_second, usd_per_wait_second` — 未設定成分は None。
  - `[model_routing.retry]` 分類別上限・total_attempts・deadline_secs・breaker — 未設定は `llm_proxy::fallback::FallbackSettings::default()`。
    `client` は 0 以外を拒否（p2-proxy-fallback の提案）、`total_attempts = 0` も拒否。
  - policy 正規化定数・weights・min_quality は Phase 1 の `[model_routing.policies.<lane>]` のまま（`RoutingPolicy::validate`）。
- `Config::routing_runtime()` → `RoutingRuntime`（`dispatch_settings()`・`self_host_rates()`・`capacity_limits()`）。
  `Config::load` が catalog の後に組み `Config.routing_runtime` に置く（不正なら load が失敗）。
- 配線: `build_dispatcher` と `reload_providers` が `Dispatcher::set_dispatch_routing` を呼ぶ（reload は `Config::load` 成功後だけ = 原子的）。
  proxy は `build_llm_proxy_state` で `ProxyState::with_fallback(runtime.fallback, SystemClock)`。
- task-dispatch の公開口に最小の追加: `DispatchRoutingSettings.window_reserves`（窓 id → reserve_value）。
  `source_state_of` が account の quota 窓に写す（無い窓は None のまま）。
- celerisctl の試験の `Config` 全欄 literal に `routing_runtime: None`。

## 証拠

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 新規試験 2 件 + 配線試験 | `cargo nextest run -p celeris routing_` | 7 passed（`routing_enforce_opt_in_validates_heuristic_only`・`routing_config_defaults_do_not_seed_unknown_as_zero`・`routing_runtime_is_wired_to_the_dispatcher_and_reload_is_atomic`・既存 `routing_config_reload_is_atomic` ほか） |
| 既存 routing / Qwen | `cargo nextest run -p celeris -p llm-proxy -p task-dispatch routing_ cheap_only_default_and_legacy_qwen_config` | 47 passed |
| workspace 全体 | `bash scripts/dev/test-parallel.sh` | exit 0、Summary「3964 tests run: 3964 passed (1 slow), 12 skipped」 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 試験コードの clippy | `cargo clippy -p celeris -p task-dispatch -p celerisctl --all-targets -- -D warnings` | exit 0 |

## 未解決事項

- proxy は起動時に 1 度だけ組むため、`[model_routing.retry]` の変更は reload では効かず再起動で効く（従来の `[llm_proxy]` と同じ）。
- `RoutingRuntime.capacity_limits()`・`self_host_rates()` は用意したが、proxy の server.rs は `select_state`・予約表をまだ使っていない（p2-proxy-select の未解決）。配線時にここから渡す。
- `enforce_routes` は検証と保持のみ。proxy の enforce 経路自体は未配線。
- dispatcher の `OBSERVATION_TTL_SECS`（account 観測の expires_at）は定数のまま。`freshness` は `DispatchRoutingSettings` に入る。

## 提案

- proxy-select の server.rs 配線時に `ProxyState` へ `RoutingRuntime` 相当（capacity・rates・freshness・window reserve）を渡す口を 1 つにまとめる。
