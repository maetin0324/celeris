---
task: 01M44ZK8GADYD9PVAYBY73F6YC
unit: gui
status: done
completed: 2026-10-05
---
# Phase 2: gui の routing 表示（source の鮮度・費用成分・除外理由・最終 source）（p2-gui）

## 変更

- `gui/app/lib/routing-source-state.ts`（新規、純関数）: 未知は「不明」（`usdLabel` などは null・NaN で 0 を出さない）。請求（cash）と機会費用（shadow・resource）を別の行にする `costRows`。鮮度（未観測・古い）`freshnessLabel`。reset までの残りは時刻を注入する `resetLabel(resetAt, nowMs)`。候補の除外理由 `candidateStatusLabel`（旧 event は生コードを出す）、score 内訳 `scoreRows`（C/L/P の未知は注記）、最終の source/model/account `finalSelection`（proxy log の実際の行き先を優先）、監査の不完全さ `auditIncompleteNote`。score や費用は再計算しない。
- `gui/app/components/RoutingSourceState.tsx`（新規）: `SourceDeployments`（deployment ごとの鮮度・到達・遅延・残量と reset・負荷・費用と未知の一覧）、`RequestAudit`（要求ごとの最終・候補の状態・費用・score 内訳）。本文色は `text-warning-soft-fg` / `text-danger-soft-fg` のみ（`--success` は使わない）。
- `gui/app/components/TaskRoutingPanel.tsx`: 最新 run の監査が不完全なら理由を出し、`run.requests` の要求ごとの決定を並べる。結べない要求（`unbound_requests`）の件数を出す。
- `gui/app/routes/accounts.tsx`: LLM source の各カードに `deployments`（状態と費用）を出す（欄が無ければ何も出さない）。
- `gui/app/lib/task-routing.ts`: 旧型 `RoutingAudit`（生成型に無く typecheck が落ちていた）を `RunRoutingAudit` に移行。試験の import も同じ。
- `gui/scripts/lib/celeris-fixture.mjs`: `/api/v1/llm/sources` の qwen に `deployments` を足し、`/api/v1/tasks/${TASK_ID}/routing` の mock を足す（pathname で照合）。fixture は試験と共有する。
- `gui/test/fixtures/api/routing-source-state.json`（新規）: routing_source_state fixture。未知の請求・機会費用、古い観測、除外理由 3 種、log と trace の最終値、監査不完全、unbound 要求を含む。
- `gui/test/unit/routing-source-state.test.tsx`（新規）: 上の純関数と SSR の表示を固定（未知が「$0」に見えないこと、請求と機会費用が別行であること、reset の時刻注入、最終 source の優先順位、--success を使わないこと）。
- `gui/test/unit/task-routing.test.ts`: 型名の移行のみ。

## 証拠

- `corepack pnpm@11.27.0 typecheck`（gui）: exit 0。エラー 0 件。
- `corepack pnpm@11.27.0 exec vitest run test/unit/routing-source-state.test.tsx test/unit/task-routing.test.ts test/unit/llm-sources.test.ts test/unit/tasks.detail.loader.test.tsx test/unit/routing-catalog.test.tsx test/unit/providers-llm-source.test.tsx`: 6 files, 84 tests passed。
- `corepack pnpm@11.27.0 test`（gui 全体の vitest）: 93 files, 1319 tests passed, exit 0。
- biome（触ったファイルだけ。`./node_modules/.bin/biome check <files>`）: exit 0（`--write` で整形後、警告 1 件も直して 0）。gui の `pnpm lint` は既存の差分で落ちるため使っていない。
- `gui/scripts/check-task-routing.mjs`（Playwright の実ブラウザで 360px の横はみ出しを見る script）: 未実行。`pnpm build` と Playwright の chromium が要るため。人が `pnpm build && node scripts/check-task-routing.mjs` で確認すること。

## 未解決事項

- 実ブラウザの表示（360px・393px の横はみ出し、mobile-audit の contrast）は未確認。
- `scripts/lib/celeris-fixture.mjs` に routing の mock を足したので、task 詳細の e2e で「ルーティング」パネルが出るようになった。既存の e2e の期待値に触れていないことは見ていない（人の e2e 実行で確かめる）。
- gui の CLAUDE.md は進捗を `docs/PROGRESS.md` に書くよう指示しているが、このタスクの指示どおり `agent-docs/progress/` に書いた。

## 提案

- 候補の表は今は要求ごとのカードの並び。候補が増えたら 1 表にまとめて、列の優先度を決める（幅の制約は mobile-audit で確かめる）。
