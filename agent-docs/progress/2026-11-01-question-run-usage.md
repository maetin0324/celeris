---
title: question で終わった run の usage が記録されない
tasks: [01M6B3APKYSRQH9ZF14QYVTXNV]
status: running
updated: 2026-11-01
---

# question で終わった run の usage が記録されない

## 原因と修正

- question: `Terminal::Question` と `WorkerMessage::Question` に usage 欄が無く、adapter が観測した usage を運べなかった。両方に `usage: Option<Usage>` を足し、全 adapter・dispatcher（worker_finish・review・cos_chat・eval_trials・planner_flow・reports）・celerisctl の変換で運ぶ。claude_code は result.json の question でも `result` メッセージの usage を入れる。
- budget_exhausted（wall-clock 打ち切り）・harness_error 系（result.json 欠落、background task 残り）: claude_code は `result` メッセージを観測する前に打ち切るため `usage: None` で終端を作っていた。`Error` だけは累積 usage（`ExplorationTracker::cumulative_usage`）で埋めていたが、他の終端は埋めていなかった。usage 欄を持つ全終端（Done・Yielded・BudgetExhausted・Error・Question・Waiting）で、欄が空なら累積 usage を入れるように直した（`terminal_usage_slot`）。
- cancelled: eval 試行の回復・停止・持ち主のいない runs 行の回収（`eval_trials.rs` の 3 か所）と、review 結果を捨てる経路（`review_verdict.rs`）は、終端の usage を持たずに `run_index_finish(usage = None)` で行を閉じる。`run_index_finish` は `usage_json = ?3` で、run の途中に `run_index_progress_usage` が書いた累積 usage を NULL で上書きしていた。`usage_json = COALESCE(?3, usage_json)` に直し、終端の usage が無いときは途中の累積値を残す（`crates/task-core/src/store/execution/runs.rs`）。途中の usage を一度も観測しなかった run（usage を流さない adapter、起動直後の停止）は NULL のまま残る。
- completed: 終端の `Done` に usage が無い run。claude_code は `result` メッセージの usage か累積 usage で埋める（上の修正）。aider・acp（opencode）など usage を出さない harness の run は取る手段が無く NULL のまま残る（直せない）。本番 DB の件数の harness 別内訳は未確認。

## 証拠（提出時の HEAD で再実行する）

- `cargo nextest run -p task-dispatch -E 'test(question_run_records_usage) | test(token_budget)'`: 4 passed
- `cargo nextest run -p task-worker -E 'test(claude_code)'`: 132 passed
- `cargo nextest run -p task-core -E 'test(run_index)'`: 2 passed（`run_index_finish_without_usage_keeps_progress_usage` を含む）

## 未解決事項

- cancelled・completed の NULL の原因
- test-affected.sh・clippy・fmt
