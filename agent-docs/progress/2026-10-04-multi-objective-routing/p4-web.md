---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: web
status: done
completed: 2026-10-05
---

# Phase 4: web の task routing に shadow 監査を primary と別欄で表示（p4-web）

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §7.1・§7.2・§10 Phase 4 に従う。

## 変更

- `web/api/generated/schema.json`・`types.ts`: `corepack pnpm@12.6.0 -C web install --offline` の後、
  `node scripts/gen-types.mjs`（Phase 3 の p3-web と同じ手順）で `docs/api/v1/api-v1.schema.json` から再生成。
  `RoutingShadowAudit`・`ShadowKind`・`ShadowStatus`・`ShadowReason`・`ShadowReservationAudit`、
  `RunRoutingAudit.routing_shadow`、Event `routing_shadow_recorded` が入る。
- `web/api/realtime/event-kinds.ts`・`invalidation-map.ts`: `routing_shadow_recorded` を追加（`{ sets: ["T", "R"] }`、
  対象 task の routing 監査だけを古くする）。schema 全体と一致させる既存の網羅試験が通る。
- `web/features/tasks/routing-audit-view.tsx`: run ごとに「shadow（primary の選択は変えない比較）」の節
  （`aria-label="shadow 監査"`）を primary の欄とは別に追加。shadow ごとに decision/execution、
  完了/失敗/破棄と理由、候補 source/model と primary との差（あり/なし/不明）、execution の token、
  上限の予約と消費（UTC 日・state・token・effective USD）を出す。欠測は「不明」で 0 や false に丸めない。
  色は使わず既存の layout class のみ（FRONTEND_CONTRACT の生の色・任意値を使わない）。
- `web/e2e/support/fake-daemon.mjs`・`.d.mts`: `routingShadowFixture`（fixture id: routing_shadow、task `T3`）。
  decision completed（差あり）、execution completed（予約・消費あり、差なし）、execution failed（timeout）、
  execution dropped（cap_exceeded、候補・予約欠測）と shadow なしの run を含む。
- `web/features/tasks/routing-audit-view.test.tsx`: 5 件追加（fixture の schema 適合、primary と別節・primary
  の値不変、種類/状態/理由/差、上限消費と欠測の「不明」、shadow 無し run で節を出さない）。
- 変更しなかったもの: `crates/`・`gui/`・`docs/`。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline`: exit 0。
- `cd web && node scripts/gen-types.mjs`: 差分は `web/api/generated/{schema.json,types.ts}` のみ。
- `corepack pnpm@12.6.0 -C web run typecheck`（tsc -b）: exit 0。
- `corepack pnpm@12.6.0 -C web run test`: exit 0。vitest `Test Files 29 passed (29)` / `Tests 212 passed (212)`
  （Phase 3 の 207 件 + 本 unit の 5 件）、node `tests 42 / pass 42 / fail 0`。
- `corepack pnpm@12.6.0 -C web run lint`: exit 0（info 1 件は既存・範囲外）。`check:boundaries`: exit 0。
- 範囲: `git diff --name-only $CELERIS_WU_BASE` は `web/` と本ファイルのみ。

## 未解決事項

- Playwright e2e・screenshot は未実行（ブラウザ起動を伴うため）。`routingShadowFixture` は fake daemon の
  既定 route には登録していない（Phase 3 の trajectory fixture と同じ扱い）。

## 提案

- なし。
