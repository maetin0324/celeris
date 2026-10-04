---
task: 01M41YNSDB3C60K4KJ7BNN3EH6
work_unit: js-budget
status: done
completed: 2026-10-03
---
# gui gate 退行: タスク詳細の初期 JS 予算超過（js-budget）

## 症状
main b742ec75（+ overview-500 の修正 f86aba40）で `pnpm mobile-audit` の perf が 6 経路で違反:
task-overview・task-tree・task-timeline・task-changes・task-files・task-artifacts の initial JS
**536.7KB > 532KB（28 chunks）**。overview は 500 が直ったことで新たに計測対象になり 6 経路になった。

## 増分の内訳（build/client/assets の chunk、生バイト）
b742ec75 で `routes/tasks.$id.tsx` と `OverviewTab` が次を静的 import し、全タブの初回 chunk に載った。

| 部品 | 載っていた chunk | 大きさ |
|---|---|---|
| `IntegrationRepairPanel`（inbox と共有） | `task-execution`（7168B。分離後 4957B） | 約 2.2KB |
| `WriteSetSection`・`TaskExecutionRoute`（+ `time-delta`） | `tasks._id`（79332B） | 数 KB |

`celeris/types.ts` の追加は型だけ（実行時コードの追加なし）。

## 修正（`gui/app/routes/tasks.$id.tsx` のみ）
- `IntegrationRepairPanel` を `React.lazy` にし、`detail.integration_repair` が有るときだけ描く（null なら chunk を取りに行かない）。
- 「概要」「経過」タブの本体（`OverviewTab`・`TimelineTab`）を、既存の木・変更・ファイルのタブと同じく `React.lazy` + `Suspense`（fallback は `TaskTabSkeleton`）にした。どのタブも自分の本体だけを読む。
  - chunk: `tasks._id` 79332B → 27954B、`OverviewTab` 43071B・`TimelineTab` 13187B・`IntegrationRepairPanel` 2461B が別 chunk。
- `scripts/mobile-audit.mjs` の `PERF_BUDGET`（jsBytes 532KB）は不変（`git diff b742ec75 -- gui/scripts/mobile-audit.mjs` は空）。

注: mobile-audit は `load` までの静的資源だけを数える（Phase 77 の定義。ハイドレーション中の React.lazy の
動的 import は数えない）。概要タブは SSR の HTML を出したうえで、ハイドレーション時に `OverviewTab` chunk
（43KB）を追加で取りに行く。

## 変更前後（mobile-audit の perf、light）
| 経路 | 変更前 | 変更後 |
|---|---|---|
| task-overview | 536.7KB / 28 chunks（違反） | 480.2KB / 26 chunks |
| task-tree | 536.7KB / 28（違反） | 480.2KB / 26 |
| task-timeline | 536.7KB / 28（違反） | 480.2KB / 26 |
| task-changes | 536.7KB / 28（違反） | 480.2KB / 26 |
| task-files | 536.7KB / 28（違反） | 480.2KB / 26 |
| task-artifacts | 536.7KB / 28（違反） | 480.2KB / 26 |

## 証拠コマンドと結果（作業 tree、gui/ で実行）
- `CI=true corepack pnpm@11.27.0 install --frozen-lockfile --prefer-offline` → exit 0
- `corepack pnpm@11.27.0 typecheck` → exit 0
- `corepack pnpm@11.27.0 test` → exit 0（91 files / 1293 tests passed。overview-500 の回帰試験を含む）
- `corepack pnpm@11.27.0 build` → exit 0
- `corepack pnpm@11.27.0 mobile-audit` → exit 0（routes=28 schemes=2 violations=0、http-status・perf とも 0）
- 変更前の同じ mobile-audit → exit 1（perf 6 件、上表）
- `cargo test --workspace` → exit 0（3832 passed / 0 failed / 13 ignored）
- `cargo clippy --workspace -- -D warnings` → exit 0

## 未解決事項
- なし。perf の最重量は project-detail の 565.5KB（JS+CSS 合計。JS は予算内）。

## 提案
- task 詳細の新しい欄は、全タブ共通の位置に置くものほど初回 JS に効く。表示条件が限られる欄は lazy を既定にする。
