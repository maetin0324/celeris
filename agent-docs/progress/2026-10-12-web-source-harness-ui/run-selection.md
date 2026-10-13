---
tasks: [01M4R2CDTXZ4N7XZ8SN2M8NHEH]
---

# Run selection display in web

## Work completed

- Run detail header (`/tasks/$id/runs/$runId`) shows the selection from the API `selection` field on one row: harness, source (link to `/sources/$sourceId`), model, and 口座 (account), next to status, start time, and duration.
- A legacy run (`selection.status = legacy_unresolved`) shows one item, `記録なし（旧 run）`, with no source link. The adapter and model are not used as a fallback.
- The task detail run list shows the same values as one line (`harness / source / model / 口座 account`), or `記録なし（旧 run）` for legacy runs.
- `features/runs/run-header.ts`: `runSelectionParts`, `runSelectionText`, `LEGACY_SELECTION_TEXT`. `runHarness` (adapter + model) is removed.
- Fake daemon: task T1 has R0 (resolved: claude-code / claude-oauth / claude-opus / main) and R1 (legacy). T2's R2 stays legacy.
- Tests: `run-header.test.ts` (selection parts and text, legacy), `run-log-view.test.tsx` (selected header with source link, legacy header, Link mock handles `params`), `overview-view.test.tsx` (fixture gets `selection`).
- New e2e `e2e/work/run-selection.spec.ts`: resolved run header, click through to `/sources/claude-oauth`, legacy run header, and the task run list.

## Verification

- `corepack pnpm@12.6.0 exec vitest run` — exit 0; 91 files, 654 tests passed.
- `corepack pnpm@12.6.0 typecheck` (`tsc -b`) — exit 0.
- `corepack pnpm@12.6.0 lint` (`biome check .`) — exit 0; 467 files checked. 4 warnings remain in `styles.css` (`!important`), which were already there.
- `corepack pnpm@12.6.0 build` — exit 0.
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 exec playwright test e2e/work/run-selection.spec.ts e2e/parity/runs-files.spec.ts` — 5 passed.
- Full `corepack pnpm@12.6.0 e2e` with `TMPDIR=/local/x/ar` (short, because the Unix socket path limit is 108 bytes): 308 passed, 4 failed. The 4 are `lease.spec.ts` (2), `links.spec.ts` (1), and `narrow-a11y.spec.ts:65` (1). The same 4 fail on the base commit `db516146` in a scratch copy, so they are not caused by this change. They are browser live-view and lease flows that depend on the launcher and cannot run in this sandbox.
- Without `TMPDIR` set, `listen EINVAL` failed 29 browser specs. This is the long socket path, not a code fault.
- `check:boundaries` — exit 0. `check:secrets` — exit 0.
- `check:parity` — exit 1 with `R30: missing parity test title`. This is pre-existing and not caused by this change. The R30 row in `agent-docs/web/feature-parity.md` names the `parity: /providers 追加・変更・削除・確認` test, which the web-purge unit removed. Fixing it belongs to the closeout unit's doc update.
- `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/ gui/` — exit 0; no diff in crates/ or gui/.

## Unresolved

- `check:parity` R30 (above) needs to be settled in closeout, by changing the feature-parity row or the test title.
- Browser live-view and lease e2e (4 tests) fail in this sandbox. They need a run on a host where the launcher works.
- The `adapter` and `model` fields are still in `RunSummary`, and the console and timeline event labels still read `worker_started` adapter and model. The ADR's adapter removal is outside this unit's scope.

## Proposals

- Make `selection` the only source for harness display in the timeline and console labels (`worker_started`), in a later unit.
