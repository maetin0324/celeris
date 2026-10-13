---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# eval-harness evidence schema

Implemented the typed `EvidenceKey`, `Interval`, and `EvidenceRow` in `task_core::routing_evidence`, including the harness version key rule: Claude Code and Codex use the parsed semver major; other harnesses retain the complete version output.

Added migration 0071 (`routing_evidence`) with the ADR D2 columns and a composite primary key including `evidence_version`. Migration 0070 is reserved because another branch uses it. `SqliteStore::insert_routing_evidence` atomically replaces all rows for one evidence version while preserving other versions; `list_routing_evidence` lists all rows or filters by version.

Verification: `cargo test -p task-core routing_evidence` passed (3 tests). The tests cover major extraction for semver harnesses, full-version preservation for other harnesses, same-version replacement, and separate-version retention. No LLM evaluation was run.
