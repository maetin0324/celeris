//! 無 LLM の DAG・外部待機・障害復旧の比較ドライバ（ADR 2026-10-19-dag-fault-comparison-driver）。
//! dataset は `control_measured`（実 LLM キャンペーンの記録とは混ぜない）。
pub mod baseline;
pub mod celeris_side;
pub mod ext;
pub mod fault;
pub mod ipc;
pub mod metrics;
pub mod process;
pub mod run;
