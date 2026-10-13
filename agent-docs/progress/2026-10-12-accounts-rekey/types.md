# accounts-rekey types: web/gui の生成型と呼び出しを (source_id, account_id) の新路に合わせる

Task: 01M4QTJR2MQYPSZC8KM3ZPRM2J

## 変えたこと

- 生成型: `web/api/generated/{types.ts,schema.json}` と `gui/app/celeris/types.ts` を `pnpm gen:types` で再生成。
  `AccountView` は `source_id`・`account_id` を持ち `adapter`・`id` を持たない。`AccountList` は `root` を持たず `roots` の key は source_id。
- web（`web/features/ops/accounts-screen.tsx`）: 一覧 `GET /api/llm/accounts`、上限 `PATCH /api/llm/sources/{s}/accounts/{a}`、
  確認・ログイン・削除は同じ `…/accounts/{a}` の下、追加は `POST /api/llm/sources/{s}/accounts`。`?adapter=` を消した。
  選択肢は source_id（`claude_oauth` / `codex_oauth` / `opencode_go`）。表示名（claude-code / codex / OpenCode Go）は変えない。
  画面の構成と見た目は変えていない。
- web の試験（`accounts-screen.test.tsx`）、e2e（`fake-daemon.mjs` の accounts 部を新路に、`ops.spec.ts`・`ops-config.spec.ts`）。
- gui（`gui/app/celeris/accounts-admin.server.ts`・`action-types.ts`・`routes/accounts.tsx`）: `AccountAdapter` を `AccountSourceId` に、
  フォーム欄 `adapter` を `source_id` に。表示の data-testid は `account-add-source`・`account-source`・`data-account-source`。
  一覧は `GET /api/v1/llm/accounts`。gui の単体試験（`accounts.test.ts`・`accounts-admin.test.ts`）と e2e `g9.spec.ts` を新路に。
- gui の fixture mock（`gui/scripts/lib/celeris-fixture.mjs`）の一覧路を新路に。
- docs: `docs/guides/providers.md`・`docs/ops/opencode-go-and-model-catalog.md`・`docs/ops/provider-llm-source-migration.md`
  の旧 `/api/v1/accounts` 路（`{adapter}/{id}`・`implicit/<id>`・`?adapter=`）を新路の記述に直した。
  `gui/docs/celeris-api-v1.md` の accounts の節（表 113〜119、§3.29〜3.35、監査表）も新路に直した。

## 検査

- `node web/scripts/gen-types.mjs --check` → exit 0（web 生成型が schema と一致）。
- gui の生成型は `pnpm gen:types` を 2 回流して md5 が同じ（`e68ad829…`）。
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0（tsc -b）。
- `corepack pnpm@12.6.0 -C web exec vitest run` → 91 files / 662 tests passed。
- `node --test server/*.test.mjs`（web）→ exit 0。
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0（エラー 0）。
- `corepack pnpm@11.27.0 -C gui test` → 1308 tests passed。
- `biome check`（web の変更 file）→ 変更 file はエラー無し。
- `biome check`（gui の変更 file）→ `app/routes/accounts.tsx` に noUnusedImports（`Tier`）と organizeImports が残る。
  どちらも base（HEAD）から既にあった。この unit では直していない。

## 残り（この unit の外）

- `crates/celeris/src/daemon/api_account_limits_tests.rs`・`crates/task-api/tests/concurrency.rs`（`/api/v1/accounts` の旧路）は
  api-tests / api-celeris の担当。統合（route 段）で確認する。
- `gui/docs/adr/0012-provider-and-account-management.md` は過去の ADR（履歴）なので書き換えない。
- `docs/api/v1/gui-api.md` は api-celeris 側で新路になっている（確認済み）。
- 画面の `/accounts`（GUI のページ path）は変えていない。
