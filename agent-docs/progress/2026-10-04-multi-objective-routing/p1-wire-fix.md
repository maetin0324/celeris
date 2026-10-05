---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
---
# Phase 1: wire 段の統合検査コンパイルエラー修正

## 原因

段 `wire`（`proxy-legacy`・`dispatch-legacy`・`config`）の統合後、`cargo test --workspace --no-run` が `E0063` で落ちた。`config` 葉（`p1-config.md`）が `celeris::Config` に `model_routing: ModelRoutingConfig`・`routing_catalog_snapshot: Option<Arc<RoutingCatalog>>`・`routing_catalog_state: Option<Arc<RwLock<Arc<RoutingCatalog>>>>` の 3 欄を足したが、`Config` は `Deserialize` のみで `Default` を持たないため、struct literal で `Config` を直接組み立てている箇所は新欄を手で埋めないとコンパイルできない。`crates/celerisctl/src/commands/worker_tests.rs:87` の `cluster_config()`（celerisctl は `toml` crate に依存しないため `Config` を直接組み立てて試験する）がこれに当たった。

## 直した箇所

- `crates/celerisctl/src/commands/worker_tests.rs`: `cluster_config()` の `Config { … }` に `model_routing: Default::default()`・`routing_catalog_snapshot: None`・`routing_catalog_state: None` を `llm_proxy` の直後（`Config` 定義と同じ並び）に追加。celeris 本体・config 葉の実装は変更していない。

ワークスペース全体で `celeris::Config` を struct literal で組み立てている箇所は `crates/celerisctl/src/commands/worker_tests.rs` のみだった（他の `Config {` 一致はすべて別の設定型 — `DbConfig`・`ClusterConfig`・`GenreConfig` 等）。`Config` は `#[serde(deny_unknown_fields)]` のみで `Default` を導出していないため、`..Default::default()` では埋められない（`Default` を今回新たに足すのは config 葉の実装変更になるため見送った）。

## 証拠

- `cargo test --workspace --no-run` → exit 0（修正前は `celerisctl` の bin test ターゲットで E0063、1 件）。
- `cargo test -p celerisctl` → 全 binary で `test result: ok`、失敗 0（worker_run 8 件・no_migrate 4 件・org_migrate_v2 2 件・skills_import 1 件・pipe 1 件・worker_run_signal 1 件ほか）。
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0、warning なし。
- `bash scripts/dev/test-parallel.sh` → exit 0。nextest: `3930 tests run: 3930 passed (1 slow), 12 skipped`（SLOW は既知の `celeris::instance_handoff::a_stale_heartbeat_promotes_the_standby`）。doctest: 10 crate すべて成功（task-worker の 1 件は `ignored`）。
