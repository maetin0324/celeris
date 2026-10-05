---
title: fix-r5 — 前の visual-qa の取り込みと目視 4 点の修正・after-r4
tasks: [01M45PRPACBEP4X1FWPNP4FRMG]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# fix-r5 — 前の visual-qa の取り込みと目視 4 点の修正・after-r4

計画: merge-qa → fix-narrow・fix-home-states（並行）→ shots-r4。各葉の詳細は `fix-r5/fix-narrow.md`・`fix-r5/fix-home-states.md`。

## 取り込み

- 7914a525（branch celeris/01M44C029SCGEZHEK57WEK3QNB）を `git merge --no-ff` で取り込み済み。`git merge-base --is-ancestor 7914a525 HEAD` → exit 0。

## 4 点の原因と修正

| 点 | 原因 | 修正 | e2e での固定 |
| --- | --- | --- | --- |
| (a) /artifacts 360 の題名が 20 行近く折れる | `artifact-table.tsx` のタスク列が全幅で `w-1/3`（table-fixed）。360 では題名に約 90px しか無く、T1 の長い題名が 1 行 5〜6 字で折れた | タスク列を md 以上だけにし、狭い幅では先頭の成果物の上に題名を全幅で積む。題名は `line-clamp-2`、`title` 属性に「題名（task id）」の全文 | `web/e2e/work/narrow-layout.spec.ts`（題名の高さ ≤ 2 行、幅 > 180px、title 全文） |
| (b) /projects/P1 360・390 の『途中目標と仕事』で状態・判断待ちが右で切れる | `project-detail-view.tsx` の WorkTree が 3 列表で、仕事セル `min-w-48`＋字下げ、他列 `whitespace-nowrap`。枠の横 scroll に手がかりが無かった | 640px 未満では行を積む（見出しは `max-sm:sr-only`、各行 flex-wrap、「状態: <badge>」「判断待ち: …」を label: value で題名の下へ） | 同 spec（枠の scrollWidth ≤ clientWidth+1、状態・判断待ちが viewport 内）。`parity/projects.spec.ts` の旧「木が横 scroll する」assert を「枠に収まる」へ置き換え（削除・skip なし） |
| (c) screenshots --states の error が保留のまま撮られる | --states が `page.goto` 直後に待たずに撮っており、loading 用の「保留で撮る」書き方を全状態に使っていた | `web/e2e/support/states.ts` に `capture: held / alert / settled` を足し、`waitForStateCapture` で error は `role=alert` 内の「再試行」が見えるまで待つ。loading は従来どおり保留 | screenshots 自体（下の error 変種で確認） |
| (d) ホーム 1440 の通知の右に空の枠の下辺が浮く | DOM で確認: 会話枠 `div[data-home-console]`（top=349）の中の ConsoleView が末尾追従で 31px 送られ、宛先・「新しい会話」の行（button top=318）が枠の上端で切れて、button の下辺 13px だけが枠内に残っていた | `features/console/console-view.tsx` の `data-console-toolbar` 行を contained（ホームの枠）のときだけ `sticky top-0 z-10 bg-surface` にした。/console・/org/:id は不変 | `web/e2e/shell/home-layout.spec.ts`（x 1300〜1415・y 340〜360 に切れた border 要素が無い。修正前 build で失敗を確認済み） |

## after-r4 の目視（shots-r4）

撮影: `corepack pnpm@12.6.0 -C web build` の後、`node web/scripts/screenshots.mjs --out <WU artifacts>/after-r4`（128 枚・32 画面、exit 0）と `--states --out 同`（116 枚・8 状態、exit 0）。after-r4 は合計 **240 枚**（同名の上書きを含む）、うち error 変種 16 枚。置き場所は `/local/celeris/data/workspaces/01M45PRPACBEP4X1FWPNP4FRMG/wu/shots-r4/artifacts/after-r4/`。

| file | 判定 |
| --- | --- |
| `_artifacts-360.png` | (a) 直った。題名は全幅 2 行＋省略記号、path も全幅 4 行で読める |
| `_projects_P1-360.png` | (b) 直った。題名の下に「状態: 実行待ち　判断待ち 1 件」が枠内に出る |
| `_projects_P1-390.png` | (b) 直った。同上、右の切れなし |
| `_-1440.png` | (d) 直った。「宛先: CoS」「新しい会話」の行が会話枠の上端に留まり、通知の右に浮く空の枠は無い（会話の先頭 block は末尾追従で上が隠れるが、これは scroll 位置で意図どおり） |
| `error-_tasks-390.png` | (c) 直った。「取得に失敗しました。表示を更新できません。再取得してください。」と「再試行」 |
| `error-_inbox-1440.png` | (c) 直った。「判断待ちを取得できません。…」と「再試行」 |
| `error-_tasks_T1-360.png` | (c) 直った。「取得に失敗しました。…」と「再試行」 |
| `error-_providers-412.png` | (c) 直った。「プロバイダを取得できません。…」と「再試行」 |

shots-r4 では追加の修正は要らなかった（コード変更なし）。

## 検査（shots-r4、HEAD cc69851c）

| コマンド | 結果 |
| --- | --- |
| `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | exit 0 |
| `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| `corepack pnpm@12.6.0 -C web lint` | exit 0（既存 warning 5） |
| `corepack pnpm@12.6.0 -C web test` | exit 0（vitest 58 files / 352 passed、node test skip 0） |
| `corepack pnpm@12.6.0 -C web build` | exit 0 |
| `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /' --retries=0` | exit 0（186 passed / 8 skipped、skip は既存） |
| `corepack pnpm@12.6.0 -C web mobile-audit` | exit 0（31 path × 4 幅 ok） |

## 未解決事項

- 無し。

## 提案

- 狭い幅で行を積む表が /artifacts・/projects/P1 の 2 か所になった。`components/ui/table` に「sm 未満で積む」variant を設けると各画面の `max-sm:` の並びを減らせる（fix-narrow の提案を再掲）。
