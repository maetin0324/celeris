# task-features export-cli

- Replaced `celerisctl routing export` with explicit `--db <path> --out <file.jsonl> [--since] [--until] [--stage shadow|enforce|eval] [--text brief|none]`; it calls `task_ops::routing_export` and does not open a default daemon DB.
- Removed `routing evaluate` and the old dataset/report/estimator replay implementation and its dedicated CLI and integration tests. `routing show` remains configuration-only. The public `task_core::route_extract::{features_for_subject,outcome_labels_for_run}` API remains available to build evaluation manifests from the original subject records.
- Updated the architecture map and ops instructions to describe JSONL training export.
- Evidence: `cargo test -p celerisctl --test routing_cli` passed (1 test); `git diff --check` passed; stale-name search for `evaluate_with_estimator`, `DatasetRowV1`, and `routing.dataset.v1` in `crates/celerisctl` and `crates/task-ops/src` returned no matches.
