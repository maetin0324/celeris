---
tasks: [01M4R2CDTXZ4N7XZ8SN2M8NHEH]
---

# GUI source and harness fixture migration

## Work completed

- Migrated every `gui/test/celeris/*.toml.tmpl` from `[[providers]]` to explicit `[llm_sources.fake]`, `[harnesses.fake]`, `[[llm_routes]]`, and `[route_defaults.fake]` declarations. The multi-account fixture keeps accounts `a` and `b` under its single fake source.
- Removed `providers_include` and provider-specific wording from the accounts fixture and `gui/scripts/celeris.sh`.
- Updated the `celeris.sh` binary paths to use `${CARGO_TARGET_DIR}/debug` when `CARGO_TARGET_DIR` is set.
- Added `gui/scripts/check-fixture-configs.sh`, which builds the daemon and checks each rendered fixture by starting it and querying authenticated or unauthenticated `/api/v1/health` as appropriate.
- Removed the unsupported fallback e2e case from `gui/e2e/g4.spec.ts`.
- Updated the configuration section and daemon/config examples in `gui/docs/celeris-api-v1.md` to describe source and harness fields.

## Multi-account fallback limitation

The old GUI fixture represented `acct-a` throttling and `acct-b` succeeding by setting per-provider `ACCOUNT` environment values. The replacement fake harness does not consume source account credentials or expose per-account failure injection, so the same-source account fallback scenario cannot be represented meaningfully by this fixture. The fake worker now succeeds deterministically and documents this limitation; no provider compatibility path was retained.

## Verification

`bash gui/scripts/check-fixture-configs.sh` passed for all nine fixture templates. GUI typecheck and lint passed. GUI Vitest passed (92 files, 1307 tests) with `TMPDIR=/dev/shm/ar`; GUI production build passed.
