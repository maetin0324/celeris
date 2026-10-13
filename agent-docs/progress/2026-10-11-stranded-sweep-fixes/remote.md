---
title: remote — 遠隔 repo を not_applicable にする
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# remote

## Decision implemented

Remote repositories are outside the local stranded sweep. The sweep records their completed root task/repository pair as `not_applicable`, with reason `remote repository: not checked by the sweep`. This avoids remote SSH work and removes both new and previously unintegrated remote rows from the actionable inbox on the next sweep.

The `task_integrations` check constraint is rebuilt by migration 0070 while copying every existing row. Versions 0068 and 0069 are reserved because other branches use them; migration 0070 is the next free version found across the available branches.

## Changes

- Added `TaskRepoIntegrationStatus::NotApplicable` and persisted value `not_applicable`.
- The daemon marks `WorkspaceSpec::Remote` records explicitly; Local path resolution failures retain `repository cannot be resolved`.
- Sweep transitions prior remote `unintegrated` rows to `not_applicable`.
- Inbox continues to list only `unintegrated` rows; CLI filtering accepts `not_applicable`; local merge/discard/hold/reopen operations reject that state.
- Added `stranded_remote` tests for migration of a prior row and inbox exclusion.

## Verification

- `cargo test -p task-ops stranded_remote`: passed (2 tests).
- `cargo test -p task-core`: passed (901 unit tests, 1 integration test, 3 doc tests).
- API schema was regenerated with `UPDATE_SCHEMA=1 cargo test -p task-api committed_schema_matches_generated`; it passed. `cargo check -p celeris -p celerisctl -p task-api` passed; targeted task-api, celerisctl, and celeris tests passed.
