---
tasks: [01M4R2CDTXZ4N7XZ8SN2M8NHEH]
---

# Web source and harness screens

## Work completed

- Added read-only `/sources`, `/sources/$sourceId`, and `/harnesses` routes using the generated LLM source and harness API types.
- Source list shows kind, reachability, deployment count, and account count. Details link accounts to their row in `/accounts` and model IDs to source-filtered `/models`.
- Harness list shows declared formats, source kinds, capabilities, defaults, and eligibility rules. The help page explains that configuration changes are made in config.
- Added admin navigation, SPA route declarations, generated route tree, screen inventory, fake daemon responses, Vitest coverage, and an end-to-end screen test.
- Added source links in the model and account screens. No screen copy contains the retired provider term.

## Verification

- `corepack pnpm@12.6.0 run lint` — passed (existing CSS `!important` warnings remain).
- `corepack pnpm@12.6.0 test` — passed (91 Vitest files, 652 tests; 93 Node tests).
- `corepack pnpm@12.6.0 run typecheck` — passed.
- `corepack pnpm@12.6.0 run build` — passed.
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 exec playwright test e2e/admin/sources-harnesses.spec.ts` — passed.
- `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/ gui/` — passed; those directories have no task diff.
