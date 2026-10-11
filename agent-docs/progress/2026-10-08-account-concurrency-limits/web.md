---
title: web: アカウント画面で上限を編集（暗黙アカウントを含む）、プロバイダ画面から concurrency を外す
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# web: アカウント画面で上限を編集（暗黙アカウントを含む）、プロバイダ画面から concurrency を外す

2026-10-08、WorkUnit `web`（run 01M4DTRN0S9KP504MFRSA3263X）。ADR 2026-10-08-account-concurrency-limits、gui-api.md §3.29・§3.29a。crates は変えていない。

## 実装

- `web/features/ops/accounts-screen.tsx`
  - 各カードに上限の編集欄（`LimitEditor`）: 数値入力（1〜32）・「上限を保存」・「既定に戻す」（保存値が無いときは押せない）。
    検査に落ちた値は送らず項目の error（`aria-invalid`・`aria-describedby`）、API のエラーは「保存できませんでした: …」を同じ欄に出す。
    403/401 は既存の権限の案内に任せ、上限の操作も止める（案内文に「上限の変更」を追加）。
  - `PATCH /api/accounts/{adapter}/{id}` を `useActionResult`（`apiMutate`）で送る。本文 `{max_runs: n | null}`。
  - 「実行中の run」の行を `in_use / 実効上限 effective_max_runs（出どころ）` に変更。出どころは `max_runs_source`
    （このアカウントに保存した上限 / 旧設定 [[providers]].concurrency の値 / 既定 max_runs_per_account = N）。
  - 暗黙アカウント（`implicit: true`）は「暗黙アカウント」の badge と説明を出し、使用量・ログイン・確認・削除を出さず、同じ編集欄だけを持つ。
  - 取り直しで保存値・実効値が変わると編集欄を作り直す（key）。
  - 純関数 `parseMaxRunsInput`・`accountLimitTarget`・`submitAccountLimit`・`limitSourceLabel` を export（node 環境の vitest で試すため）。
- `web/features/ops/providers-form.ts`: `ProviderInput`・`buildProviderBody`・`validateProviderForm` から concurrency を除去（送らない）。
- `web/features/ops/providers-screen.tsx`: 設定・追加 form の「同時実行数（concurrency）」欄を除去。要約から `concurrency N` を除去。
  状態の表の「同時実行」列は `in_use / concurrency` をやめ「アカウントで管理」（/accounts への Link）。カードと追加 form に `ConcurrencyNotice`
  （アカウント画面への導線と暗黙アカウントの説明）。`ProviderCard`・`ProviderCreateForm` を試験用に export。
- e2e: `e2e/support/fake-daemon.mjs` の accounts に上限の欄・暗黙アカウント `qwen-local`・`PATCH /accounts/{adapter}/{id}` を追加。
  `e2e/admin/ops-config.spec.ts` の providers 追加 form の試験を concurrency 無しに直し、accounts の上限（保存・検査・既定に戻す・暗黙アカウント・API エラー）を追加。
  `e2e/parity/ops.spec.ts`（変更は model で確認）、`e2e/work/narrow-r6.spec.ts`（「アカウントで管理」）を更新。

## 試験

| 受け入れ条件 | 試験 |
| --- | --- |
| 0: 保存・既定に戻す・検証とエラー | `accounts-screen.test.tsx` の「アカウントごとの上限」: 入力検査、保存の PATCH、既定に戻す（null）、検査に落ちた値は送らない、API エラー文言・403 は項目にしない、画面の実効上限・上書きの有無・既定に戻すの無効、API エラーの表示。e2e `ops-config.spec.ts` の accounts 2 件 |
| 1: 暗黙アカウントを同じ UI で編集 | 同 describe の「暗黙アカウント」（印・ログイン/確認/削除なし・`/api/accounts/implicit/opencode-qwen` へ PATCH）、e2e の `qwen-local` |
| 2: プロバイダ画面に concurrency 入力が無い | `providers-screen.test.tsx` の「同時実行数はアカウントで管理する」（body に無い、表は導線、要約に無い、ProviderCard / ProviderCreateForm に入力欄が無い） |
| 3: typecheck・lint・test | 下記 |

- `node_modules/.bin/tsc -b`（= `pnpm typecheck`）: exit 0
- `biome check .`（= `pnpm lint`）: exit 0（styles.css の既存 warning 4 件のみ）
- `TMPDIR=<短い dir> corepack pnpm@12.6.0 test`: exit 0（vitest 85 files / 640 tests、node --test 78 pass）。
  workspace の長い TMPDIR では `server/browser-live.test.mjs` が UNIX socket のパス長で止まる（環境要因。短い TMPDIR で 19/19 通過。この WU で server/ は触っていない）。
- `WEB_E2E_SCOPE=functional playwright test e2e/admin/ops-config.spec.ts e2e/parity/ops.spec.ts e2e/work/narrow-r6.spec.ts e2e/admin/models.spec.ts`: 38 passed
- `WEB_E2E_SCOPE=all playwright test e2e/a11y e2e/states --grep "accounts|providers"`: 6 passed（axe /accounts を含む）
- `node scripts/check-boundaries.mjs`・`node scripts/check-parity.mjs`: exit 0

本番 config・systemd・daemon・本番 DB は変更していない。

## 再開時の確認（run 01M4E2GVDW7HM2BXW4RHH7YQ9F）

先行 run の実装・e2e fixture をそのまま使用し、今回の変更は `accounts-screen.tsx`・
`accounts-screen.test.tsx` とこの進捗記録のみ。crates と本番環境は変更していない。

- PATCH 本文 `{max_runs: n | null}` に `satisfies Pick<AccountView, "max_runs">` を付け、生成型で欄名と値を確認する。
- adapter/id に予約文字がある場合のパスのエンコードを試験に追加。
- 暗黙アカウントに保存値がある場合の実効上限・入力値・「既定に戻す」が有効であることを試験に追加。
- `node_modules/.bin/tsc -b`: exit 0。
- `node_modules/.bin/biome check .`: exit 0（既存 CSS warning 4 件）。
- `TMPDIR=<workspace>/t corepack pnpm@12.6.0 test`: exit 0（vitest 85 files / 642 tests、node --test 78 pass）。
  短い workspace 内 TMPDIR を使用し、UNIX socket のパス長制約を回避。`/tmp` へのコピーは行っていない。
- `node scripts/gen-types.mjs --check`・`node scripts/check-boundaries.mjs`・`node scripts/check-parity.mjs`: exit 0。

- `TMPDIR=<workspace>/t WEB_E2E_SCOPE=functional node_modules/.bin/playwright test e2e/admin/ops-config.spec.ts e2e/parity/ops.spec.ts e2e/work/narrow-r6.spec.ts e2e/admin/models.spec.ts`: exit 0、38 passed。
- `TMPDIR=<workspace>/t WEB_E2E_SCOPE=all node_modules/.bin/playwright test e2e/a11y e2e/states --grep 'accounts|providers'`: exit 0、6 passed。
- 対象画面・試験と進捗記録の `git diff --check`: exit 0。

全受け入れ条件を確認済み。今回の検証ログは workspace の `artifacts/web-*-current.log` に保存。
