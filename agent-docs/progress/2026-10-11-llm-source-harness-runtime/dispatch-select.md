---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: Dispatcher harness and source×model selection
updated: 2026-10-11
---

# Dispatcher harness and source×model selection

## Done (session 1)

- `crates/task-dispatch/src/dispatcher/route_select.rs`: a pure `route_select(input, registry, state)` that runs these steps in order. Each step fails closed.
  1. Pick the harness with `select_harness`: the task `harness_id` first, otherwise the `[harness_selection]` rules.
  2. Take the `route_defaults.<harness>` (or CoS) priority.
  3. Use `enumerate_candidates`. An explicit model narrows the set, and an unmatched model fails.
  4. Apply `constrain_for_harness`, which needs a direct API format or a registered `[[llm_routes]]` entry.
  5. Run `admit_source` against the per `(source, account)` limits.
  6. Choose with `DefaultRoutePolicy`.

  It returns a `RouteTrace` (candidates, rejections with reasons). `trace.metadata()` is the versioned payload (`version: 1`).
- `Dispatcher::set_llm_route_registry`, `Dispatcher::route_state` and `Dispatcher::select_route`:
  - Admission state comes from the OAuth account dirs (`claude_oauth`/`codex_oauth`/`opencode_go` → account pool) and the account books (cooldown, exhausted windows).
  - The stored `(source_id, account_id)` limits are also used.
  - Runs holding a `RunEntry.route` count as in use.
- `Dispatcher::record_route_selection` appends `harness_selected` and `llm_route_selected` (metadata = candidates, rejections, decision). It writes `runs.selected_harness_id/source/model/account/selection_metadata_json` through the new `TaskStore::run_record_selection`. A refusal appends a `routing` worker progress line that carries the reason and the trace.
- `task_core::llm_route::RunSelection` holds what is written to the run row.
- Daemon wiring: `bootstrap.rs` and the reload in `admin.rs` pass `config.llm_route_registry()` to the dispatcher. A reload with an invalid registry is refused.
- Tests (`crates/task-dispatch/src/dispatcher/tests/route_select.rs`):
  - `harness_select_`: determinism, explicit field wins, unknown explicit is refused, ambiguous and missing rules fail closed, only structured task fields are read.
  - `route_select_`: declared order with fallback to the next declared candidate, model hint narrowing and unmatched failure, the lane label being ignored, admission exclusions, rejection of an unregistered combination before dispatch, no defaults failing closed, events and the run row being recorded.

## Checks

- `cargo test -p task-dispatch --lib -- harness_select_ route_select_` — exit 0, 9 passed.
- `cargo build -p celeris` — exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0.

## Remaining (next session)

1. Switch `dispatch_run.rs` from `select_provider_with_default` to `select_route`.
   - The adapter is the harness adapter, then `with_model(selected model)`, then the account env through `adapter_for_account` for OAuth sources.
   - Set `RunEntry.route` and call `record_route_selection` after `run_index_start`.
   - Have the harness adapter map come from the daemon: the `[adapters.<kind>]` base plus the harness `command`/`args`, as the ADR says ("既存 `[adapters.*]` の process 固有設定と `command`/`args` は対応する harness 宣言へ移す").
2. Move `review_spawn.rs` the same way.
3. Delete `provider_select.rs`, `coding_default.rs`, `LocalProviderSpec` and local providers, `effective_lane_model` and assignment re-binding, `ProviderSpec` and the `StaticPolicy` provider rows (policy.rs), and `config.provider_specs()` and `local_cheap_providers()`.
4. `routing_shadow`, `routing_enforce` and `routing_members` read `legacy_provider_profiles`. Re-key them to harness and source/model, or delete them.
5. Delete the old tests that assume providers: `provider_and_retry`, `routing_and_quota`, `cheap_local_first`, `coding_harness_default`, `cos_source_fallback`, `cos_capacity`, `knowledge_fallback`, `browser_fallback`, `role_assignments`, and `routing_shadow`. The shared test helper `dispatcher()` needs a registry and a harness adapter.

## Unresolved

- Harness adapters need the source endpoint and credential at run time for OpenAI-compatible sources (pi/acp base URL). That handover is part of the worker-harness unit's protocol change.

