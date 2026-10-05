---
title: 受信箱の長い本文・ID・path の横溢れ修正（r2）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
updated: 2026-10-05
---

# 受信箱（/inbox）の長文・長 ID 横溢れ修正

final review（attempt 2）で指摘された「`e2e/states/states.spec.ts` の『long-text /inbox』が横 scroll 57px で落ちる」の再修正 WorkUnit（r2）。

## 調査

- 統合後の HEAD（`9fc0016c`）で `playwright test states/states.spec.ts -g "state long-text"` と `-g "/inbox"` を実行したところ、すでに全件 pass していた。
- 原因は既存コミット `3b368d81`（`受信箱の参照題名 link を wrap-anywhere で折り返す`、前回の run 内で commit 済み）が `linkClass` を `break-words` から `wrap-anywhere` に変えていたこと。このコミットは今回のベース（`9fc0016c`）に既に含まれており、受信箱の長文題名・項目は `min-w-0` + `break-words`/`wrap-anywhere` の組で 360px でも折り返し、横 scroll は出ない。
- つまり横溢れそのものは前回の run 内で直っていたが、記録・再確認がこの WorkUnit の仕事として残っていた。

## 修正（今回の差分）

受信箱の案件リンクが、一覧から案件名を解決できないとき `shortId(item.project_id)`（先頭 8 文字 + `…`）で見た目だけを縮めて表示していたが、`title` 属性が無く全文を読む手段が無かった（受け入れ条件 1「省略した項目は全文を読める」）。`web/features/inbox/inbox-screen.tsx` の `InboxRow` 内、案件リンクに `title={projectTitle ? undefined : item.project_id}` を追加し、省略時は `title` 属性で全文を読めるようにした。

他の省略表示（`Blocking` の `blocked_by` 経由の id）は既に `title={id}` を持っており対象外。

変更は `web/features/inbox/inbox-screen.tsx` の 1 ファイルのみ（`web/routes/inbox.tsx` は無変更）。

## 検証

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0
- `corepack pnpm@12.6.0 -C web lint`: exit 0（担当外の既存警告 5 件のみ、前回 pass 時と同じ）
- `corepack pnpm@12.6.0 -C web test`: exit 0（42 tests pass）
- `corepack pnpm@12.6.0 -C web build`: exit 0
- `corepack pnpm@12.6.0 -C web check:boundaries` / `check:parity` / `check:secrets` / `gen:types --check` / `mobile-audit`: 全て exit 0（mobile-audit: 31 path × 4 幅 ok）
- `corepack pnpm@12.6.0 -C web e2e states/states.spec.ts`（build 後）: 30 passed、`long-text /inbox`・`long-id /inbox` を含め全 `/inbox` 行が pass（横 scroll 0）
- `corepack pnpm@12.6.0 -C web e2e work/inbox-notifications.spec.ts`: 3 passed（既存の受信箱操作 e2e に回帰なし）
- 範囲 check: `git status --porcelain` → `web/features/inbox/inbox-screen.tsx` のみ

## 残課題

なし（この WorkUnit の受け入れ条件 3 件は上記検証で満たす）。Task 全体の受け入れ条件 5（ホームの空白帯・script focus 枠・既定 fixture の取得失敗帯・/tasks/T1 の生値 `状態 ready`）は並行する別 WorkUnit（fix-home-r2・fix-focus-r2・fix-fixtures-r2・fix-tasks-detail-r2）の担当。
