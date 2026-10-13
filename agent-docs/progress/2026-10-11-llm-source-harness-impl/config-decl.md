# config-decl

- Added ADR-shaped `[llm_sources.<id>]` and `[harnesses.<id>]` declarations, `[harness_selection]`, `[[llm_routes]]`, `[route_defaults.<harness>]`, and CoS route candidates.
- Added conversion into `task_core::llm_route::Registry`; validates undeclared harness/source/model references, duplicate route candidates, selection defaults, and source-kind/API incompatibility.
- Added config tests under `llm_sources_` / `harness_decl_`; provider config remains unchanged for the later removal unit.
- `cargo test -p celeris --lib config::llm_route::tests`: 3 passed.
- `cargo test -p celeris --lib config::tests`: 124 passed.
- `sh "$CELERIS_WU_SCOPE_PATHS"` and `git diff --check`: passed.
