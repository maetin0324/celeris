---
title: fix-r5 fix-narrow — 360/390 の /artifacts 題名と /projects/P1 表の横溢れ
tasks: [01M45PRPACBEP4X1FWPNP4FRMG]
status: done
updated: 2026-10-05
---

# fix-r5 fix-narrow

基点 b39a6f34（merge-qa 後）。範囲は web/features/artifacts・web/features/projects・web/e2e。

## (a) /artifacts 360: タスク列が 1/4 に潰れ題名が 20 行近く折れる

- 原因: `artifact-table.tsx` のタスク列が全幅で `w-1/3`（table-fixed）。360 では本文の幅が約 320px、セルの padding を引くと題名に使えるのは約 90px で、rich fixture の T1（約 110 字）が 1 行 5〜6 字で折れて 20 行前後になっていた。成果物の列も 2/3 に狭まり path が細かく折れていた。
- 修正: タスクの列（見出しと rowSpan のセル）を `md` 以上だけにした（大きさ・記録と同じ規則）。狭い幅では、同じタスクの先頭の成果物の欄の上にタスクの題名を積む。題名は `TaskLink` に寄せ、`line-clamp-2` で 2 行を超える分を省略し、`title` 属性に「題名（task id）」の全文を置く。md 以上の列の題名も同じ 2 行省略。
- 目視: 360 で題名が全幅 2 行＋省略記号、path も全幅で 4 行に収まる（`artifacts/check/artifacts-360.png`、WU の artifacts）。

## (b) /projects/P1 360・390: 『途中目標と仕事』の状態・判断待ち列が右で切れる

- 原因: `project-detail-view.tsx` の WorkTree は 3 列の表で、仕事のセルが `min-w-48`（192px）＋字下げ、状態・判断待ちは `whitespace-nowrap`。360/390 では合計幅が枠を超え、枠は `overflow-auto` で横に動くが手がかりが無く、状態・判断待ちが右で切れて見えていた。
- 修正: 640px（`sm`）未満では行を積む。見出しの行は `max-sm:sr-only`（表の意味は残す）、各 `tr` を `max-sm:flex flex-wrap`、仕事のセルを全幅（`min-w-0`）、状態・判断待ちのセルを題名の下に「状態: <badge>」「判断待ち: —／判断待ち N 件」の label: value で並べる。状態の行は仕事と同じ字下げ。640px 以上は従来の 3 列のまま。部品（web/components）は変えていない。
- 目視: 360・390 で題名の下に「状態: 実行待ち　判断待ち 1 件」が枠内に出る（`artifacts/check/P1-360.png`・`P1-390.png`）。

## e2e での固定

- 新規 `web/e2e/work/narrow-layout.spec.ts`（functional、rich fixture = screenshots と同じ data）:
  - /artifacts 360: 見えている題名要素の高さ ≤ line-height×2+2px、幅 > 180px、`title` 属性あり、T1 の title に全文。
  - /projects/P1 360・390: 枠と表の右端 ≤ viewport 幅、枠の scrollWidth ≤ clientWidth+1、各行の状態（`[data-status]`）が viewport 内で「状態:」付き、判断待ちが見える。
  - /projects/P1 1280: 3 列の見出しが見え、積みの label は隠れる。
- `web/e2e/parity/projects.spec.ts` 166 行: 390 で木の枠が横に scroll することを assert していた（旧挙動）。DAG は従来どおり scroll を assert し、木は枠に収まる（scrollWidth ≤ clientWidth+1）に置き換えた。skip・削除はしていない。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存の warning 5） |
| `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 352 passed、node test 42） |
| `corepack pnpm@12.6.0 -C web build` | exit 0 |
| `corepack pnpm@12.6.0 -C web e2e --retries=0` | exit 0（185 passed / 8 skipped、skip は既存） |
| `corepack pnpm@12.6.0 -C web e2e:nfr --retries=0` | exit 0（95 passed） |
| `corepack pnpm@12.6.0 -C web mobile-audit` | exit 0（31 path × 4 幅） |

## 未解決事項

- after-r4 の撮影と 4 点の目視記録は shots-r4 の担当（fix-r5.md）。

## 提案

- 狭い幅で行を積む表が増えてきた（/artifacts・/projects/P1）。`components/ui/table` に「sm 未満で積む」variant を設けると各画面の `max-sm:` の並びを減らせる。
