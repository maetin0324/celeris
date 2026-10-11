---
tasks: [01M4N7NVVAJY9CVF8BG9JHD163]
status: completed
---
# Stranded branch merge execution

## core

`stranded::merge` advances the configured default branch directly without rebasing. It fast-forwards when possible, otherwise creates a detached temporary worktree and runs `git merge --no-ff`; the temporary worktree is removed and pruned on success and failure. A clean default-branch checkout is advanced with `git merge --ff-only`, a dirty checkout returns `Busy` (API 409 `checkout_busy`), and another checked-out branch uses compare-and-swap `git update-ref`. The task head is verified as an ancestor before the integration row is marked integrated. Merge failures keep the integration unintegrated, store a reason, and record an integration request. No push is performed.

Tests: `stranded_merge_fast_forward`, `stranded_merge_no_ff`, `stranded_merge_conflict_keeps_default_branch`, `stranded_merge_dirty_checkout_is_busy`, and `stranded_merge_other_branch_checked_out_uses_update_ref`. Each checks temporary worktree cleanup. The successful fast-forward test verifies exactly one `IntegrationMerged` event.

## e2e

The route round-trip test now proves API merge itself makes task head an ancestor of main, reports `integrated: true`, updates detail and list, emits one merge event, and removes the Unintegrated inbox item. A second task fixture changes the same line on task branch and main; API merge returns `integrated: false` with a reason, leaves main SHA unchanged and state unintegrated, records an `IntegrationRequested` event, and removes the temporary worktree.

## Evidence

- `cargo fmt --all -- --check` — exit 0.
- `cargo test -p celeris --test integration_api` — exit 0, 2 tests passed (round-trip and conflict).
- Core evidence from merge-core: `cargo test -p task-ops stranded` — 9 tests passed; `cargo clippy -p task-ops -p task-api -p celeris -p celerisctl --tests -- -D warnings` — passed.

## Unresolved items and proposals

No unresolved implementation items for this WorkUnit. Deployment and push remain separate operations; this API intentionally does not push to origin.
