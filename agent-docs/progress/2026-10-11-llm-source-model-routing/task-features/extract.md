# extract

- Added public `task_core::route_extract::{features_for_subject, outcome_labels_for_run}` APIs. They read task/WU records, ancestry, plans, run aggregates, and routing feature metadata, then use the pinned `build_task_features` formatter. Prompt, comment text, logs, tool output, and credentials are not copied into the brief.
- `AsOf::Timestamp` bounds run start times and event timestamps; `AsOf::EventSeq` bounds events. Outcome labels include run status, review/check summary, wall time, cost/token values when present, and explicit `missing` names for unavailable quota, git-derived changes, retries-after, and repair status.
- Tests: fixture task and WU extraction; repeated extraction byte equality; timestamp cutoff excluding a later run; sensitive comment text excluded; outcome missing-data behavior.
- Verification: `cargo test -p task-core route_extract_ --no-fail-fast` (2 tests).
