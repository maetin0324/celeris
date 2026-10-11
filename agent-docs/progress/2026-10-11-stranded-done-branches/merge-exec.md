# Stranded branch merge execution

## core

`stranded::merge` now advances the configured default branch directly without rebasing. It fast-forwards when possible, otherwise creates a detached temporary worktree and runs `git merge --no-ff`; the temporary worktree is removed and pruned on success and failure. A clean default-branch checkout is advanced with `git merge --ff-only`, a dirty checkout returns `Busy` (API 409 `checkout_busy`), and another checked-out branch uses compare-and-swap `git update-ref`. The task head is verified as an ancestor before the integration row is marked integrated. Merge failures keep the integration unintegrated, store a reason, and record an integration request.

Tests: `stranded_merge_fast_forward`, `stranded_merge_no_ff`, `stranded_merge_conflict_keeps_default_branch`, `stranded_merge_dirty_checkout_is_busy`, and `stranded_merge_other_branch_checked_out_uses_update_ref`. Each checks temporary worktree cleanup.

Checks: `cargo fmt --all -- --check`, `cargo test -p task-ops stranded` (9 tests, including the five fixed-name merge cases and idempotence), `UPDATE_SCHEMA=1 cargo test -p task-api schema` (3 tests), `node web/scripts/gen-types.mjs --check`, `cargo test -p celeris --test stranded_integration`, and `cargo clippy -p task-ops -p task-api -p celeris -p celerisctl --tests -- -D warnings` passed. `node web/scripts/gen-types.mjs` regenerated web types. GUI type output was updated to match the schema; `corepack pnpm@11.27.0 -C gui gen:types` could not run because pnpm failed to open its SQLite store database. The successful fast-forward test also verifies exactly one `IntegrationMerged` event.

The pre-existing `crates/celeris/tests/stranded_integration.rs` call site still expects the former boolean API. It is outside this core unit's permitted change paths; the prior out-of-scope edit was reverted. The planned `crates/celeris/tests/integration_api.rs` follow-up owns integration-test migration.
