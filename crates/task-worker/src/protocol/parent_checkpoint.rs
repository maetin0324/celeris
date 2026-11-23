//! ADR 2026-11-22-parent-checkpoint-evaluator D3.1: 親の途中評価の run の入力 `context.review.parent_checkpoint`。
//!
//! 欄は dispatcher が判断の行・snapshot・root の events から決定的に集める（集めるのは後続の controller-checkpoint の
//! WU）。worker・CoS の報告の文は入力の欄にしない（controller の I11）。Project.request の全文は既存の
//! `context.lineage.project` で届く。子 0 の answer key は入力に入らない（D8.3）。
//! 出力 `ReviewOutput.parent_evaluation` の形は `task_core::parent_checkpoint::ParentEvaluation` が正本で、ここに写さない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::strategy_ref::StrategyRef;
use task_core::{Status, TaskId};

use super::RubricItem;

/// D3.1: 親の途中評価の入力。`Some` のとき `ReviewRequest.criteria` は空で、reviewer は task の受け入れ条件を判定しない
/// （`verdicts` は空）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointRequest {
    /// controller の `evaluate` の判断の id（評価の記録の name `parent-evaluation/<decision_id>.json` に使う）。
    pub decision_id: String,
    /// 評価する strategy root。
    pub root_id: TaskId,
    /// 発火した trigger の key（`children:<k>`・`child_failed:<task id>`・`refute:<strategy_id>:v<n>:<evidence id>`・
    /// `budget:<p>`。1〜8 件）。
    pub triggers: Vec<String>,
    pub goal: ParentCheckpointGoal,
    /// root が実行している戦略の版（CoS の `task.create` の `strategy`。子 2 D3.4。型は子 5 の gate と同じ
    /// `task_core::strategy_ref::StrategyRef`。ADR D1.1）。
    pub strategy: StrategyRef,
    /// root の直下の子。
    #[serde(default)]
    pub children: Vec<ParentCheckpointChild>,
    pub metrics: ParentCheckpointMetrics,
    /// 同じ root の前の評価の記録（無ければ省略）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous: Vec<ParentCheckpointPrevious>,
    /// 固定の基準 E1〜E6（[`ParentCheckpointRequest::fixed_rubric`]）。
    pub rubric: Vec<RubricItem>,
}

impl ParentCheckpointRequest {
    /// `task_core::parent_checkpoint::PARENT_EVALUATION_RUBRIC` を wire の形にしたもの（基準の定義はそちらだけ）。
    pub fn fixed_rubric() -> Vec<RubricItem> {
        RubricItem::from_table(&task_core::parent_checkpoint::PARENT_EVALUATION_RUBRIC)
    }
}

/// 親 Outcome。`reached` は Rust の D6（`recorded_outcomes`・`root_satisfies`）の値で、評価は到達を書かない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointGoal {
    /// Project.request の sha256（全文は `context.lineage.project`）。
    pub request_sha256: String,
    #[serde(default)]
    pub outcomes: Vec<ParentCheckpointOutcome>,
}

/// 承認済み Roadmap の Outcome 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointOutcome {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// Rust の判定（D6）。
    pub reached: bool,
}

/// root の直下の子 1 件（events から導いた事実）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointChild {
    pub task_id: TaskId,
    pub title: String,
    pub status: Status,
    /// 子の reviewer の判定が合格か（子の done は親の進捗ではない）。
    pub reviewer_passed: bool,
    #[serde(default)]
    pub criteria: Vec<ParentCheckpointCriterion>,
}

/// 子の受け入れ条件 1 件と、その判定の結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointCriterion {
    pub text: String,
    /// check の種類（`command`・`artifact_exists`・`reviewer`・`human` など）。
    pub kind: String,
    /// 判定の結果（`pass`・`fail`・`not_run`）。
    pub result: String,
}

/// 決定的なメトリクス（D1.2 の trigger と同じ値）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointMetrics {
    pub children_terminal: u32,
    pub children_total: u32,
    /// root の subtree の全 run の使った token。
    pub tokens_used: u64,
    /// root の token 予算（無ければ省略）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

/// 前の評価の記録 1 件（root の `ArtifactProduced`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParentCheckpointPrevious {
    pub name: String,
    pub sha256: String,
}

#[cfg(test)]
pub(crate) mod tests;
