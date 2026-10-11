---
title: web-state — web の統合状態に not_applicable を出す
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# web-state

## Decision implemented

The web shows a repository whose integration state is `not_applicable` (remote repository, ADR 2026-10-11-stranded-done-branches F2) as a read-only state: label `対象外（遠隔 repo）`, the stored reason, and no operation buttons. The API answers 409 for merge/discard/hold/reopen from this state, so the web does not offer them.

## Changes

- `web/features/tasks/task-integration.tsx`: `STATE_LABELS` gets `not_applicable`. The method list already yields no buttons for any state other than `unintegrated`, `held`, and `discarded`; a comment records that `not_applicable` is intentionally left out.
- `web/features/tasks/fixtures/task-integration-app.tsx`: the fixture page renders a `not_applicable` section.
- `web/features/tasks/task-integration.test.tsx`: a test renders `not_applicable` with a reason and asserts the label, the reason, `data-state`, and that no `<button>` is rendered.
- `web/api/generated/types.ts` and `schema.json`: regenerated with `corepack pnpm@12.6.0 -C web gen:types` (the committed copy predated the remote WU, so `TaskRepoIntegrationStatus` lacked `not_applicable`).
- No change in `web/features/inbox/inbox-model.ts`: the inbox lists only `unintegrated` rows, so no inbox code branches on this state.
- `task-list-view.tsx` uses `integrationLabel`, so it shows the new label without a change.

## Verification

- `corepack pnpm@12.6.0 -C web install --offline`: done.
- `corepack pnpm@12.6.0 -C web typecheck` (`tsc -b`): exit 0.
- `corepack pnpm@12.6.0 -C web lint` (`biome check .`): exit 0, "Checked 481 files". It reports 4 warnings in files this change does not touch.
- `corepack pnpm@12.6.0 -C web exec vitest run features/tasks/task-integration.test.tsx`: 11 passed, 0 failed.
- `corepack pnpm@12.6.0 -C web exec vitest run`: 94 files passed, 688 tests passed, 0 failed.

## 未解決事項

- The overlay test at mobile widths runs only for the `unintegrated` section; it is not extended to `not_applicable`.
