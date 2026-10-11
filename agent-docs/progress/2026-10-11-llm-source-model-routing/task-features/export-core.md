# task-features export-core

- `task_ops::routing_export::export(db_path, &ExportOptions { since, until, stage, text }, out)` opens the explicit SQLite path with read-only URI flags and emits one `celeris.route-training/1` JSON object per line. It returns the written row count.
- Rows are selected by inclusive `created_at` bounds and stage, then ordered by `decision_id`. JSON object keys are emitted deterministically; `ExportText::None` omits `features.brief` while retaining the feature hash and auxiliary record.
- Each row carries subject, stored feature and options records, decision input/output/judge, outcome or `null`, and `evidence_version` from `input.context.evidence_version` (or `null` when absent). No prompt or credential fields are constructed by the exporter.
- If a legacy or damaged DB has a decision whose feature record is absent, export still emits the row with `features: { "hash": ... }`. The current migration's foreign key normally prevents this state; the exporter handles it fail-soft as requested for partially recorded decisions.
- Evidence: `cargo test -p task-ops routing_export_ --lib` covers repeat byte equality, decision id ordering, outcomes, time/stage filters, brief omission, and absent feature records.
