---
tasks: [01M4QHBJW4GBG10QA8F231W7XZ]
status: done
title: model_routing の Legacy mode と llm-proxy の legacy catalog・旧経路を消す
updated: 2026-10-12
completed: 2026-10-12
---

# Legacy routing purge progress

## mode-drop

- Removed `RoutingMode::Legacy` and the optimizer's legacy rank branch.
- `[model_routing]` now requires an explicit `mode = "shadow"` or `mode = "enforce"`. A config without the table still loads with routing disabled. The dispatcher carries whether routing was explicitly configured so the absent-table case does not install or enable proxy shadow behavior.
- Updated routing catalog fixtures, the additive event fixture, and the API/event schemas. Added a load-error test that checks both accepted mode spellings.
- Added the ADR note that runtime source/model selection belongs exclusively to `task_core::llm_route::route_select`; model_routing is for shadow observation or input to task `01M4M8D4AQ`.

### Evidence

- `cargo check --workspace --tests` — pass.
- `cargo test -p celeris` — pass (378 unit tests plus integration tests).
- `UPDATE_SCHEMA=1 cargo test -p task-api` — generated `docs/api/v1/api-v1.schema.json`; test run exposed a stale event fixture, corrected below.
- `UPDATE_SCHEMA=1 cargo test -p task-core` — schema generation completed; one additive event fixture still encoded `mode = legacy`, corrected to `shadow`.
- `cargo test -p task-core` — pass (898 tests, sidecar integration test, and doc tests).
- `cargo test -p llm-proxy` — pass (55 unit tests and integration tests).
- `cargo test -p task-api --test routing_catalog` — pass (2 tests).
- `cargo test -p task-api --test daemon_providers_config schema_endpoint_returns_the_committed_file` — pass.
- `cargo test -p task-dispatch routing` — pass (8 tests).
- `cargo test -p celerisctl routing_cli` — pass (2 tests).
- `sh "$CELERIS_WU_SCOPE_PATHS"` — listed only this unit's intended paths.
- `rg -n 'RoutingMode::Legacy' crates` — no matches.
- `git diff --check` — pass.

## catalog-drop

- Removed `llm-proxy/src/legacy_catalog.rs` and its `[llm_proxy]`/role-assignment normalizer and tests.
- Added `llm_proxy::catalog::ProxyCatalog`; daemon wiring copies the validated `routing_catalog_snapshot`, or uses an empty catalog when no snapshot exists. Config declarations reach the proxy through the validated catalog built during config loading. Estimator shadow only consumes the startup catalog.
- Renamed `rank_pool` to `rank_source_accounts`; selection documentation now defines account identity as `(source_id, account_id)`.
- Added a daemon test for the empty-catalog fallback and a declared source catalog flowing through the validated snapshot.
- `cargo test -p llm-proxy` — pass (49 unit, 28 integration).
- `cargo test -p celeris proxy_catalog_uses_validated_snapshot_and_empty_when_absent` — pass.
- `cargo clippy -p llm-proxy -p celeris --all-targets -- -D warnings` — pass.
- `sh "$CELERIS_WU_SCOPE_PATHS"` — pass; all changed paths are in the catalog-drop scope.
- `! rg -n 'legacy_catalog|normalize_legacy_config|rank_pool|LegacyCatalog' crates` — pass.
- `git diff --check` — pass.
- `cargo test -p task-api --test routing` — pass (6 tests), including the corrected additive routing event fixture.
- `UPDATE_SCHEMA=1 TMPDIR=/tmp/xd cargo test -p task-api` — generated schema and unit/most integration suites passed; exposed the stale `mode = legacy` fixture in `tests/routing.rs`. After updating it, the complete routing integration suite passed.
- `cargo test -p celeris routing_table_requires_explicit_mode` — pass, verifying `Config::load` rejects an omitted mode while a config without `[model_routing]` still loads.
- `! grep -rn 'RoutingMode::Legacy' crates/ && ! grep -rn 'unwrap_or(RoutingMode' crates/celeris/src/config/` — pass after replacing the two validated-config fallback expressions with `unwrap_or_default()` (the Shadow default is only a safe internal value; `[model_routing]` without `mode` is rejected during parsing).
- `cargo check --workspace --tests` — pass after the final mode fallback cleanup.
- `cargo test -p celeris routing_table_requires_explicit_mode` — pass (1 test).
- `cargo test -p task-core` — pass (898 unit tests, 1 sidecar integration test, 3 doc tests).
- `sh "$CELERIS_WU_SCOPE_PATHS"` — pass; changed paths remain within the mode-drop scope.
- `git diff --check` — pass.

