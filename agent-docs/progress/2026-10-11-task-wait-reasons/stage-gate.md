---
title: "stage-gate: 人の gate を task_ops::wait::human_gate_waits にまとめ StageReview を出す"
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# stage-gate（final review 差し戻し: StageReview を出さず human_gate_hold と非共有）

## 変えたもの
- `crates/task-ops/src/wait.rs`: `gate_waits` を消し `human_gate_waits(task, plan, events)` を置いた（PlanApproval / StageReview / PhaseCheckpoint）。
  `task_waits` は `execution_plan_active` を渡して呼ぶ。
- `crates/task-dispatch/src/dispatcher/tree_units.rs`: `human_gate_hold` は `human_gate_waits` の結果から決める（呼び出し元 `work_units.rs`・
  `tree_units.rs` の署名は不変）。
- 試験: `crates/task-ops/src/view/tests.rs`（旧 `task_wait_api_gate_waits_and_runnable` を `task_wait_api_human_gate_waits_and_runnable` に書き換え、
  `task_wait_api_stage_review_*` 3 件）、`crates/task-dispatch/src/dispatcher/tests/task_waits.rs`（`task_wait_stage_gate_hold_iff_waits`: 8 通りの入力で
  hold が Some ⇔ waits 非空）、`.../tests/human_gates.rs`（`task_wait_stage_gate_review_human_hold_matches_the_api_waits`: 実 dispatcher で s1 の
  `review: human` に止め、hold=`stage_review`、`task_waits_one` に `StageReview{s1}`、「続ける」で両方解ける）。
- ADR 付記「葉 stage-gate」。API schema は変わらない（`TaskWait`・`HumanGateKind` は既存）。

## 証拠
- `cargo test -p task-ops task_wait_api` → 5 passed（stage_review 3 件含む）。
- `cargo test -p task-dispatch task_wait_stage_gate` → 2 passed。
- `cargo clippy -p task-ops -p task-dispatch -p task-api --all-targets -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → passed 5188, failed 0, ignored 14, nextest_exit 0, doctest_exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決
- web の wait-reason の StageReview ラベルは並行の葉 web-label の担当。

## 提案
- なし。
