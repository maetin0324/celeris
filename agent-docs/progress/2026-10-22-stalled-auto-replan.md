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

## 未解決事項
- repair WU（`repair-<phase>-<n>`）の plan_issue からの replan は key が回ごとに違うので連なりにならない（`max_repairs_per_class`
  で止まる。自動 replan は高々 repair の数 + 1）。

## 本番への移行
- 無し（config・migration の変更なし）。
