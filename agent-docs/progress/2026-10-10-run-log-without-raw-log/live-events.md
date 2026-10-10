---
task: 01M4HRZ55BK3VB3PMSKXFS5W24
work_unit: live-events
status: done
date: 2026-10-10
---

# browser イベント欄に browser.* の進捗を出す（live-events）

## 変更

- `web/features/browser/live-events.tsx`
  - 取得の `types` を `browser_updated,worker_progress` にした（既存の `after_seq` 追いを共用）。
  - `liveEventItems` が、同じ task・同じ run の `worker_progress` のうち `kind=tool_result` かつ `tool` が `browser.` で始まるものを表示文（scrub 済みの `msg`、例 `browser.credential_use: success`）にする。
  - `browser_updated` と混ぜて seq 順に返す（入力の順序に依らず sort する）。
  - live_view_url などの値は文に入れない（`browser_updated` は従来どおり状態だけ）。
- `web/features/browser/live-events.test.ts`（新規、5 件）
  - browser_updated と browser.* の tool_result の混在を seq 順に出す。
  - 別 run・別 task・browser 以外の tool（bash）・`kind=tool_use` を除外する。
  - live_view_url が文に入らない。
  - 順序が逆で届いても seq 順になる。
  - `mergeLiveEvents` で既見の seq を重ねず足す。

`web/features/browser/` の外（`web/e2e/support/`、`features/runs/`）は触っていない。

## 証拠

- `cd web && pnpm install --offline --frozen-lockfile` → exit 0（worktree の node_modules を作成）
- `pnpm exec vitest run features/browser/live-events.test.ts` → Test Files 1 passed、Tests 5 passed
- `pnpm exec vitest run features/browser` → Test Files 11 passed、Tests 99 passed
- `pnpm exec tsc -b` → exit 0
- `pnpm exec biome check features/browser/live-events.tsx features/browser/live-events.test.ts` → exit 0

## 確認した前提

- worker_progress の形は `web/api/generated/types.ts` の `worker_progress`（`kind`・`tool`・`msg`・`run_id`）。emit は `crates/task-worker/src/browser.rs`（`browser.<op>: <status>`、`kind=ToolResult`、`tool=browser.<op>`）。
- events API の `types` はカンマ区切りを受ける（`crates/task-api/src/query.rs` の `list`）。`worker_progress` は `EVENT_TYPES` に入っている。

## 未解決事項

- e2e（`web/e2e/browser/proxy-authz.spec.ts`）は `types=browser_updated` の直接取得のみで、この画面の URL を固定していないので影響なし。Playwright の画面試験は未実行（範囲外の e2e/support を触らない方針のため）。
- 表示の確認（実 browser の run での見え方）は未実施。

## 提案

- 最終レビューの「browser run の画面のイベント欄に browser.credential_use・browser.post_login 等の ToolResult が出る」は、この単体試験で文の生成まで確かめた。画面の結線は run-log/browser 画面の統合試験で見るのがよい。
