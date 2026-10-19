---
task: 01M59KY5FNM0K4XGPPS61D0CA2
wu: baseline
status: done
completed: 2026-10-19
---
# baseline: 合理的な非同期 DAG 実行器

- 実装: `crates/campaign/src/dagbench/baseline.rs`（`Baseline`）。依存解決・worker 枠・外部 job の非同期 poll・原子的 state 書込み・論理 ID 再照合・重複通知の無視。仕様は同 module の doc comment と `docs/ops/dag-fault-comparison.md` の「baseline 仕様」（手動復旧手順を含む）。
- 試験 `cargo test -p celeris-campaign --test dagbench_baseline`: 6 passed（k2-mini の makespan 1320 と外部 ledger で起動・消費 1 回ずつ、k2-wide 完走、保存点ごとの中断→再開（応答喪失注入あり）、通知重複、壊れた state はエラー）。
- 未解決/提案: 停止点は fsync が遅い fs のため 3 点おきに間引いた。faults WU は `stop_after_saves` を使わず event 到達 trigger で止める想定。
