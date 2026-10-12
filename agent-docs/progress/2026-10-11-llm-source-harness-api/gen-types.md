---
tasks: [01M4Q6GQV76NM3EGZ3P6EN378J]
---
# gen-types progress

## 状態
- 生成型の再生成: 完了。web（`gen-types -- --check` exit 0）・gui（退避→`gen:types`→`cmp` 差分ゼロ）。
- web: typecheck exit 0、test exit 0（91 files / 662 tests）。check コマンドそのものも既定 TMPDIR で exit 0。
- gui: typecheck exit 0、test（`TMPDIR=/local/x`）exit 0（93 files / 1308 tests）。既定 TMPDIR（93 byte）の check は `browser-phase2` の owner socket 試験 1 件が timeout で exit 1（下の「未解決」）。
- crates/ への差分: 0 行（`git diff --stat -- crates` 空）。

## 行った変更
- 前回の未解決（provider 画面の型参照で tsc が落ちる）は、ADR 2026-10-11-llm-source-harness-selection の「API と画面」の削除対象として、provider 画面一式を削除して解いた（作り直しはしない）。
  - web: `features/ops/providers-{screen,screen.test,form,llm-source}`・`routes/providers.tsx` を削除。`isDenied`/`deniedMessage` は `features/ops/denied.ts` に移し、models-screen・secrets-section の import を付け替えた。nav（`components/shell/nav-items.ts`）と `routeTree.gen.ts` の `/providers` を削除。models の breadcrumb の `/providers` リンクを外した。
  - gui: `routes/providers.tsx`・`celeris/providers-admin.server.ts`・`test/unit/providers{,-admin,-llm-source}.test.*` を削除。`routes.ts`・`root.tsx` nav・`help.tsx` の provider 項目を削除。`celeris/action-types.ts` の provider 型（ProviderOpOutcome・ProviderCheckOutcome・ProviderActionResult）と `components/Flash.tsx` の `ProviderActionFlash` を削除。`routes/accounts.tsx` の「モデル階層」節（`GET /providers` 読み）を削除。`test/unit/routing-catalog.test.tsx` は label 試験だけ残した（描画試験は provider 画面の部品だったため削除）。
  - 試験の期待値: `web/server/spa-routes.test.mjs` の画面数を 40→39（`/providers` 削除分）。
  - e2e: `web/server/spa-routes.js`・`e2e/support/screens.ts`・`e2e/support/states.ts`・`e2e/states/states.spec.ts` の `/providers` 行を削除。

## 証拠コマンド（抜粋）
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0（error TS 0 件）
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0（error TS 0 件）
- `corepack pnpm@12.6.0 -C web gen:types -- --check` → exit 0
- gui: `gen:types` の後 `cmp` で退避版と一致 → exit 0
- `TMPDIR=/local/x corepack pnpm@12.6.0 -C web test` → 91 files / 662 tests passed、exit 0（既定 TMPDIR の check 形でも同じ数で exit 0）
- `TMPDIR=/local/x corepack pnpm@11.27.0 -C gui test` → 93 files / 1308 tests passed、exit 0
- 既定 TMPDIR（93 byte）で gui の check 形を実行 → `test/unit/browser-phase2.test.tsx` の「the local control socket is 0600 ...」が 5000ms timeout（1307 passed / 1 failed、exit 1）
- `git diff --stat -- crates` → 空

## 未解決
- gui check の `browser-phase2` 1 件が既定 TMPDIR で落ちる。原因は試験が `tmpdir()` 直下に作る unix socket の path が SUN_LEN（108）を超えること。本 unit では試験の socket 置き場を変えていない（置き場は `/tmp` などの共有の場所になり、`$TMPDIR` の規則に反するため）。`TMPDIR=/local/x` では通る。
- web の `features/ops/model-assignments.tsx` が `/api/providers`（`providerKeys`）を呼ぶ。型は通るが実行時は 404。model role assignment の画面・client は ADR で削除対象。ui-v2 もしくは assign 系の削除で扱う。
- cache domain の `providers`（`api/queries/keys.ts`・`stale-time.ts`・`realtime/invalidation-map.ts` と試験）が残る。実 endpoint は無い。
- e2e `web/e2e/work/narrow-r6.spec.ts` の (1)(7) は `/providers` の試験。他の試験を含むので、この unit では消していない。
- gui `lib/llm-sources.ts` の `ProviderLlmSourceDisplay` と、`routes/clusters.tsx`・`celeris/org-admin.server.ts` の comment に provider の語が残る（型・動作には影響なし）。

## 提案
- check の TMPDIR を短くする（`mkdir -p /local/x && TMPDIR=/local/x`）か、`browser-phase2` の socket 置き場を短い固定 path にする。どちらか。後者は試験の書き換えになるので人が選ぶ。
- ui-v2 で model-assignments の画面・client（`web/api/model-assignments.ts`・`model-assignments.tsx`）と provider の cache domain を削除し、e2e の `/providers` 試験を消す。

## 再実行（attempt 2）
- check の TMPDIR を `/local/x` に固定して再実行。web: `gen-types --check`・typecheck・test（91 files / 662 tests）exit 0。gui: gen:types と cmp・typecheck・test（93 files / 1308 tests）exit 0。
- 試験は書き換えていない（browser-phase2 は短い TMPDIR で通る）。crates/ 差分 0。
