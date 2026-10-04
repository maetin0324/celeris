---
tasks: [01M436BZBC24ZSSA11CJV4AJCK]
wu: files-artifacts
status: done
completed: 2026-10-04
---
# files・artifacts の作り直し（WU files-artifacts）

## 変更点
- `web/features/files/task-files-view.tsx`: 作業ツリーを Table（名前・大きさ）に、本文を CodeBlock にした。長い名前・path は折り返し、全文は `title` 属性。大きさは `formatBytes`（原数は title）。lg 以上は一覧 1/3・本文 2/3 の横並び。repo 切替は `nav`。不正 path は danger の枠で path 全文と「作業ツリーの先頭へ」/「本文を閉じる」の戻り道。空ディレクトリ・空 file・too_large・binary（Badge + 文）を揃えた。
- `web/features/artifacts/artifact-table.tsx`（新規）: `/artifacts` と task の成果物 tab が共有する Table。360px では大きさ・記録日時を成果物欄の下に置き、md 以上で列にする。同じタスクの行はタスク欄を rowSpan で結ぶ。読めない・無い file は Badge（danger / warning）。本文は従来どおり ArtifactPreview（H8）。
- `web/features/artifacts/artifacts-view.tsx`: select は `border-input`、送信は Button primitive。空は EmptyState（`data-testid="artifacts-empty"` を維持）。案件一覧の到着前の仮 option が消えて選択が外れる不具合を `key` で直した。
- `web/features/artifacts/task-artifacts-view.tsx`: ArtifactTable と EmptyState（`task-artifacts-empty` を維持）。
- route（`tasks.$id.files.tsx`・`artifacts.tsx`）は配置だけなので変更なし。h1・accessible name・URL・`data-entry`・`data-artifact`・各 testid は維持。

## before / after の要点
screenshot は run の artifacts（`shots/before`・`shots/after`、6 状態 × 360/390/412/1440）。両方とも横溢れ 0。
- before: 一覧が下線リンクの縦列で大きさが生 byte、本文は折り返しの pre（4000 文字の行が縦に長く伸びる）、select の枠が token でない。
- after: 一覧が Table で行の区切りと選択行の強調、本文は CodeBlock（横は枠内で scroll）、desktop は 2 列で一覧と本文を同時に見られる。成果物は Table で path・大きさ・日時を並べる。

## 検査結果
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile && typecheck && lint && test` → exit 0（vitest 42 files / 269 tests passed、server 42 pass。lint は既存の warning 4 件で今回のファイルには無し）
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/runs-files.spec.ts e2e/parity/task-detail.spec.ts e2e/parity/projects.spec.ts e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts` → 79 passed
- `git diff --quiet 171c8e02 -- crates/ web/components/ web/styles.css web/e2e/support/` → 差分なし
- 生の色・任意値 grep（`-\[..\]`・hex・rgb・palette 名・style=）→ 0 件

## 要望（primitive・fixture）
- 汎用の Alert / Notice primitive（danger・warning の枠 + 本文 + 操作）。`ErrorNotice` は「再取得」文言と onRetry 固定のため、不正 path のような「再取得では直らない」error に使えず、画面で token の class を組んだ。
- Select / Input primitive（`border-input`・focus ring・44px を持つ）。画面ごとに class を並べている。
- CodeBlock の高さ上限 variant（LogSurface の size に相当）。長い file で page が縦に長くなる。
- fake-daemon の default fixture に tree / tree/file / artifacts の長い path 例があると、360px 試験と screenshot を spec ごとに書かずに済む。

## 未解決
- 中間省略は使わず折り返し + title にした（mono の path は折り返しの方が全文を読めるため）。
