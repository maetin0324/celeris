---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
---

# Selector export rewire

- Removed `task_ops::routing_replay` and its estimator/evaluation implementation; `task_ops::routing_export` is now public.
- Restored `celerisctl routing export` to emit `celeris.route-training/1` JSONL to `--out <file.jsonl>`, with `--text none` support. Removed `routing evaluate`; retained the current source/harness `routing show` table.
- The routing operations guide, Pi Hashline guide, and architecture map describe the new JSONL export command and do not document the retired dataset/evaluate flow.
- No production config change is required for this export command. To export, provide an explicit read-only database path and output file; use `--text none` to omit task briefs.

## Checks

- `cargo test -p celerisctl --test routing_cli`: passed (1 test).
- `cargo clippy -p task-ops -p celerisctl -- -D warnings`: passed.
- `sh scripts/dev/check-doc-links.sh docs/ops/model-routing-migration.md docs/ops/coding-harness-pi-hashline.md docs/architecture-map.md agent-docs/progress/2026-10-11-llm-source-model-routing/selector/export-rewire.md`: passed.

## Unresolved

- None known.
