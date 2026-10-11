---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: running
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
