---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: provider 管理 API と model assignments を削除
updated: 2026-10-11
---
# provider 管理 API と model assignments を削除

## 変更

- task-api router と CoS admin registry から `/providers` CRUD/list/check と model assignment API を削除。`model_assignments.rs`、provider 管理 handler・file type・CRUD tests を削除した。`POST /reload` は config reload として残した。
- `ConfigView.providers` と provider API 型（`ProviderConfigView`、`ProviderView`、`Providers`、provider stats/check response）を削除。account check result は `AccountCheckResult` として account API に残す。
- celeris 側から provider check の AdminRequest、daemon handler、tick-loop channel を削除。`/config` の provider summary も削除。
- API schema、`docs/api/v1/gui-api.md`、GUI と web の生成型を更新した。
- `gui/app/routes/providers.tsx` と accounts の provider summary が削除された API/types をまだ参照する。画面移行は `ui-v2` WorkUnit が担当するため、この WorkUnit では未解決として記録する。

## 検証

- 前回 check 失敗の原因: provider API のコメントが `crates` に残り、CoS の操作一覧・ADR の固定表には provider API と model assignment 操作が残っていた。task-api test では `/api/v1/reload` が router に二重登録されていた。コメントと一覧を更新し、重複 route を除いた。
- 同じ check `! test -e .../handlers/providers.rs && ! test -e .../model_assignments.rs && ! grep ...`: pass。
- `TMPDIR=/tmp cargo test -p task-api`: pass（79 passed, 2 ignored と integration tests 全件）。既定 TMPDIR では browser_e2e 4 件が fixture readiness timeout したが、短い `/tmp` path で再実行すると pass。
- `TMPDIR=/tmp cargo test -p celerisctl`: pass（123 unit tests と integration tests）。古い `models assign` / `models unassign` CLI 操作と対応 test を削除した。
- `cargo clippy --workspace --all-targets -- -D warnings`: pass。
- `cargo check -p task-api -p celeris --all-targets`: pass。
- `cargo clippy -p task-api -p celeris --all-targets -- -D warnings`: pass。
- `UPDATE_SCHEMA=1 cargo test -p task-api schema_matches`: 1 passed。
- `corepack pnpm@11.27.0 -C gui install --frozen-lockfile --store-dir "$TMPDIR/pnpm-store"`、続いて `corepack pnpm@11.27.0 -C gui gen:types`: pass。
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile --store-dir "$TMPDIR/pnpm-store-web"`、続いて `corepack pnpm@12.6.0 -C web gen:types`: pass。`install --offline` はローカル store に `browserslist@4.29.0` tarball が無く失敗したため、一時 store で frozen lockfile install を実行した。
- `cargo fmt --all`: pass。

## 未解決

- GUI の provider route/admin 導線と accounts の `GET /providers` 依存は ui-v2 に引き継ぐ。生成型はすでに更新されているので GUI typecheck はその移行後に通す。
- 本番の設定/API移行操作は行っていない。`[[providers]]` と providers.d の設定撤去は config-drop WorkUnit の対象。
- 追試: `UPDATE_SCHEMA=1 cargo test -p task-api schema_matches`: pass（1 passed）。
- 追試: `cargo test -p task-core -p task-api -p task-worker schema_matches`: pass（task-api schema 1 passed、task-core/task-worker は対象試験なし）。
- 追試: 対象 crate/CLI から provider 管理 API・model assignment の識別子を否定検索、OpenAPI paths の `/api/v1/providers` 不在、`git diff --check`、`sh "$CELERIS_WU_SCOPE_PATHS"`: pass。
