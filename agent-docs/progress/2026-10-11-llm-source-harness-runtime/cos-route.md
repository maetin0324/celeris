---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: CoS llm_route candidate selection
updated: 2026-10-11
---
# CoS llm_route candidate selection

CoS launch selects with `select_route(cos=true)` and explicit `claude_code`, uses the route-selected adapter/model/account, records the source/model/account and selection trace in the CoS run route JSON, and reserves `(source, account)` while a run is active. CoS retries now begin at the next candidate after a supply failure, preserving declared priority and avoiding retrying a failed candidate. A new-session UUID is also minted for the declared `claude_code` harness ID, while the worker session adapter remains the CLI slug `claude-code`.

The daemon no longer resolves a provider snapshot for CoS. CoS config and launch types no longer carry provider/source/account/model/tier pins or legacy route fallbacks. Four CoS launch tests tied to provider policy/account pinning were removed; route-level admission and candidate selection tests cover the replacement contract. Provider admin reload CoS tests that loaded `[[providers]]` were removed as obsolete. The `cos_route_candidates_keep_priority_and_fail_closed_when_none_is_eligible` test covers candidate order, fallback past an ineligible candidate, and fail-closed behavior.

## Verification

- `cargo test -p task-dispatch route_select --lib` — passed (19 tests).
- `cargo test -p task-dispatch cos_chat_harness_e2e_claude_initial_resume_events_and_native_image --lib` — passed after using the declared harness ID for session generation and the `claude-code` worker adapter slug.
- `cargo test -p task-dispatch cos_chat_ --lib` — passed (97 tests).
- `cargo test -p celeris cos_chat_config --lib` — passed (3 tests) after updating the reload fixture.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed after removing the unused provider reload test module and updating the worker Config literal for the removed CoS provider field.
- `sh "$CELERIS_WU_SCOPE_PATHS"` and `git diff --check` — passed.

## Remaining

- None for this WorkUnit. `route_state` uses the shared source/account admission state, and the CoS launch regression verifies persisted harness/source/model/account fields.

## Integration note

The clippy-reported legacy helpers `pool_adapter_of`, `implicit_account_full`, `reserved_for_cos`, and `account_usable` remain only under `cfg(test)` for old provider selector unit helpers. The production CoS path no longer calls them; dispatch-prune can remove those remaining provider-selection test helpers without colliding with CoS routing work.
