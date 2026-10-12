---
status: done
unit: web-provider-ui
task: 01M4R6TS1J62NSXYYW597RK44Z
date: 2026-10-12
---

# web-provider-ui: routing 監査表示を harness・source・model・account の語にする

## 条件ごとの結果

- 0. routing 監査表示が harness・source・model・account を出す: 完了。`web/features/tasks/routing-audit-view.tsx` の実行枠行を `実行枠: harness {harness} / source {provider} / model {model} / 口座 {account}` に変えた（欄名 `provider` は API の欄名のまま、表示は source）。harness が無い run は `harness 不明`。
- 1. web の画面文言に provider・プロバイダが無い: 完了。`web/routes`・`web/features`・`web/components`・`web/lib` を grep し、残るのは `run.provider`（生成型の欄参照、画面文言ではない）だけ。`QueryClientProvider` と URL の `providerId=lms`（試験の入力値）は対象外。
- 2. web の typecheck・lint・test・build が通る: 完了（下の証拠）。
- 3. crates/・gui/ に差分が無い: 完了。`git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/` が exit 0。

## 変更

- `web/features/tasks/routing-audit-view.tsx`: 実行枠行の語と並び順。
- `web/features/tasks/routing-audit-view.test.tsx`: 既存の文字列 3 箇所を新しい語に直し、「harness・source・model・account の順」「harness 無しは不明」の試験を 1 件足した。
- `web/e2e/support/fake-daemon.mjs`: `routingShadowFixture` の R5 に `harness: "claude-code"` を足した（試験 fixture）。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → exit 0。
- `corepack pnpm@12.6.0 -C web lint` → exit 0。既存の styles.css の `!important` 警告 4 件（今回の変更とは無関係）。
- `corepack pnpm@12.6.0 -C web typecheck`（tsc -b） → exit 0。
- `corepack pnpm@12.6.0 -C web test` → Test Files 91 passed (91)、Tests 655 passed (655)。
- `corepack pnpm@12.6.0 -C web build` → exit 0（`built in 370ms`）。
- 初回の lint は整形の指摘 1 件で失敗したので `biome check --write` で 2 file を整形し、再実行して通した。初回の test は estimator 試験の文字列 1 件が落ちたので直して通した。

## 未解決事項

- なし（web の範囲）。gui/ の run 一覧・run 詳細・accounts の provider 表示は `gui-provider-ui` の範囲で、この unit では触っていない。

## 提案

- 生成型の `RunRoutingAudit.provider` を `source` へ改名するのは API 変更になるため、この unit の範囲外（ADR の API 名の扱いに従い据え置き）。
