---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# Cell bootstrap interval evidence

`aggregate` now computes a problem-level bootstrap 95% interval for each candidate cell with fixed seed 42 and 2,000 resamples, alongside the existing Wilson interval. Both bounds are stored on `EvidenceRow`, persisted by migration 0073, and included in aggregate JSON, the Markdown evidence table, and each feature-joined JSONL trial row.

The bootstrap samples one outcome per problem (the first numbered trial, matching existing pass aggregation), so repeated runs do not overweight a problem. `routing_evidence` insert/list round-trip covers the added bounds. The celerisctl no-migrate check now treats schema 72 as the immediately preceding version.

Dry-run usage remains `celeris-routing-eval dry-run --manifest <file> --db <path>`; this change concerns post-run `aggregate --db <path> --manifest <file> --out <dir> [--write-evidence]` and `export --db <path> --manifest <file> --out <file.jsonl>`. No live evaluation was run.

Verification: `cargo test -p task-core` passed (937 unit tests plus sidecar and doctests); `cargo test -p routing-eval` passed (23 unit tests and the stub pipeline); `cargo test -p celerisctl --test no_migrate` passed (4 tests); and `cargo fmt --check` plus the WorkUnit scope check passed. During the first task-core run, five existing schema-version assertions exposed the expected migration-version bump; they were updated and the rerun passed.
