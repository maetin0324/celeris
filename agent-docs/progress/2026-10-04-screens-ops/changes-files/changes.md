---
tasks: [01M436BZBC24ZSSA11CJV4AJCK]
wu: changes
status: done
completed: 2026-10-04
---
# changes 画面（/tasks/$id/changes・?tab=changes）の作り直し

## 変更点
- `web/features/changes/changes-view.tsx`: repo ごとの枠を `Section`（token の枠・余白）にし、ブランチ・先行・取り込みを `DataList` に。未コミット・作業ツリーなし・取り込み状態は `Badge`（tone は token の状態色）。
- 変更ファイルの一覧を `Table`（状態 / ファイル / 行）に。状態は M/A/D/R/C/? を「変更・追加・削除…」の可視ラベル付き Badge、行数は `+n`/`−n` を success/danger の前景色と sr-only の「追加」「削除」で示し色だけに頼らない。
- 長い path は折り返し（`break-all`）、全文は `title` 属性。状態・行の列は `w-px whitespace-nowrap` で最小幅にし、360px でも path 列に幅を残す。
- 差分を `CodeBlock`（label「差分 <path>」、`max-h-96` で縦も内側 scroll）に。行の種類（追加・削除・hunk・meta）を `web/features/changes/diff-lines.ts` で判定し、`bg-success`/`bg-danger`/`bg-info` の組で塗る。行頭の +/− は原文のまま。行は `grid w-max min-w-full` に並べ、横 scroll 先まで背景が続く。
- 空・読み込み中・エラーは `FetchFrame`（subject「変更」「差分」）と `EmptyState` の共通文言に揃えた。
- 取り込み欄: select・textarea は `border-input`（--color-input）、`取り込む` は primary（破棄を選んだときは destructive）。確認 checkbox は label 全体を 44px の tap 領域にした。
- 保った契約: h1「変更 T1」、`data-testid`（task-changes・changes-empty・integration-state・changed-files・change-diff・change-diff-body・integrate-result）、`data-file`・`data-repo`、label「取り込みの方法」「取り込みの note（任意）」「取り返しがつかないことを確認した」、button 名「取り込む」「Celeris で merge（…）」。API 型・query・gateway は不変。

## before / after の要点
screenshot は run の成果物ディレクトリ `shots/before`・`shots/after`（360/390/412/1440、長い path・長い行の差分・PR open の fixture を page.route で差し込む台本 `changes-shots.mjs`）。
- before: 枠が素の `border`、ファイルは `M path` の下線リンクの羅列、行数は `text-neutral-600`（生の色）、差分は単色の `pre`、select/textarea の枠は既定色。
- after: 枠・表・Badge・差分の色がすべて token。360px で path は折り返し、差分だけが内側で横 scroll。4 幅ともページの横溢れ 0（`scrollWidth - clientWidth = 0`）。

## 要望（範囲外のため作らなかったもの）
- `CodeBlock` に行ごとの装飾（diff の add/remove）を primitive として持たせたい。いまは画面側で `span` を grid に並べている。
- `EmptyState` に `data-testid` 等の属性を渡す口が無いので、`changes-empty` は外側の `div` に付けた。
- path の中間省略（先頭と末尾を残す）を共通の primitive にしたい（今回は折り返し + title で足りた）。
- `Icon` に git の状態用（追加・削除）の icon は無い。Badge の文字で足りているので必須ではない。

## 検査結果
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存の 4 warning・1 info は今回の差分外）
- `corepack pnpm@12.6.0 -C web exec vitest run` → 43 files / 273 tests passed（新規 `features/changes/diff-lines.test.ts` 4 件を含む）。`pnpm test` の node --test 42 pass
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/task-detail.spec.ts e2e/parity/runs-files.spec.ts` → 8 passed
- `git diff --quiet 171c8e02 -- crates/ web/components/ web/styles.css web/e2e/support/` → exit 0
- 生の色・任意値 grep（`-\[[^]]+\]`・`#hex`・`neutral-数字`）を `web/features/changes/` に → 0 件

## 自己レビュー（ui-ux-quality-gate）
- 状態の伝達は文字 + 色、差分は +/− の原文 + 色。focus は primitive の既定（CodeBlock・Table 枠は tabIndex 0 の名前付き region）。
- 操作の tap 領域は 44px 以上（path の button は `min-h-11`）。
- 未解決: なし。
