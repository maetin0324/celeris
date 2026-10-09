# Launcher credential release: formatting repair

tasks: [01M4GG51P9WY3F4EVBCR160PGP]

## Cause

The release preparation for HEAD `7ce2d65790a0bd51bc7910ccd01773fc8c16e60e` stopped at `cargo-fmt-check`. The recorded `prepare.log` identifies two formatting differences in `crates/task-worker/src/browser_ledger_tests.rs` and `crates/task-worker/src/browser_tests.rs`.

## Repair

Ran `cargo fmt --all` in the task worktree. The formatter changed only Rust formatting in the eight `crates/` files listed in the diff; no logic was changed.

## Verification

Results are recorded below after checks complete.

- `cargo fmt --all -- --check`: exit 0.
- `cargo clippy --workspace -- -D warnings`: exit 0.
- `cargo check --workspace --tests`: exit 0.
