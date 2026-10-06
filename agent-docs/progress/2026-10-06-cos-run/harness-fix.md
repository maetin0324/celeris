# CoS harness compile fix

tasks: [01M4873XNNA33FQB9KB0JERVCP]

`CosChatLaunchConfig` gained the required `triage: CosTriageSettings` field. The harness adapter fixture in `crates/task-dispatch/src/dispatcher/cos_chat/harness_tests.rs` was the only literal missing it; the other literals already supply `triage: Default::default()`.

Added `triage: Default::default()` to that fixture. This keeps the harness adapter tests' behavior unchanged and restores all-target compilation.

Checks:

- `cargo check --workspace --all-targets` — exit 0.
- `cargo test -p task-dispatch cos_chat_harness` — exit 0; 3 passed.
- `cargo test -p task-dispatch cos_chat_triage` — exit 0; 18 unit tests and 11 integration tests passed.
- `git diff --check` — exit 0.
