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

## 差し戻し修正（attempt 2）
- main `adc0373e` と前回 commit `9288657f` を再確認。残っていた注入障害の識別を `faults.rs` に追加した。
- manifest の事前登録と試行別のログ hash 付き観測を結合し、checker/export に exposure × result の独立した内訳を出す。
  注入未観測・未実施は未確定。不正な結合やログ改変は拒否する。詳細は ADR D4。
- 注入器の原ログは外部の実施確認が正本。checker はログの意味そのものを判定しない。本格測定・HPC 投入・汎用注入器の新設は行っていない。
- 追加した無 LLM 試験では局所 subprocess の SIGTERM 観測と実 grader→bind→check→export を通し、再採点前後の分母・消費・注入記録不変と不正記録の拒否を検証した。
- 最終検証: `bash scripts/dev/test-parallel.sh` exit 0（6,193 passed / 0 failed / 15 ignored）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
  初回の CSV ヘッダ互換性エラーは新列を末尾に追加する形に直し、全体検査を再実行して通過した。
