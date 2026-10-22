---
title: 進展の無い自動 replan を止める
status: done
completed: 2026-10-22
updated: 2026-10-22
tasks: [01M5HZ7JR0K9C9EN6BGE8T25XV]
---
# 進展の無い自動 replan を止める（同じ WU・同じ失敗）

- ADR: agent-docs/adr/2026-10-22-stalled-auto-replan.md
- A: `Event::AutoReplanRequested{key, failure, source}` を自動 replan のたびに積む（`integration_gives_up_with`・
  `finish_worker_result`・`deferred_work_unit_trigger`）。`task_ops::plan_gate::{failure_signature, normalize_check_cmd,
  stalled_replans}`（`plan_gate/stall.rs`）。同じ `(key, failure)` が変更のある `WorkUnitCommitted` なしに 2 回続いたら
  自動 replan をやめ、`REPLAN_STALLED_QUESTION_PREFIX` の質問にする（回答は人の replan、上限に数えない）。
- B: `RepairClass::Build`（`is_compile_error`: `e: file://` 行・`Compilation error`・`error[E`・`error: could not compile`）。
- C: `plan_replan` が統合の失敗からの自動 replan に対する done の checks だけの planner 計画を「no progress」で拒む。
  planner prompt（/2・/3 の replan 節）に「新しい葉か repair を足す」。
- D: `limit_decision` の文言は `count == max` で「使い切りました（{count}/{max}）」。
- schema 再生成: docs/api/v1/{api-v1,event}.schema.json、web/api/generated（`node web/scripts/gen-types.mjs`）。

## 証拠
- `cargo test -p task-dispatch --lib stalled_replan`: 3 passed（check だけの replan が 2 回以内で質問になる・counted_replans ≤ 2 /
  commit なしの同じ統合失敗は 2 回の replan の後に質問 / compile error の統合失敗で repair WU class build）。
- `cargo test -p task-ops --lib plan_gate`: stall の unit 試験 6 件含め pass（正規化・commit を挟むと数えない・人の依頼で切る）。
- `cargo test -p task-core --lib -- classifies_a_compile_error daemon_decisions_have_the_d7_shape`: pass。
- `bash scripts/dev/test-affected.sh` exit 0（6274 passed, 16 skipped）。1 回目は schema drift 2 件（再生成で解消）と
  celeris の jev_key / classify_tick の 3 件が負荷下で落ちた（単独再実行 3 passed、2 回目の全体実行で pass。変更とは無関係）。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。

## 再試行（attempt 2、レビューの差し戻し）
- web の realtime: `web/api/realtime/event-kinds.ts`（EVENT_KINDS）と `invalidation-map.ts`（`auto_replan_requested: T・E`）に
  新しい event を足した（足さないと invalidation-map.test.ts が 2 件落ち、SSE で届いた event で invalidator が例外）。
  ADR の影響節の「API の schema には出ない」を直した。
- 軽微: compile error の判定を fmt・lint の cmd の判定の後へ移した（clippy の失敗は Lint のまま。task-core の試験に追加）。
- 証拠: `pnpm exec vitest run api/realtime`（web）3 files・61 passed。`bash scripts/dev/test-affected.sh` exit 0
  （6274 passed, 16 skipped、ログは artifacts/test-affected.log）。`cargo clippy --workspace -- -D warnings` exit 0、
  `cargo fmt --all -- --check` exit 0。

## 未解決事項
- repair WU（`repair-<phase>-<n>`）の plan_issue からの replan は key が回ごとに違うので連なりにならない（`max_repairs_per_class`
  で止まる。自動 replan は高々 repair の数 + 1）。

## 本番への移行
- 無し（config・migration の変更なし）。

## 再試行（attempt 3、検査の再確認）
- 開始時 HEAD と task branch はともに `6bbf25a8`、作業ツリーはクリーン。前回の実装を保持した。
- reviewer の `arm_set_cli_normal::ab_only_counts_repetitions_retries_grading_and_cost_and_resumes_without_reposting`
  における `bad_auth == 2` は再現せず、原因は未特定。認証の検査は維持し、HTTP stub に不一致時の method/path の診断を
  追加した。認証 header・token は出力しない。認証失敗の根本修正を済ませたという主張ではない。
- `bash scripts/dev/test-affected.sh`: exit 0、6274 passed / 16 skipped、doc test も成功。
- `cargo nextest run -p celeris-campaign --test arm_set_cli_normal`: exit 0、6 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `pnpm exec vitest run api/realtime`（web）: exit 0、3 files / 61 passed。
- 全出力は `/local/celeris/data/workspaces/01M5HZ7JR0K9C9EN6BGE8T25XV/artifacts/` の
  `attempt3-test-affected.log`、`attempt3-campaign.log`、`attempt3-clippy.log`、`attempt3-fmt.log`、
  `attempt3-web-realtime.log` に保存。
