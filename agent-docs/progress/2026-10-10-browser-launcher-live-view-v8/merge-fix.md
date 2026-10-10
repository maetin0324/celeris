---
title: "merge-fix: artifact v8 と Live View v9 の merge 崩れ修正"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
---

# Live View v9 merge fix

## Cause and changes

- `browser_launcher_run.rs::artifact_refusal_progress` was missing its closing brace, so `live_registration` was parsed inside the function.
- `browser_launcher/protocol.rs` had the `LiveStart` and `LiveStop` request variants stranded after `artifact_name_verb`, outside the `Request` enum. Moved them into `Request`; removed the orphaned delimiter.
- Kept artifact transfer at `ARTIFACT_PROTOCOL = 8` and the integrated launcher / Live View protocol at `PROTOCOL_VERSION = LIVE_FRAME_PROTOCOL = 9`.
- Updated Live View protocol and compatibility test names/comments: a protocol 8 launcher has artifacts but no frames; protocol 9 enables Live View. The no-frame checks refuse before opening a frame connection and preserve ordinary session actions.

## Verification

- `cargo clippy -p task-worker --all-targets -- -D warnings` — passed.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed.
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` — passed: 5,070 passed, 0 failed, 14 ignored, including doctests.

The first full `test-parallel.sh` run found four stale protocol assumptions in frame tests: two used launcher protocol 8 to open a frame stream, one expected the integrated protocol to equal the artifact-only protocol, and one daemon relay fixture used version 8. These tests/fixtures now use protocol 9 for Live View and retain protocol 8 only as `ARTIFACT_PROTOCOL`. After fixes, `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass. The rerun `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` passed: 5,070 passed, 0 failed, 14 ignored, including doctests. `git diff --check` and `sh "$CELERIS_WU_SCOPE_PATHS"` also passed; all changed paths are in scope.
