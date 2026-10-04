---
tasks: [01M436BZBC24ZSSA11CJV4AJCK]
wu: verify
status: done
completed: 2026-10-04
---
# changes・files・artifacts 作り直し: 360px 長い path 試験と記録のまとめ

build 段の 2 葉（`changes-files/changes.md`・`changes-files/files-artifacts.md`）を要約し、verify 葉が足した試験と検査結果をまとめる。

## 変更点
- `web/e2e/parity/runs-files.spec.ts` に `parity: 360px で深い階層・長い file 名の file 一覧・本文・成果物一覧が溢れない` を追加。fixture は spec の中だけで書き、`web/e2e/support/` は触っていない。
  - file 一覧: `src/<長いディレクトリ名>` に 1 段降りた一覧の `data-entry` 行が `title` に完全な path を持ち、360px で `document.documentElement.scrollWidth - clientWidth === 0`。
  - file 本文: 深い path の file を開いた見出し（`h2[title]`）が全文を持ち、5000 文字の 1 行があっても横溢れ 0。
  - 成果物一覧（`/artifacts`）: 長い path（`ops/.../*.md`）の行が `title` に完全な path を持ち、横溢れ 0。
- 画面側（`web/features/{changes,files,artifacts}/**`）は build 段（`changes` 葉・`files-artifacts` 葉）で既に token・primitive だけで作り直し済みで、今回の試験で不具合は見つからなかったため画面の変更は無し。

### build 段の要約（詳細は `changes-files/changes.md`・`changes-files/files-artifacts.md`）
- **changes**: repo 枠を `Section`、一覧を `Table`、差分を `CodeBlock`、状態を文字 + token 色の `Badge` に。長い path は折り返し + `title`。
- **files-artifacts**: 作業ツリー一覧を `Table`・本文を `CodeBlock`、成果物一覧は共有 `ArtifactTable`（360px では大きさ・記録日時を欄の下に畳む）。長い path は折り返し + `title`。

## before / after
build 段の 2 葉に screenshot（360/390/412/1440、6〜数状態）を記録済み（詳細は上記 2 ファイル）。verify 葉は新規 screenshot を追加していない（画面変更が無いため）。

## 要望（範囲外のため作らなかったもの。build 段からの引き継ぎ）
- `CodeBlock` に diff の行ごとの装飾を primitive として持たせたい。
- `EmptyState` に `data-testid` 等を渡す口が無い。
- path の中間省略（先頭・末尾を残す）を共通の primitive にしたい（今回は折り返し + `title` で足りている）。
- 汎用の Alert/Notice primitive（danger・warning の枠 + 本文 + 操作）。
- Select / Input primitive（`border-input`・focus ring・44px を持つ）。
- CodeBlock の高さ上限 variant（LogSurface の size 相当）。
- fake-daemon の default fixture に tree / tree/file / artifacts の長い path 例があると、360px 試験を spec ごとに書かずに済む。

## 検査結果
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0
- `corepack pnpm@12.6.0 -C web lint` → exit 0（既存の 4 warning・1 info は今回差分外、`web/e2e/parity/runs-files.spec.ts` には無し）
- `corepack pnpm@12.6.0 -C web test` → vitest 43 files / 273 tests passed、node --test 42 pass
- `corepack pnpm@12.6.0 -C web build` → exit 0（`dist/index.html` 生成）
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/runs-files.spec.ts e2e/parity/task-detail.spec.ts` → 9 passed（新規試験 1 件を含む）
- `corepack pnpm@12.6.0 -C web e2e e2e/parity/mobile-gate.spec.ts e2e/a11y/axe.spec.ts` → 62 passed（axe critical/serious 0・4 幅横溢れ 0、`/tasks/T1/files`・`/tasks/T1/changes`・`/artifacts` を含む）
- `corepack pnpm@12.6.0 -C web mobile-audit` → `mobile-audit: 30 path(s) x 4 widths ok`
- `corepack pnpm@12.6.0 -C web check:secrets`（build 後）→ `token absent from build output, HTML, /api responses, errors and logs`
- `git diff --name-only 171c8e02 -- . ':!web/features/tasks' ':!web/features/runs' ':!web/features/console' ':!web/features/changes' ':!web/features/files' ':!web/features/artifacts' ':!web/routes/tasks.*' ':!web/routes/graph.tsx' ':!web/routes/artifacts.tsx' ':!web/e2e/parity/tasks.spec.ts' ':!web/e2e/parity/task-detail.spec.ts' ':!web/e2e/parity/runs-files.spec.ts' ':!web/e2e/parity/console.spec.ts' ':!agent-docs/progress/' ':!agent-docs/adr/'` → 差分なし（exit 0）
- `git diff --quiet 171c8e02 -- crates/ web/components/ web/styles.css web/e2e/support/` → 差分なし（exit 0）

## 自己レビュー（ui-ux-quality-gate）
- 画面側の変更が無いため build 段の自己レビューを踏襲。追加した試験は「省略した path の全文が title にある」ことと「360px で横溢れ 0」であることを決定的に検査する。
- 未解決: なし。
