---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# eval-harness e2e-stub progress

Implemented `crates/routing-eval/tests/pipeline_stub.rs`, an integration test that creates a migrated temporary SQLite fixture with 30 completed coding work units, freezes a stratified manifest with features, computes the seven-candidate dry-run estimate, submits the standard candidate's 60 trials to a loopback stub API, writes aggregate evidence through `aggregate --write-evidence`, reads the evidence back, and exports a feature-joined JSONL result. The API stub does not invoke a model or access the network beyond loopback.

The aggregate and export argument handlers are exposed as library functions so the integration test exercises the same command paths as the CLI. Run submission likewise uses the normal request builder and response handling.

Dry-run is invoked via `dryrun::estimate` because it is a pure calculation entry point; its report confirms 324 calls total across seven candidates. Trial outcome rows are inserted into the temporary DB as the scoring daemon's stubbed result.

Evidence: `cargo test -p routing-eval --test pipeline_stub` passed (1 test). The test asserts 30 manifest problems, 12 subset problems, 324 planned calls, 60 submitted calls for `claude-opus-5-5`, one persisted evidence row with a Wilson interval, and exported `celeris.task-features/1` features.