## close

- 生成型を schema に合わせた。`UPDATE_SCHEMA=1 cargo test -p task-api` で `docs/api/v1/api-v1.schema.json` は変化なし。`gui` と `web` の `gen:types` で `RoutingMode` から `legacy` が消えた（`gui/app/celeris/types.ts`、`web/api/generated/types.ts`、`web/api/generated/schema.json`）。
- 運用文書から Legacy mode と legacy catalog の説明を消した。`docs/ops/model-routing-migration.md` は `mode` 必須・route_select だけが実行の選択、旧形からの導出表・警告表・`mode = "legacy"` の手順を削除。`docs/architecture-map.md` の `legacy_catalog` の参照を消した。`config/*.example.toml` に `[model_routing]` は無く、変更は不要。
- ADR 2026-10-11-llm-source-harness-selection.md の「2026-10-12 付記（Legacy routing の撤去）」は mode-drop の unit で書かれ、この unit では変えていない。

### 本番への移行（人が実行する。本番 host は agent は触らない）

対象は `~/.config/celeris/config.toml`（と `config.toml` が読み込む include）。新しい release の daemon は `[model_routing]` の表に `mode` が無いと起動を拒否する（`[model_routing].mode is required; set mode = "shadow" or mode = "enforce"`）。`mode = "legacy"` は読み込みエラーになる。`[model_routing]` の表が無い config は今までどおり読み込み、routing の観測は止まる（選択は変わらない）。

1. 確認: `grep -n -E '^\[model_routing\]|^mode *=' ~/.config/celeris/config.toml`
   - `[model_routing]` が無ければ何もしない（この節は終わり）。
   - `[model_routing]` があり、直下に `mode = "shadow"` か `mode = "enforce"` が無い、または `mode = "legacy"` があるなら 2 へ。
2. 控え: `cp -a ~/.config/celeris/config.toml ~/.config/celeris/config.toml.bak-legacy-purge-YYYYMMDD`
3. 編集: `[model_routing]` の見出しの直下に `mode = "shadow"` を書く（既に `enforce` を使っていたなら `mode = "enforce"`。`mode = "legacy"` の行は消す）。`enforce` は `[model_routing.estimator] kind = "heuristic"` が要る（[migration 手順 §8](../../docs/ops/model-routing-migration.md)）。
4. 確認（daemon を起動しない）: `celerisctl config to-harnesses --config ~/.config/celeris/config.toml > /dev/null; echo $?` が `0`。出力の一時ファイルは作らない。
5. 反映: 新しい release に切り替える前に設定を直す（旧 binary は `mode` 無しを legacy として読むので、旧 release に戻すときは控えの config と組で戻す）。切り替え後は `POST /api/v1/reload` か service の再起動で反映し、`journalctl --user -u 'celeris@<release>.service' -n 200 --no-pager | grep model_routing` にエラーが無いことを見る。

### 全体検査（本 unit の証拠）

