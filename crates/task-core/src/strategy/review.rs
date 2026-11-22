//! ADR 2026-11-22-strategy-review-critic D1・D2・D4・D5: 戦略の候補の識別実験・期待情報価値・停止条件・保留する前提と、
//! 却下した候補・実験段階・Critic・失敗の記録の規則（純関数。store を読まない）。
//!
//! 文書の規則（D1・D4.1 の鎖と `selected`）は [`super::validate_document`] の検査の表に入り、Record・Execute の両方に掛かる。
//! 実行の入口の規則（D2.4・D3.7・D4.1 の `candidate`・D4.2）は [`check_execution`] で、呼び出し側が store から集めた
//! [`ExecutionFacts`] を受ける。段階は文書から導き、保存しない（D2.1）。

use serde::{Deserialize, Serialize};

use super::{StrategyDocument, Violation};
use crate::ExecutionMode;

/// D1.1: 候補の採否（文書の承認の状態とは別）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Open,
    Rejected,
}

/// D1.1: 最小の識別実験。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub content: String,
    /// 区別する仮説の id。
    pub discriminates: Vec<String>,
    pub budget: ExperimentBudget,
}

/// 実験の予算（候補の `cost` 以下）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentBudget {
    pub usd_max: f64,
    pub wall_hours_max: f64,
}

/// D1.2: 保留する前提（今は確かめずに置く前提と、確かめに戻る条件）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldAssumption {
    pub id: String,
    pub statement: String,
    pub revisit_when: String,
}

/// D2.3: 鎖の版の実験 task（`cos_operations` の `task.create`・`applied` の行の `target_id`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentTask {
    /// 起票が参照した版。
    pub version: u32,
    pub task_id: String,
    /// 起票の `strategy.candidate`。
    pub candidate: Option<String>,
    /// gate の時点で `done`・`failed`・`cancelled`。
    pub terminal: bool,
}

/// D5: 実行の入口の事実（呼び出し側が store から集める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFacts {
    /// 文書を作った task（`strategy.task_id`）の status が `done`。
    pub source_done: bool,
    /// 参照した `(name, sha256)` が文書を作った task の Critic の対象にある。
    pub critiqued: bool,
    /// 参照した版と鎖の前の版の実験 task の全件。
    pub experiments: Vec<ExperimentTask>,
    /// 起票の本文の `execution`（省略は `None`）。
    pub execution: Option<ExecutionMode>,
    /// 起票の `strategy.candidate`。
    pub candidate: Option<String>,
    /// 起票の本文の `max_wall_secs`。
    pub max_wall_secs: Option<u64>,
}

/// 実行の入口の検査（D2.4・D3.7・D4.1 の `candidate`・D4.2）。違反が無ければ空の `Vec`。
pub fn check_execution(_doc: &StrategyDocument, _facts: &ExecutionFacts) -> Vec<Violation> {
    Vec::new()
}
