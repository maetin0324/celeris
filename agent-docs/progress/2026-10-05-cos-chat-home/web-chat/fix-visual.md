---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
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

## 再試行の検証（2026-10-07）

人の replan v9 で範囲検査の基点が `CELERIS_WU_BASE` に訂正された。修正 `bc0f30e4` をそのまま維持し、以下を再実行した。

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`・`lint`・`test`・`build`: 全て exit 0。vitest 490 件、server 57 件成功。既存 lint warning 4 件と chunk size warning のみ。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: exit 0、31 passed（長い題名の badge の矩形検証を含む）。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0、変更なし。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/ web/styles.css`: exit 0。
- `$CELERIS_WU_BASE` からの差分と untracked を threads/messages、`web/e2e/chat/threads.spec.ts`、この進捗ファイルに限定した範囲検査: exit 0、範囲外 0 件。
- `git diff --check`: exit 0。

## 未解決・提案

この葉の未解決事項なし。screenshot は撮り直していない。後続 `shots` が更新し、`reclose` が一覧と ADR 付記を更新する。
