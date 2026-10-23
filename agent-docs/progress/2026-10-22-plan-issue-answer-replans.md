---
title: plan_issue への人の回答で計画（check・範囲）を直す
updated: 2026-10-22
status: done
completed: 2026-10-22
tasks: [01M5HMXMZ8CJ0JQRJNPD707N2P]
---
# plan_issue への人の回答で計画（check・範囲）を直す

- ADR: agent-docs/adr/2026-10-22-plan-issue-answer-replans.md（回答の意図 `resume` / `replan`、人の回答起点の replan は `max_replans` の外）
- 実装: `task_ops::gate::AnswerIntent`・`answer_with_intent`・`answer_intent_events`（`replan` は `blocked(plan_issue)` の WU があるときだけ、
  `ExecutionHintSet{replan: true}` を同じ tx で書く → 既存の `pending_replan_request` で planner が回答を指示に replan）。
  `AnswerBody.intent`、CoS の `answer_op` も同じ。受信箱の質問は `QuestionItem.plan_issue_units` があれば `answer`（回答して再開）と
  `replan`（計画を直す。推奨）を出し、`POST /inbox/{id}/answer` の `option: replan` を `intent: replan` に写す。
  上限を使い切った後の plan_issue の質問文に `PLAN_ISSUE_REPLAN_HINT` を添える。
- 試験: `a_plan_issue_answer_with_replan_intent_fixes_the_check_after_replans_are_exhausted`（max_replans=0、b の check `false` →
  回答 intent=replan → v2 の b の check `true`、task done、plan_issue の申告 1 回、counted_replans 0、planner の replan_reason に回答）、
  `a_plan_issue_answer_with_resume_keeps_the_check_and_replan_needs_a_plan_issue`（resume は同じ check で 2 回目の申告、plan_issue の無い質問への replan は Validation）、
  `human_inbox_plan_issue_question_offers_resume_and_replan`。
- 証拠: `bash scripts/dev/test-affected.sh` exit 0（5054 passed, 16 skipped）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- schema: `UPDATE_SCHEMA=1 cargo test -p task-api schema --lib`、`node web/scripts/gen-types.mjs` で再生成。
- 本番への移行: 無し（config・migration の変更なし）。

## 未解決事項
- web の task 詳細の回答欄は `intent` を出さない（受信箱から選ぶ）。必要なら UI 側で選択肢を足す。
- 木の節点では `max_tree_replans` が今どおり効く（超えると limit の決定の要求になる）。
