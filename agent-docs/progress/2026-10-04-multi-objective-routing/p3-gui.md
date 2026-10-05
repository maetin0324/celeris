---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: gui
status: done-in-branch
completed: 2026-10-05
---

# Phase 3 gui: task routing panel に実行 lane・provider 理由・実 source/model・escalation 理由・outcome の区別（p3-gui）

## 変更

- `gui/app/celeris/types.ts`: `corepack pnpm@11.27.0 -C gui gen:types` で再生成（audit-api 段で更新された `docs/api/v1/api-v1.schema.json` を反映。`RunRoutingAudit` に `actual_sources`・`decision_id`・`escalation_audit`・`routing_features`・`routing_outcome`・`outcome_state`、`RequestRoutingAudit` に `actual`・`attempts`・`fallback_reason`・`parent_decision_id`、`EscalationAudit`・`RoutingOutcome`・`RoutingOutcomeState`・`RoutingFeaturesRecord`・`RequestSourceAttempt`・`ActualSource`、event 3 種 `routing_features_recorded`/`routing_request_decided`/`routing_outcome_recorded` が新たに生成される）。
- `gui/app/lib/task-routing.ts`（純関数の追加）:
  - `escalationLine(run)`: 構造化監査 `escalation_audit` があれば `requested → selected` と理由・品質失敗の回数（`1 段` / `上げない`）、無ければ旧 event の自由文字列 `escalation`。両方無ければ null。
  - `escalationHistory(view)`: `escalation` か `escalation_audit` の付いた run だけ、`escalationLine` で 1 行に（旧 event 互換のまま）。
  - `OUTCOME_STATE_LABEL` / `outcomeStateLabel(run)`: `outcome_state` の 3 値（`not_recorded`=未追記 / `unreviewed`=未レビュー / `judged`=判定済み）を人の語に。未追記と未レビューを混ぜない。
  - `outcomeLine(outcome)`: `routing_outcome_recorded` の 1 行。review/acceptance は `true`/`false` のときだけ出し、`null`（未判定）は「判定なし（未レビュー/中断）」と `reward 未判定`。supersede した旧 outcome id も併記。
  - `executedLaneNote(run)`: 実行 lane（`run.lane`）と希望 lane（要求 trace の `requested_lane`）が違えば注記、同じなら null。
- `gui/app/lib/routing-source-state.ts`: `actualSourceLine(actual)` を追加（`source / model / account` と出所 `from`（proxy log / 試した source の最後 / proxy の決定）を人の語に）。
- `gui/app/components/TaskRoutingPanel.tsx`:
  - lane 欄に希望→実行の注記（`data-testid="task-routing-lane-note"`）。
  - run の `actual_sources` を「実 source」欄（`data-testid="task-routing-actual-sources"`）で出所付きで表示。
  - レビュー欄の後に outcome 欄（`data-testid="task-routing-outcome"`）: 状態ラベル + `routing_outcome` の 1 行。
  - escalation の履歴は構造化監査から理由・回数が出るように（`escalationLine` 経由）。
  - `routing_outcome.supersedes` があるとき置き換え注記（`data-testid="task-routing-outcome-supersede"`）。
- `gui/app/components/RoutingSourceState.tsx`（`RequestAudit`）:
  - 要求ごとの「選択理由」（trace の `reasons`、`data-testid="routing-request-reasons"`）。
  - 「試した順」（`attempts` の source 順）+ 最後の source へ落ちた原因（`fallback_reason`、`data-testid="routing-request-attempts"` / `routing-request-fallback`）。
  - 「実際」（`actual`、出所付き、`data-testid="routing-request-actual"`）。
  - run 単位の実 source は `actual_sources` 欄、要求単位は `actual` で出し分ける（両方出ても推定しない。値は celeris の監査をそのまま並べるだけ）。
- `gui/test/fixtures/api/routing-trajectory.json`（新規）: `routing_trajectory` fixture。run 2 件で区別を固定:
  - run 1（cheap）: `outcome_state=unreviewed`（review/acceptance/reward が全部 null）・`audit_incomplete=true`（`request_log_missing`）・実 source は `from=request_attempts`。
  - run 2（standard、最新）: `escalation_audit`（cheap→standard、品質失敗 2 回）・`outcome_state=judged`（review/acceptance 合格、reward 0.982、supersede）・`audit_incomplete=false`・試した順 2 件（`rate_limit` で account 交代）・実 source は `from=proxy_log`。
- `gui/test/unit/routing-trajectory.test.tsx`（新規、15 件）: outcome 状態の 3 値区別（未レビューは false/0 に見せない、未追記と未レビューを別にする、判定済みは reward・supersede）、escalation 構造化監査の 1 行化と履歴、実行/希望 lane の注記、実 source の出所、SSR の HTML で testid と文案を固定。

## 証拠

- `corepack pnpm@11.27.0 -C gui gen:types` + `git diff --stat app/celeris/types.ts`: types.ts 再生成（+207/-1）。
- `corepack pnpm@11.27.0 -C gui typecheck`（react-router typegen && tsc -b）: exit 0。
- `corepack pnpm@11.27.0 -C gui test`（vitest 全体）: 94 files, 1334 tests passed, exit 0（新規 15 件含む）。
- `corepack pnpm@11.27.0 -C gui build`: exit 0（既有の ineffective dynamic import 警告のみ、本変更由来ではない）。
- biome check（触った 5 ファイル）: exit 0。
- `git diff --stat -- crates/`: 差分なし（crates は変えていない）。

## 未解決

- 本番の `~/.config/celeris`・daemon・DB は触れていない。fixture（JSON）と SSR での確認のみ。
- `scripts/check-task-routing.mjs`（Playwright の実ブラウザ）は新しい欄（実 source・outcome）の期待値を持っていないため、今回は更新していない。既存の期待値（summary 行・features 9 軸・escalations 1 件）は新表示でも成立する（追加欄は既存の testid に衝突しない）。close 段で実ブラウザ確認するなら期待値の追加を検討。
- web/（event-kinds・invalidation-map と task routing 表示）は姉妹 WorkUnit（`web`）の担当。gui の生成型は両者が共通の `docs/api/v1/api-v1.schema.json` 由来で、web 側も同じ schema から再生成する。

## 提案

- run 単位の `routing_features`（特徴 snapshot と provenance・missing_fields）は現在パネルに出していない。次フェーズで出すなら `data-testid="task-routing-routing-features"` を別に作るのが良い（既存の `task-routing-features`（9 軸の旧 features）と混同しないため）。
