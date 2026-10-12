---
tasks: [01M4QHBJW4GBG10QA8F231W7XZ]
---

# Legacy routing purge progress

## mode-drop

- Removed `RoutingMode::Legacy` and the optimizer's legacy rank branch.
- `[model_routing]` now requires an explicit `mode = "shadow"` or `mode = "enforce"`. A config without the table still loads with routing disabled. The dispatcher carries whether routing was explicitly configured so the absent-table case does not install or enable proxy shadow behavior.
- Updated routing catalog fixtures, the additive event fixture, and the API/event schemas. Added a load-error test that checks both accepted mode spellings.
- Added the ADR note that runtime source/model selection belongs exclusively to `task_core::llm_route::route_select`; model_routing is for shadow observation or input to task `01M4M8D4AQ`.

### Evidence

- `cargo check --workspace --tests` — pass.
- `cargo test -p celeris` — pass (378 unit tests plus integration tests).
- `UPDATE_SCHEMA=1 cargo test -p task-api` — generated `docs/api/v1/api-v1.schema.json`; test run exposed a stale event fixture, corrected below.
- `UPDATE_SCHEMA=1 cargo test -p task-core` — schema generation completed; one additive event fixture still encoded `mode = legacy`, corrected to `shadow`.
- `cargo test -p task-core` — pass (898 tests, sidecar integration test, and doc tests).
- `cargo test -p llm-proxy` — pass (55 unit tests and integration tests).
- `cargo test -p task-api --test routing_catalog` — pass (2 tests).
- `cargo test -p task-api --test daemon_providers_config schema_endpoint_returns_the_committed_file` — pass.
- `cargo test -p task-dispatch routing` — pass (8 tests).
- `cargo test -p celerisctl routing_cli` — pass (2 tests).
- `sh "$CELERIS_WU_SCOPE_PATHS"` — listed only this unit's intended paths.
- `rg -n 'RoutingMode::Legacy' crates` — no matches.
- `git diff --check` — pass.
- `cargo test -p task-api --test routing` — pass (6 tests), including the corrected additive routing event fixture.
- `UPDATE_SCHEMA=1 TMPDIR=/tmp/xd cargo test -p task-api` — generated schema and unit/most integration suites passed; exposed the stale `mode = legacy` fixture in `tests/routing.rs`. After updating it, the complete routing integration suite passed.
- `cargo test -p celeris routing_table_requires_explicit_mode` — pass, verifying `Config::load` rejects an omitted mode while a config without `[model_routing]` still loads.
- `! grep -rn 'RoutingMode::Legacy' crates/ && ! grep -rn 'unwrap_or(RoutingMode' crates/celeris/src/config/` — pass after replacing the two validated-config fallback expressions with `unwrap_or_default()` (the Shadow default is only a safe internal value; `[model_routing]` without `mode` is rejected during parsing).
- `cargo check --workspace --tests` — pass after the final mode fallback cleanup.
- `cargo test -p celeris routing_table_requires_explicit_mode` — pass (1 test).
- `cargo test -p task-core` — pass (898 unit tests, 1 sidecar integration test, 3 doc tests).
- `sh "$CELERIS_WU_SCOPE_PATHS"` — pass; changed paths remain within the mode-drop scope.
- `git diff --check` — pass.
