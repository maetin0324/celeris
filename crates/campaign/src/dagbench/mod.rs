//! 無 LLM の DAG・外部待機・障害復旧の比較ドライバ（`docs/ops/dag-fault-comparison.md`）。
//! dataset は `control_measured`（実 LLM キャンペーンの記録とは混ぜない）。
pub mod baseline;
pub mod celeris_side;
pub mod ext;
pub mod fault;
pub mod graded;
pub mod ipc;
pub mod metrics;
pub mod overhead;
pub mod process;
pub mod run;
