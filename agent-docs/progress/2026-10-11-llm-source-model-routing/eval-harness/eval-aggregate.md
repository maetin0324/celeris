---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
---
# Eval aggregate and feature export

`celeris-routing-eval aggregate --db <path> --manifest <file> --out <dir> [--write-evidence]` reads only stage `eval` route decisions and joins their outcomes and runs. It emits `evidence.json`, `evidence.md`, and trial detail JSON with Wilson 95% intervals, pass counts, median wall time/cost/quota, timeout rate, low-confidence flags, Pareto candidates, and paired McNemar/bootstrap comparisons. The optional write flag stores the computed evidence through `SqliteStore::insert_routing_evidence`; the version combines manifest SHA and aggregation date, so a same-day rerun replaces that version.

`celeris-routing-eval export --db <path> --manifest <file> --out <file.jsonl>` emits one `celeris.routing-eval-result/1` object per trial, joined with the frozen problem features and candidate identity. Check commands and prompt/response content are omitted.

Verification: aggregate and export tests cover interval/metric output, k=1 low confidence, candidate Pareto inclusion, feature-bearing export schema, and SQL collection from a temporary evaluation database. Run with `cargo test -p routing-eval aggregate` and `cargo test -p routing-eval export`.
