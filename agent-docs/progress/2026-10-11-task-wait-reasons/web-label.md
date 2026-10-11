# web-label: 人の gate の待ち理由を gate ごとに分ける

- task: 01M4NAKDCCS5XYAAJ518MVZ134
- 実装: `web/features/tasks/wait-reason.tsx` の `human_gate` を gate で分けた。
  - `plan_approval` →「計画の承認待ち」
  - `stage_review` →「段の確認待ち: <段 key>」（段 key が無ければ「段の確認待ち」）
  - `phase_checkpoint` →「途中確認待ち: <段 key>」（段 key が無ければ「途中確認待ち」）
- 生成型 `HumanGateKind`（`web/api/generated/types.ts`）をそのまま使い、生成型は変えていない。`crates/` は触っていない。
- 試験: `web/features/tasks/wait-reason.test.ts` に 3 種のラベルを足した（StageReview は段 key を含む）。
- 注意: API 側が `stage_review` を出すのは stage-gate 葉（task-ops の `task_wait_api_stage_review`）の仕事。この葉では表示だけを決めた。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → Done（exit 0）
- `corepack pnpm@12.6.0 -C web typecheck` → `tsc -b`、exit 0
- `corepack pnpm@12.6.0 -C web exec vitest run features/tasks/wait-reason.test.ts` → 3 passed (3)
- `corepack pnpm@12.6.0 -C web lint` → exit 0（Found 4 warnings。いずれも wait-reason 以外のファイル。この葉では新規の warning なし）
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 95 files / 691 tests passed、node --test fail 0）
- `node web/scripts/gen-types.mjs --check` → exit 0
- `git diff --quiet be6fa7f9 -- crates/` → 差分なし

## 未解決

- なし（この葉の範囲で）。e2e（`web/e2e/work/wait-reasons.spec.ts`）は対象外。
