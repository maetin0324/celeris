---
tasks: [01M5GE1BGEJDRK1AQ1Y1TK85X0]
status: running
updated: 2026-10-22
---
# 統合の自動解決で Cargo.lock の衝突を再生成で解く

ADR: `agent-docs/adr/2026-10-21-cargo-lock-regenerate-on-integrate.md`

## 実装
- `crates/task-dispatch/src/auto_resolve/{generated.rs,classify.rs}`、`auto_resolve.rs`（`ResolveContext.generated`）
- `crates/task-dispatch/src/integration.rs`（WU の段・子 task の親への統合: `rules(None)`）
- `crates/celeris/src/delivery/auto_resolve.rs`（delivery: `rules(schema_cmd)`、`review_free` に Cargo.lock）

## 試験（path 依存だけの fixture、`CARGO_NET_OFFLINE=true`、LLM なし）
- `auto_resolve::generated::tests::cargo_lock_*`（5 本: 直下と入れ子で再生成し両 crate を含み `cargo metadata --locked` が通る / Cargo.toml も衝突→人 / 隣に Cargo.toml 無し→人 / 再生成が他 file を変えた→人 / 失敗・timeout→人）
- `integration::tests::wu_cargo_lock_only_conflict_is_regenerated`（WU の段）
- `integration::tests::child_cargo_lock_conflict_is_regenerated_and_toml_conflict_needs_human`（子 task → 親）
- `delivery::tests::auto_resolve_cargo_lock_conflict_is_regenerated_and_regated`、`auto_resolve_cargo_lock_with_cargo_toml_conflict_requests_integration`（delivery）

## 証拠
- `cargo test -p task-dispatch --lib auto_resolve` → exit 0、40 passed
- `cargo test -p task-dispatch --lib integration::tests::` → exit 0、15 passed
- `cargo test -p celeris --lib auto_resolve` → exit 0、9 passed

## 本番への移行
設定の書き換えは不要（Cargo.lock 規則は内蔵、`[delivery.auto_resolve.generated]` は従来の欄のまま）。release で入れ替わるだけ。

## 未解決事項
- なし

## 提案
- なし
