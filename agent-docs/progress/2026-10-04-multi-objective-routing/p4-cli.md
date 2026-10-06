---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: cli
status: done
completed: 2026-10-05
---

# Phase 4 cli: `celerisctl routing export` / `routing evaluate`

ADR 2026-10-04-multi-objective-model-routing §7.2・§10 Phase 4 の CLI。

- `celerisctl routing export --db <path> --out <dir> [--since <rfc3339>] [--until <rfc3339>] [--seed <n>] [--policy-hash/--catalog-hash/--estimator-hash <s>]`:
  全体の `--db` を明示したときだけ動く（`CELERIS_DB`・`CELERIS_RUN_DB`・`./celeris.sqlite3` の既定は解決せず拒否）。store を開かない（migration しない）で `task_ops::routing_replay::export` を呼び、`dataset.jsonl` と `manifest.json` を書く。無い DB は作らない。hash は export が設定を読まないため呼び手が渡す（既定 `unspecified`）。
- `celerisctl routing evaluate --dataset <dir> [--policy legacy|heuristic|shadow ...] [--baseline <file>] --out <report.json> [--markdown <report.md>]`:
  DB を開かない。report v1（`celeris.routing.report.v1`）を書く。`--policy` は report の `policies` を絞る（`shadow` → `shadow_recorded`）。`--baseline` は RouterBench 型の `BenchmarkBaselineV1` JSON を `external_benchmark_baseline` に別欄で入れる。Markdown は JSON と同じ値を並べるだけ。
- `task_ops::routing_replay::export` の読み取り専用 open を直した: WAL mode の DB に `-wal` が無いと `SQLITE_OPEN_READ_ONLY` でも空の `-wal`/`-shm` を作っていた。`-wal` が無いときは `file:…?mode=ro&immutable=1` で開き（WAL が無いので読み落としは無い）、ある（daemon 稼働中）ときは従来の read-only で開く。

## 証拠

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run --no-fail-fast -p celerisctl -E 'test(routing_cli_export_evaluate_is_read_only_and_reproducible)'` | 1 passed（2 回の export/evaluate で report.json と report.md がバイト一致、DB と -wal/-shm/-journal の中身・mtime 不変、--db なしと無い DB は失敗、baseline が別欄） |
| `cargo nextest run --no-fail-fast -p celerisctl` | 131 passed |
| `cargo test -p task-ops routing_` | 9 passed |
| `celerisctl routing --help` | `show` / `export` / `evaluate` が出る（試験 `routing_help_lists_export_and_evaluate`） |
| `bash scripts/dev/test-parallel.sh` | exit 0、4016 passed, 12 skipped |
| `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |

## 未解決事項

- paired metrics（APGR/AIQ/IBC）の入力（`PairedMetricsInputV1`）を CLI から渡す口は無い（objective の範囲外）。必要なら `--paired <file>` を足す。

## 提案

- なし
