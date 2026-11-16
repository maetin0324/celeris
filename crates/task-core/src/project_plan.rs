//! ADR-0074 D3.3（Phase F4a (b)）: CoS が起こす案件レベルの計画（マイルストーン Task の DAG）の
//! schema `celeris.project-plan/1`。書き込みの入口と検証は ADR-0079 D13 で外し、既存の行を読むための型だけを残す。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出しはしない。ADR-0001 D2 / DESIGN 原則 1）。
//! 既存の行は `task_ops::project_plan` が履歴として読む。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// schema 版（`docs/protocol/project-plan.schema.json`）。
pub const PROJECT_PLAN_SCHEMA: &str = "celeris.project-plan/1";

/// ADR-0074 D3.3（Phase F4a）: この Plan タスクが「案件計画（マイルストーンの DAG）」の run であることの
/// 印。既存の `kind = plan` の分解タスク（`ADR-0033 D4`）と同じ `TaskKind::Plan` を使うが、schema と
/// 完了時の扱いが違うので、`Task.labels` にこの固定値を持たせて見分ける（新しい列・フラグを増やさない。
/// D3.1 が「位置で決める」のと同じ考え方を、こちらは「印で決める」に倣う。`[a-z0-9-]` の規則に合う）。
pub const MILESTONES_PLAN_LABEL: &str = "project-plan-milestones";

/// `task.labels` にこの印があれば、案件計画（マイルストーン DAG）の run。
pub fn is_milestones_plan_task(task: &crate::model::Task) -> bool {
    task.kind == crate::model::TaskKind::Plan
        && task.labels.iter().any(|l| l == MILESTONES_PLAN_LABEL)
}

/// D3.3: 1 マイルストーンの spec。`assignee` / `tier` / `model` / `lane` は持たない
/// （`deny_unknown_fields`。ADR-0069 D1: 担当は matching が決める）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MilestoneSpec {
    /// `[a-z0-9-]{1,32}`。計画の中で一意。
    pub key: String,
    pub title: String,
    pub objective: String,
    /// 何が示せたら途中目標の達成か（人の判定の材料。SPEC §7「検証の合格線」）。
    pub reach_criteria: String,
    #[serde(default)]
    pub acceptance: Vec<crate::model::Criterion>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub repos: Vec<String>,
    /// ADR-0069 D3: `TaskFeatureHints` の上書きヒント（型として検証する。D5.1 と同じ規律）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<crate::model_policy::TaskFeatureHints>,
    /// ADR-0072 D13 のヒント（+2）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<crate::execution_gate::ExecutionMode>,
    /// ADR-0074 D2.1 / D3.3（Phase F4b）: このマイルストーン Task の途中確認（F3 の `PausePolicy`）。
    /// 書かなければ既定（止めない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_after: Option<crate::pause::PausePolicy>,
}

/// D3.3: CoS の計画 run が `<artifacts>/project-plan.json` に書く JSON そのもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlanSpec {
    pub schema: String,
    pub rationale: String,
    pub milestones: Vec<MilestoneSpec>,
}

/// 生成したスキーマ（`docs/protocol/project-plan.schema.json`。`UPDATE_SCHEMA=1` で再生成）。
pub fn schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(ProjectPlanSpec);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

// ---- ADR-0074 D3.4（Phase F4b (e)）: 案件の replan（差分 `celeris.project-plan-delta/1`）----

/// 差分の schema 版（`docs/protocol/project-plan-delta.schema.json`）。
pub const PROJECT_PLAN_DELTA_SCHEMA: &str = "celeris.project-plan-delta/1";

/// D3.4: 案件計画の replan の run（既に承認済みの計画がある案件の計画 run）の印。
/// `MILESTONES_PLAN_LABEL` と一緒に付く。ワーカーのプロンプト（差分の書き方）と完了時の検証が見る。
pub const MILESTONES_REPLAN_LABEL: &str = "project-plan-replan";

/// この Plan タスクが案件計画の replan（差分を書く run）か。
pub fn is_milestones_replan_task(task: &crate::model::Task) -> bool {
    is_milestones_plan_task(task) && task.labels.iter().any(|l| l == MILESTONES_REPLAN_LABEL)
}

/// D3.4: 既存のマイルストーン 1 件の変更（書いた欄だけを変える。`key` は変えられない）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MilestoneModify {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach_criteria: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<Vec<crate::model::Criterion>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repos: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<crate::model_policy::TaskFeatureHints>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<crate::execution_gate::ExecutionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_after: Option<crate::pause::PausePolicy>,
}

/// D3.4: replan の run が `<artifacts>/project-plan.json` に書く差分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlanDelta {
    pub schema: String,
    /// この差分の元になった（承認済みの）版。現行の版と違えば古い差分として拒む。
    pub base_version: u32,
    pub rationale: String,
    /// 新しいマイルストーン（key は既存のどれとも衝突しない）。
    #[serde(default)]
    pub add: Vec<MilestoneSpec>,
    /// まだ dispatch されていないマイルストーンの変更。
    #[serde(default)]
    pub modify: Vec<MilestoneModify>,
    /// まだ dispatch されていないマイルストーンを計画から外す。
    #[serde(default)]
    pub remove: Vec<String>,
    /// 走っている・まだ終わっていないマイルストーンを明示して取り下げる（承認で `Cancel` を適用）。
    #[serde(default)]
    pub cancel: Vec<String>,
}

/// 差分の生成スキーマ（`docs/protocol/project-plan-delta.schema.json`）。
pub fn delta_schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(ProjectPlanDelta);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
#[path = "project_plan/tests.rs"]
mod tests;
