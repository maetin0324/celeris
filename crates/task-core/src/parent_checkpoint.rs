//! ADR 2026-11-22-parent-checkpoint-evaluator: 試験を先に置くための型と署名（実装前）。
#![allow(unused_variables, dead_code)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Status;
use crate::strategy::Reference;

pub const EVALUATION_SCHEMA: &str = "celeris.parent-evaluation/1";
pub const RECORD_SCHEMA: &str = "celeris.parent-evaluation-record/1";
pub const LIST_MAX: usize = 16;
pub const TEXT_MAX_CHARS: usize = 2_000;
pub const SHORT_MAX_CHARS: usize = 200;
pub const REFS_PER_ITEM_MAX: usize = 16;
pub const EVALUATION_MAX_BYTES: usize = 262_144;
pub const REASON_EXPERIMENT: &str = "checkpoint_experiment";
pub const REASON_PIVOT: &str = "checkpoint_pivot";
pub const PARENT_EVALUATION_RUBRIC: [(&str, &str); 6] =
    [("", ""), ("", ""), ("", ""), ("", ""), ("", ""), ("", "")];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointSettings {
    children_per_checkpoint: u32,
    budget_thresholds_pct: Vec<u8>,
    max_evaluation_attempts: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub field: &'static str,
    pub detail: String,
}
impl Default for CheckpointSettings {
    fn default() -> Self {
        todo!("実装前")
    }
}
impl CheckpointSettings {
    pub fn new(n: u32, p: Vec<u8>, a: u32) -> Result<Self, SettingsError> {
        todo!("実装前")
    }
    pub fn children_per_checkpoint(&self) -> u32 {
        todo!("実装前")
    }
    pub fn budget_thresholds_pct(&self) -> &[u8] {
        todo!("実装前")
    }
    pub fn max_evaluation_attempts(&self) -> u32 {
        todo!("実装前")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildTransition {
    pub to: Status,
    pub at: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildFacts {
    pub task_id: String,
    pub transitions: Vec<ChildTransition>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyDocumentFacts {
    pub name: String,
    pub bytes: Option<Vec<u8>>,
    pub at: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyFacts {
    pub strategy_id: String,
    pub version: u32,
    pub documents: Vec<StrategyDocumentFacts>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerFacts {
    pub strategy: Option<StrategyFacts>,
    pub children: Vec<ChildFacts>,
    pub tokens_used: u64,
    pub max_tokens: Option<u64>,
    pub evaluated: BTreeSet<String>,
    pub requested: BTreeSet<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    Children,
    ChildFailed,
    Refute,
    Budget,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerState {
    Due,
    Requested,
    Evaluated,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub key: String,
    pub kind: TriggerKind,
    pub state: TriggerState,
    pub at: Option<String>,
}
pub fn triggers(facts: &TriggerFacts, settings: &CheckpointSettings) -> Vec<Trigger> {
    todo!("実装前")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    OnTrack,
    AtRisk,
    OffTrack,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Next {
    Continue,
    DiscriminatingExperiment,
    Pivot,
    AskHuman,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAxis {
    Succeeded,
    Partial,
    Failed,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicAxis {
    Advanced,
    None,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalAxis {
    Met,
    NotMet,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Axes {
    pub execution: ExecutionAxis,
    pub epistemic: EpistemicAxis,
    pub goal: GoalAxis,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildOutcomes {
    pub done_passed: u32,
    pub done_unpassed: u32,
    pub failed: u32,
    pub cancelled: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationFacts {
    pub resolved: Vec<Reference>,
    pub goal_reached: bool,
    pub children: ChildOutcomes,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Shape,
    Limit,
    Unsupported,
    OnTrackUnsupported,
    GoalNotD6,
    ExecutionMismatch,
    EpistemicMismatch,
    NextNotAllowed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub rule: Rule,
    pub path: String,
    pub detail: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Checked,
    Invalid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effective {
    pub verdict: Verdict,
    pub next: Next,
    pub axes: Axes,
    pub unsupported: u32,
    pub negative_result: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedEvaluation {
    pub status: RecordStatus,
    pub effective: Effective,
    pub violations: Vec<Violation>,
}
pub fn references_to_resolve(raw: &Value) -> Vec<Reference> {
    todo!("実装前")
}
pub fn check_evaluation(raw: Option<&Value>, facts: &EvaluationFacts) -> CheckedEvaluation {
    todo!("実装前")
}
pub fn evaluation_artifact_name(decision_id: &str) -> String {
    todo!("実装前")
}
pub fn evaluation_decision_id(name: &str) -> Option<&str> {
    todo!("実装前")
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentEvaluationRecord {
    pub schema: String,
    pub decision_id: String,
    pub root_id: String,
    pub triggers: Vec<String>,
    pub status: RecordStatus,
    pub evaluation: Option<Value>,
    pub effective: Effective,
    pub violations: Vec<String>,
}
impl ParentEvaluationRecord {
    pub fn new(
        decision_id: &str,
        root_id: &str,
        triggers: Vec<String>,
        evaluation: Option<Value>,
        checked: &CheckedEvaluation,
    ) -> Self {
        todo!("実装前")
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedAction {
    Continue,
    Wait,
    Replan { reason_code: &'static str },
    Escalate,
}
impl AllowedAction {
    pub fn kind(&self) -> &'static str {
        todo!("実装前")
    }
}
pub fn allowed_actions(status: RecordStatus, effective: &Effective) -> Vec<AllowedAction> {
    todo!("実装前")
}
pub fn checkpoint_question_key(decision_id: &str) -> String {
    todo!("実装前")
}
pub fn action_allowed(
    allowed: &[AllowedAction],
    decision_id: &str,
    kind: &str,
    reason_code: Option<&str>,
    question_key: Option<&str>,
) -> bool {
    todo!("実装前")
}

#[cfg(test)]
mod tests;
