---
task: 01M44ZK8GADYD9PVAYBY73F6YC
unit: web
status: done
completed: 2026-10-05
---
# Phase 2: web の routing 表示と sources 表示（p2-web）

## 変更

- `web/features/ops/routing-state.ts`（新）: 動的 source 状態・費用・score・除外理由の純粋な言い換え。請求（cash）と機会費用（shadow・resource）を別の行にし、欠測は「不明」。鮮度は観測無し・期限切れを区別。
- `web/features/ops/routing-state-view.tsx`（新）: `CostRows`（費用 4 成分の行）と `DeploymentStateList`（deployment の到達・鮮度・残量・圧力・遅延と費用）。
- `web/features/tasks/routing-audit-view.tsx`（新）: `RoutingAuditView`。run ごとの実行枠・実測の請求・監査の不完全さ（理由付き）、要求単位の決定（候補の選択・除外理由・score 内訳・費用 4 成分・最終 source/model/account）、結べない要求、optimizer の比較。旧 run（requests 無し）は「不明」と書く。
- `web/features/tasks/execution-panel.tsx`: routing 節を `RoutingAuditView` に差し替え（`data-testid` は維持）。
- `web/features/ops/providers-screen.tsx`: LLM source 節に deployment の状態（`DeploymentStateList`）を足す。
- `web/e2e/support/fake-daemon.mjs` / `.d.mts`: `routingAuditFixture`（`/api/v1/tasks/T1/routing` の既定）と `llmSourcesFixture`（`GET /llm/sources` の deployment 付き。欠測の残量・観測・費用を含む）を追加。`GET /llm/sources` の inline 応答を fixture に置き換えた。
- 試験: `web/features/ops/routing-state.test.tsx`（9 件）、`web/features/tasks/routing-audit-view.test.tsx`（7 件）。両 fixture は生成型の schema（`schema.$defs.TaskRoutingView` / `LlmSourcesView`）に `validateFixture` で合わせている。
- 変更しなかったもの: `event-kinds.ts`、`invalidation-map.ts`（event 型は不変）、crates、docs/。

## 証拠

- `corepack pnpm@12.6.0 -C web typecheck`: exit 0（`tsc -b`）。
- `corepack pnpm@12.6.0 -C web test`（引数なし）: exit 0。vitest `Test Files 29 passed (29)`、`Tests 203 passed (203)`。node `--test server/*.test.mjs` は `tests 42 / pass 42 / fail 0`。
- `corepack pnpm@12.6.0 -C web lint`（biome check .）: exit 0。残りの info 1 件は `features/knowledge/skills-screen.tsx`（本 unit の範囲外・既存）。
- `corepack pnpm@12.6.0 -C web check:boundaries`: exit 0。`check:parity`: exit 0。
- `git diff HEAD --name-only -- crates docs`: 0 件。変更は `web/` の 9 ファイルのみ。

## 未解決事項

- Playwright の e2e（`web/e2e/parity/task-detail.spec.ts` の routing 表示）は実行していない（ブラウザ起動を伴うため）。spec 内の inline routing fixture は `requests` を持たないので「旧 run」の経路を通り、`m-1` の表示は残る想定。実行は人が `corepack pnpm@12.6.0 -C web e2e` で確認する。
- 本 unit の受け入れ条件「crates・docs に差分が無い」は、記録先に指定された `agent-docs/progress/` を含めない（`docs/` は差分なし）と解釈した。

## 提案

- sources の鮮度・費用の言い換えは web と gui で同じ区別（請求と機会費用、欠測は不明）に揃える。gui の `TaskRoutingPanel` も `RunRoutingAudit` へ移行する（gui unit の担当）。
- candidate の `cost_usd`（旧欄）は web では出さない。旧 event 互換の欄として残すなら別途、請求と混同しない名前にする。
