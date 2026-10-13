# web-tree: 詳細木の待ち理由

tasks: [01M4NAKDCCS5XYAAJ518MVZ134]

## 完了

- WorkUnit 行に kind と段内の順番を表示し、サーバー由来 `TaskDetail.waits` の未完 key と先行段を表示。
- 子 task 行で詳細 API の `waits` を取得して既存の待ち理由表示を再利用。
- 木の依存理由・段の順序・子 task の待ちを確認する Vitest と 390×844 E2E を追加。

## 検証

- `corepack pnpm@12.6.0 -C web typecheck` — 合格。
- `corepack pnpm@12.6.0 -C web exec vitest run features/tasks/overview-view.test.tsx features/tasks/wait-reason.test.ts` — 2 files / 5 tests 合格。
- `corepack pnpm@12.6.0 -C web exec playwright test e2e/work/task-tree-waits.spec.ts` — 1 test 合格。390×844 で横溢れなし。
- `corepack pnpm@12.6.0 -C web exec biome check --write ...` — 対象 4 files 合格。
- `git diff --check` — 合格。

## 実装メモ

子 task の一覧型 `TaskRef` に待ち欄が無いため、各行は既存の task detail query から待ち情報を読む。WU の未完依存は UI で status から推測せず、task-core が作る `waits.work_units` を参照する。
