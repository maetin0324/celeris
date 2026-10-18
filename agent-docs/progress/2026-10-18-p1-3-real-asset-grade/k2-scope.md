---
task: P1-3 k2-scope
status: done
completed: 2026-10-18
---
# k2-scope: K2 の判定範囲

- ADR: agent-docs/adr/2026-10-18-k2-judged-scope.md（選択肢 2「範囲を事前明示」を採用）
- case.json: `judged_scope: [implementation, experiment]`（analysis, review は not_judged）。grade_case_ref.sha256 を
  bbaca17a… → d944a6c1… に更新。lock は case から計算するため旧 lock は新 grade-case と不一致（試験で固定）。
- grade-case: 任意の `judged_scope`、analysis⇔recompute・review⇔review の一致を validate。`GradeReport` に judged_scope / not_judged。
- 報告: `report::scope_line` / `check_scope_claims`、`check-report --cases-root <dir>`。
- 試験: crates/campaign/tests/k2_scope.rs（6 件）。
- 未解決: analysis/review を判定したくなったら別 ADR で case を追加（今回は新設しない）。
