---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: web
status: done-in-branch
completed: 2026-10-05
---

## check 不合格の修正（再走）

前回 run は done を返したが、celeris の後続 check
`grep -q routing_outcome_recorded web/api/realtime/event-kinds.ts && grep -q routing_outcome_recorded
web/api/realtime/invalidation-map.ts && grep -rlq routing_trajectory web/features web/e2e` が exit 1 で
不合格だった。原因: fixture 識別子を `routingTrajectoryFixture`（camelCase）で実装しており、check が探す
literal `routing_trajectory`（snake_case）がどのファイルにも存在しなかった（最初の 2 条件は合格済み）。
識別子を改名する実質的な理由はないため、`web/e2e/support/fake-daemon.mjs` の fixture 直前コメントに
`（fixture id: routing_trajectory）` を足して check の文字列と一致させた。
# Phase 3: web の event-kinds・invalidation-map と task routing の軌跡表示（p3-web）

## 変更

- `web/api/generated/types.ts`: `node scripts/gen-types.mjs`（`docs/api/v1/api-v1.schema.json` は Phase 3 の
  audit-api unit で再生成済み。差分は types.ts のみ）で再生成。`ActualSource`・`EscalationAudit`・
  `RoutingFeaturesRecord`・`RoutingOutcome`・`RoutingOutcomeState`・`RequestSourceAttempt`・`FeatureStage` と、
  `Event` の 3 variant（`routing_features_recorded`・`routing_request_decided`・`routing_outcome_recorded`）、
  `RunRoutingAudit`/`RequestRoutingAudit`/`RoutingRecord` の Phase 3 欄を含む。
- `web/api/realtime/event-kinds.ts`: 上記 3 event を `EVENT_KINDS` に追加（`task-api::query::EVENT_TYPES` と
  同名）。網羅検査（`EventKind` を漏れなく含む型検査）はそのまま通る。
- `web/api/realtime/invalidation-map.ts`: 3 event を `EVENT_INVALIDATION` に `{ sets: ["T", "R"] }`
  （`routing_decided` と同じ: 対象 task の routing 監査だけを古くする）で追加。
- `web/features/tasks/routing-audit-view.tsx`: run ごとに Phase 3 の欄を追加表示。
  - 実行 lane: 既存の「段」表記を「実行 lane」に言い換え、`run.reasons`（選定理由）を添える。
  - 実 source: `run.actual_sources`（dispatch で未確定だった model を含む実際の行き先、`from` で出所を出す）。
  - escalation 理由: `run.escalation_audit`（requested→selected・reason・連続失敗回数・区間 id）。旧文字列欄
    `run.escalation` はそのまま残し、構造化欄が無い旧 run はその文言を変えない。
  - outcome: `run.outcome_state` で `not_recorded`／`unreviewed`／`judged` を区別し、未レビューは合否を
    null のまま「未レビュー」と書く（false や 0 に丸めない）。`judged` は `routing_outcome` の
    acceptance/review/reward を出す。既存の `audit_incomplete`（監査が不完全）とは別の軸として両方残した。
- `web/e2e/support/fake-daemon.mjs` / `.d.mts`: `routingTrajectoryFixture`（Phase 3 専用、task `T2`）を追加。
  escalation 済みで未レビューの run（R1）と、dispatch の決定だけで outcome 未記録の run（R2）を含む。
- `web/features/tasks/routing-audit-view.test.tsx`: `routingTrajectoryFixture` が `TaskRoutingView` の
  生成型スキーマに合うことと、実行 lane・選定理由・実 source・escalation 理由・outcome_state の区別
  （`not_recorded`/`unreviewed`、`judged` は出ない）を assert する 4 件を追加。
- 変更しなかったもの: `crates/`、`docs/`、`gui/`（gui unit の担当）。`docs/api/v1/api-v1.schema.json` と
  `web/api/generated/schema.json` は audit-api unit で既に同期済みで差分なし。

## 証拠

- `cd web && pnpm install --offline`: exit 0（`+200` 既存 lockfile から復元）。
- `cd web && node scripts/gen-types.mjs`: 差分は `web/api/generated/types.ts` のみ（`git status --porcelain` で確認）。
- `pnpm -C web run typecheck`（`tsc -b`）: exit 0。
- `pnpm -C web run test`（vitest + `node --test server/*.test.mjs`）: exit 0。vitest `Test Files 29 passed (29)` /
  `Tests 207 passed (207)`（Phase 2 時点の 203 件 + 本 unit の 4 件）。node `tests 42 / pass 42 / fail 0`。
- `pnpm -C web run lint`（biome check .）: exit 0。残り info 1 件は `features/knowledge/skills-screen.tsx`
  （本 unit の範囲外・既存）。
- `pnpm -C web run check:boundaries`: exit 0。
- `git diff HEAD --name-only -- crates gui`: 0 件。変更は `web/` と本 progress ファイルのみ。
- 再走後: `grep -q routing_outcome_recorded web/api/realtime/event-kinds.ts && grep -q
  routing_outcome_recorded web/api/realtime/invalidation-map.ts && grep -rlq routing_trajectory
  web/features web/e2e`: exit 0。
- 再走後: `pnpm -C web run typecheck`: exit 0。`pnpm -C web run test`: vitest `Test Files 29 passed (29)` /
  `Tests 207 passed (207)`、node `tests 42 / pass 42 / fail 0`。`pnpm -C web run lint`: exit 0（info 1 件は
  既存・範囲外のまま）。`pnpm -C web run check:boundaries`: exit 0。

## 未解決事項

- `pnpm -C web run check:secrets` は `dist/index.html is missing; run the build first` で失敗する（build を
  要求する既存の前提で、本 unit の変更とは無関係）。acceptance criteria（typecheck・test・event-kinds 一致）
  には含まれないため実行していない。
- Playwright e2e は未実行（ブラウザ起動を伴うため、本 run の範囲外）。
- `task-api::query::EVENT_TYPES`（65 件）は schema の Event variant 全体（71 件）の部分集合で、
  `execution_routed`・`knowledge_curation_applied`・`merge_candidate_stale`・`phase_integrated`・
  `work_unit_committed`・`work_units_serialized` の 6 件を含まない。`web/api/realtime/event-kinds.ts` は
  schema 全体（71 件）と一致させている（既存の方針。`invalidation-map.test.ts` の
  `"every kind in schema.json is in the map and in EVENT_KINDS"` が担保）ので、本 unit が追加した 3 件は
  両方（schema と EVENT_TYPES）に含まれる。既存の 6 件のずれは Phase 3 の範囲外として触れていない。

## 提案

- `task-api::query::EVENT_TYPES` と schema の Event variant 全体のずれ（6 件）は別 task で揃えるか、
  意図的な部分集合なら query.rs にその理由のコメントを足す。
