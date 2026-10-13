# class-routes / config

- 変更: `[route_defaults.<h>].classes.<class>.candidates` を読み、`Registry.route_class_defaults` に入れる（`crates/celeris/src/config/llm_route.rs`）。
- class 名は `coding` / `planner` / `review` だけ。それ以外・空の candidates は `ConfigError`。
- class 候補は harness 既定と同じ検査（source・model 宣言、harness の kind・API format、proxy 経由要件、重複）を通す。`validate_route_candidates` に共通化した。
- `apply_verify_smoke` は class 候補からも smoke source を外す（`crates/celeris/src/config/harness.rs`）。
- `celerisctl routing show` は `<harness> class <class>: <source>/<model>` を出す（`crates/celerisctl/src/commands/routing.rs`）。
- docs/ops の runbook・`migration_runbook_toml_loads` の期待値は触っていない（runbook leaf の担当）。

## 試験

- `celeris::config::tests::route_defaults_classes_build_the_registry`
- `celeris::config::tests::claude_code_class_route_override_picks_opus_for_coding_only`（claude_code 既定 sonnet、coding だけ opus）
- `celeris::config::tests::class_candidates_are_validated_like_harness_candidates`
- `celeris::config::tests::empty_class_candidates_are_a_config_error`
- `celeris::config::tests::unknown_execution_class_is_a_config_error`
- `celeris::config::tests::apply_verify_smoke_drops_smoke_from_class_candidates`
- `celerisctl::commands::routing::tests::show_prints_class_candidates`

## 結果

- `cargo test -p celeris --lib config::` 143 passed（`migration_runbook_toml_loads` を含む）
- `cargo test -p celerisctl routing` 4 passed（lib 側の routing 試験も通過）
- `cargo clippy -p celeris -p celerisctl --all-targets -- -D warnings` exit 0

## 注意

- `RouteDefaultConfig` には元から `deny_unknown_fields` があった。綴り違いの `classes` 欄も読み込みエラーになる。
- 重複候補のエラー文言は `duplicate route candidate for harness {h}` から `duplicate route candidate in route_defaults.{h}`（class は `...classes.{class}`）に変わった。試験は文言に依存しない。
- daemon の harness API view（`daemon/api.rs` の `route_defaults`）には class 候補をまだ出していない。今回の Objective の範囲外。
