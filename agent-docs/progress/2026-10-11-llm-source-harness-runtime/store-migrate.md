---
tasks: [01M4MEEKX485VD5SWKX0P30D4Y]
status: done
title: Store migration and source keyed account limits
updated: 2026-10-11
---
# Store migration and source keyed account limits

## Result

Added task and run routing columns, source-keyed account limits, the daemon startup resolution call, and the `HarnessMigrationUnresolved` event. The `migration_harness_` tests cover the columns plus resolution, null for an unregistered value, one audit event, and idempotence. Task struct literals across the workspace initialize the new field to `None`.

## Checks

- `cargo test -p task-core migration_harness_` — passed (2 tests).
- `cargo test -p task-core account_limits_source_` — passed (1 test).
- `cargo test -p task-core --no-run` — passed.
- `cargo test --workspace --no-run` — passed.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed after switching account limit consumers to `source_id`.
- `sh "$CELERIS_WU_SCOPE_PATHS"` — passed; the authorized `tests/e2e/` task literals initialize the new `harness_id` field.

## Production migration

Migration 0068 adds task harness and run selection columns and remaps account limit sources (`claude-code` to `claude_oauth`, `codex` to `codex_oauth`, other keys preserved). At startup the daemon resolves migrated task harness values against declared harness IDs. Values without a declaration stay null and receive one unresolved event. No production database was modified; the migration will apply when the updated daemon opens it.
