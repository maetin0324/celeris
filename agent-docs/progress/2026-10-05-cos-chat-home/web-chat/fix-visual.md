---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: blocked
updated: 2026-10-07
---
# web-chat / fix-visual（badge と tool 状態の視認性）

final review の指摘 (2)(3) を修正した。

- 会話一覧の「旧会話」「受信箱 N 件待ち」は `shrink-0 whitespace-nowrap` で一行を保ち、題名は `min-w-0 truncate` で省略する。共通 Badge の折り返し設定は変更しない。
- tool の成功・失敗・実行中は既存の `text-success-foreground`・`text-danger-foreground`・`text-running-foreground` を使う。白地での contrast 比は順に 7.13:1・8.31:1・7.27:1、hover の accent 背景でも 6.07:1・7.07:1・6.18:1。いずれも WCAG AA の 4.5:1 以上。`web/styles.css` の token は変更していない。
- vitest は長い題名と両 badge の class/構造、3 状態のラベル・前景色 class と実 token の contrast を固定する。
- `threads.spec.ts` は 1440 px で旧会話を長い題名に変更し、badge の矩形が一行分の高さ・会話ボタン内に収まることと、題名の実際の省略を検証する。sleep・負荷試験は追加していない。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: 初回は `ERR_PNPM_NO_OFFLINE_TARBALL`。`install --frozen-lockfile` で不足依存を取得後、offline を再実行して exit 0。lockfile 差分なし。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0。
- `corepack pnpm@12.6.0 -C web exec vitest run features/chat/threads/threads.test.tsx features/chat/messages/messages.test.tsx`: exit 0、38 passed。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `corepack pnpm@12.6.0 -C web lint`: exit 0、既存 `styles.css` の `!important` warning 4 件。
- `corepack pnpm@12.6.0 -C web test`: exit 0、vitest 70 files / 490 tests、node server 57 tests passed。
- `corepack pnpm@12.6.0 -C web build`: exit 0、既存 chunk size warning のみ。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat/threads.spec.ts`: exit 0、3 passed。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: exit 0、31 passed。
- `git diff --check`: exit 0。
- 範囲 check: `$CELERIS_WU_BASE` からの差分と untracked を threads/messages、`web/e2e/chat/threads.spec.ts`、この進捗ファイルに限定して検査。範囲外 0 件。gui/・crates/・styles.css・screenshots の差分なし。

## 未解決・提案

再試行で計画の範囲 check の問題を確認した。`git diff --quiet $(git merge-base HEAD main) -- crates/ gui/` は exit 1。merge-base は `ea6f0e243acf1632852c1f8ad39c5cb1b622afc3` で、WU base `65e344c1a1ed53106618fa7ac5f15088146e1bed` より前を比較しており、親タスクから継承した crates/・gui/ の162ファイルを検出する。

`git diff --quiet "$CELERIS_WU_BASE" HEAD -- crates/ gui/` は exit 0。修正コミット `bc0f30e4` の差分は web の5ファイルとこの進捗だけであり、検出された162ファイルは全て WU base 時点で存在した。親の実装を戻して検査を通すことはしない。計画の検査基点を `$CELERIS_WU_BASE` に訂正することを提案する。検査はこの run では変更できないため、done ではなく plan_issue を返す。今回の再試行では UI 変更も試験の再実行も行っていない。

screenshot は撮り直していない。後続 `shots` が更新し、`reclose` が一覧と ADR 付記を更新する。
