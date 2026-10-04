---
task: 01M41YNSDB3C60K4KJ7BNN3EH6
work_unit: overview-500
status: done
completed: 2026-10-03
---
# gui gate 退行: タスク詳細 overview の 500（overview-500）

## 原因
main b742ec75 の mobile-audit で `GET /tasks/01BOARDTASK00000000000001?tab=overview -> 500`（light・dark）。

- **コンポーネント側（loader は無関係）**: 90e3ca26..b742ec75 で OverviewTab に足された `WriteSetSection`
  （ADR-0130）が `detail.actual_run_write_sets` / `actual_work_unit_write_sets` を必須とみなし、
  `WriteSetList` の `items.length` で `TypeError: Cannot read properties of undefined (reading 'length')`
  を投げて SSR が 500 になった。`loadTaskDetail` は詳細をそのまま返すだけで投げない（回帰試験で確認）。
  同じ差分の `TaskExecutionRoute`（`decision.reasons.length`）と `IntegrationRepairPanel`（`target_sha.slice`）
  も欄の欠落で投げる形だった。overview タブだけが落ちたのは `WriteSetSection` が overview にしか無いため。
- **mock 側**: mobile-audit の偽 celeris（`gui/scripts/lib/celeris-fixture.mjs` の BOARDTASK 詳細）に
  ADR-0130 の欄（`actual_*_write_sets`・`behind_target`・`expected_write_paths`）が無かった。
  実 API（`crates/task-ops/src/view.rs` の `TaskDetail`）はこれらを常に返すが、ADR-0130 より前の celeris
  （GUI だけ先に新しい release）では欠落しうる。単体試験の fixture（`test/fixtures/api/task-detail.json` 等）は
  差分で欄を足していたので vitest では見えなかった。`gui/test/mock-celeris` には BOARDTASK の詳細応答は無い。

## 変更
- `gui/app/components/task-detail/WriteSetSection.tsx`: `actual_*_write_sets`・`behind_target`・`paths` の欠落/null に耐える。
- `gui/app/components/task-detail/TaskExecutionRoute.tsx`: `route.reasons` 欠落に耐える。
- `gui/app/components/IntegrationRepairPanel.tsx`: sha 欠落で「不明」と出す（投げない）。
- `gui/scripts/lib/celeris-fixture.mjs`: BOARDTASK 詳細に実 API の形の `expected_write_paths`・`actual_run_write_sets`・
  `actual_work_unit_write_sets`・`behind_target`・`integration_repair: null` を追加。
- 回帰試験 `gui/test/unit/task-detail.missing-fields.test.tsx`（6 件）: 欄を欠いた詳細で loader が投げない、
  WriteSetSection / TaskExecutionRoute / IntegrationRepairPanel が描画できる。

## 証拠
- 修正前: `corepack pnpm@11.27.0 -C gui exec vitest run test/unit/task-detail.missing-fields.test.tsx` → exit 1、
  6 failed（`TypeError: Cannot read properties of undefined (reading 'length')` ほか）。
- 修正後: 同コマンド → 6 passed。
- `CI=true corepack pnpm@11.27.0 -C gui install --frozen-lockfile --prefer-offline` → exit 0
- `corepack pnpm@11.27.0 -C gui typecheck` → exit 0
- `corepack pnpm@11.27.0 -C gui test` → exit 0、91 files / 1293 tests passed
- `corepack pnpm@11.27.0 -C gui build` → exit 0
- `corepack pnpm@11.27.0 -C gui mobile-audit` → exit 1、violations 6 件はすべて `perf`（http-status 0 件）。
  task-overview / task-tree / task-timeline / task-changes / task-files / task-artifacts の initial JS 536.7KB > 532KB。
- `cargo test --workspace` → exit 0、3832 passed / 0 failed / 13 ignored
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決事項
- initial JS の予算超過は葉 js-budget の担当。overview が 500 でなくなったため、**task-overview も perf 違反
  （536.7KB、28 chunks）として新たに数えられる**（以前は 500 で計測されていなかった）。js-budget は 5 経路でなく
  6 経路を予算内に戻す必要がある。

## 提案
- mobile-audit の偽 celeris の詳細応答を `gui/app/celeris/types.ts` の `TaskDetail` で型検査する（`// @ts-check` + JSDoc）と、
  必須欄の追加漏れを typecheck で拾える。
