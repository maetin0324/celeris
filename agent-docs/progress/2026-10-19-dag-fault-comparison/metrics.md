---
task: 01M59KY5FNM0K4XGPPS61D0CA2
wu: metrics
status: done
completed: 2026-10-19
---
# metrics: 生ログからの指標再生成と固定点掃引

- 実装: `crates/campaign/src/dagbench/metrics.rs`（raw 形式・`compute`・`write_tables`・`sweep_items`・`/proc` 標本）、
  `run.rs`（`dagbench run` baseline arm・`dagbench metrics`）、`main.rs` の subcommand。判断は ADR `2026-10-19-dag-fault-comparison-metrics.md`。
- baseline に `set_worker_log` と node `release_at` を足した（N campaign を同一 DAG に並べるため）。既存 6 試験は通る。
- 試験 `cargo test -p celeris-campaign --test dagbench_metrics` → 6 passed
  （baseline・Celeris 両方の raw から 2 回再生成して byte 同一／手計算した障害・重複・再照合・復旧・失われた進捗・介入・CPU の期待値／
  dataset ラベルと LLM 成功率の欄が無いこと／CLI／掃引一覧 84 項）。
- `cargo test -p celeris-campaign --test dagbench_baseline --test dagbench_ext --test runner_checker` → 6 / 4 / 16 passed。
- `cargo clippy -p celeris-campaign --lib --bins --test dagbench_* --test runner_checker -- -D warnings` → 0。`cargo fmt --all` 済み。
- 負荷生成なし。標本は `/proc` の読み取りだけ。

## 未解決・提案
- CLI の `--arm celeris` は未対応（M7）。accept WU は試験内の駆動（`dagbench_metrics.rs` の `celeris_metrics_…`）を再利用する。
- faults WU は `control.jsonl`（`control_restart` / `control_resumed` / `intervention`）を `metrics::append_control` で書くこと。
  `ext.jsonl` の `fault_fired` も撹乱の起点として読む。
- 制御 overhead は in-process のため上限値（M4）。daemon 単体の CPU は子 process 化後に pid を分ける。
