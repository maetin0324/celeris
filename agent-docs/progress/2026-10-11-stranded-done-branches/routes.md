# Stranded integration API routes

## Implemented

- Added `GET /api/v1/integrations?state=`, `GET /api/v1/tasks/{id}/integrations`, and task integration `merge`, `discard`, `hold`, and `reopen` actions.
- Human actions require the existing admin bearer authentication and record actor `human`. Discard and hold require a nonblank `reason`; invalid state transitions return a 409 problem.
- Merge uses the task worktree marker, configured project repository default branch, and `task_ops::stranded::merge`; an already integrated branch is recorded as integrated, otherwise the existing integration request is recorded.
- The Unintegrated inbox answer delegates merge/discard/hold choices to the corresponding route.
- Added schemas and API reference rows. Added focused tests for listing/filtering, invalid filters, authorization, and required reasons.

## Verification

- `cargo check -p task-api` passed.
- `cargo test -p task-api --test integration_api` passed (3 tests).
- `UPDATE_SCHEMA=1 cargo test -p task-api schema` passed and regenerated `docs/api/v1/api-v1.schema.json`.
- `sh "$CELERIS_WU_SCOPE_PATHS"` and `git diff --check` passed.
- GUI/web generated types were not regenerated: the pinned pnpm 11.27.0 was not installed for the worktree, and Corepack's install failed opening its package store database. The configured pnpm 12.6.0 was rejected because this repository requires 11.27.0.
