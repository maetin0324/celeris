---
tasks: [01M44QPM9FJHYD08BJK5470MVT]
---
# Phase 1: llm-proxy の旧設定からの正規化と legacy 選択

`llm_proxy::legacy_catalog::normalize_legacy_config` を公開し、旧 `[llm_proxy.models]` と `[llm_proxy.sources]` から `ModelProfile`、`DeploymentProfile`、`RoutingPolicy` を導く。旧設定で分からない能力、品質、価格、モデル revision は unknown または欠測とし、source の資格情報や URL は catalog に複写しない。wire model 名は deployment に置き、旧 alias だけでは model identity を統合しない。Qwen の deployment は `allowed_lanes = [cheap]` に固定する。

`selection::legacy_deployments` は kernel の `legacy_rank` から候補順を受ける。到達性のある relay、OAuth pool の cooldown と残量順位、stream 開始前の fallback は既存 `rank_relays`、`rank_pool`、`rank_across_pools` と server の送信経路を引き続き使う。具体モデル指定は既存の素通り経路のまま扱う。

`routing_proxy_legacy_equivalence_tiers_and_fallback` は旧写像と新 catalog の選択を表で比較する。frontier/standard/cheap、relay の不通と fallback、pool の使用中枠・観測残量に応じた優先順位、source 固定の wire 名、Qwen の cheap 制限を覆う。

検証: `cargo test -p llm-proxy`（単体 40 件、結合 27 件成功）、`cargo clippy -p llm-proxy --all-targets -- -D warnings`（成功）、`cargo fmt --all -- --check`（成功）。
