---
task: 01M6B3APKYSRQH9ZF14QYVTXNV
title: question で終わった run の usage が記録されない
status: running
started: 2026-11-01
---

# question で終わった run の usage が記録されない

## 原因と修正

- question: `Terminal::Question` と `WorkerMessage::Question` に usage 欄が無く、adapter が観測した usage を運べなかった。両方に `usage: Option<Usage>` を足し、全 adapter・dispatcher（worker_finish・review・cos_chat・eval_trials・planner_flow・reports）・celerisctl の変換で運ぶ。claude_code は result.json の question でも `result` メッセージの usage を入れる。
- budget_exhausted（wall-clock 打ち切り）・harness_error 系（result.json 欠落、background task 残り）: claude_code は `result` メッセージを観測する前に打ち切るため `usage: None` で終端を作っていた。`Error` だけは累積 usage（`ExplorationTracker::cumulative_usage`）で埋めていたが、他の終端は埋めていなかった。usage 欄を持つ全終端（Done・Yielded・BudgetExhausted・Error・Question・Waiting）で、欄が空なら累積 usage を入れるように直した（`terminal_usage_slot`）。
- cancelled・completed: 調査中（次の run）。

## 証拠（提出時の HEAD で再実行する）

- `cargo nextest run -p task-dispatch -E 'test(question_run_records_usage) | test(token_budget)'`: 4 passed
- `cargo nextest run -p task-worker -E 'test(claude_code)'`: 132 passed

## 未解決事項

- cancelled・completed の NULL の原因
- test-affected.sh・clippy・fmt
