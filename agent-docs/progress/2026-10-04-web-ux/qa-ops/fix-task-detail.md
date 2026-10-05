---
title: ops 画面群 visual QA — task 詳細（/tasks/$id）の修正
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: fixed
updated: 2026-10-04
---

# task 詳細（/tasks/$id）の修正

`critique.md` の task 詳細の指摘を重い順に扱った。critique の task 詳細の指摘は 1 件（一覧の #3、major）。

## 修正

| critique | 重大度 | 対応 | 変更 |
|---|---|---|---|
| #3 run へのリンクのタップ領域が横に狭い（17×44px 相当、`overview-view.tsx:94-99,390-399`） | major | 修正 | `web/features/tasks/overview-view.tsx` の ShortId リンク 5 箇所（概要カードの現在の run、概要 tab の run、親 task、WU の子 task、WU の run）を `min-w-0` → `min-w-11` + `justify-center` にした。短い ID（`R1`）でも 44×44px、長い ID は `max-w-full` と ShortId の省略で横溢れしない |
| （critique 外・mobile-audit で検出）判断・実行 tab の textarea が「名前なし」と数えられる | minor | 修正 | `decision-panel.tsx`（4 箇所）・`execution-panel.tsx`（1 箇所）の textarea に、包む label と同じ文の `aria-label` を足した。`mobile-audit.mjs` は `labels` を input にしか見ないため、包む label だけでは違反になる。accessible name は label 文と同じで変わらない |

生の色・任意値 class は足していない。入力欄の枠（`border-input`）、h1・accessible name・URL は変えていない。

## 証拠

- `corepack pnpm@12.6.0 -C web build` exit 0
- `corepack pnpm@12.6.0 -C web typecheck` exit 0、`lint` exit 0（既存の warning 5 件）、`test` exit 0（pass 42 / fail 0）
- `corepack pnpm@12.6.0 -C web e2e parity/task-detail.spec.ts` 7 passed
- `corepack pnpm@12.6.0 -C web mobile-audit`: `/tasks/T1` の違反 0 件（修正前は `@1440` の textarea 1 件）
- post screenshot: WU artifacts の `qa-ops-task-detail-post/_tasks_T1-{360,390,412,1440}.png`（`R1` が 44px 幅の枠の中央に出る）

## 残課題

- mobile-audit 全体は、範囲外の画面の textarea（`/projects/P1`、`/tasks/T1/changes`）の「名前なし」でまだ exit 1。`/tasks/T1/changes` は兄弟 WU（fix-changes-files-artifacts）の範囲、`/projects/P1` は本 task の範囲外。根本は `web/scripts/mobile-audit.mjs` が `HTMLTextAreaElement` の `labels` を見ないことで、script 側を直す方が筋が良い（verify 葉で判断）。
- critique の「loading/error の screenshot が空スケルトンで撮れる」問題は screenshot tooling 側（範囲外）。task 詳細の loading/error の見た目はこの WU でも screenshot では確かめていない。
