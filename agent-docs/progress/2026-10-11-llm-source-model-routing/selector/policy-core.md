---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---
# policy-core: selector の純粋規則

## 変更

- `task-core::route_select` に evidence key/row、Wilson 区間、D2 の決定的順位・fallback・run_id 固定 ε 探索、D3 guardrail、判断 trace 型を追加した。
- `routing.rs` は既存 harness 境界として残るため、新 selector は `route_select/` に置く。ADR D5 の読み替えを ADR に付記した。
- I/O・DB・dispatcher との接続はこの unit の範囲外。候補の admission 済み列を入力にする純粋関数である。

## 検査

- `cargo test -p task-core --lib route_select` — exit 0 — 8 passed; 0 failed（selector 関連試験。既存 llm_route の route_select 名試験を含む）。
- 前回不合格だった `cargo clippy -p task-core --all-targets -- -D warnings` は `policy.rs` の redundant closure を `map(candidate_key)` に直して exit 0。

## 未解決

- 同工程の `evidence-db` が永続化を担当する。lane ごとの既定、daemon tick、RoutePolicy 配線は後続 unit。
