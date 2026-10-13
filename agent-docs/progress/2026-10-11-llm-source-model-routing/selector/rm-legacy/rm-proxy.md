---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
---
# rm-proxy: llm-proxy の shadow・estimator・state 選択の撤去

完了日: 2026-10-13

## 変更
- 削除: `crates/llm-proxy/src/{shadow,shadow_tests,shadow_budget,estimator_shadow,estimator_sidecar,selection_state,selection_state_tests,catalog}.rs`、`crates/llm-proxy/tests/estimator_sidecar.rs`
  - `selection_state.rs` は optimizer・estimator・policy で lane の候補を並べる state 選択で、呼び出し元は shadow だけだった。旧挙動なので試験ごと消した
  - `catalog.rs`（`ProxyCatalog`）は shadow・estimator だけが使っていた
- `server.rs`: `ProxyState` の shadow・estimator shadow 欄と `with_shadow`・`with_estimator_shadow*`、primary 後の shadow 投入、候補待ちの shadow 圧力通知を外した。要求は具体的な `<source>/<model>`（dispatcher の RoutePolicy が決めたもの）で、proxy はその source 内の account の可用性・要求内 fallback・breaker だけを行う
- `selection.rs`: `state` 子 module を外した（account の順位付けと relay の到達性だけが残る）
- `docs/architecture-map.md`: 消えた file（llm-proxy・先行葉の daemon/dispatcher 分）の行を直した

## 証拠
- `cargo check --workspace --all-targets` exit 0
- `cargo clippy --workspace --all-targets -- -D warnings` exit 0
- `cargo nextest run -p llm-proxy` 65 passed
- `bash scripts/dev/test-parallel.sh` passed 5026・failed 0・ignored 13
- `python3 scripts/dev/check-architecture-map.py` OK、`sh scripts/dev/check-doc-links.sh` exit 0
- `git grep -nE 'model_router::(estimator|optimizer|policy|shadow)' -- crates/llm-proxy` hit 0

## 未解決（後の葉へ）
- `llm_proxy::reservation` の `ReservationTable`・`CapacityLimits` は `crates/celeris/src/config/model_routing.rs` だけが使う。rm-config が model_routing.rs を消すときに一緒に消す（`Clock`・`SystemClock` は fallback が使うので残す）
- `[llm_proxy] prefer_free` は旧 routing policy 用の欄で、読み手は `config/model_routing.rs` だけ。rm-config で欄を消し読み込みエラーにする
- `crates/task-ops/src/routing_audit.rs` の comment が消えた `llm_proxy::selection_state::routing_trace` を指す（rm-store の範囲）
