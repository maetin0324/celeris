---
title: flake-build-cache: sccache dispatcher tests wait for terminal task state
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-03
---
# flake-build-cache: sccache dispatcher tests wait for terminal task state

## Cause

The latest main sync is `c48856559e12` (HEAD). The existing `run_with_sccache_and_cache` in
`crates/task-dispatch/src/dispatcher/tests/mod.rs` called `run_until_idle(&mut d, 60)` and then immediately
asserted `Status::Done`. Each tick yields for only 20 ms, so the test could exhaust its fixed tick budget while the
task was still `Reviewing`; the parent-env test and its child-process rerun then both failed. The same pattern existed
in build-cache tests in `build_cache.rs`.

## Change

Added `run_until_task_terminal`: it keeps ticking until the stored task reaches a terminal state, requires `Done`, and
uses 30 seconds only as a failure guard. If the guard expires, the panic includes the final task state, task run list,
and last tick report. Replaced the fixed 60-tick completion check in the sccache helper and same-family build-cache
tests. Environment capture and assertions remain unchanged. No CPU-load reproduction was used.

## Evidence

- `git show HEAD:crates/task-dispatch/src/dispatcher/tests/mod.rs | sed -n '3020,3035p'` — sync HEAD still had the
  60-tick helper followed by `Done` assertion.
- `rg -n "run_until_idle|assert_eq!\(.*Status::Done" crates/task-dispatch/src/dispatcher/tests/{mod.rs,build_cache.rs}`
  — found the same pattern in build-cache tests.
- `cargo test -p task-dispatch --lib dispatcher::tests::build_cache` — exit 0; 15 passed, 0 failed.
- `cargo fmt --all -- --check` — exit 0.
- `git diff --check` — exit 0.
