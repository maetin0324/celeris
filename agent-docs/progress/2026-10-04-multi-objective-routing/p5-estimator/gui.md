---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: gui
status: done
completed: 2026-10-05
---

# Phase 5 gui: routing 画面に estimator shadow（heuristic primary との差）を別欄で出す

## 変更

- `gui/app/components/TaskRoutingPanel.tsx`:
  - `run.routing_shadow` を `kind === "estimator"` の行と旧（decision/execution）の行に分離。旧 shadow 欄（`data-testid="task-routing-shadow"`）は estimator 以外の行だけ出す（旧表示は不変）。
  - 別欄 `data-testid="task-routing-estimator-shadow"`（「estimator shadow（heuristic primary との比較・本番には効かない）」）を新設:
    - 要約（`TaskRoutingView.estimator_shadow`、`data-testid="task-routing-estimator-summary"`）: 対象・完了・coverage%・失敗・timeout・見送り・prompt 要・heuristic 首位と異なる件数・heuristic primary と異なる件数・平均 overhead・estimator 一覧。
    - 1 件（`EstimatorAudit`、`data-testid="task-routing-estimator-shadow-row"`）: estimator id/version（`routellm/1`）・状態（completed/failed/timeout/dropped/prompt_required）・heuristic 首位との差・heuristic primary との差（same/differs/no_candidate）・理由（reason・unavailable_reason）・overhead・依存（prompt 要 / network 要 / 外部 embeddings / daemon の許可外）。
  - 本番切替の操作（opt-in 切替・on/off）は出さない。primary の outcome・attempts・review とは別欄。
- `gui/test/fixtures/api/routing_estimator_shadow.json`（新設）: completed / timeout / dropped / prompt_required の 4 種の estimator shadow 行 + `estimator_shadow` 要約。
- `gui/test/unit/routing-estimator-shadow.test.tsx`（新設）: 別欄の位置（primary・旧 shadow と別）・id/version・差・timeout・prompt_required・dropped 理由・欠測は不明・要約・estimator 欄が無い旧 run では別欄を出さない・旧 shadow 表示が不変。

## 証拠

- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0。
- `corepack pnpm@11.27.0 -C gui test` → 96 files / 1341 tests passed（exit 0）。
- `corepack pnpm@11.27.0 exec biome check gui/app/components/TaskRoutingPanel.tsx gui/test/unit/routing-estimator-shadow.test.tsx gui/test/fixtures/api/routing_estimator_shadow.json` → No fixes applied（exit 0）。
- 生成型（`gui/app/celeris/types.ts` の `EstimatorShadowAudit` / `EstimatorShadowSummary` / `EstimatorShadowOutcome` / `EstimatorComparison` / `EstimatorDependencyAudit`）は audit-api 葉の再生成済みを使い、本 leaf では変更していない。

## 未解決事項

- 無。
