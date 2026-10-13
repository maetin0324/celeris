---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---

# Web routing UI

- Replaced `/models` with `/routing`, which reads `GET /api/v1/routing/stages` and `GET /api/v1/routing/evidence`. It shows genre/harness stages, stage/evidence versions, score intervals, guardrail window and the latest stage change. It has no enforce action; human approval through celerisctl or API is stated.
- Removed the legacy model assignment/catalog editing screen, route, old routing-state view and their obsolete tests. Updated navigation, source/help links, SPA route declarations, e2e route fixture and fake-daemon data.
- Added a Vitest fixture render test for stage, evidence, last change, and read-only approval guidance.
- Checks passed: `corepack pnpm@12.6.0 -C web typecheck`; `corepack pnpm@12.6.0 -C web lint` (exit 0; four existing `!important` warnings in `web/styles.css`); `corepack pnpm@12.6.0 -C web test` (650 Vitest tests and 93 Node tests); `node --test web/server/spa-routes.test.mjs` (2 passed); `git diff --check`.
- Scope check (`sh "$CELERIS_WU_SCOPE_PATHS"`) listed only web/ and this progress file; `crates/` is unchanged.
- Initial explicit offline install reported a missing `serve-static@2.2.1` tarball. The subsequent pnpm typecheck invocation populated dependencies and completed successfully; no manual edits to generated API artifacts were made.
