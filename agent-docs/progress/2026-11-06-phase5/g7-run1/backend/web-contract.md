---
title: web 生成型の検証と永続契約の受け渡し（web-contract、Phase 5）
tasks: [01M6RJTQQ629D0NP953708JMS4]
status: done
updated: 2026-11-06
completed: 2026-11-06
---
# web-contract（Phase 5）

完了日: 2026-11-06。正本 ADR: [2026-11-06-cos-run-launch-accounting-and-drain](../../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)。新しい設計判断は無く、ADR は足していない。Rust・web の実装コードは変えていない（この WU の変更は本ファイルのみ）。

**backend 全体はまだ完成していない。** 本 WU は永続契約（store API・通知・集計・API view・schema・web 生成型）の検証と記録で終わる。worker/dispatcher の起動接続・同時実行枠の待機・driver drain・judge は後続範囲で、未実装。

## 修正前の回帰
- 修正対象の回帰は無かった（生成型・fixture はすでに schema と一致）。そのため修正前の失敗 exit は無い。

## 実行結果（基準は `$CELERIS_WU_BASE`、提出時の HEAD で実行）
- `node web/scripts/gen-types.mjs --check`: exit 0（`docs/api/v1/api-v1.schema.json` から再生成した `web/api/generated` と一致）。
- `cd web && pnpm install --offline --frozen-lockfile`（packageManager は `pnpm@12.6.0`）: exit 0。
- `pnpm run typecheck`（`tsc -b`）: exit 0。
- `pnpm test`（`vitest run && node --test server/*.test.mjs`）: exit 0。vitest 104 ファイル・711 件 pass、node:test 113 件 pass・fail 0。
- 生成型の `ChatRun` は `launched_at`・`prelaunch_reason`・`cost_state`（`ChatCostState = "known" | "unknown" | "not_launched"`）を持つ。API field を手書きで重複定義した箇所は足していない。fixture の修正は不要だった。

## 次 task が使う契約
- store API と通知 payload: [durable-contract.md](durable-contract.md) の「永続 API」。起動側は `chat_run_mark_launched` が `Ok` のときだけ spawn する。
- 集計と view: `cost_state_of`（`task-core/src/chat/run_store.rs`）と予算の和は同じ判定。`budget_today.not_launched_runs` は [api-contract.md](api-contract.md) と `docs/api/v1/api.md`。

## 未接続・未解決
- 未接続（後続 task）: `launch.rs` の spawn 直前の `mark_launched`、`record_attempt` からの `chat_run_record_attempt_usage`、`routes.rs` の `fail_routes` の `finish_prelaunch`、起動時回収の `chat_run_recover`、人の stop の起動前後分岐、枠待ち、driver drain、judge。
- `web/features/projects/project-controller-panel.tsx` の `budget_today` 型は手書きで、`not_launched_runs` を持たない（`ProjectControllerView` は schema 対象外のため生成型に無い）。API は欄を返すが UI は読まない。UI 機能は本 WU の範囲外なので足していない。
- `ChatRun.launched_at`・`prelaunch_reason` は None のとき欄ごと省略される（api-contract.md の提案を参照）。

## 提案
- 表示が要るなら別 task で `project-controller-panel.tsx` の `budget_today` に `not_launched_runs` を足し、`web/e2e/parity/project-controller.spec.ts` の fixture にも欄を足す（e2e は release gate で実行）。
