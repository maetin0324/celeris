---
tasks: [01M44NN2DCZXV0TZ5FXX5TMN58]
---
# Phase 1 (p1-model): 型・kernel・旧設定 adapter の close

完了日: 2026-10-05

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` の Phase 1 の範囲を、統合後 HEAD（`d3c65029`）で検査した。実装は段 sync・kernel・dispatch-legacy・proxy-legacy・config・wire-fix・catalog-api・gui-catalog・web-catalog・migration-doc・surface-fix の各段で入っており、この段（close）では新しいコードは書いていない。検査で落ちた箇所はなかった。

## 検査の結果（完了検査 ADR §10）

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| 書式 | `cargo fmt --all -- --check` | exit 0（差分なし） |
| 全 workspace 試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary 原文: `3934 tests run: 3934 passed (1 slow), 12 skipped`。doctest exit 0 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| clippy（試験コード） | `cargo clippy -p <crate> --all-targets -- -D warnings`（task-core、llm-proxy、task-dispatch、task-api、task-ops、celeris、celerisctl） | 全て exit 0 |
| 文書検査 1 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | `check-doc-layout: ok`、exit 0 |
| 文書検査 2 | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| 文書検査 3 | `python3 scripts/dev/check-architecture-map.py` | `OK: 232 件のパスを確認した`、exit 0 |
| ADR 番号 | `bash scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (137 files)`、exit 0 |

## routing_* / legacy_equivalence / 既存回帰の実行件数

`cargo nextest run --workspace --no-fail-fast --status-level pass -E 'test(/routing_|legacy_equivalence|…4 件…/)'` → exit 0、`55 tests run: 55 passed, 3891 skipped`。0 件実行は無い。

既存回帰 4 件（すべて PASS）:
- `llm-proxy config::tests::cheap_only_default_and_legacy_qwen_config`
- `celeris config::tests::provider_kind_legacy_production_inference_warnings_and_cheap_tier`
- `task-core model_routing::tests::tier_resolution_never_substitutes_a_missing_or_disabled_model`
- `task-dispatch dispatcher::tests::cheap_local_first::cheap_local_first_picks_local_when_free`

ADR §10 Phase 1 の表の試験（PASS）:
- `routing_profile_shared_across_deployments`、`routing_kernel_constraints_before_score`、`routing_kernel_stable_ties_and_unknowns`、`routing_old_events_deserialize_without_optimizer`（task-core `model_router::tests`）
- `routing_legacy_config_normalizes_with_warnings`（celeris `config::tests`）、`routing_config_reload_is_atomic`（celeris `tests`）
- `routing_catalog_redacts_secrets_and_keeps_legacy_fields`（task-api `tests/routing_catalog.rs`）
- `legacy_equivalence` 系: `routing_legacy_equivalence_qwen_is_cheap_only`（celeris）、`routing_proxy_legacy_equivalence_tiers_and_fallback`（llm-proxy）、`routing_dispatch_legacy_equivalence_matrix`（task-dispatch）

GUI/web の表の試験（各 runner で実行）:
- `routing_catalog_missing_metadata`: web `pnpm exec vitest run features/ops/routing-catalog.test.tsx` → 1 passed。`pnpm typecheck`（tsc -b）exit 0。
- gui `pnpm exec vitest run test/unit/routing-catalog.test.tsx` → 3 passed（うち `routing_catalog_missing_metadata` の 2 件）。`pnpm typecheck`（react-router typegen && tsc -b）exit 0。
- 依存は `pnpm install --offline` で入れた（gitignore 済み、差分は無い）。

## ADR 付記

`agent-docs/adr/2026-10-04-multi-objective-model-routing.md` の末尾に「付記（2026-10-05、Phase 1 の実装済み範囲）」を追加した。Phase 2 以降の機能は含めていない。

## 差分

`git status` は付記・本進捗ファイルだけが変わる見込み。コードの差分は作らなかった。

## 未解決事項

- `scripts/dev/test-parallel.sh` の集計行は `passed=0` と出す（nextest Summary の `N passed (M slow)` を数えられない既知の問題）。件数は上の Summary 原文で確認した。
- gui の `pnpm lint`（biome）は既存の差分で以前から落ちる。この段では触っていない。
- `docs/ops/model-routing-migration.md` の本番手順は未実施（本番の config・daemon・DB は変えていない）。本番への昇格と mode の切り替えは人の判断。

## 提案

- Phase 2 に入る前に、`scripts/dev/test-parallel.sh` の集計パーサーを nextest の `N passed (M slow)` に対応させる。
- 次の close 葉の検査には、統合後の `cargo clippy -p <crate> --all-targets` を全 crate の範囲で含める（clippy --workspace は #[cfg(test)] を見ない）。
