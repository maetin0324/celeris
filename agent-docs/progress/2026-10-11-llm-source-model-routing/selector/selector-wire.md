---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---
# Selector wire

## 2026-10-13

- Replaced the dispatcher `DefaultRoutePolicy` call with `task_core::route_select::select`: shadow uses the harness×lane `route_defaults` column; enforce uses the evidence rule; admission walks the selected route's ranked fallbacks and fails closed.
- Added the `celeris.route-decision/1` trace with stage, rule, evidence version, propensity, exclusions, options hash, and feature reference fields. `RouteFeaturesRef` carries this decision and the store records it in `route_decisions`.
- Dispatch now assigns its run id before selection and uses that id as the deterministic exploration seed and decision id. The dispatch test confirms the `RoutingDecided` feature ref and `route_decisions` row share the decision id and feature hash/version.
- Removed the temporary `RoutePolicy` / `DefaultRoutePolicy` implementation and its dictionary/default-policy test from `llm_route.rs`.
- Updated the routing API schema.

### Checks

- `cargo check -p task-dispatch` — passed.
- `cargo test -p task-core route_select:: --lib` — 6 passed (shadow defaults, evidence selection, deterministic exploration, decision record, guardrail).
- `cargo test -p task-dispatch dispatcher::tests::route_select:: --lib` — 12 passed (including dispatch event and DB record).
- `UPDATE_SCHEMA=1 cargo test -p task-api schema::tests:: --lib` — 2 passed.
- Rustfmt check on touched Rust files, `git diff --check`, and WorkUnit scope check — passed.
- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` could not complete: the local pnpm store lacks the `siginfo@2.0.0` tarball. Web `gen:types` could not run without dependencies; the shared plan forbids external network access.

### Remaining

- Web generated type regeneration remains unverified because the required package tarball is absent from the offline store.

### Recheck after feature snapshot ordering correction

- Moved the existing `record_route_features` call before route selection and pass that same `RouteFeaturesRef` into `select_route_with`; `route_decisions` and `RoutingDecided` now use the selector's exact feature hash/schema/template reference. No second feature snapshot is created.
- Rechecks: `cargo test -p task-dispatch dispatcher::tests::route_select:: --lib` (12 passed); `cargo test -p task-core route_select:: --lib` (6 passed); `UPDATE_SCHEMA=1 cargo test -p task-api schema::tests:: --lib` (2 passed); `cargo fmt --check -p task-core -p task-dispatch`; `git diff --check`; WorkUnit scope check all passed.
- Web `gen:types` remains unverified: the offline pnpm store does not contain `siginfo@2.0.0`; no network was used.

### Recheck after WorkUnit post-run failures

- Removed the `clone()` from the `Copy` task-features value flagged by clippy.
- `cargo clippy -p task-dispatch -p task-core --all-targets -- -D warnings && cargo test -p task-core --lib store::tests::event_row_schema` passed (the schema test passed).
- The prescribed web command was retried. Offline dependency installation stopped before `gen:types`: the local pnpm store lacks `side-channel-map@1.0.1` (`ERR_PNPM_NO_OFFLINE_TARBALL`). Per the common plan, no network install was attempted. Generated API files and source schemas remain included together; generation freshness could not be rechecked in this environment.
- `git diff --check` and `$CELERIS_WU_SCOPE_PATHS` passed.

### Run #6 retry

- Retried the prescribed offline web generation command. Install still stops before `gen:types`: the local pnpm store lacks `caniuse-lite@1.0.30001810` (`ERR_PNPM_NO_OFFLINE_TARBALL`). No network install was attempted.
- Rechecked `cargo fmt --check -p task-core -p task-dispatch`, `git diff --check`, and `$CELERIS_WU_SCOPE_PATHS`; all passed. The generated files remain paired with the source schemas; freshness cannot be rechecked until the offline store is populated.

### Retry after selector-wire post-run failure

- Reproduced the prescribed web check from this worktree. It exits 1 during `pnpm install --offline --frozen-lockfile` because the local store lacks `react-remove-scroll-bar@2.3.8`; `gen:types` therefore does not run. The human instruction allows generated outputs to remain in this unit and requires source schemas and outputs to ship together; all four schema/generated files are present in the current diff. No online install was attempted because the shared plan forbids external network use.
- Rechecked `cargo check -p task-dispatch`, `cargo test -p task-core route_select:: --lib` (6 passed), `cargo test -p task-dispatch dispatcher::tests::route_select:: --lib` (12 passed), `cargo fmt --check -p task-core -p task-dispatch`, `git diff --check`, and `$CELERIS_WU_SCOPE_PATHS`; all passed.
- Remaining environment limitation: web generated-file freshness cannot be rerun until the offline pnpm store contains the locked dependencies.