- `UPDATE_SCHEMA=1 cargo test -p task-api` — exit 0。`task-api` の各 test binary は ok（最大 `79 passed; 2 ignored`）。
- `corepack pnpm@11.27.0 -C gui gen:types`、`corepack pnpm@12.6.0 -C web gen:types` — exit 0（差分は上の `RoutingMode` 3 ファイル）。
- `TMPDIR=/local/x bash scripts/dev/test-parallel.sh` — exit 0（`test-parallel: ok`）。`CELERIS_TEST_SUMMARY`: `passed 4898, failed 0, ignored 14`、nextest `exit 0`、doctest `exit 0`。
- `cargo clippy --workspace -- -D warnings` — exit 0（`Finished`、警告 0）。
- `sh scripts/dev/check-doc-links.sh` — `check-doc-links: ok`。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — `check-doc-layout: ok`。
- `grep -rn "legacy_catalog\|normalize_legacy\|RoutingMode::Legacy" docs config crates scripts gui/app web/api` — 0 件。
- 前回の不合格の修正（check 2 件）: `gui/test/mock-celeris/fixtures.ts` の `RoutingCatalogView` の `mode: "legacy"` を `"shadow"` にし、`web/features/ops/routing-catalog.ts` の `legacy` の表示行を消した。`web/e2e/support/fake-daemon.mjs` の catalog fixture も `mode: "shadow"` にした（これが `routing_catalog_missing_metadata` の web 試験を落としていた）。`corepack pnpm@12.6.0 -C web typecheck` — exit 0。`corepack pnpm@11.27.0 -C gui typecheck` — exit 0。
- 前回の不合格の修正（check 1 件）: この進捗ファイルの front matter に `title` と `updated: 2026-10-12` を足した。`sh scripts/dev/check-doc-links.sh` — `check-doc-links: ok`。`sh scripts/dev/check-adr-numbers.sh` — `ok (182 files)`。`sh scripts/dev/progress-index.sh --check` — `progress-index --check: ok`。
- `corepack pnpm@11.27.0 -C gui test` — 93 files、1308 tests passed。`corepack pnpm@12.6.0 -C web test` — 91 files、662 tests passed。
- `sh scripts/dev/check-adr-numbers.sh` — `ok (182 files)`。`sh scripts/dev/progress-index.sh --check` — ok。
- 範囲: `crates/` は今回の unit の範囲外なので触らなかった。前の試行が残した `crates/` の 7 file の差分（整形 4 file と注釈 3 箇所）は HEAD に戻した（`git restore --source=HEAD`）。範囲 check は通る。

## 未解決事項（この unit の範囲外。crates/ を触る次の unit で直す）

- `cargo fmt --check` は HEAD（前の unit の変更）で落ちる: `crates/task-api/src/llm_sources.rs:52`、`crates/task-core/src/model_catalog/tests.rs:184,208`、`crates/task-core/src/store/migrations.rs:227`、`crates/task-core/src/store/model_catalog.rs:116,313,357`。空白の整形だけ。
- 注釈が旧仕様のまま残る（コメントだけ）: `crates/celeris/src/daemon/bootstrap.rs:584`（「既定 legacy」）、`crates/celeris/src/daemon/routing_shadow_tests.rs:213`（「mode legacy」）、`crates/task-dispatch/src/dispatcher.rs` の `dispatch_routing` の doc（「既定 legacy」）。final review の「mode の既定を legacy とする記述」に当たる。
- `crates/celeris/src/config/model_routing.rs:642,671-674` の policy 上書きは今も `[llm_proxy] prefer_free` を読み、`model_routing.policies.<lane> overrides llm_proxy.prefer_free` の互換警告（`crates/celeris/src/config/mod.rs:514` の `model routing compatibility warning`）を出す。旧 llm_proxy 設定からの導出の残り。

- `docs/ops/model-routing-migration.md` の §9.5（replay report）は policy key `legacy`（実際の primary）を使う。`celerisctl routing evaluate` の report の key で、Legacy mode ではないが語は残る。key を変えると report schema が変わるため、残した。改名するかは人の判断。

## 提案

- 次の unit（crates/ の範囲）で、上の未解決 3 点（fmt の 7 箇所、注釈 3 箇所、`prefer_free` 導出と警告）を消す。`prefer_free` は `[model_routing.policies.<lane>]` の欄だけを残す。同じ変更で migration 手順の `prefer_free` 行（§2 の書き方の表）を確認する。
- replay の `legacy` key は `primary` などへの改名を人に確認してから行う。
