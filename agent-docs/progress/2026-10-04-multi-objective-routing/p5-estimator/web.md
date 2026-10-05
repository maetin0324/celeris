---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: web
status: done
completed: 2026-10-05
---

# Phase 5 web: routing 監査 view に estimator shadow を別欄表示

## 変更

- `web/features/tasks/routing-audit-view.tsx`
  - `TaskRoutingView.estimator_shadow`（task を跨いだ要約）を `EstimatorShadowSummaryView` として task の先頭・run 一覧より前に別節（`aria-label="estimator shadow 要約"`）で出す。targets・completed/failed/timeout/dropped/prompt_required・coverage・primary/heuristic と違った件数・平均推論 overhead・estimator 一覧。欠測（`mean_overhead_ms` 等）は「不明」のまま、0 に丸めない。
  - `ShadowList`/`ShadowRow` の `SHADOW_KIND_LABEL` に `estimator` を追加（primary の選択には効かないことを明記）。`kind === "estimator" && shadow.estimator` のとき `EstimatorShadowDetail` を追加表示: estimator id/version、outcome（completed/failed/timeout/dropped/prompt_required）、timeout・prompt_required・dropped の理由（`unavailable_reason`）、primary（実行した決定）との差・heuristic 首位との差（`vs_primary`/`vs_heuristic` を same/differs/no_candidate → 日本語ラベル）、推論 overhead_ms、依存（needs_prompt・not_allowed）。primary の実行枠・outcome の表示は変えない（既存の decision/execution shadow 表示もそのまま）。
  - 本番切替 UI は追加していない（shadow の表示のみ）。
- `web/e2e/support/fake-daemon.mjs`: fixture `routingEstimatorShadowFixture`（fixture id: `routing_estimator_shadow`）を追加。1 run に estimator-kind の shadow 5 件（primary/heuristic 両方と一致した completed、両方と違った completed、timeout の failed、prompt を送らず評価不能な dropped、依存が許可外で評価不能な dropped）と、task を跨いだ `estimator_shadow` 要約を含める。`web/e2e/support/fake-daemon.d.mts` に型宣言を追加。
- `web/features/tasks/routing-audit-view.test.tsx`: fixture が生成型 `TaskRoutingView` に合うことの検証、要約節が run より前に出ること、estimator id/version・primary=heuristic との差・timeout/prompt_required/dropped の理由・欠測（overhead 不明）を確認する試験を追加（既存の decision/execution shadow 試験は変更なし）。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline` → exit 0。
- `corepack pnpm@12.6.0 -C web run typecheck`（`tsc -b`） → exit 0。
- `corepack pnpm@12.6.0 -C web exec vitest run features/tasks/routing-audit-view.test.tsx` → Test Files 1 passed (1), Tests 19 passed (19)。
- `corepack pnpm@12.6.0 -C web run test`（vitest run 全体 + server/*.test.mjs） → vitest: Test Files 29 passed (29), Tests 216 passed (216)。node --test: tests 42, pass 42, fail 0。
- `corepack pnpm@12.6.0 -C web run lint`（biome check） → この unit の変更ファイルに起因するエラーなし（`features/knowledge/skills-screen.tsx` の既存 info 1 件のみ残るが本 unit の範囲外で既存）。
- 範囲 check: `git diff --name-only "${CELERIS_WU_BASE}"` + `git ls-files --others` → `web/e2e/support/fake-daemon.d.mts`・`web/e2e/support/fake-daemon.mjs`・`web/features/tasks/routing-audit-view.test.tsx`・`web/features/tasks/routing-audit-view.tsx` の 4 件のみ（すべて `web/` 配下）。

## 未解決事項

- なし（本 unit の範囲）。gui 側の同等表示は並行 WU（gui）の担当。
