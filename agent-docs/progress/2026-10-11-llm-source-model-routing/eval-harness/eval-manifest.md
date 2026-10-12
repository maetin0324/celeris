---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# eval-harness eval-manifest

Added the `routing-eval` workspace crate and `celeris-routing-eval` binary. The candidate registry contains the seven ADR D1 source/account/model rows, including the 30×2 and 12×1 allocations and 15% quota budget. The CLI exposes manifest, recheck, dry-run, run, aggregate, export, and stats; the later-stage commands currently return an explicit not-implemented error.

The manifest command reads a database in SQLite read-only mode, selects completed coding work units with command checks, splits by the configured creation-time cutoff, stratifies by genre and kind, and deterministically selects a shared 30-problem set plus a 12-problem subset. It attaches `celeris.task-features/1` snapshots using the work-unit creation timestamp, hashes each feature/problem and the complete manifest, and refuses a same-version overwrite when the manifest SHA changes.

Verification: `cargo test -p routing-eval` exercises a temporary migrated database fixture for split, subset, strata, features, deterministic rerun, and changed-SHA rejection. No model evaluation was run.
