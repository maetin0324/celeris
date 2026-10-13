# Stranded sweep item identity

tasks: [01M4NM5720BYCYHXT02297GFKK]

`stranded::sweep` now skips `task_integration_upsert` when state, branch, head SHA, and reason match the stored row. This preserves `checked_at` and `updated_at`; state-change event behavior remains in the store upsert path. A `stranded_identity` test runs two sweeps five minutes apart, verifies the inbox Unintegrated task id and `at` stay identical, then changes the head and verifies `at` changes. The notification feed uses the same task-ops inbox view as its source, so it receives the preserved timestamp and identity from that same row.

Checks: `cargo test -p task-ops stranded_identity -- --nocapture` (1 passed); `cargo test -p task-ops stranded_ -- --nocapture` (16 passed); `git diff --check`; `sh "$CELERIS_WU_SCOPE_PATHS"` (only `crates/task-ops/src/inbox/tests.rs` and `crates/task-ops/src/stranded.rs`).
