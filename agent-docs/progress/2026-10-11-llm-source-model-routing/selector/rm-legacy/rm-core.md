---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
unit: rm-core
status: implemented
date: 2026-10-13
---
# rm-core: config・catalog API・旧 model router の撤去

## 変更

- celeris config から旧 routing module、runtime/catalog snapshot/state と daemon の差し替えを削除。[model_routing] を含む TOML は route_defaults を案内して拒否し、`model_routing_section_rejected_with_route_defaults_guidance` で確認する。
- `/llm/routing/catalog` の task-api route・型・試験を削除し、model catalog API は維持。
- model catalog の適用を source × model の route candidate に対する純関数に変更。disabled と catalog 上で明示的に unavailable の候補だけを除外する。tier override は候補選択に使わない。対象の2試験が通る。
- task-core の estimator / optimizer / policy / shadow など旧 model_router 群、LaneResolution と tier→model 解決を削除。Tier と selector、event の読み取り専用履歴型は維持。
- API schema と web generated types を UPDATE_SCHEMA=1 / gen:types で更新。config の移行手順と architecture map を更新。
- workspace clippy で検出された DispatchRoutingSettings の Default 実装を derive に置換。

## 検証

- celeris の model_routing 拒否試験、model catalog の候補除外2試験、task-api model catalog 応答試験: pass。
- task-ops `legacy_shadow_history_fixture_deserializes_and_appears_in_audit`: 先行 hist unit で pass。
- `UPDATE_SCHEMA=1 cargo test -p task-api --lib schema::tests::committed_schema_matches_generated`: pass。
- `corepack pnpm@12.6.0 -C web gen:types`: pass。
- `cargo clippy --workspace -- -D warnings`: pass。
- 再生成前の全体テストは schema drift 1 件で失敗。`UPDATE_SCHEMA=1` と `gen:types` の後、test-parallel は 4,982 件 pass、12 skipped（doctest 含む summary 4,985 pass）。
- web typecheck は web-audit が削除する routing catalog /旧 audit UI 参照で失敗する。該当画面は後続 web-audit unit の担当範囲。

## 残作業

- 後続 web-audit が web/features の旧 catalog・予約 audit UI を削除し、web typecheck/test を通す。
- record unit で全体の最終確認を行い、`selector/rm-legacy.md` の本番移行節と総括を記録する。
