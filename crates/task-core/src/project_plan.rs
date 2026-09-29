//! ADR-0074 D3.3（Phase F4a (b)）: CoS が起こす案件レベルの計画（マイルストーン Task の DAG）の
//! schema `celeris.project-plan/1` と、決定的な検証。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出しはしない。ADR-0001 D2 / DESIGN 原則 1）。
//! 永続化（マイルストーン・Task・`Event::ProjectPlanProposed` の作成）は `task_ops::project_plan` が行う。

use std::collections::{BTreeMap, BTreeSet};

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

/// 検証の上限（既定値。D3.3「1..=12 件」・文字数の上限）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectPlanLimits {
    pub max_milestones: usize,
    pub max_rationale_chars: usize,
    pub max_title_chars: usize,
    pub max_objective_chars: usize,
    pub max_reach_criteria_chars: usize,
    pub max_plan_json_bytes: usize,
}

impl Default for ProjectPlanLimits {
    fn default() -> Self {
        ProjectPlanLimits {
            max_milestones: 12,
            max_rationale_chars: 2_000,
            max_title_chars: 120,
            max_objective_chars: 2_000,
            max_reach_criteria_chars: 2_000,
            max_plan_json_bytes: 24 * 1024,
        }
    }
}

/// D3.3: 検証エラー（すべて拒否理由。1 回だけ再試行し、それでも駄目なら何も作らない。呼び出し側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectPlanValidationError {
    WrongSchema {
        found: String,
    },
    NoMilestones,
    TooManyMilestones {
        count: usize,
        max: usize,
    },
    InvalidKey {
        key: String,
    },
    DuplicateKey {
        key: String,
    },
    /// D3.4（F4b）の申し送り: 差分（`add`）が既存の（前の版の）マイルストーンの key と衝突する。
    /// F4a では常に空集合を渡すので起きない。
    KeyCollidesWithExisting {
        key: String,
    },
    UnknownDependency {
        key: String,
        depends_on: String,
    },
    CyclicDependency {
        cycle: Vec<String>,
    },
    NoAcceptance {
        key: String,
    },
    InvalidAcceptance {
        key: String,
        detail: String,
    },
    RationaleTooLong {
        len: usize,
        max: usize,
    },
    TitleTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    ObjectiveTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    ReachCriteriaTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    PlanTooLarge {
        bytes: usize,
        max: usize,
    },
}

impl std::fmt::Display for ProjectPlanValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectPlanValidationError::WrongSchema { found } => {
                write!(f, "schema must be {PROJECT_PLAN_SCHEMA}, found {found}")
            }
            ProjectPlanValidationError::NoMilestones => {
                write!(f, "milestones must not be empty")
            }
            ProjectPlanValidationError::TooManyMilestones { count, max } => {
                write!(f, "too many milestones: {count} > {max}")
            }
            ProjectPlanValidationError::InvalidKey { key } => {
                write!(
                    f,
                    "invalid milestone key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            ProjectPlanValidationError::DuplicateKey { key } => {
                write!(f, "duplicate milestone key: {key}")
            }
            ProjectPlanValidationError::KeyCollidesWithExisting { key } => {
                write!(f, "milestone key {key} collides with an existing milestone")
            }
            ProjectPlanValidationError::UnknownDependency { key, depends_on } => {
                write!(f, "milestone {key} depends on unknown key {depends_on}")
            }
            ProjectPlanValidationError::CyclicDependency { cycle } => {
                write!(f, "cyclic dependency: {}", cycle.join(" -> "))
            }
            ProjectPlanValidationError::NoAcceptance { key } => {
                write!(f, "milestone {key}: acceptance must not be empty")
            }
            ProjectPlanValidationError::InvalidAcceptance { key, detail } => {
                write!(f, "milestone {key}: {detail}")
            }
            ProjectPlanValidationError::RationaleTooLong { len, max } => {
                write!(f, "rationale is too long: {len} > {max} characters")
            }
            ProjectPlanValidationError::TitleTooLong { key, len, max } => {
                write!(
                    f,
                    "milestone {key}: title is too long: {len} > {max} characters"
                )
            }
            ProjectPlanValidationError::ObjectiveTooLong { key, len, max } => {
                write!(
                    f,
                    "milestone {key}: objective is too long: {len} > {max} characters"
                )
            }
            ProjectPlanValidationError::ReachCriteriaTooLong { key, len, max } => {
                write!(
                    f,
                    "milestone {key}: reach_criteria is too long: {len} > {max} characters"
                )
            }
            ProjectPlanValidationError::PlanTooLarge { bytes, max } => {
                write!(f, "project plan JSON is too large: {bytes} > {max} bytes")
            }
        }
    }
}

