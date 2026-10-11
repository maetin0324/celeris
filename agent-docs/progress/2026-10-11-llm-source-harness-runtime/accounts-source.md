---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: blocked
title: Account ledgers keyed by LLM source and account
updated: 2026-10-11
---

# Account ledgers keyed by LLM source and account

- Started from base `d88e2d24207c`; initial `sh "$CELERIS_WU_SCOPE_PATHS"` reported no changed paths.
- Changed the `Dispatcher.account_limits` in-memory index to use `task_core::llm_route::{SourceId, AccountId}` and converted loaded and updated database keys into those types.
- `cargo check -p task-dispatch --lib`: exit 0.
- `cargo clippy --workspace -- -D warnings`: exit 0.
- `git diff --check`: exit 0.
- Remaining: AccountBook observations/cooldowns, quota activity and selection reservations still use adapter keys; account limit admission is not yet evaluated with both the account and source keys; `account_limits_source_` admission tests are not added. Acceptance criteria 0 and 1 are therefore incomplete.
- Production migration: none performed by this work unit.
