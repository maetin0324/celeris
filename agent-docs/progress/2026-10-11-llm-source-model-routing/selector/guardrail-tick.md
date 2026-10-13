---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---
# Guardrail tick

## 2026-10-13

- Added `daemon::routing_guardrail`: every 15 minutes the active daemon evaluates recent `routing_outcome_recorded` windows against routing evidence for enforced genre×harness pairs. The decision uses the existing pure `task_core::route_select::evaluate_guardrail` rules and injected clock/run inputs in tests; no LLM is called.
- A stop condition moves the pair to `shadow` with actor `daemon`, reason, evidence version, and numeric window in `routing_stage_changed`, then records an inbox notice. Pairs already in shadow are skipped. Quota below the configured 10% floor records a deduplicated notice only and leaves the stage enforced; there is no automatic promotion.
- `task_ops::routing_outcome::route_windows` projects the latest non-superseded outcomes, associating each run with its decision harness or task harness fallback.

## Checks

- `cargo test -p celeris --lib daemon::routing_guardrail::tests`: passed, 8 tests (each stop reason, below-threshold behavior, quota-only notice, event+notice, deduplication, and injected clock).
- `cargo test -p task-ops --lib routing_outcome::tests::route_windows`: passed, 2 tests.
- `git diff --check`: passed.
- `sh "$CELERIS_WU_SCOPE_PATHS"`: passed; changed paths are within daemon, task-ops routing outcome, and this progress note.

## Open items

- The production daemon currently has no live harness-version probe. Unknown versions are left unknown and do not trigger a version-mismatch rollback; quality and quota guardrails still run.
- Quota notification needs an observable selected-source account measurement. If none is available, the tick does not infer low quota.
