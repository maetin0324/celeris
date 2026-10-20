---
title: "[研究評価接続 2/P0] 独立採点の失敗原因を checker/export まで保持し、分母と再採点方針を確定"
tasks: [01M5BT8CNN9AYC6NN036DVR6F9]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# 失敗原因の保持（grade_failure_class）

- 着手時の main で所見は残存（`grades::bind` が final_pass の bool に潰す）。解消済みの項目なし。
- 確定 schema・分母定義は ADR `agent-docs/adr/2026-10-20-grade-failure-class-and-denominators.md`（4 human_ops 比較表と共有）。
- 変更: `grader.rs`（`FailureClass::Undetermined`・`CheckResult.undetermined`）、`grades.rs`（`GradeOutcome`）、`checker.rs`（Totals 3 欄・complete 理由）、
  `export.rs`（列・`failure_breakdown`）、`grade_out.rs`（class 別件数・`grades.history.jsonl`）。
- 本番への移行: なし（旧 grades.jsonl で不合格行に failure_class が無いものは再採点が必要。grade-out の再実行で足りる）。
- 未解決: 科学成果物の意味的無効化・scheduler 受理不明の照合は範囲外。
