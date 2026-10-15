---
unit: policy-core
status: done
completed: 2026-10-15
---
# policy-core

- ADR `agent-docs/adr/2026-10-15-mlsys-min-routing-policy.md`（親 D1 の優先順位は不変）。
- `crates/task-core/src/route_select/{lane,availability,escalation}.rs` と試験 `mlsys_policy_tests.rs`（純粋関数のみ）。
- 証拠: `cargo test -p task-core route_select` 全通過（既存試験は無変更）、`bash scripts/dev/test-parallel.sh` 5310 passed / 0 failed、`cargo clippy --workspace -- -D warnings` exit 0。

## 未解決・提案
- task-core は jev-classifier に依存せず `ClassificationHint` を持つ。変換は wire-mode が行う。
- Jev 実 API 統合は未検証（fixed fallback とは別記録）。
