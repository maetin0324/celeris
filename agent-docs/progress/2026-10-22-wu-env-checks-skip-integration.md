---
title: 統合の検査で CELERIS_WU_* を使う WU の check を流さない
tasks: [01M5HPMDFY1DDV6NCQ9KNNEPH5]
status: done
updated: 2026-10-22
completed: 2026-10-22
---
# 統合の検査で CELERIS_WU_BASE が空になり必ず落ちる件

- 決定: `cmd` が `CELERIS_WU_` を含む WU の check は `scope` の有無によらず WU の作業時だけで判定し、段の統合の検査と子 task の acceptance に入れない（`task_core::WorkUnitCheck::is_work_unit_only`）。
- ADR: agent-docs/adr/2026-10-22-wu-env-checks-skip-integration.md（ADR-0074 付記 2026-10-05 の 5 を置き換え）
- planner 指示（`PLANNER_CHECK_GUIDANCE`）・agent-docs/guides/work-unit-checks.md・docs/protocol/worker-protocol.md に同じ規則を書いた。
- 試験: `scope_checks::wu_env_checks_without_scope_flag_pass_at_work_unit_time_and_skip_integration`（WU で通り、統合では流れず done。env 無しなら exit 128 `bad revision` になる形も固定）、`integration_checks_for_phase_skips_scope_checks`、`promote_to_task_drops_wu_env_checks_from_acceptance`。
- 証拠: `bash scripts/dev/test-affected.sh` exit 0（6264 passed）、`cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 本番への移行: config 変更なし。release 後、止まっている 01M5HJ4XEG（moshi-brewed）は統合をやり直せば当該 check を流さずに進む。

## 未解決事項
- `cargo test … && git diff "$CELERIS_WU_BASE" …` のように連ねた check は、全体が統合で流れなくなる（planner 指示で分けるよう書いた）。