## Production migration

- None yet. Once dispatch uses the registry, production config must declare `[llm_sources.*]`, `[harnesses.*]`, `[harness_selection]` and `[route_defaults.*]` before the daemon reload. Without them, every task fails closed as unroutable.

## Continuation check (session 2)

- Rechecked `cargo test -p task-dispatch harness_select_ -- --nocapture` — exit 0, 4 passed.
- Rechecked `cargo test -p task-dispatch route_select_ -- --nocapture` — exit 0, 5 passed.
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0; all paths are within this unit's recorded scope.
- The runtime switch remains incomplete: `dispatch_run.rs` still calls `select_provider_with_default`, and `review_spawn.rs` still calls `select_provider`. The legacy `provider_select.rs` and `ProviderSpec` remain. Continue by integrating the worker harness/source/model/account inputs at the dispatcher boundary, then remove the legacy selector and dependent tests. No runtime migration was attempted.

## Continuation check (session 3)

- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0; changed paths remain within this unit's recorded scope.
- The pure route selector, daemon registry wiring, and run selection persistence are present from the prior session.
- Normal dispatch still calls the provider selector. The current `RunRequest` has no selected source/model/account fields, and adapters resolve the model from `worker_hint.tier`. Mapping the new route onto that interface would preserve the tier-bound behavior this ADR removes. Dispatcher runtime integration and legacy path deletion remain pending the worker-harness protocol boundary; no runtime migration was attempted in this session.

## Continuation check (session 4)

- `cargo test -p task-dispatch route_select_` — exit 0, 5 passed.
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0; changed paths remain within this unit's recorded scope.
- Runtime integration remains blocked on the parallel worker-harness protocol boundary: `task_worker::RunRequest` still has no selected source/model/account fields, and dispatcher adapters still resolve model from tier. The normal and reviewer dispatch paths still use provider selection, so provider removal and workspace acceptance checks are not yet satisfied. No edits were made to worker-owned protocol files.

## Final leaf (session 5, replan v4): landing and closure

This leaf lands the selector and its records. It does not switch dispatch. The switch and the removals move to the next leaves:

- **dispatch-switch**: switch `dispatch_run.rs` and `review_spawn.rs` from provider selection to `select_route`. Set `RunEntry.route`, call `record_route_selection`, and pass the chosen route in the worker start input.
- **dispatch-prune**: delete `provider_select.rs`, `coding_default.rs`, `ProviderSpec`, the `StaticPolicy` provider rows, `routing_shadow`/`routing_enforce`/`routing_members`, and the provider-bound tests listed in Remaining.
- **worker-harness**: extend `task_worker::RunRequest` with the selected harness, source, model, and account.

Changes in this leaf:

- `crates/task-core/src/llm_route.rs`: removed two `unwrap()` calls from non-test code (`select_harness` best-match and `unique_harness`). Both now fail closed through `if let` and `ok_or`. The behavior is the same. `cargo fmt -p task-core` also reflowed the `id_type!` derive list in this file.

### Commands and results

- `cargo test -p task-dispatch --lib -- harness_select_ route_select_` — exit 0, 9 passed (4 `harness_select_`, 5 `route_select_`).
- `cargo test -p task-core --lib -- harness_select_ route_select_` — exit 0, 5 passed.
- `cargo build --workspace` — exit 0.
- `cargo clippy --workspace -- -D warnings` — exit 0.
- `cargo fmt -p task-core -- --check` — exit 0 after the reflow. The fmt run also touched four store-migrate files (`cluster_job/tests.rs`, `store/migrated_harness.rs`, `store/migration_harness_tests.rs`, `store/tests.rs`). I restored them with `git checkout`, so this unit does not touch them.
- `sh "$CELERIS_WU_SCOPE_PATHS"` — exit 0; all changed paths are within this unit's scope.

### Not run in this leaf

- `bash scripts/dev/test-parallel.sh` was not run. It is the phase-level gate for the integration step (`integrate-select`), not this leaf's check.

### Known gap carried forward

- Normal and reviewer dispatch still use provider selection. A run's selected source and model are not yet used to start the worker, so the new selection is recorded but does not drive execution until dispatch-switch lands.
