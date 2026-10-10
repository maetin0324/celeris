# Browser Live View disabled reason: SPA implementation

tasks: [01M4HRQBN8XHKQRD1DAXHXBSEX]

## Completed

- `liveViewState` now renders a frame only for `RUNNING` runs. `WAITING_FOR_AUTH` uses the auth interval reason; other non-running states use `not_running`.
- Disabled live responses render the gateway reason, the required “映像なし — イベントで監視中” message, and a link to `#browser-live-events`.
- Control polling now stops for `COMPLETED`/`FAILED` runs and gateway `404` responses. All current query callers pass the run state where available.
- Added browser feature tests for disabled and stopped-run iframe behavior, event monitoring copy/link, terminal polling, and 404 polling.

## Verification

- `pnpm typecheck` — passed.
- `pnpm exec vitest run features/browser` — passed (10 files, 96 tests).
- `pnpm exec biome check` on the 8 changed web source/test files — passed.
- `pnpm lint` — failed on four existing `!important` diagnostics in `web/styles.css`; it also found formatting in changed files, which was corrected. Focused lint passes after formatting.
- `git diff --check` — passed.

Gateway implementation remains with the parallel `gateway-live` WorkUnit.
