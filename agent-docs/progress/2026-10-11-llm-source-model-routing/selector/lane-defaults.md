---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---
# lane-defaults: lane 別 route_defaults と選択設定

## 変更

- `route_defaults.<harness>.lanes.<lane>.candidates` を追加し、`Registry::route_defaults_for(harness, lane)` は lane 列を優先、無ければ harness の `candidates` を返す。
- lane 列も harness/source/model の宣言・互換性と、各列内の重複を検証する。
- `[route_selection]` に exploration epsilon（既定 0.1）、guardrail 窓（50）、quota floor（0.1）、source ごとの評価同時実行予約数を追加した。未宣言 source、範囲外 epsilon/quota、0 の窓を拒否する。品質停止条件の固定閾値は ADR D3 の値を使用する。
- TOML 例と ADR を更新。

## 検査結果

- `cargo test -p celeris config::llm_route`: 成功（6 tests）。
- `git diff --check`: 成功。
- `sh "$CELERIS_WU_SCOPE_PATHS"`: 変更は WorkUnit の scope 内。

## 未解決

- なし。この WorkUnit は selector の dispatcher 配線や旧設定撤去を担当しない。
