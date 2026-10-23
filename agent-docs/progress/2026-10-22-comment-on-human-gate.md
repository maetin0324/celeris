---
title: comment が承認待ちを answer に化かす件
tasks: [01M5HD1FGG2KJFDH8BPJ50AEW1]
updated: 2026-10-22
status: done
completed: 2026-10-22
---
# comment が承認待ちを answer に化かす件

- 修正: `task_ops::comment::plan_human_comment` が `is_human_gate` の blocked を Stored（遷移なし）にする。
- 修復: dispatcher `wu_dispatch_gate` が、直前遷移 `answer` で ready に浮いた PlanGate 未通過の task を `PlanGate`（Ready からも可）で承認待ちに戻す。
- ADR: agent-docs/adr/2026-10-22-comment-on-human-gate.md
- 証拠: `bash scripts/dev/test-affected.sh` exit 0（試験 comment_on_plan_approval_keeps_the_gate_and_a_stranded_ready_task_returns_to_it）、clippy・fmt exit 0。
- 本番への移行: 無し。01M5HABE5N は取り消し済み。
