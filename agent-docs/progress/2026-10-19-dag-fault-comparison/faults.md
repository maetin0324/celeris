---
task: 01M59KY5FNM0K4XGPPS61D0CA2
wu: faults
status: done
completed: 2026-10-19
---
# faults: event 到達トリガの障害注入 runner と独立観測

- 実装: `crates/campaign/src/dagbench/fault.rs`（`FaultCase`・`FaultRunner`・観測 ledger・`report`）。
  ext-sim に `StopControl`・停止 hook・`external_wait_started`・`drop_torn_tail`。baseline に進捗 log。
  celeris_side に `with_faults`（ledger からの再開・孤児回収の登録・応答喪失の照会）。設計は
  `agent-docs/adr/2026-10-19-dag-fault-comparison-faults.md`（F1〜F5）。
- 依存追加: `celeris-campaign` に `time`（`daemon_instances` 行の時刻）。

## 証拠
- `cargo test -p celeris-campaign --test dagbench_fault --test dagbench_ext --test dagbench_baseline --test dagbench_celeris --test runner_checker` → 6 / 4 / 6 / 2 / 16 passed
- `cargo test -p celeris-campaign --test dagbench_fault` を全体で 4 回 + a2 単独 6 回連続 pass
- `cargo clippy -p celeris-campaign --lib --bins --test dagbench_fault --test dagbench_ext --test dagbench_baseline --test dagbench_celeris -- -D warnings` → 0
- `cargo fmt --all -- --check` → 0

## 未解決事項
- 修正前に一度だけ a2 が全体実行中に失敗した（runner のエラー。ログ収集を入れる前で原因は未特定）。最も有力な原因は、
  別 thread が書いていた worker ledger の行が SIGKILL で途中で切れ、再起動時の読み込みが壊れたこと。
  `drop_torn_tail` を入れてからは再現していない。失敗時は制御 process の stderr と観測 ledger が panic 文に出る。
- 障害の点は k2-mini × camp-1（ADR F5）。center（2 campaign）は baseline が複数 campaign を持たないため未実施。
- `bash scripts/dev/test-parallel.sh`（workspace 全体）はこの WU では未実行（統合段で実行）。

## 提案
- metrics WU は `fault::report` の `FaultReport`（復旧時間・失われた進捗・介入数）を指標の生ログ集計に取り込める。