impl std::error::Error for ProjectPlanValidationError {}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 検証を通った計画（トポロジカル順を添える。`milestones` の index）。
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedProjectPlan {
    pub spec: ProjectPlanSpec,
    pub topological_order: Vec<usize>,
}

/// D3.3: 計画を検証する。`existing_keys` は前の版で使われている（変更できない）マイルストーンの key
/// （D3.4 の replan 用。F4a の初回提案では常に空集合）。
pub fn validate(
    spec: &ProjectPlanSpec,
    limits: ProjectPlanLimits,
    existing_keys: &BTreeSet<String>,
) -> Result<ValidatedProjectPlan, Vec<ProjectPlanValidationError>> {
    let mut errors = Vec::new();

    if spec.schema != PROJECT_PLAN_SCHEMA {
        errors.push(ProjectPlanValidationError::WrongSchema {
            found: spec.schema.clone(),
        });
    }
    if spec.milestones.is_empty() {
        errors.push(ProjectPlanValidationError::NoMilestones);
    }
    if spec.milestones.len() > limits.max_milestones {
        errors.push(ProjectPlanValidationError::TooManyMilestones {
            count: spec.milestones.len(),
            max: limits.max_milestones,
        });
    }

    let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
    for m in &spec.milestones {
        if !valid_key(&m.key) {
            errors.push(ProjectPlanValidationError::InvalidKey { key: m.key.clone() });
            continue;
        }
        if existing_keys.contains(&m.key) {
            errors.push(ProjectPlanValidationError::KeyCollidesWithExisting { key: m.key.clone() });
        }
        if !seen_keys.insert(m.key.as_str()) {
            errors.push(ProjectPlanValidationError::DuplicateKey { key: m.key.clone() });
        }
    }

    let known_keys: BTreeSet<&str> = spec.milestones.iter().map(|m| m.key.as_str()).collect();
    for m in &spec.milestones {
        for dep in &m.depends_on {
            if !known_keys.contains(dep.as_str()) {
                errors.push(ProjectPlanValidationError::UnknownDependency {
                    key: m.key.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
    }

    for m in &spec.milestones {
        if m.acceptance.is_empty() {
            errors.push(ProjectPlanValidationError::NoAcceptance { key: m.key.clone() });
        } else if let Err(detail) =
            crate::model::validate_human_checks_have_deliverable(&m.acceptance)
        {
            errors.push(ProjectPlanValidationError::InvalidAcceptance {
                key: m.key.clone(),
                detail,
            });
        }
    }

    if spec.rationale.chars().count() > limits.max_rationale_chars {
        errors.push(ProjectPlanValidationError::RationaleTooLong {
            len: spec.rationale.chars().count(),
            max: limits.max_rationale_chars,
        });
    }
    for m in &spec.milestones {
        let title_len = m.title.chars().count();
        if title_len > limits.max_title_chars {
            errors.push(ProjectPlanValidationError::TitleTooLong {
                key: m.key.clone(),
                len: title_len,
                max: limits.max_title_chars,
            });
        }
        let objective_len = m.objective.chars().count();
        if objective_len > limits.max_objective_chars {
            errors.push(ProjectPlanValidationError::ObjectiveTooLong {
                key: m.key.clone(),
                len: objective_len,
                max: limits.max_objective_chars,
            });
        }
        let reach_len = m.reach_criteria.chars().count();
        if reach_len > limits.max_reach_criteria_chars {
            errors.push(ProjectPlanValidationError::ReachCriteriaTooLong {
                key: m.key.clone(),
                len: reach_len,
                max: limits.max_reach_criteria_chars,
            });
        }
    }
    let plan_bytes = serde_json::to_vec(spec).map(|v| v.len()).unwrap_or(0);
    if plan_bytes > limits.max_plan_json_bytes {
        errors.push(ProjectPlanValidationError::PlanTooLarge {
            bytes: plan_bytes,
            max: limits.max_plan_json_bytes,
        });
    }

    // トポロジカルソート（循環の検出も兼ねる。Kahn's algorithm、決定的に key 昇順で tie-break）。
    let mut topological_order = Vec::new();
    if errors.is_empty() {
        match topo_sort(&spec.milestones) {
            Ok(order) => topological_order = order,
            Err(cycle) => errors.push(ProjectPlanValidationError::CyclicDependency { cycle }),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(ValidatedProjectPlan {
        spec: spec.clone(),
        topological_order,
    })
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

impl MilestoneModify {
    /// `base` に書いた欄だけを重ねる。
    pub fn apply_to(&self, base: &MilestoneSpec) -> MilestoneSpec {
        let mut out = base.clone();
        if let Some(v) = &self.title {
            out.title = v.clone();
        }
        if let Some(v) = &self.objective {
            out.objective = v.clone();
        }
        if let Some(v) = &self.reach_criteria {
            out.reach_criteria = v.clone();
        }
        if let Some(v) = &self.acceptance {
            out.acceptance = v.clone();
        }
        if let Some(v) = &self.depends_on {
            out.depends_on = v.clone();
        }
        if let Some(v) = &self.genre {
            out.genre = Some(v.clone());
        }
        if let Some(v) = &self.skills {
            out.skills = v.clone();
        }
        if let Some(v) = &self.repos {
            out.repos = v.clone();
        }
        if let Some(v) = self.features {
            out.features = Some(v);
        }
        if let Some(v) = self.execution {
            out.execution = Some(v);
        }
        if let Some(v) = &self.pause_after {
            out.pause_after = Some(v.clone());
        }
        out
    }
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

/// 差分の検証で見る、現行の計画の 1 節点の実行状態（ストアから決定的に作る）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanNodeState {
    /// マイルストーン Task が一度でも dispatch された（`draft` / `ready` のまま run が 0、でない）。
    pub dispatched: bool,
    /// Task が終端（done / failed / cancelled）。
    pub terminal: bool,
}

/// 差分の検証エラー（当てた後の計画そのものの検証エラーは `Plan` で運ぶ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectPlanDeltaError {
    WrongSchema {
        found: String,
    },
    StaleBaseVersion {
        base_version: u32,
        current: u32,
    },
    EmptyDelta,
    UnknownMilestone {
        key: String,
    },
    /// 同じ key を `modify` / `remove` / `cancel` の 2 つ以上に書いた（または同じ所に 2 回）。
    ConflictingChange {
        key: String,
    },
    /// dispatch 済みのマイルストーンを `modify` / `remove` しようとした（`cancel` を明示する）。
    ModifiesStartedMilestone {
        key: String,
    },
    /// 既に終端のマイルストーンを `cancel` しようとした。
    CancelsFinishedMilestone {
        key: String,
    },
    /// `add` の key が既存（外すものも含む）の key と衝突する。
    KeyCollidesWithExisting {
        key: String,
    },
    /// 差分を当てた後の計画が `validate` に通らない。
    Plan(ProjectPlanValidationError),
}

impl std::fmt::Display for ProjectPlanDeltaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectPlanDeltaError::WrongSchema { found } => {
                write!(
                    f,
                    "schema must be {PROJECT_PLAN_DELTA_SCHEMA}, found {found}"
                )
            }
            ProjectPlanDeltaError::StaleBaseVersion {
                base_version,
                current,
            } => write!(
                f,
                "base_version {base_version} is not the current approved version {current}"
            ),
            ProjectPlanDeltaError::EmptyDelta => write!(f, "the delta changes nothing"),
            ProjectPlanDeltaError::UnknownMilestone { key } => {
                write!(f, "unknown milestone key: {key}")
            }
            ProjectPlanDeltaError::ConflictingChange { key } => write!(
                f,
                "milestone {key} appears more than once in modify / remove / cancel"
            ),
            ProjectPlanDeltaError::ModifiesStartedMilestone { key } => write!(
                f,
                "milestone {key} was already dispatched; it cannot be modified or removed (use cancel)"
            ),
            ProjectPlanDeltaError::CancelsFinishedMilestone { key } => {
                write!(
                    f,
                    "milestone {key} has already finished; it cannot be cancelled"
                )
            }
            ProjectPlanDeltaError::KeyCollidesWithExisting { key } => {
                write!(f, "milestone key {key} collides with an existing milestone")
            }
            ProjectPlanDeltaError::Plan(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProjectPlanDeltaError {}

/// 検証を通った差分。`result` は差分を当てた後の計画全体（外した・取り下げたものを除き、既存の並びの後に
/// `add` を足した順）、`topological_order` はその `result.milestones` の index。
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedProjectPlanDelta {
    pub delta: ProjectPlanDelta,
    pub result: ProjectPlanSpec,
    pub topological_order: Vec<usize>,
}

impl ValidatedProjectPlanDelta {
    /// `add` に書かれた key か。
    pub fn is_added(&self, key: &str) -> bool {
        self.delta.add.iter().any(|m| m.key == key)
    }
}

/// D3.4: 差分を検証する（純粋関数）。`base` は現行の（承認済みの `current_version` の）計画全体、
/// `states` はその各 key の実行状態（無い key は「dispatch されていない・終端でない」扱い）。
///
/// 規則: schema・`base_version == current_version`・空でない、`modify` / `remove` / `cancel` の key は
/// 既存で重複しない、`modify` / `remove` は dispatch されていないものだけ（走っている・終わったものは
/// `cancel` を明示）、`cancel` は終端でないものだけ、`add` の key は既存と衝突しない、当てた後の計画が
/// `validate`（件数・依存・循環・acceptance・文字数）に通る。
pub fn validate_delta(
    delta: &ProjectPlanDelta,
    base: &ProjectPlanSpec,
    current_version: u32,
    states: &BTreeMap<String, PlanNodeState>,
    limits: ProjectPlanLimits,
) -> Result<ValidatedProjectPlanDelta, Vec<ProjectPlanDeltaError>> {
    let mut errors = Vec::new();
    if delta.schema != PROJECT_PLAN_DELTA_SCHEMA {
        errors.push(ProjectPlanDeltaError::WrongSchema {
            found: delta.schema.clone(),
        });
    }
    if delta.base_version != current_version {
        errors.push(ProjectPlanDeltaError::StaleBaseVersion {
            base_version: delta.base_version,
            current: current_version,
        });
    }
    if delta.add.is_empty()
        && delta.modify.is_empty()
        && delta.remove.is_empty()
        && delta.cancel.is_empty()
    {
        errors.push(ProjectPlanDeltaError::EmptyDelta);
    }
    let existing: BTreeSet<&str> = base.milestones.iter().map(|m| m.key.as_str()).collect();
    let state_of = |key: &str| states.get(key).copied().unwrap_or_default();

    let mut touched: BTreeSet<&str> = BTreeSet::new();
    let changed = delta
        .modify
        .iter()
        .map(|m| m.key.as_str())
        .chain(delta.remove.iter().map(String::as_str))
        .chain(delta.cancel.iter().map(String::as_str));
    for key in changed {
        if !existing.contains(key) {
            errors.push(ProjectPlanDeltaError::UnknownMilestone {
                key: key.to_string(),
            });
        } else if !touched.insert(key) {
            errors.push(ProjectPlanDeltaError::ConflictingChange {
                key: key.to_string(),
            });
        }
    }
    for key in delta
        .modify
        .iter()
        .map(|m| m.key.as_str())
        .chain(delta.remove.iter().map(String::as_str))
    {
        if existing.contains(key) {
            let st = state_of(key);
            if st.dispatched || st.terminal {
                errors.push(ProjectPlanDeltaError::ModifiesStartedMilestone {
                    key: key.to_string(),
                });
            }
        }
    }
    for key in &delta.cancel {
        if existing.contains(key.as_str()) && state_of(key).terminal {
            errors.push(ProjectPlanDeltaError::CancelsFinishedMilestone { key: key.clone() });
        }
    }
    for m in &delta.add {
        if existing.contains(m.key.as_str()) {
            errors.push(ProjectPlanDeltaError::KeyCollidesWithExisting { key: m.key.clone() });
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let dropped: BTreeSet<&str> = delta
        .remove
        .iter()
        .chain(delta.cancel.iter())
        .map(String::as_str)
        .collect();
    let mut milestones: Vec<MilestoneSpec> = Vec::new();
    for m in &base.milestones {
        if dropped.contains(m.key.as_str()) {
            continue;
        }
        match delta.modify.iter().find(|x| x.key == m.key) {
            Some(change) => milestones.push(change.apply_to(m)),
            None => milestones.push(m.clone()),
        }
    }
    milestones.extend(delta.add.iter().cloned());
    let result = ProjectPlanSpec {
        schema: PROJECT_PLAN_SCHEMA.to_string(),
        rationale: delta.rationale.clone(),
        milestones,
    };
    match validate(&result, limits, &BTreeSet::new()) {
        Ok(v) => Ok(ValidatedProjectPlanDelta {
            delta: delta.clone(),
            result: v.spec,
            topological_order: v.topological_order,
        }),
        Err(es) => Err(es.into_iter().map(ProjectPlanDeltaError::Plan).collect()),
    }
}

fn topo_sort(milestones: &[MilestoneSpec]) -> Result<Vec<usize>, Vec<String>> {
    let index_of: BTreeMap<&str, usize> = milestones
        .iter()
        .enumerate()
        .map(|(i, m)| (m.key.as_str(), i))
        .collect();
    let mut in_degree: Vec<usize> = vec![0; milestones.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); milestones.len()];
    for (i, m) in milestones.iter().enumerate() {
        for dep in &m.depends_on {
            if let Some(&dep_i) = index_of.get(dep.as_str()) {
                dependents[dep_i].push(i);
                in_degree[i] += 1;
            }
        }
    }
    let mut ready: BTreeSet<(&str, usize)> = milestones
        .iter()
        .enumerate()
        .filter(|(i, _)| in_degree[*i] == 0)
        .map(|(i, m)| (m.key.as_str(), i))
        .collect();
    let mut order = Vec::new();
    while let Some((_, i)) = ready.iter().next().copied() {
        ready.remove(&(milestones[i].key.as_str(), i));
        order.push(i);
        for &dep in &dependents[i] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                ready.insert((milestones[dep].key.as_str(), dep));
            }
        }
    }
    if order.len() == milestones.len() {
        Ok(order)
    } else {
        let cycle: Vec<String> = (0..milestones.len())
            .filter(|i| !order.contains(i))
            .map(|i| milestones[i].key.clone())
            .collect();
        Err(cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Check, Criterion};

    fn acceptance() -> Vec<Criterion> {
        vec![Criterion {
            text: "done".into(),
            check: Check::Human,
        }]
        .into_iter()
        .chain(std::iter::once(Criterion {
            text: "artifact".into(),
            check: Check::ArtifactExists {
                name: "report.md".into(),
            },
        }))
        .collect()
    }

    fn milestone(key: &str, depends_on: &[&str]) -> MilestoneSpec {
        MilestoneSpec {
            pause_after: None,
            key: key.into(),
            title: format!("title-{key}"),
            objective: "objective".into(),
            reach_criteria: "criteria".into(),
            acceptance: acceptance(),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            genre: None,
            skills: Vec::new(),
            repos: Vec::new(),
            features: None,
            execution: None,
        }
    }

    fn plan(milestones: Vec<MilestoneSpec>) -> ProjectPlanSpec {
        ProjectPlanSpec {
            schema: PROJECT_PLAN_SCHEMA.to_string(),
            rationale: "rationale".into(),
            milestones,
        }
    }

    #[test]
    fn a_valid_plan_is_accepted_and_topologically_ordered() {
        let spec = plan(vec![
            milestone("survey", &[]),
            milestone("poc", &["survey"]),
        ]);
        let validated =
            validate(&spec, ProjectPlanLimits::default(), &BTreeSet::new()).expect("valid plan");
        let order: Vec<&str> = validated
            .topological_order
            .iter()
            .map(|&i| validated.spec.milestones[i].key.as_str())
            .collect();
        assert_eq!(order, vec!["survey", "poc"]);
    }

    #[test]
    fn rejects_cycles_and_unknown_keys() {
        let unknown = plan(vec![milestone("a", &["ghost"])]);
        let errors = validate(&unknown, ProjectPlanLimits::default(), &BTreeSet::new())
            .expect_err("rejected");
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ProjectPlanValidationError::UnknownDependency { key, depends_on }
                    if key == "a" && depends_on == "ghost"
            )),
            "{errors:?}"
        );

        let cyclic = plan(vec![milestone("a", &["b"]), milestone("b", &["a"])]);
        let errors = validate(&cyclic, ProjectPlanLimits::default(), &BTreeSet::new())
            .expect_err("rejected");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProjectPlanValidationError::CyclicDependency { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_wrong_schema_duplicate_keys_and_count_limits() {
        let wrong_schema = ProjectPlanSpec {
            schema: "celeris.project-plan/2".into(),
            ..plan(vec![milestone("a", &[])])
        };
        let errors = validate(
            &wrong_schema,
            ProjectPlanLimits::default(),
            &BTreeSet::new(),
        )
        .expect_err("rejected");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProjectPlanValidationError::WrongSchema { .. })),
            "{errors:?}"
        );

        let empty = plan(vec![]);
        let errors =
            validate(&empty, ProjectPlanLimits::default(), &BTreeSet::new()).expect_err("rejected");
        assert!(errors.contains(&ProjectPlanValidationError::NoMilestones));

        let dup = plan(vec![milestone("a", &[]), milestone("a", &[])]);
        let errors =
            validate(&dup, ProjectPlanLimits::default(), &BTreeSet::new()).expect_err("rejected");
        assert!(
            errors.iter().any(
                |e| matches!(e, ProjectPlanValidationError::DuplicateKey { key } if key == "a")
            ),
            "{errors:?}"
        );

        let too_many: Vec<MilestoneSpec> =
            (0..13).map(|i| milestone(&format!("m{i}"), &[])).collect();
        let too_many = plan(too_many);
        let limits = ProjectPlanLimits::default();
        let errors = validate(&too_many, limits, &BTreeSet::new()).expect_err("rejected");
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ProjectPlanValidationError::TooManyMilestones { count: 13, max } if *max == limits.max_milestones
            )),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_empty_acceptance_and_human_checks_without_a_deliverable() {
        let mut no_acceptance = milestone("a", &[]);
        no_acceptance.acceptance = Vec::new();
        let errors = validate(
            &plan(vec![no_acceptance]),
            ProjectPlanLimits::default(),
            &BTreeSet::new(),
        )
        .expect_err("rejected");
        assert!(
            errors.iter().any(
                |e| matches!(e, ProjectPlanValidationError::NoAcceptance { key } if key == "a")
            ),
            "{errors:?}"
        );

        let mut bare_human = milestone("a", &[]);
        bare_human.acceptance = vec![Criterion {
            text: "done".into(),
            check: Check::Human,
        }];
        let errors = validate(
            &plan(vec![bare_human]),
            ProjectPlanLimits::default(),
            &BTreeSet::new(),
        )
        .expect_err("rejected");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ProjectPlanValidationError::InvalidAcceptance { key, .. } if key == "a")),
            "{errors:?}"
        );
    }

    #[test]
    fn is_milestones_plan_task_needs_the_kind_and_the_label() {
        use crate::model::{
            Budget, Check, Criterion, Status, Task, TaskId, TaskKind, Tier, WorkerHint,
            WorkspaceSpec,
        };
        let now = time::OffsetDateTime::now_utc();
        let base = Task {
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Plan,
            title: "t".into(),
            objective: "o".into(),
            acceptance: vec![Criterion {
                text: "c".into(),
                check: Check::Human,
            }],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Draft,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "ws".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 1,
                max_wall_secs: 1,
                max_retries: 0,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        };
        assert!(!is_milestones_plan_task(&base), "no label yet");

        let mut labeled = base.clone();
        labeled.labels = vec![MILESTONES_PLAN_LABEL.to_string()];
        assert!(is_milestones_plan_task(&labeled));

        let mut wrong_kind = labeled.clone();
        wrong_kind.kind = TaskKind::Execute;
        assert!(!is_milestones_plan_task(&wrong_kind));
    }

    /// ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
    #[test]
    fn committed_schema_matches_generated() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/protocol/project-plan.schema.json"
        );
        let generated = serde_json::to_string_pretty(&schema_value()).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
        assert_eq!(
            committed, generated,
            "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-core`"
        );
    }

    #[test]
    fn committed_delta_schema_matches_generated() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/protocol/project-plan-delta.schema.json"
        );
        let generated = serde_json::to_string_pretty(&delta_schema_value()).unwrap() + "\n";
        if std::env::var_os("UPDATE_SCHEMA").is_some() {
            std::fs::write(path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e} (run with UPDATE_SCHEMA=1 to generate)"));
        assert_eq!(
            committed, generated,
            "schema drift: run `UPDATE_SCHEMA=1 cargo test -p task-core`"
        );
    }

    fn spec(key: &str, deps: &[&str]) -> MilestoneSpec {
        MilestoneSpec {
            key: key.into(),
            title: key.into(),
            objective: "o".into(),
            reach_criteria: "r".into(),
            acceptance: acceptance(),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            genre: None,
            skills: vec![],
            repos: vec![],
            features: None,
            execution: None,
            pause_after: None,
        }
    }

    fn base_plan() -> ProjectPlanSpec {
        ProjectPlanSpec {
            schema: PROJECT_PLAN_SCHEMA.into(),
            rationale: "r".into(),
            milestones: vec![spec("survey", &[]), spec("poc", &["survey"])],
        }
    }

    fn delta() -> ProjectPlanDelta {
        ProjectPlanDelta {
            schema: PROJECT_PLAN_DELTA_SCHEMA.into(),
            base_version: 1,
            rationale: "見直し".into(),
            add: vec![],
            modify: vec![],
            remove: vec![],
            cancel: vec![],
        }
    }

    /// ADR-0074 D3.4（Phase F4b (e)）: dispatch 済みのマイルストーンは `modify` / `remove` できない
    /// （`cancel` を明示する）。dispatch 前のものは変えられ、当てた後の計画が返る。
    #[test]
    fn delta_modify_and_remove_only_touch_undispatched_milestones() {
        let mut states = BTreeMap::new();
        states.insert(
            "survey".to_string(),
            PlanNodeState {
                dispatched: true,
                terminal: false,
            },
        );
        let mut d = delta();
        d.modify.push(MilestoneModify {
            key: "survey".into(),
            title: Some("x".into()),
            ..MilestoneModify::default()
        });
        let errs =
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap_err();
        assert!(
            errs.contains(&ProjectPlanDeltaError::ModifiesStartedMilestone {
                key: "survey".into()
            })
        );

        let mut d = delta();
        d.remove.push("survey".into());
        assert!(
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default())
                .unwrap_err()
                .contains(&ProjectPlanDeltaError::ModifiesStartedMilestone {
                    key: "survey".into()
                })
        );

        // cancel は明示すれば通る（poc は survey に依存するので一緒に外す必要がある）。
        let mut d = delta();
        d.cancel.push("survey".into());
        d.remove.push("poc".into());
        let ok = validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default());
        assert!(ok.is_err(), "empty result plan is rejected: {ok:?}");
        d.add.push(spec("redo", &[]));
        let ok =
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap();
        assert_eq!(ok.result.milestones.len(), 1);
        assert_eq!(ok.result.milestones[0].key, "redo");

        // dispatch 前の poc の modify は通り、書いた欄だけが変わる。
        let mut d = delta();
        d.modify.push(MilestoneModify {
            key: "poc".into(),
            title: Some("PoC v2".into()),
            ..MilestoneModify::default()
        });
        d.add.push(spec("paper", &["poc"]));
        let ok =
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap();
        let poc = ok
            .result
            .milestones
            .iter()
            .find(|m| m.key == "poc")
            .unwrap();
        assert_eq!(poc.title, "PoC v2");
        assert_eq!(poc.depends_on, vec!["survey".to_string()]);
        assert!(ok.is_added("paper"));
        assert!(!ok.is_added("poc"));
    }

    #[test]
    fn delta_rejects_stale_base_unknown_keys_collisions_and_dangling_dependencies() {
        let states = BTreeMap::new();
        let mut d = delta();
        d.base_version = 0;
        d.remove.push("nope".into());
        d.add.push(spec("survey", &[]));
        let errs =
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap_err();
        assert!(errs.contains(&ProjectPlanDeltaError::StaleBaseVersion {
            base_version: 0,
            current: 1
        }));
        assert!(errs.contains(&ProjectPlanDeltaError::UnknownMilestone { key: "nope".into() }));
        assert!(
            errs.contains(&ProjectPlanDeltaError::KeyCollidesWithExisting {
                key: "survey".into()
            })
        );

        assert_eq!(
            validate_delta(
                &delta(),
                &base_plan(),
                1,
                &states,
                ProjectPlanLimits::default()
            )
            .unwrap_err(),
            vec![ProjectPlanDeltaError::EmptyDelta]
        );

        // survey を外すと、残る poc の依存が宙に浮く。
        let mut d = delta();
        d.remove.push("survey".into());
        let errs =
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default()).unwrap_err();
        assert!(matches!(
            errs.as_slice(),
            [ProjectPlanDeltaError::Plan(
                ProjectPlanValidationError::UnknownDependency { .. }
            )]
        ));

        let mut d = delta();
        d.remove.push("poc".into());
        d.cancel.push("poc".into());
        assert!(
            validate_delta(&d, &base_plan(), 1, &states, ProjectPlanLimits::default())
                .unwrap_err()
                .contains(&ProjectPlanDeltaError::ConflictingChange { key: "poc".into() })
        );

        // 終端のものは cancel できない。
        let mut st = BTreeMap::new();
        st.insert(
            "survey".to_string(),
            PlanNodeState {
                dispatched: true,
                terminal: true,
            },
        );
        let mut d = delta();
        d.cancel.push("survey".into());
        assert!(
            validate_delta(&d, &base_plan(), 1, &st, ProjectPlanLimits::default())
                .unwrap_err()
                .contains(&ProjectPlanDeltaError::CancelsFinishedMilestone {
                    key: "survey".into()
                })
        );
    }
}
