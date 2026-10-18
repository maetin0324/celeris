---
tasks: [01M576C2STNWSMHANHY43H6NJN]
status: done
completed: 2026-10-18
---
# result.json 回収の候補範囲

ADR: agent-docs/adr/2026-10-18-result-location-scope.md。
証拠: test-parallel 5757 passed / 0 failed、clippy exit 0、fmt --check exit 0。
未解決・提案: なし。

## attempt 3 (2026-10-18)
- 事故の形そのもの（task root の run、cwd = repos/agent-platform、wu/<key>/artifacts/result.json が 14 件、自分の result.json なし）の試験 `other_wus_canonical_reports_are_not_this_runs_strays` を追加し、`misplaced_result` の doc comment を ADR に合わせて更新。
- `cargo nextest run -p task-worker result_location misplaced` → 5 passed。
