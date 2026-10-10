---
tasks: [01M4JF3C0K4E97X2W9SDMH9KGR]
title: "sizing-impl: scratch shared extent measurement"
---

# sizing-impl: scratch shared extent measurement

## Current status

Implementation is connected to the production measurement path. `ScratchState` retains the pool extent index; `measure_one` replaces the measured owner's contribution and writes its deduplicated allocated size into the existing cache/lease consumed by GC planning and watermarks. FIEMAP errors and extent/time limits fall back to `st_blocks` for the owner and remove its previous FIEMAP contribution.

## Changes

- `default_scratch_targets_max_gb` is 160 GiB and `default_scratch_total_max_gb` is 200 GiB; `ScratchSettings::with_dir` matches those values.
- `config/celeris.example.toml` documents both values.
- `scratch::gc::account_shared_extents` accepts injected FIEMAP extent results, deduplicates physical ranges in a caller-owned pool index, and returns conservative allocated-block accounting on FIEMAP failure.
- `scratch::reflink::fiemap_extents` exposes FIEMAP physical ranges for measurement callers.
- `scratch::gc::measure_tree_shared` walks an owner tree, deduplicates hardlinks by `(dev, ino)`, refreshes its ranges in `SharedExtentIndex`, and applies the ADR extent and time budgets.
- Dispatcher measurement threads share that index across owners, so the regular `SizeCache` values used by `plan_gc` and high/low watermark checks are pool-deduplicated.
- Added `scratch_shared_` tests for range deduplication/fallback and settings defaults.

## Verification

- `cargo test -p task-worker scratch_shared_ --lib`: passed (3 tests, including injected unsupported-FIEMAP fallback).
- `cargo test -p task-dispatch scratch_gc --lib`: passed (8 tests).
- `cargo test -p celeris scratch_defaults_follow_the_build_cache_parent --lib`: passed (1 test; verifies config defaults 160/200 GiB).
- `cargo clippy --workspace -- -D warnings`: passed.
- `cargo fmt --all`: passed.
- `git diff --check`: passed.
- `sh "$CELERIS_WU_SCOPE_PATHS"`: passed; only the intended config, scratch measurement, dispatcher wiring, and progress files are listed.

## Remaining

- No remaining implementation or verification items for this WorkUnit.
