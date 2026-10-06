---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: config
status: done-in-branch
completed: 2026-10-05
---

# Phase 4 shadow 設定

`[model_routing.shadow]` を追加した。未設定では `execute = false`、`sample_rate = 0` で、従来の選択を変えない。`mode = "shadow"` は decision shadow の選択であり、実行 shadow は `execute = true` を別途指定する。候補 policy 名と実行上限を celeris の設定から task-core の `ShadowPolicy` に変換し、検証済み `RoutingRuntime` の snapshot に保存する。

実行を有効にするには、4 次元をすべて埋めた allowlist、標本率、日次 request/token/effective USD、同時実行数、queue 深さ、timeout の指定が必要。`ShadowPolicy::validate` により欠落・0 上限・範囲外の標本率を `Config::load` で拒否し、空の allowlist 次元も拒否する。reload は先に新設定を丸ごと読み込むため、不正な設定では旧 runtime と catalog の snapshot を保持する。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo test -p celeris routing_shadow_config_defaults_off_and_requires_caps -- --nocapture` | 1 passed |
| `cargo test -p celeris routing_shadow_invalid_reload_keeps_previous_snapshot --lib` | 1 passed |
| `cargo test -p celeris routing_config_defaults_do_not_seed_unknown_as_zero --lib` | 1 passed |
| `cargo test -p celeris routing_config_reload_is_atomic --lib` | 1 passed |
| `cargo test -p celeris config::tests::routing --lib --quiet` | 6 passed |
| `cargo clippy -p celeris --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check`、`git diff --check` | exit 0 |
| `CELERIS_WU_BASE` 基準の変更 path と未追跡 path の範囲 check | 範囲外 0 件 |

本番設定・daemon・DB は変更していない。試験 DB は一時ディレクトリに作成する。
