---
title: task一覧・graph・作成 画面の修正（critique 対応）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: fixed
updated: 2026-10-04
---

# task一覧・graph・作成 画面の修正

`agent-docs/progress/2026-10-04-web-ux/qa-ops/critique.md` の指摘のうち、この WU の対象画面
（`/tasks`・`/graph`・`/tasks/new`、ファイル: `web/features/tasks/task-list-view.tsx`・
`web/features/tasks/graph-view.tsx`・`web/features/tasks/task-list-query.ts`・
`web/features/tasks/graph-layout.ts`・`web/routes/tasks.index.tsx`・`web/routes/graph.tsx`・
`web/routes/tasks.new.tsx`）に当たるものを、重い順に対応した。

## 修正

### #1 — task 一覧 `/tasks`: status 絞り込みチップが生の英語コードのまま（major）

`web/features/tasks/task-list-view.tsx` の status チップの可視文字を `StatusBadge` と同じ
`statusView(s).label`（例 `ready` → 「実行待ち」）に差し替えた。チップの `value`（フォーム送信・
URL の `status=` クエリ）は元の英語コードのまま変えていない。

parity e2e（`web/e2e/parity/tasks.spec.ts`）は `getByRole("checkbox", { name: "ready" })` で
accessible name を見るため、accessible name を元の英語コードに固定する必要があった。チェックボックスに
`aria-label={s}`（例 `aria-label="ready"`）を明示し、ラベルで包む可視文字だけを日本語にした
（`aria-label` は wrapping `<label>` のテキストより優先されるため accessible name は変わらない）。

### #2 — 依存グラフ `/graph`: `root`/`depth` ラベルが生の API パラメータ名のまま（major）

`web/features/tasks/graph-view.tsx` の絞り込みフォームの可視ラベルを「起点の task id（root）」
「深さ（depth）」に差し替えた。parity e2e は `page.getByLabel("root")`/`page.getByLabel("depth")` で
accessible name を見るため、各 `<input>` に `aria-label="root"`/`aria-label="depth"` を明示して
accessible name を元の値に固定した（#1 と同じ手法）。

## 対象外として残した指摘（この WU のファイル範囲外）

- **#4 — タスクの作成 `/tasks/new`: 条件種類 select が API 型名のまま（minor）**:
  critique の対象ファイルは `web/features/tasks/create-screen.tsx` だが、この WU の Objective の
  ファイル一覧には含まれていない（`web/routes/tasks.new.tsx` はこのコンポーネントへの配置のみで
  中身を持たない）。修正せず残課題として残す。他の fix WU（task 詳細担当）の範囲にも入っていないため、
  次段（verify もしくは followup）での対応が必要。
- **#3 — task 詳細 `/tasks/$id`: run リンクの touch target 不足（major）**、
  **#5 — 変更 `/tasks/$id/changes`: `integration.state` が英語のまま（minor）**、
  **#6 — 成果物 `/artifacts`: 案件選択必須（minor）**:
  いずれも本 WU のファイル範囲外（`fix-task-detail`・`fix-changes-files-artifacts` WU の担当画面）。
  対応はそちらの記録を参照。

## 検査

- `corepack pnpm@12.6.0 -C web typecheck` exit 0
- `corepack pnpm@12.6.0 -C web lint` exit 0（既存の `styles.css` の reduced-motion `!important` 警告のみ、
  新規の警告なし）
- `corepack pnpm@12.6.0 -C web build` exit 0
- `corepack pnpm@12.6.0 -C web test` exit 0（vitest + server node:test、42 件 + vitest 全件 pass、fail 0）
- `corepack pnpm@12.6.0 -C web check:boundaries` exit 0
- `corepack pnpm@12.6.0 -C web check:parity` exit 0
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/parity/tasks.spec.ts`
  exit 0（7 passed、`ready` チップの check・`root`/`depth` の fill を含む）

## 残課題

- 上記「対象外として残した指摘」の 4 件（#3・#4・#5・#6）。
- critique.md に記録済みの screenshot tooling のタイミング問題・状態別 screenshot の欠落は、この WU の
  範囲外（`web/scripts/screenshots.mjs`・`web/e2e/support/states.ts`）のため未着手。
