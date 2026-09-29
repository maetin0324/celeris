//! ADR-0072（Task execution decomposition）Phase E2: ExecutionPlan / WorkUnit のデータモデルと
//! 決定的な scheduler（D5・D14・D15）。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出しはしない。ADR-0001 D2）。永続化（`execution_plans`
//! / `work_units` / `runs` の 3 表。D5）は `task_core::store` が行う。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// D14: 計画 JSON の schema 版（`docs/protocol/execution-plan.schema.json`）。
pub const EXECUTION_PLAN_SCHEMA: &str = "celeris.execution-plan/1";

/// ADR-0074 D1.1（Phase F2）: `phases`（工程ごとの並列）を持つ計画の schema 版。v1 と同じ
/// `ExecutionPlanSpec` 型を使うが、`phases` が 1 つ以上、各 WorkUnit に `phase` が要る点が違う
/// （`validate` が schema の値で分ける。D1.1）。
pub const EXECUTION_PLAN_SCHEMA_V2: &str = "celeris.execution-plan/2";

/// ADR-0079 D2（Phase R1a）: 段階（`stages`）と unit（leaf | 子 task、`units`）と決定（`decisions`）を
/// 持つ計画の schema 版。`[execution.tree] enabled = true` のときだけ採用できる（`false` なら検証で
/// `TreeDisabled`）。内部では /2 の `phases` / `work_units` と同じ行に写す（[`internal_view`]）。
pub const EXECUTION_PLAN_SCHEMA_V3: &str = "celeris.execution-plan/3";

/// 工程（/2 の `phases`、/3 の `stages`）を持つ schema か（統合 WU・工程の障壁の対象）。
pub fn is_phased_schema(schema: &str) -> bool {
    schema == EXECUTION_PLAN_SCHEMA_V2 || schema == EXECUTION_PLAN_SCHEMA_V3
}

/// `execution_plans.id` / `work_units.id` に使う ULID の発行（`TaskId` 等と同じ ULID 系を使う。
/// 型付きの id にしていないのは、この 2 表が対応する Event の中に既に `plan_id` / `work_unit_id` が
/// 文字列で入っているため。I/O は無い純粋な採番）。
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

// ---------------------------------------------------------------------------
// D14: Planner が出す（または人が書く）計画の形
// ---------------------------------------------------------------------------

/// D14: WorkUnit の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitKind {
    Investigate,
    Design,
    Implement,
    Test,
    Release,
    Repair,
    Other,
    /// ADR-0074 D1.4（Phase F2）: 工程末尾の統合 WU（daemon が計画の採用時に足す system WU。
    /// `runs = 0`、LLM run を起こさない）。planner の出力に書かれていれば検証で拒否する
    /// （`validate` の `ReservedKind`。system WU 専用の予約語）。
    Integrate,
    /// ADR-0079 D2（Phase R1a）: 子 task の unit（`celeris.execution-plan/3` だけ。/1・/2 では検証で拒否）。
    /// `work_units` の行は子 task の代理で、LLM run を起こさない（子 task の生成は R1b）。
    Task,
}

impl WorkUnitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkUnitKind::Investigate => "investigate",
            WorkUnitKind::Design => "design",
            WorkUnitKind::Implement => "implement",
            WorkUnitKind::Test => "test",
            WorkUnitKind::Release => "release",
            WorkUnitKind::Repair => "repair",
            WorkUnitKind::Other => "other",
            WorkUnitKind::Integrate => "integrate",
            WorkUnitKind::Task => "task",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "investigate" => Some(WorkUnitKind::Investigate),
            "design" => Some(WorkUnitKind::Design),
            "implement" => Some(WorkUnitKind::Implement),
            "test" => Some(WorkUnitKind::Test),
            "release" => Some(WorkUnitKind::Release),
            "repair" => Some(WorkUnitKind::Repair),
            "other" => Some(WorkUnitKind::Other),
            "integrate" => Some(WorkUnitKind::Integrate),
            "task" => Some(WorkUnitKind::Task),
            _ => None,
        }
    }
}

/// D14: `checks` は決定的な検査だけ（`Command`。E4 で実行する。E2 は schema と検証のみ）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitCheck {
    pub cmd: String,
    #[serde(default)]
    pub expect_exit: i32,
}

/// D14: WU が読むべき context のヒント。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitContext {
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub from_work_units: Vec<String>,
    #[serde(default)]
    pub knowledge: Vec<String>,
}

/// D14/D18: WU ごとの予算（任意。書かなければ D18 の既定を使う）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_secs: Option<u64>,
}

/// D14: 計画の中の 1 WorkUnit の spec。**`assignee` / `tier` / `model` の欄は持たない**
/// （`deny_unknown_fields` により、書かれていれば schema 違反になる。D14）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitSpec {
    /// `[a-z0-9-]{1,32}`。計画の中で一意（D14）。
    pub key: String,
    pub kind: WorkUnitKind,
    pub title: String,
    pub objective: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub checks: Vec<WorkUnitCheck>,
    #[serde(default)]
    pub context: WorkUnitContext,
    /// `[[genres]]` にある id だけを許す（D14。検証は担当の profile を知る呼び出し側が行う。
    /// ここでは形だけ見る）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// D21: `TaskFeatures` の上書きヒント（任意の JSON。E3 以降の routing が読む）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<WorkUnitBudget>,
    #[serde(default)]
    pub outputs: Vec<String>,
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/2` では必須（`phases` にある key の
    /// いずれか）。`celeris.execution-plan/1` では無い（`Some` なら検証エラー。v1 の計画・
    /// プロンプトを 1 バイトも変えないため、この欄自体は常に存在するが v1 は `None` のまま）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

/// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/2` の工程。配列の順が実行順（D1.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseSpec {
    /// `[a-z0-9-]{1,32}`。計画の中で一意。
    pub key: String,
    pub kind: WorkUnitKind,
    pub title: String,
}

/// D14: Planner の出力（または人が `PUT`/`POST` で書く計画）そのもの。
///
/// ADR-0074 D1.1（Phase F2）: `schema` の値で v1 / v2 を分ける（`validate` が判定する）。
/// `phases` / `children` は v1 では常に空（`serde(default)` で省略も読める。v1 の JSON を
/// 1 バイトも変えないため、この 2 欄自体は型として増えるが v1 の出力・検証は変わらない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanSpec {
    pub schema: String,
    pub rationale: String,
    /// v2 のみ。v1 では空でなければならない（`validate`）。
    #[serde(default)]
    pub phases: Vec<PhaseSpec>,
    /// /1・/2 では 1 件以上。ADR-0079（Phase R1a）: /3 は `units` を使い、ここは空（省略可）。
    #[serde(default)]
    pub work_units: Vec<WorkUnitSpec>,
    /// ADR-0074 D3.7（Phase F4b (f)）: 子 Task の提案（v2 のみ。v1 では空でなければならない）。
    /// 採用と同じトランザクションで既存の委譲の検証を通して子 Task になり、WU は
    /// `depends_on: ["child:<key>"]` でその子の `done` を待てる。
    #[serde(default)]
    pub children: Vec<ExecutionChildSpec>,
    /// ADR-0079 D2（Phase R1a）: /3 の段階（/1・/2 では空。空なら出力しない〈/1・/2 の JSON は不変〉）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<StageSpec>,
    /// ADR-0079 D2: /3 の unit（leaf | 子 task）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub units: Vec<PlanUnitSpec>,
    /// ADR-0079 D2 / D7: /3 の決定（人に選んでもらう点）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<crate::decision::DecisionSpec>,
}

/// ADR-0079 D2 / D5: 段階の後の人の確認（`review: human` は ADR-0074 D2 の `pause_after` をその段階に
/// 指定したのと同じ意味。既定 none）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StageReview {
    #[default]
    None,
    Human,
}

impl StageReview {
    pub fn is_none(&self) -> bool {
        *self == StageReview::None
    }
}

/// ADR-0079 D2: /3 の段階（/2 の `PhaseSpec` に `review` を足した形）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StageSpec {
    /// `[a-z0-9-]{1,32}`。計画の中で一意。`work_units.phase` の値になる。
    pub key: String,
    pub kind: WorkUnitKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "StageReview::is_none")]
    pub review: StageReview,
}

/// ADR-0079 D4 (2): leaf の `context.repo`（1 つの文字列、または配列。leaf は高々 1 つ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RepoSelector {
    One(String),
    Many(Vec<String>),
}

impl RepoSelector {
    pub fn count(&self) -> usize {
        match self {
            RepoSelector::One(_) => 1,
            RepoSelector::Many(v) => v.len(),
        }
    }
}

/// ADR-0079 D2: /3 の unit の context（/1・/2 の `WorkUnitContext` に `repo` を足した形）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnitContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_work_units: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoSelector>,
}

impl UnitContext {
    pub fn is_empty(&self) -> bool {
        *self == UnitContext::default()
    }
}

/// ADR-0079 D2: /3 の unit。`kind = task` なら子 task（`acceptance` 必須、`checks` / `budget` /
/// `harness` / `context.paths` は持たない）、それ以外は leaf（今の WorkUnit の欄 + `needs_decisions`。
/// `checks` 1 本以上）。`assignee` / `tier` / `model` / `lane` は持たない（`deny_unknown_fields`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanUnitSpec {
    /// `[a-z0-9-]{1,32}`。計画の中で一意。
    pub key: String,
    /// `stages` の key。
    pub stage: String,
    pub kind: WorkUnitKind,
    pub title: String,
    pub objective: String,
    /// unit の key（leaf・task を問わない）。/2 の `child:<key>` は書けない。
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// 回答を待つ決定の key（計画の `decisions`・unit の `decisions`）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_decisions: Vec<String>,
    /// この unit が持ち込む決定（計画の `decisions` に `needed_before: [<この unit>]` を付けて移した
    /// ものとして読む。D2 の糖衣）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<crate::decision::DecisionSpec>,
    // ---- kind task だけ ----
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<crate::model::Criterion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// 親の repos の部分集合（repo の名前。子の生成〈R1b〉で検証する）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
    /// D15: 既存の task をこの unit の子として採用する（人の計画〈origin human〉だけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt: Option<crate::model::TaskId>,
    // ---- leaf だけ ----
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_when: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<WorkUnitCheck>,
    #[serde(default, skip_serializing_if = "UnitContext::is_empty")]
    pub context: UnitContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<WorkUnitBudget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    // ---- 両方 ----
    /// ADR-0069 D3: `TaskFeatureHints` の上書きヒント。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<serde_json::Value>,
}

impl PlanUnitSpec {
    /// kind task の unit（子 task）か。
    pub fn is_task(&self) -> bool {
        self.kind == WorkUnitKind::Task
    }

    /// ADR-0079 D15（Phase R5b-prep）: 新しい子 task を作る kind task の unit か（`adopt` の unit は既存の task を
    /// 結ぶだけで子を作らず run も費やさないので、計画あたりの子 task の上限〈`max_child_tasks_per_plan`〉に数えない）。
    pub fn creates_child(&self) -> bool {
        self.is_task() && self.adopt.is_none()
    }

    /// `work_units` の行の spec（/2 の `WorkUnitSpec` と同じ形。`phase` = 段階）。
    pub fn to_work_unit_spec(&self) -> WorkUnitSpec {
        WorkUnitSpec {
            key: self.key.clone(),
            kind: self.kind,
            title: self.title.clone(),
            objective: self.objective.clone(),
            depends_on: self.depends_on.clone(),
            done_when: self.done_when.clone(),
            checks: self.checks.clone(),
            context: WorkUnitContext {
                paths: self.context.paths.clone(),
                from_work_units: self.context.from_work_units.clone(),
                knowledge: self.context.knowledge.clone(),
            },
            harness: self.harness.clone(),
            features: self.features.clone(),
            budget: self.budget,
            outputs: self.outputs.clone(),
            phase: Some(self.stage.clone()),
        }
    }
}

/// ADR-0079 D2: /3 の計画を /2 の形（`phases` / `work_units`）に写す（DB の列は増やさず
/// `work_units.phase` に段階の key。統合 WU・工程の障壁・scheduler をそのまま使うため）。
/// /1・/2 はそのまま返す。
pub fn internal_view(spec: &ExecutionPlanSpec) -> std::borrow::Cow<'_, ExecutionPlanSpec> {
    if spec.schema != EXECUTION_PLAN_SCHEMA_V3 {
        return std::borrow::Cow::Borrowed(spec);
    }
    std::borrow::Cow::Owned(ExecutionPlanSpec {
        schema: spec.schema.clone(),
        rationale: spec.rationale.clone(),
        phases: spec
            .stages
            .iter()
            .map(|s| PhaseSpec {
                key: s.key.clone(),
                kind: s.kind,
                title: s.title.clone(),
            })
            .collect(),
        work_units: spec
            .units
            .iter()
            .map(PlanUnitSpec::to_work_unit_spec)
            .collect(),
        children: Vec::new(),
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
    })
}

/// ADR-0079 D2 / D7: 計画の決定を 1 列にする（計画の `decisions` の後に、unit の `decisions` を
/// `needed_before` にその unit を足して）。
pub fn normalized_decisions(spec: &ExecutionPlanSpec) -> Vec<crate::decision::DecisionSpec> {
    let mut out: Vec<crate::decision::DecisionSpec> = spec.decisions.clone();
    for u in &spec.units {
        for d in &u.decisions {
            let mut d = d.clone();
            if !d.needed_before.iter().any(|n| n == &u.key) {
                d.needed_before.push(u.key.clone());
            }
            out.push(d);
        }
    }
    out
}

/// ADR-0079 D7: unit が回答を待つ決定の key（`needs_decisions` と、`needed_before` がその unit か
/// その段階〈`stage:<key>`〉を指す決定。重複なし・昇順）。`work_units.needs_decisions_json` の値。
pub fn effective_needs_decisions(spec: &ExecutionPlanSpec, unit_key: &str) -> Vec<String> {
    let Some(unit) = spec.units.iter().find(|u| u.key == unit_key) else {
        return Vec::new();
    };
    let stage_ref = format!(
        "{}{}",
        crate::decision::NEEDED_BEFORE_STAGE_PREFIX,
        unit.stage
    );
    let mut keys: BTreeSet<String> = unit.needs_decisions.iter().cloned().collect();
    for d in normalized_decisions(spec) {
        if d.needed_before
            .iter()
            .any(|n| n == unit_key || *n == stage_ref)
        {
            keys.insert(d.key);
        }
    }
    keys.into_iter().collect()
}

/// ADR-0074 D3.7（Phase F4b (f)）: WU の `depends_on` で子 Task を指す接頭辞（`child:<key>`）。
pub const CHILD_DEP_PREFIX: &str = "child:";

/// ADR-0074 D3.7（Phase F4b (f)）: 子 Task の印（`Task.labels` に `child-<key>`）。scheduler が
/// `child:<key>` の依存を子 Task の状態へ決定的に引くのに使う（`Event::Created` に残るので正本は events）。
pub fn child_label(key: &str) -> String {
    format!("child-{key}")
}

/// ADR-0074 D3.7（Phase F4b (f)）: planner が提案する子 Task 1 件（`delegate` メッセージと同じ中身。
/// `assignee` / `tier` / `model` は持たない〈担当は matching が決める。ADR-0069 D1〉）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionChildSpec {
    /// `[a-z0-9-]{1,32}`。`children` の中で一意（WU の key とは別の名前空間。`child:<key>` で指す）。
    pub key: String,
    pub title: String,
    pub objective: String,
    /// 1 件以上。
    pub acceptance: Vec<crate::model::Criterion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    /// ADR-0069 D3: `TaskFeatureHints` の上書きヒント。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<crate::model_policy::TaskFeatureHints>,
    /// 同じ `children` の中の他の子の key。
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// 生成したスキーマ（`docs/protocol/execution-plan.schema.json`。`UPDATE_SCHEMA=1` で再生成）。
pub fn schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(ExecutionPlanSpec);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

// ---------------------------------------------------------------------------
// ADR-0074 D5.3（Phase F1）: replan の差分出力 `celeris.execution-plan-delta/1`
// ---------------------------------------------------------------------------

/// D5.3: 差分の schema 版（`docs/protocol/execution-plan-delta.schema.json`）。
pub const EXECUTION_PLAN_DELTA_SCHEMA: &str = "celeris.execution-plan-delta/1";

/// D5.3: 既存の WorkUnit の変える欄だけを書く（`key` は必須、他は書いた欄だけが上書きされる）。
/// 書かなかった欄は変わらない。**欄を「消す」ことはできない**（`harness`/`features`/`budget` を
/// 空に戻したいときは、値を書き換えるのではなく `remove` + `add` で作り直す。Phase F1 の簡略化。
/// `Option<Option<T>>` の二重オプションは素の serde では「省略」と「明示 null」を区別できないため）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitPatch {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkUnitKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<WorkUnitCheck>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<WorkUnitContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<WorkUnitBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<String>>,
}

/// D5.3: replan の差分出力。`base_version` は差分を当てる旧版（`ExecutionPlanRow.version`）。
/// `done` の WU・統合済みの工程は書かない（daemon が旧版から持ち越す）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlanDelta {
    pub schema: String,
    pub base_version: u32,
    pub rationale: String,
    #[serde(default)]
    pub add: Vec<WorkUnitSpec>,
    #[serde(default)]
    pub modify: Vec<WorkUnitPatch>,
    #[serde(default)]
    pub remove: Vec<String>,
}

/// 生成したスキーマ（`docs/protocol/execution-plan-delta.schema.json`）。
pub fn delta_schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(ExecutionPlanDelta);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// D5.3: 差分を旧版（`base`）に当てて新しい版の全体を作る（純粋関数。検証は呼び出し側が
/// `validate` で行う）。`remove` → `modify` → `add` の順に適用する。
///
/// - `remove` に無い key の WU は `base` のまま残る（`done` の WU を書かなくても持ち越される）。
/// - `modify` は既知の key だけに適用できる（`remove` された直後の key、または存在しない key を
///   指せば拒否する）。
/// - `add` の key が既存（`base` に残っているもの）と衝突すれば拒否する。
pub fn apply_delta(
    base: &ExecutionPlanSpec,
    delta: &ExecutionPlanDelta,
) -> Result<ExecutionPlanSpec, String> {
    let mut units: Vec<WorkUnitSpec> = base.work_units.clone();

    let remove_set: BTreeSet<&str> = delta.remove.iter().map(|s| s.as_str()).collect();
    units.retain(|w| !remove_set.contains(w.key.as_str()));

    for patch in &delta.modify {
        let Some(existing) = units.iter_mut().find(|w| w.key == patch.key) else {
            return Err(format!(
                "modify: work unit {:?} does not exist in the base plan (or was removed)",
                patch.key
            ));
        };
        if let Some(v) = patch.kind {
            existing.kind = v;
        }
        if let Some(v) = &patch.title {
            existing.title = v.clone();
        }
        if let Some(v) = &patch.objective {
            existing.objective = v.clone();
        }
        if let Some(v) = &patch.depends_on {
            existing.depends_on = v.clone();
        }
        if let Some(v) = &patch.done_when {
            existing.done_when = v.clone();
        }
        if let Some(v) = &patch.checks {
            existing.checks = v.clone();
        }
        if let Some(v) = &patch.context {
            existing.context = v.clone();
        }
        if let Some(v) = &patch.harness {
            existing.harness = Some(v.clone());
        }
        if let Some(v) = &patch.features {
            existing.features = Some(v.clone());
        }
        if let Some(v) = patch.budget {
            existing.budget = Some(v);
        }
        if let Some(v) = &patch.outputs {
            existing.outputs = v.clone();
        }
    }

    for spec in &delta.add {
        if units.iter().any(|w| w.key == spec.key) {
            return Err(format!(
                "add: work unit key {:?} already exists in the base plan",
                spec.key
            ));
        }
        units.push(spec.clone());
    }

    Ok(ExecutionPlanSpec {
        schema: base.schema.clone(),
        rationale: delta.rationale.clone(),
        work_units: units,
        // ADR-0074 D5.3 は work_units の差分だけを扱う（phases の再構成は扱わない）。F2 で
        // schema v2 が増えたので、差分は phases/children を base のまま持ち越す（replan で
        // 工程構成自体を作り直すことは無い。Phase F2 実装時の逸脱・明確化）。
        phases: base.phases.clone(),
        children: base.children.clone(),
        // ADR-0079（Phase R1a）: /3 の差分（段階・unit の replan）は R2b。ここでは持ち越すだけ。
        stages: base.stages.clone(),
        units: base.units.clone(),
        decisions: base.decisions.clone(),
    })
}

// ---------------------------------------------------------------------------
// D14: 検証
// ---------------------------------------------------------------------------

/// D18: 検証・丸めに使う上限（既定値は ADR-0072 D18 の表）。
/// ADR-0074 D5.3（Phase F1）: 計画のサイズ上限（§4）を足す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionLimits {
    pub max_work_units: usize,
    pub work_unit_max_turns: u32,
    pub work_unit_max_wall_secs: u64,
    /// `rationale` の文字数上限（既定 1,500）。
    pub max_rationale_chars: usize,
    /// WU の `title` の文字数上限（既定 120）。
    pub max_title_chars: usize,
    /// WU の `objective` の文字数上限（既定 2,000）。
    pub max_objective_chars: usize,
    /// WU の `done_when` の件数上限（既定 8）。
    pub max_done_when_items: usize,
    /// WU の `done_when` の 1 件あたりの文字数上限（既定 300）。
    pub max_done_when_chars: usize,
    /// WU の `checks` の件数上限（既定 6）。
    pub max_checks: usize,
    /// 計画の JSON 全体の大きさの上限（バイト、既定 24 KiB）。
    pub max_plan_json_bytes: usize,
    /// ADR-0074 §4（Phase F2）: `celeris.execution-plan/2` の `work_units` の件数上限（既定 10。
    /// 統合 WU・repair は数えない）。`max_work_units` は v1 専用のまま（既定 8。§4 の表）。
    pub max_work_units_v2: usize,
    /// ADR-0074 §4（Phase F2）: `phases` の件数上限（既定 5）。
    pub max_phases: usize,
    /// ADR-0074 D3.7（Phase F4b (f)）: `children` の件数上限（既定 8 = 委譲の 1 run あたりの上限）。
    pub max_children: usize,
    /// ADR-0079 D3（Phase R1a）: `[execution.tree]` の上限。`celeris.execution-plan/3` の検証だけが
    /// 見る（/1・/2 の検証・丸めは 1 バイトも変えない）。
    pub tree: crate::tree::TreeLimits,
}

impl ExecutionLimits {
    /// replay（採用済みの計画のトポロジカル順の復元）用: 形の検査だけをし、上限では拒否しない。
    pub fn permissive() -> Self {
        ExecutionLimits {
            max_children: 8,
            max_work_units: usize::MAX,
            work_unit_max_turns: u32::MAX,
            work_unit_max_wall_secs: u64::MAX,
            max_rationale_chars: usize::MAX,
            max_title_chars: usize::MAX,
            max_objective_chars: usize::MAX,
            max_done_when_items: usize::MAX,
            max_done_when_chars: usize::MAX,
            max_checks: usize::MAX,
            max_plan_json_bytes: usize::MAX,
            max_work_units_v2: usize::MAX,
            max_phases: usize::MAX,
            tree: crate::tree::TreeLimits::permissive(),
        }
    }
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        ExecutionLimits {
            max_work_units: 8,
            work_unit_max_turns: 80,
            work_unit_max_wall_secs: 3600,
            max_rationale_chars: 1_500,
            max_title_chars: 120,
            max_objective_chars: 2_000,
            max_done_when_items: 8,
            max_done_when_chars: 300,
            max_checks: 6,
            max_plan_json_bytes: 24 * 1024,
            max_work_units_v2: 10,
            max_phases: 5,
            max_children: 8,
            tree: crate::tree::TreeLimits::default(),
        }
    }
}

/// D14: 検証エラー（すべて拒否理由。1 回だけ再試行し、それでも駄目なら atomic に倒す。呼び出し側の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanValidationError {
    /// `schema` 欄が `celeris.execution-plan/1` でも `/2`（ADR-0074 D1.1、Phase F2）でもない。
    WrongSchema {
        found: String,
    },
    NoWorkUnits,
    TooManyWorkUnits {
        count: usize,
        max: usize,
    },
    InvalidKey {
        key: String,
    },
    DuplicateKey {
        key: String,
    },
    UnknownDependency {
        key: String,
        depends_on: String,
    },
    CyclicDependency {
        cycle: Vec<String>,
    },
    /// title の正規化一致、または objective のトークン Jaccard ≥ 0.9。
    DuplicateWorkUnit {
        a: String,
        b: String,
        reason: String,
    },
    /// replan（D14）: done の WU の key/spec が変わっている。E2 では replan を発行しないので、
    /// 呼び出し側が既存の done WU を渡したときだけ検査する。
    DoneWorkUnitChanged {
        key: String,
    },
    /// ADR-0079 R5b-fix1: 人の replan（origin human）は done の WU の spec を上書きできるが、構造の欄
    /// （`kind` / `phase`〈/3 の段階〉/ `depends_on`）は変えられない。`field` は変わった欄の名前。
    DoneWorkUnitStructureChanged {
        key: String,
        field: &'static str,
    },
    /// ADR-0074 D5.1（Phase F1）: WU の `features` が `TaskFeatureHints` として読めない
    /// （`deny_unknown_fields` を含む型として不正）。
    InvalidFeatures {
        key: String,
        detail: String,
    },
    /// ADR-0074 D5.3（Phase F1）: `rationale` が上限を超える。
    RationaleTooLong {
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `title` が上限を超える。
    TitleTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `objective` が上限を超える。
    ObjectiveTooLong {
        key: String,
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `done_when` の件数が上限を超える。
    TooManyDoneWhen {
        key: String,
        count: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `done_when` の 1 件が上限を超える。
    DoneWhenItemTooLong {
        key: String,
        index: usize,
        len: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: WU の `checks` の件数が上限を超える。
    TooManyChecks {
        key: String,
        count: usize,
        max: usize,
    },
    /// ADR-0074 D5.3（Phase F1）: 計画の JSON 全体が上限を超える。
    PlanTooLarge {
        bytes: usize,
        max: usize,
    },
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/1` に `phases` が書かれている
    /// （v1 は工程を持たない。1 バイトも変えない）。
    PhasesNotAllowedInV1,
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/2` に `phases` が 1 つも無い。
    NoPhases,
    /// ADR-0074 §4（Phase F2）: `phases` の件数が上限を超える。
    TooManyPhases {
        count: usize,
        max: usize,
    },
    /// ADR-0074 D1.1（Phase F2）: 工程の `key` が `[a-z0-9-]{1,32}` に合わない。
    InvalidPhaseKey {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: 工程の `key` が重複している。
    DuplicatePhaseKey {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/1` の WorkUnit に `phase` が書かれている
    /// （v1 は工程を持たない）。
    WorkUnitPhaseNotAllowedInV1 {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: `celeris.execution-plan/2` の WorkUnit に `phase` が無い（必須）。
    WorkUnitMissingPhase {
        key: String,
    },
    /// ADR-0074 D1.1（Phase F2）: WorkUnit の `phase` が `phases` に無い key を指している。
    UnknownWorkUnitPhase {
        key: String,
        phase: String,
    },
    /// ADR-0074 D1.1（Phase F2）: 依存先が自分より後の工程にある（拒否。D1.1 の規則 1）。
    DependencyInLaterPhase {
        key: String,
        depends_on: String,
    },
    /// ADR-0074 D1.1（Phase F2）: 同じ工程の中の依存が 2 つ以上ある（規則 2。鎖か木のみ許す）。
    TooManyIntraPhaseDependencies {
        key: String,
        phase: String,
    },
    /// ADR-0074 D3.7: `children` は `celeris.execution-plan/2` だけ（v1 では空でなければならない）。
    NonEmptyChildren,
    /// ADR-0074 D3.7（Phase F4b (f)）: 子の件数・key・依存・受け入れ条件。
    TooManyChildren {
        count: usize,
        max: usize,
    },
    InvalidChildKey {
        key: String,
    },
    DuplicateChildKey {
        key: String,
    },
    UnknownChildDependency {
        key: String,
        depends_on: String,
    },
    CyclicChildDependency {
        cycle: Vec<String>,
    },
    ChildNoAcceptance {
        key: String,
    },
    ChildInvalidAcceptance {
        key: String,
        detail: String,
    },
    /// ADR-0074 D1.4（Phase F2）: `kind = integrate` は daemon が足す system WU 専用の予約語で、
    /// 計画（planner・人）が自分の WorkUnit にこの kind を書くことはできない。
    ReservedKind {
        key: String,
    },
    /// ADR-0074 D1.4（Phase F2b）: `integrate-` で始まる key は daemon が足す統合 WU
    /// （`integrate-<phase>`）の予約語。
    ReservedKey {
        key: String,
    },
    // ---- ADR-0079（Phase R1a）: /1・/2 に /3 の欄・語彙が書かれている ----
    /// `stages` / `units` / `decisions` は /3 だけ。
    V3FieldNotAllowed {
        field: &'static str,
    },
    /// `kind = task` は /3 だけ。
    TaskKindRequiresV3 {
        key: String,
    },
    // ---- ADR-0079 D2 / D3 / D4（Phase R1a）: /3 の検証 ----
    /// `[execution.tree] enabled = false` なので /3 は採用できない。
    TreeDisabled,
    /// /3 に /2 の欄（`phases` / `work_units` / `children`）が書かれている。
    V2FieldInV3 {
        field: &'static str,
    },
    NoStages,
    TooManyStages {
        count: usize,
        max: usize,
    },
    InvalidStageKey {
        key: String,
    },
    DuplicateStageKey {
        key: String,
    },
    /// 段階の kind に `task` / `integrate` は使えない。
    InvalidStageKind {
        key: String,
    },
    NoUnits,
    UnknownUnitStage {
        key: String,
        stage: String,
    },
    TooManyUnitsInStage {
        stage: String,
        count: usize,
        max: usize,
    },
    TooManyChildTasks {
        count: usize,
        max: usize,
    },
    /// この深さの task の計画は kind task の unit を持てない（U-R1: `depth < max_depth` のときだけ）。
    ChildTaskTooDeep {
        key: String,
        depth: u32,
        max_depth: u32,
    },
    /// /2 の `child:<key>` は /3 では書けない（kind task の unit を依存先にする）。
    ChildDependencyNotAllowed {
        key: String,
        depends_on: String,
    },
    TaskUnitNoAcceptance {
        key: String,
    },
    TaskUnitInvalidAcceptance {
        key: String,
        detail: String,
    },
    /// kind task の unit に `checks` / `budget` / `harness` / `context.paths` が書かれている。
    TaskUnitFieldNotAllowed {
        key: String,
        field: &'static str,
    },
    /// leaf に kind task 専用の欄（`acceptance` / `genre` / `skills` / `repos` / `adopt`）が書かれている。
    LeafFieldNotAllowed {
        key: String,
        field: &'static str,
    },
    /// D4 (2) (c): leaf に機械的な検査が無い。
    LeafWithoutChecks {
        key: String,
    },
    /// D4 (2) (b): leaf の `context.repo` が 2 つ以上。
    LeafMultipleRepos {
        key: String,
        count: usize,
    },
    /// D4 (2) (a): leaf の予算が 1 run の上限を超える（/3 は丸めずに拒否する）。
    LeafBudgetOverLimit {
        key: String,
        detail: String,
    },
    /// D2 / D15: `adopt` は人の計画（origin human）だけ。
    AdoptNotAllowed {
        key: String,
    },
    /// D7: 決定の形の誤り。
    InvalidDecision {
        detail: String,
    },
    DuplicateDecisionKey {
        key: String,
    },
    /// D7: `needed_before` が計画に無い unit / 段階を指している。
    UnknownNeededBefore {
        key: String,
        target: String,
    },
    /// D2: `needs_decisions` が計画に無い決定を指している。
    UnknownNeedsDecision {
        key: String,
        decision: String,
    },
    /// D3 / D7: 計画あたりの未回答の決定の上限。
    TooManyDecisions {
        count: usize,
        max: usize,
    },
}

impl std::fmt::Display for PlanValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanValidationError::WrongSchema { found } => {
                write!(
                    f,
                    "schema must be {EXECUTION_PLAN_SCHEMA}, {EXECUTION_PLAN_SCHEMA_V2} or {EXECUTION_PLAN_SCHEMA_V3}, found {found}"
                )
            }
            PlanValidationError::NoWorkUnits => write!(f, "work_units must not be empty"),
            PlanValidationError::TooManyWorkUnits { count, max } => {
                write!(f, "too many work_units: {count} > {max}")
            }
            PlanValidationError::InvalidKey { key } => {
                write!(
                    f,
                    "invalid work unit key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            PlanValidationError::DuplicateKey { key } => {
                write!(f, "duplicate work unit key: {key}")
            }
            PlanValidationError::UnknownDependency { key, depends_on } => {
                write!(f, "work unit {key} depends on unknown key {depends_on}")
            }
            PlanValidationError::CyclicDependency { cycle } => {
                write!(f, "cyclic dependency: {}", cycle.join(" -> "))
            }
            PlanValidationError::DuplicateWorkUnit { a, b, reason } => {
                write!(f, "work units {a} and {b} look like duplicates ({reason})")
            }
            PlanValidationError::DoneWorkUnitChanged { key } => {
                write!(
                    f,
                    "done work unit {key} must not change on replan（done の WU は差分に書かない・全体形式なら旧版のまま写す。{DAEMON_ADDED_HINT}。done の WU の spec〈例: check のコマンド〉の誤りを直す必要があるなら、planner は直さずに質問で人に伝える: 人は `PUT /tasks/{{id}}/execution-plan`〈origin human の replan〉で done の WU の spec を上書きできる〈ADR-0079 R5b-fix1〉）"
                )
            }
            PlanValidationError::DoneWorkUnitStructureChanged { key, field } => {
                write!(
                    f,
                    "done work unit {key}: {field} must not change on replan (a human replan may override the spec of a done work unit, but not its kind / phase / stage / depends_on, and may not remove it; ADR-0079 R5b-fix1)"
                )
            }
            PlanValidationError::InvalidFeatures { key, detail } => {
                write!(
                    f,
                    "work unit {key}: features must parse as TaskFeatureHints: {detail}"
                )
            }
            PlanValidationError::RationaleTooLong { len, max } => {
                write!(f, "rationale is too long: {len} > {max} characters")
            }
            PlanValidationError::TitleTooLong { key, len, max } => {
                write!(
                    f,
                    "work unit {key}: title is too long: {len} > {max} characters"
                )
            }
            PlanValidationError::ObjectiveTooLong { key, len, max } => {
                write!(
                    f,
                    "work unit {key}: objective is too long: {len} > {max} characters"
                )
            }
            PlanValidationError::TooManyDoneWhen { key, count, max } => {
                write!(
                    f,
                    "work unit {key}: too many done_when items: {count} > {max}"
                )
            }
            PlanValidationError::DoneWhenItemTooLong {
                key,
                index,
                len,
                max,
            } => {
                write!(
                    f,
                    "work unit {key}: done_when[{index}] is too long: {len} > {max} characters"
                )
            }
            PlanValidationError::TooManyChecks { key, count, max } => {
                write!(f, "work unit {key}: too many checks: {count} > {max}")
            }
            PlanValidationError::PlanTooLarge { bytes, max } => {
                write!(f, "execution plan JSON is too large: {bytes} > {max} bytes")
            }
            PlanValidationError::PhasesNotAllowedInV1 => {
                write!(f, "phases must be empty in {EXECUTION_PLAN_SCHEMA}")
            }
            PlanValidationError::NoPhases => {
                write!(f, "phases must not be empty in {EXECUTION_PLAN_SCHEMA_V2}")
            }
            PlanValidationError::TooManyPhases { count, max } => {
                write!(f, "too many phases: {count} > {max}")
            }
            PlanValidationError::InvalidPhaseKey { key } => {
                write!(
                    f,
                    "invalid phase key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            PlanValidationError::DuplicatePhaseKey { key } => {
                write!(f, "duplicate phase key: {key}")
            }
            PlanValidationError::WorkUnitPhaseNotAllowedInV1 { key } => {
                write!(
                    f,
                    "work unit {key}: phase must not be set in {EXECUTION_PLAN_SCHEMA}"
                )
            }
            PlanValidationError::WorkUnitMissingPhase { key } => {
                write!(
                    f,
                    "work unit {key}: phase is required in {EXECUTION_PLAN_SCHEMA_V2}"
                )
            }
            PlanValidationError::UnknownWorkUnitPhase { key, phase } => {
                write!(f, "work unit {key}: unknown phase {phase:?}")
            }
            PlanValidationError::DependencyInLaterPhase { key, depends_on } => {
                write!(
                    f,
                    "work unit {key}: depends on {depends_on}, which is in a later phase"
                )
            }
            PlanValidationError::TooManyIntraPhaseDependencies { key, phase } => {
                write!(
                    f,
                    "work unit {key}: depends on more than one work unit within phase {phase} \
                     (at most one intra-phase dependency is allowed)"
                )
            }
            PlanValidationError::NonEmptyChildren => {
                write!(
                    f,
                    "children are only allowed in {EXECUTION_PLAN_SCHEMA_V2} (must be empty in v1)"
                )
            }
            PlanValidationError::TooManyChildren { count, max } => {
                write!(f, "too many children: {count} > {max}")
            }
            PlanValidationError::InvalidChildKey { key } => {
                write!(
                    f,
                    "invalid child key: {key:?} (must match [a-z0-9-]{{1,32}})"
                )
            }
            PlanValidationError::DuplicateChildKey { key } => {
                write!(f, "duplicate child key: {key}")
            }
            PlanValidationError::UnknownChildDependency { key, depends_on } => {
                write!(f, "child {key} depends on unknown child {depends_on}")
            }
            PlanValidationError::CyclicChildDependency { cycle } => {
                write!(f, "cyclic child dependency: {}", cycle.join(" -> "))
            }
            PlanValidationError::ChildNoAcceptance { key } => {
                write!(f, "child {key}: acceptance must not be empty")
            }
            PlanValidationError::ChildInvalidAcceptance { key, detail } => {
                write!(f, "child {key}: {detail}")
            }
            PlanValidationError::ReservedKind { key } => {
                write!(
                    f,
                    "work unit {key}: kind \"integrate\" is reserved for daemon-created integration work units（{DAEMON_ADDED_HINT}）"
                )
            }
            PlanValidationError::ReservedKey { key } => {
                write!(
                    f,
                    "work unit {key}: keys starting with \"{INTEGRATE_KEY_PREFIX}\" are reserved for daemon-created integration work units（{DAEMON_ADDED_HINT}）"
                )
            }
            PlanValidationError::V3FieldNotAllowed { field } => write!(
                f,
                "{field} is only allowed in {EXECUTION_PLAN_SCHEMA_V3} (must be empty in v1/v2)"
            ),
            PlanValidationError::TaskKindRequiresV3 { key } => write!(
                f,
                "work unit {key}: kind \"task\" (a child task unit) is only allowed in {EXECUTION_PLAN_SCHEMA_V3}"
            ),
            PlanValidationError::TreeDisabled => write!(
                f,
                "{EXECUTION_PLAN_SCHEMA_V3} (recursive task decomposition, ADR-0079) is disabled: \
                 set [execution.tree] enabled = true to adopt it, or write a {EXECUTION_PLAN_SCHEMA_V2} plan"
            ),
            PlanValidationError::V2FieldInV3 { field } => {
                let hint = match *field {
                    "children" => "use units with kind \"task\" instead",
                    "phases" => "use stages instead",
                    _ => "use units instead",
                };
                write!(
                    f,
                    "{field} is not allowed in {EXECUTION_PLAN_SCHEMA_V3} ({hint})"
                )
            }
            PlanValidationError::NoStages => {
                write!(f, "stages must not be empty in {EXECUTION_PLAN_SCHEMA_V3}")
            }
            PlanValidationError::TooManyStages { count, max } => {
                write!(f, "too many stages: {count} > {max}")
            }
            PlanValidationError::InvalidStageKey { key } => write!(
                f,
                "invalid stage key: {key:?} (must match [a-z0-9-]{{1,32}})"
            ),
            PlanValidationError::DuplicateStageKey { key } => {
                write!(f, "duplicate stage key: {key}")
            }
            PlanValidationError::InvalidStageKind { key } => write!(
                f,
                "stage {key}: kind \"task\" / \"integrate\" cannot be used for a stage"
            ),
            PlanValidationError::NoUnits => {
                write!(f, "units must not be empty in {EXECUTION_PLAN_SCHEMA_V3}")
            }
            PlanValidationError::UnknownUnitStage { key, stage } => {
                write!(f, "unit {key}: unknown stage {stage:?}")
            }
            PlanValidationError::TooManyUnitsInStage { stage, count, max } => write!(
                f,
                "stage {stage}: too many units: {count} > {max} (leaf + task; split the stage or group units into a child task)"
            ),
            PlanValidationError::TooManyChildTasks { count, max } => {
                write!(f, "too many units with kind \"task\": {count} > {max}")
            }
            PlanValidationError::ChildTaskTooDeep {
                key,
                depth,
                max_depth,
            } => write!(
                f,
                "unit {key}: a task at depth {depth} cannot have child task units (max_depth = {max_depth} task levels); make it a leaf"
            ),
            PlanValidationError::ChildDependencyNotAllowed { key, depends_on } => write!(
                f,
                "unit {key}: depends_on {depends_on:?} uses the {EXECUTION_PLAN_SCHEMA_V2} \"{CHILD_DEP_PREFIX}\" prefix; depend on the key of a unit with kind \"task\" instead"
            ),
            PlanValidationError::TaskUnitNoAcceptance { key } => write!(
                f,
                "unit {key}: a unit with kind \"task\" must have at least one acceptance criterion"
            ),
            PlanValidationError::TaskUnitInvalidAcceptance { key, detail } => {
                write!(f, "unit {key}: {detail}")
            }
            PlanValidationError::TaskUnitFieldNotAllowed { key, field } => write!(
                f,
                "unit {key}: a unit with kind \"task\" must not set {field} (the child task decides it)"
            ),
            PlanValidationError::LeafFieldNotAllowed { key, field } => write!(
                f,
                "unit {key}: {field} is only allowed on a unit with kind \"task\""
            ),
            PlanValidationError::LeafWithoutChecks { key } => write!(
                f,
                "unit {key}: a leaf must have at least one mechanical check (make it a unit with kind \"task\" or add checks)"
            ),
            PlanValidationError::LeafMultipleRepos { key, count } => write!(
                f,
                "unit {key}: a leaf works in at most one repository, found {count} in context.repo (make it a unit with kind \"task\" or split it)"
            ),
            PlanValidationError::LeafBudgetOverLimit { key, detail } => write!(
                f,
                "unit {key}: {detail} exceeds what one run can do (make it a unit with kind \"task\" or shrink it)"
            ),
            PlanValidationError::AdoptNotAllowed { key } => write!(
                f,
                "unit {key}: adopt is only allowed in a plan written by a human (origin human) (a done unit copied verbatim from the previous version may keep its adopt)"
            ),
            PlanValidationError::InvalidDecision { detail } => write!(f, "{detail}"),
            PlanValidationError::DuplicateDecisionKey { key } => {
                write!(f, "duplicate decision key: {key}")
            }
            PlanValidationError::UnknownNeededBefore { key, target } => write!(
                f,
                "decision {key}: needed_before {target:?} is neither a unit key nor stage:<key> of this plan"
            ),
            PlanValidationError::UnknownNeedsDecision { key, decision } => {
                write!(
                    f,
                    "unit {key}: needs_decisions names unknown decision {decision}"
                )
            }
            PlanValidationError::TooManyDecisions { count, max } => {
                write!(f, "too many decisions in one plan: {count} > {max}")
            }
        }
    }
}

impl std::error::Error for PlanValidationError {}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn normalize_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn token_set(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// D14: 検証を通った計画（budget を D18 の上限に丸めた写しと、丸めた記録）。
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedPlan {
    pub spec: ExecutionPlanSpec,
    /// 丸めたことの記録（`work unit <key>: max_turns 120 -> 80` のような 1 行ずつ）。
    pub rounding_notes: Vec<String>,
    /// トポロジカル順（`work_units` の index）。`work_units.seq` に使う。
    pub topological_order: Vec<usize>,
}

/// ADR-0079 R5b-fix1: /3 の unit が、前の版の done の unit をそのまま写したものか（内部の形
/// 〈[`PlanUnitSpec::to_work_unit_spec`]〉が done の spec と一致する）。
fn is_done_carry_over(unit: &PlanUnitSpec, done_work_units: &[(String, WorkUnitSpec)]) -> bool {
    done_work_units
        .iter()
        .any(|(k, s)| *k == unit.key && *s == unit.to_work_unit_spec())
}

/// replan の done の不変条件（D14 / D17）。done の WU は新しい計画に同じ key で残り、spec も変わらない。
///
/// ADR-0079 R5b-fix1: 人の replan（`origin == Human`）だけは done の WU の spec を上書きできる（人がその仕事は
/// 済んだと言い、記録した spec〈典型的には `checks` のコマンド〉を直す）。ただし消すこと（`DoneWorkUnitChanged`）と、
/// 構造の欄（`kind` / `phase` = /3 の段階 / `depends_on`）を変えること（`DoneWorkUnitStructureChanged`）は人でも拒む。
/// planner / repair / fixture の計画は従来どおり完全一致だけ。
fn done_carry_over_errors(
    work_units: &[WorkUnitSpec],
    done_work_units: &[(String, WorkUnitSpec)],
    origin: PlanOrigin,
) -> Vec<PlanValidationError> {
    let by_key: BTreeMap<&str, &WorkUnitSpec> =
        work_units.iter().map(|w| (w.key.as_str(), w)).collect();
    let mut errors = Vec::new();
    for (key, done_spec) in done_work_units {
        let Some(new_spec) = by_key.get(key.as_str()) else {
            errors.push(PlanValidationError::DoneWorkUnitChanged { key: key.clone() });
            continue;
        };
        if *new_spec == done_spec {
            continue;
        }
        if origin != PlanOrigin::Human {
            errors.push(PlanValidationError::DoneWorkUnitChanged { key: key.clone() });
            continue;
        }
        for (field, same) in [
            ("kind", new_spec.kind == done_spec.kind),
            ("phase", new_spec.phase == done_spec.phase),
            ("depends_on", new_spec.depends_on == done_spec.depends_on),
        ] {
            if !same {
                errors.push(PlanValidationError::DoneWorkUnitStructureChanged {
                    key: key.clone(),
                    field,
                });
            }
        }
    }
    errors
}

/// ADR-0079 R5b-fix1: 検証を通った計画（`spec`。/3 は内部の形に写して比べる）で、spec が done の spec から
/// 変わった done の WU（人の replan の上書き）と、変わった欄の名前（`WorkUnitSpec` の JSON の最上位の key、
/// 昇順）。検証が planner の計画にこれを許さないので、planner の replan では常に空。
pub fn done_work_unit_overrides(
    spec: &ExecutionPlanSpec,
    done_work_units: &[(String, WorkUnitSpec)],
) -> Vec<(String, WorkUnitSpec, Vec<String>)> {
    let internal = internal_view(spec);
    let mut out = Vec::new();
    for (key, done_spec) in done_work_units {
        let Some(new_spec) = internal.work_units.iter().find(|w| &w.key == key) else {
            continue;
        };
        if new_spec == done_spec {
            continue;
        }
        let as_map = |w: &WorkUnitSpec| match serde_json::to_value(w) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        let (old, new) = (as_map(done_spec), as_map(new_spec));
        let fields: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
        let changed: Vec<String> = fields
            .into_iter()
            .filter(|f| old.get(*f) != new.get(*f))
            .cloned()
            .collect();
        out.push((key.clone(), new_spec.clone(), changed));
    }
    out
}

/// D14: 計画を検証する。`done_work_units`（replan で持ち越す既存の done WU の `(key, spec)`）が
/// 空でなければ、それらの key と spec が新しい計画でも変わっていないことを確かめる（E2 では常に空。
/// replan は E4）。
pub fn validate(
    spec: &ExecutionPlanSpec,
    limits: ExecutionLimits,
    done_work_units: &[(String, WorkUnitSpec)],
) -> Result<ValidatedPlan, Vec<PlanValidationError>> {
    validate_with(spec, limits, done_work_units, PlanContext::default())
}

/// ADR-0079（Phase R1a）: 計画を書いた者と、計画を持つ task の木の中の深さ。/3 の検証だけが見る
/// （`adopt` は origin human だけ、kind task の unit は `depth < max_depth` のときだけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanContext {
    pub origin: PlanOrigin,
    /// task の層数で数えた深さ（root = 1。`task_core::tree::depth_of`）。
    pub depth: u32,
}

impl Default for PlanContext {
    fn default() -> Self {
        PlanContext {
            origin: PlanOrigin::Planner,
            depth: 1,
        }
    }
}

/// [`validate`] に計画の出どころと深さを添えたもの。/1・/2 は done の不変条件でだけ `ctx.origin` を見る
/// （ADR-0079 R5b-fix1: 人の replan は done の WU の spec を上書きできる。done が空なら [`validate`] と同じ結果）。
pub fn validate_with(
    spec: &ExecutionPlanSpec,
    limits: ExecutionLimits,
    done_work_units: &[(String, WorkUnitSpec)],
    ctx: PlanContext,
) -> Result<ValidatedPlan, Vec<PlanValidationError>> {
    if spec.schema == EXECUTION_PLAN_SCHEMA_V3 {
        return validate_v3(spec, limits, done_work_units, ctx);
    }
    let mut errors = Vec::new();

    // ADR-0074 D1.1（Phase F2）: `schema` の値で v1 / v2 を分ける。どちらでもなければ
    // `WrongSchema` を出すが、それ以外の検査（v1 の形として）は続けて行う（複数のエラーを
    // 一度に返す既存の流儀。E2 のまま）。
    let is_v2 = spec.schema == EXECUTION_PLAN_SCHEMA_V2;
    if spec.schema != EXECUTION_PLAN_SCHEMA && !is_v2 {
        errors.push(PlanValidationError::WrongSchema {
            found: spec.schema.clone(),
        });
    }
    if spec.work_units.is_empty() {
        errors.push(PlanValidationError::NoWorkUnits);
    }
    let max_work_units = if is_v2 {
        limits.max_work_units_v2
    } else {
        limits.max_work_units
    };
    if spec.work_units.len() > max_work_units {
        errors.push(PlanValidationError::TooManyWorkUnits {
            count: spec.work_units.len(),
            max: max_work_units,
        });
    }

    // ADR-0074 D3.7（Phase F4b (f)）: `children` は v2 だけ。件数・key・子同士の依存（循環なし）・
    // 受け入れ条件（1 件以上、human には deliverable）。
    if !is_v2 && !spec.children.is_empty() {
        errors.push(PlanValidationError::NonEmptyChildren);
    }
    let child_keys: BTreeSet<&str> = spec.children.iter().map(|c| c.key.as_str()).collect();
    if is_v2 {
        errors.extend(validate_children(&spec.children, limits));
    }

    // ADR-0074 D1.1（Phase F2）: `phases`（v2 のみ）。
    if is_v2 {
        if spec.phases.is_empty() {
            errors.push(PlanValidationError::NoPhases);
        }
        if spec.phases.len() > limits.max_phases {
            errors.push(PlanValidationError::TooManyPhases {
                count: spec.phases.len(),
                max: limits.max_phases,
            });
        }
        let mut seen_phase_keys: BTreeSet<&str> = BTreeSet::new();
        for p in &spec.phases {
            if !valid_key(&p.key) {
                errors.push(PlanValidationError::InvalidPhaseKey { key: p.key.clone() });
                continue;
            }
            if !seen_phase_keys.insert(p.key.as_str()) {
                errors.push(PlanValidationError::DuplicatePhaseKey { key: p.key.clone() });
            }
        }
    } else if !spec.phases.is_empty() {
        errors.push(PlanValidationError::PhasesNotAllowedInV1);
    }

    // ADR-0074 D1.4（Phase F2）: `kind = integrate` は system WU 専用の予約語。
    for wu in &spec.work_units {
        if wu.kind == WorkUnitKind::Integrate {
            errors.push(PlanValidationError::ReservedKind {
                key: wu.key.clone(),
            });
        }
        // ADR-0074 D1.4（Phase F2b）: `integrate-<phase>` の key も予約（v2 のみ。v1 は不変）。
        if spec.schema == EXECUTION_PLAN_SCHEMA_V2 && wu.key.starts_with(INTEGRATE_KEY_PREFIX) {
            errors.push(PlanValidationError::ReservedKey {
                key: wu.key.clone(),
            });
        }
        // ADR-0079（Phase R1a）: `kind = task`（子 task の unit）は /3 だけ。
        if wu.kind == WorkUnitKind::Task {
            errors.push(PlanValidationError::TaskKindRequiresV3 {
                key: wu.key.clone(),
            });
        }
    }
    // ADR-0079（Phase R1a）: /3 の欄は /1・/2 では空（`serde(default)` で読めるが採用しない）。
    for (field, present) in [
        ("stages", !spec.stages.is_empty()),
        ("units", !spec.units.is_empty()),
        ("decisions", !spec.decisions.is_empty()),
    ] {
        if present {
            errors.push(PlanValidationError::V3FieldNotAllowed { field });
        }
    }

    // ADR-0074 D1.1（Phase F2）: WorkUnit の `phase` は v2 で必須・既知の工程を指す、v1 で禁止。
    let phase_index: BTreeMap<&str, usize> = spec
        .phases
        .iter()
        .enumerate()
        .map(|(i, p)| (p.key.as_str(), i))
        .collect();
    for wu in &spec.work_units {
        if is_v2 {
            match &wu.phase {
                None => errors.push(PlanValidationError::WorkUnitMissingPhase {
                    key: wu.key.clone(),
                }),
                Some(phase) if !phase_index.contains_key(phase.as_str()) => {
                    errors.push(PlanValidationError::UnknownWorkUnitPhase {
                        key: wu.key.clone(),
                        phase: phase.clone(),
                    });
                }
                Some(_) => {}
            }
        } else if wu.phase.is_some() {
            errors.push(PlanValidationError::WorkUnitPhaseNotAllowedInV1 {
                key: wu.key.clone(),
            });
        }
    }

    let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
    for wu in &spec.work_units {
        if !valid_key(&wu.key) {
            errors.push(PlanValidationError::InvalidKey {
                key: wu.key.clone(),
            });
            continue;
        }
        if !seen_keys.insert(wu.key.as_str()) {
            errors.push(PlanValidationError::DuplicateKey {
                key: wu.key.clone(),
            });
        }
    }

    let known_keys: BTreeSet<&str> = spec.work_units.iter().map(|w| w.key.as_str()).collect();
    for wu in &spec.work_units {
        for dep in &wu.depends_on {
            // ADR-0074 D3.7（Phase F4b (f)）: `child:<key>` は子 Task への依存（v2 の既知の子だけ）。
            if let Some(child) = dep.strip_prefix(CHILD_DEP_PREFIX) {
                if !is_v2 || !child_keys.contains(child) {
                    errors.push(PlanValidationError::UnknownDependency {
                        key: wu.key.clone(),
                        depends_on: dep.clone(),
                    });
                }
                continue;
            }
            if !known_keys.contains(dep.as_str()) {
                errors.push(PlanValidationError::UnknownDependency {
                    key: wu.key.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
    }

    // ADR-0074 D1.1（Phase F2）: v2 の依存の規則 1・2（既知の key・既知の phase を持つ WU 同士に
    // 限る。上の 2 つの検査で既にエラーが出ている key/phase は静かに飛ばす。二重にエラーを
    // 積まないため）。
    if is_v2 {
        let wu_phase_of: BTreeMap<&str, usize> = spec
            .work_units
            .iter()
            .filter_map(|w| {
                w.phase
                    .as_deref()
                    .and_then(|p| phase_index.get(p))
                    .map(|&idx| (w.key.as_str(), idx))
            })
            .collect();
        for wu in &spec.work_units {
            let Some(&own_phase_idx) = wu_phase_of.get(wu.key.as_str()) else {
                continue;
            };
            let mut intra_phase_deps = 0usize;
            for dep in &wu.depends_on {
                let Some(&dep_phase_idx) = wu_phase_of.get(dep.as_str()) else {
                    continue;
                };
                match dep_phase_idx.cmp(&own_phase_idx) {
                    std::cmp::Ordering::Greater => {
                        errors.push(PlanValidationError::DependencyInLaterPhase {
                            key: wu.key.clone(),
                            depends_on: dep.clone(),
                        });
                    }
                    std::cmp::Ordering::Equal => intra_phase_deps += 1,
                    std::cmp::Ordering::Less => {}
                }
            }
            if intra_phase_deps > 1 {
                errors.push(PlanValidationError::TooManyIntraPhaseDependencies {
                    key: wu.key.clone(),
                    phase: wu.phase.clone().unwrap_or_default(),
                });
            }
        }
    }

    // ADR-0074 D5.1（Phase F1）: `features` は書かれていれば `TaskFeatureHints` として読めなければ
    // ならない（黙って捨てない）。5 軸のうちいくつか欠けているだけなら拒否しない
    // （`decide_for_work_unit` が Task から推定した値のまま補う）。
    for wu in &spec.work_units {
        if let Some(features) = &wu.features
            && let Err(e) =
                serde_json::from_value::<crate::model_policy::TaskFeatureHints>(features.clone())
        {
            errors.push(PlanValidationError::InvalidFeatures {
                key: wu.key.clone(),
                detail: e.to_string(),
            });
        }
    }

    // ADR-0074 D5.3（Phase F1）: 計画のサイズ上限（§4）。
    if spec.rationale.chars().count() > limits.max_rationale_chars {
        errors.push(PlanValidationError::RationaleTooLong {
            len: spec.rationale.chars().count(),
            max: limits.max_rationale_chars,
        });
    }
    for wu in &spec.work_units {
        let title_len = wu.title.chars().count();
        if title_len > limits.max_title_chars {
            errors.push(PlanValidationError::TitleTooLong {
                key: wu.key.clone(),
                len: title_len,
                max: limits.max_title_chars,
            });
        }
        let objective_len = wu.objective.chars().count();
        if objective_len > limits.max_objective_chars {
            errors.push(PlanValidationError::ObjectiveTooLong {
                key: wu.key.clone(),
                len: objective_len,
                max: limits.max_objective_chars,
            });
        }
        if wu.done_when.len() > limits.max_done_when_items {
            errors.push(PlanValidationError::TooManyDoneWhen {
                key: wu.key.clone(),
                count: wu.done_when.len(),
                max: limits.max_done_when_items,
            });
        }
        for (index, item) in wu.done_when.iter().enumerate() {
            let len = item.chars().count();
            if len > limits.max_done_when_chars {
                errors.push(PlanValidationError::DoneWhenItemTooLong {
                    key: wu.key.clone(),
                    index,
                    len,
                    max: limits.max_done_when_chars,
                });
            }
        }
        if wu.checks.len() > limits.max_checks {
            errors.push(PlanValidationError::TooManyChecks {
                key: wu.key.clone(),
                count: wu.checks.len(),
                max: limits.max_checks,
            });
        }
    }
    let plan_bytes = serde_json::to_vec(spec).map(|v| v.len()).unwrap_or(0);
    if plan_bytes > limits.max_plan_json_bytes {
        errors.push(PlanValidationError::PlanTooLarge {
            bytes: plan_bytes,
            max: limits.max_plan_json_bytes,
        });
    }

    // トポロジカルソート（循環の検出も兼ねる。Kahn's algorithm、決定的に `(phase, key)` 昇順で
    // tie-break。D1.1）。
    let mut topological_order = Vec::new();
    if errors.is_empty() {
        match topo_sort(&spec.work_units, &phase_index) {
            Ok(order) => topological_order = order,
            Err(cycle) => errors.push(PlanValidationError::CyclicDependency { cycle }),
        }
    }

    // 重複の検出: title を正規化して一致、または objective のトークン Jaccard >= 0.9。
    for i in 0..spec.work_units.len() {
        for j in (i + 1)..spec.work_units.len() {
            let a = &spec.work_units[i];
            let b = &spec.work_units[j];
            if normalize_title(&a.title) == normalize_title(&b.title) && !a.title.is_empty() {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: "identical normalized title".to_string(),
                });
                continue;
            }
            let sim = jaccard(&token_set(&a.objective), &token_set(&b.objective));
            if sim >= 0.9 {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: format!("objective token overlap {sim:.2}"),
                });
            }
        }
    }

    // replan の不変条件: done の WU の key/spec は変わらない（ADR-0079 R5b-fix1: 人の replan は spec の
    // 上書きだけを許す。[`done_carry_over_errors`]）。
    errors.extend(done_carry_over_errors(
        &spec.work_units,
        done_work_units,
        ctx.origin,
    ));

    if !errors.is_empty() {
        return Err(errors);
    }

    // D18: budget を丸める（丸めたことを記録する）。
    let mut rounded = spec.clone();
    let mut rounding_notes = Vec::new();
    for wu in &mut rounded.work_units {
        if let Some(budget) = &mut wu.budget {
            if let Some(turns) = budget.max_turns
                && turns > limits.work_unit_max_turns
            {
                rounding_notes.push(format!(
                    "work unit {}: max_turns {} -> {}",
                    wu.key, turns, limits.work_unit_max_turns
                ));
                budget.max_turns = Some(limits.work_unit_max_turns);
            }
            if let Some(wall) = budget.max_wall_secs
                && wall > limits.work_unit_max_wall_secs
            {
                rounding_notes.push(format!(
                    "work unit {}: max_wall_secs {} -> {}",
                    wu.key, wall, limits.work_unit_max_wall_secs
                ));
                budget.max_wall_secs = Some(limits.work_unit_max_wall_secs);
            }
        }
    }

    Ok(ValidatedPlan {
        spec: rounded,
        rounding_notes,
        topological_order,
    })
}

/// ADR-0074 D3.7（Phase F4b (f)）: `children` の検証（v2 のみ呼ぶ）。
fn validate_children(
    children: &[ExecutionChildSpec],
    limits: ExecutionLimits,
) -> Vec<PlanValidationError> {
    let mut errors = Vec::new();
    if children.len() > limits.max_children {
        errors.push(PlanValidationError::TooManyChildren {
            count: children.len(),
            max: limits.max_children,
        });
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for c in children {
        if !valid_key(&c.key) {
            errors.push(PlanValidationError::InvalidChildKey { key: c.key.clone() });
            continue;
        }
        if !seen.insert(c.key.as_str()) {
            errors.push(PlanValidationError::DuplicateChildKey { key: c.key.clone() });
        }
    }
    let known: BTreeSet<&str> = children.iter().map(|c| c.key.as_str()).collect();
    for c in children {
        for dep in &c.depends_on {
            if !known.contains(dep.as_str()) || dep == &c.key {
                errors.push(PlanValidationError::UnknownChildDependency {
                    key: c.key.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
        if c.acceptance.is_empty() {
            errors.push(PlanValidationError::ChildNoAcceptance { key: c.key.clone() });
        } else if let Err(detail) =
            crate::model::validate_human_checks_have_deliverable(&c.acceptance)
        {
            errors.push(PlanValidationError::ChildInvalidAcceptance {
                key: c.key.clone(),
                detail,
            });
        }
    }
    if errors.is_empty() {
        // 子同士の循環（Kahn 法。`topo_sort` を WU と同じ形で使う）。
        let as_units: Vec<WorkUnitSpec> = children
            .iter()
            .map(|c| WorkUnitSpec {
                key: c.key.clone(),
                kind: WorkUnitKind::Implement,
                title: c.title.clone(),
                objective: c.objective.clone(),
                depends_on: c.depends_on.clone(),
                done_when: Vec::new(),
                checks: Vec::new(),
                context: WorkUnitContext::default(),
                harness: None,
                features: None,
                budget: None,
                outputs: Vec::new(),
                phase: None,
            })
            .collect();
        if let Err(cycle) = topo_sort(&as_units, &BTreeMap::new()) {
            errors.push(PlanValidationError::CyclicChildDependency { cycle });
        }
    }
    errors
}

/// ADR-0079 D2 / D3 / D4 (2)（Phase R1a）: `celeris.execution-plan/3` の検証（純粋関数）。
///
/// - `[execution.tree] enabled = false` なら `TreeDisabled` だけを返す（他は見ない）。
/// - 段階: 1..=`max_stages`、key の形・一意、kind に `task` / `integrate` を使わない。
/// - unit: key の形・一意・`integrate-` の予約、段階は既知、段階あたり `max_units_per_stage`、
///   kind task は計画あたり `max_child_tasks_per_plan` で `depth < max_depth` のときだけ。
/// - kind task: `acceptance` 1 件以上（human には成果物）、`checks` / `budget` / `harness` /
///   `context.paths` を持たない、`adopt` は origin human だけ。
/// - leaf: kind task 専用の欄を持たない、`checks` 1 本以上、`context.repo` は高々 1、予算は 1 run の
///   上限以内（/3 は丸めずに拒否）。
/// - 依存: /2 の規則（既知の key、同じか前の段階、同じ段階の中は高々 1 つ）、`child:` は拒否、循環なし。
/// - 決定: 形（D7）、key の一意、`needed_before` は既知の unit か `stage:<key>`、計画あたり
///   `max_open_decisions_per_plan`。`needs_decisions` は既知の決定。
/// - /2 と同じ: `features`・サイズの上限・重複の検出・replan の done の不変条件。
fn validate_v3(
    spec: &ExecutionPlanSpec,
    limits: ExecutionLimits,
    done_work_units: &[(String, WorkUnitSpec)],
    ctx: PlanContext,
) -> Result<ValidatedPlan, Vec<PlanValidationError>> {
    let tree = limits.tree;
    if !tree.enabled {
        return Err(vec![PlanValidationError::TreeDisabled]);
    }
    let mut errors = Vec::new();

    for (field, present) in [
        ("phases", !spec.phases.is_empty()),
        ("work_units", !spec.work_units.is_empty()),
        ("children", !spec.children.is_empty()),
    ] {
        if present {
            errors.push(PlanValidationError::V2FieldInV3 { field });
        }
    }

    // 段階。
    if spec.stages.is_empty() {
        errors.push(PlanValidationError::NoStages);
    }
    if spec.stages.len() > tree.max_stages {
        errors.push(PlanValidationError::TooManyStages {
            count: spec.stages.len(),
            max: tree.max_stages,
        });
    }
    let mut seen_stage_keys: BTreeSet<&str> = BTreeSet::new();
    for s in &spec.stages {
        if !valid_key(&s.key) {
            errors.push(PlanValidationError::InvalidStageKey { key: s.key.clone() });
            continue;
        }
        if !seen_stage_keys.insert(s.key.as_str()) {
            errors.push(PlanValidationError::DuplicateStageKey { key: s.key.clone() });
        }
        if matches!(s.kind, WorkUnitKind::Task | WorkUnitKind::Integrate) {
            errors.push(PlanValidationError::InvalidStageKind { key: s.key.clone() });
        }
    }
    let stage_index: BTreeMap<&str, usize> = spec
        .stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key.as_str(), i))
        .collect();

    // unit の key・kind・段階。
    if spec.units.is_empty() {
        errors.push(PlanValidationError::NoUnits);
    }
    let mut seen_keys: BTreeSet<&str> = BTreeSet::new();
    for u in &spec.units {
        if !valid_key(&u.key) {
            errors.push(PlanValidationError::InvalidKey { key: u.key.clone() });
            continue;
        }
        if !seen_keys.insert(u.key.as_str()) {
            errors.push(PlanValidationError::DuplicateKey { key: u.key.clone() });
        }
        if u.key.starts_with(INTEGRATE_KEY_PREFIX) {
            errors.push(PlanValidationError::ReservedKey { key: u.key.clone() });
        }
        if u.kind == WorkUnitKind::Integrate {
            errors.push(PlanValidationError::ReservedKind { key: u.key.clone() });
        }
        if !stage_index.contains_key(u.stage.as_str()) {
            errors.push(PlanValidationError::UnknownUnitStage {
                key: u.key.clone(),
                stage: u.stage.clone(),
            });
        }
    }
    for s in &spec.stages {
        let count = spec.units.iter().filter(|u| u.stage == s.key).count();
        if count > tree.max_units_per_stage {
            errors.push(PlanValidationError::TooManyUnitsInStage {
                stage: s.key.clone(),
                count,
                max: tree.max_units_per_stage,
            });
        }
    }
    // ADR-0079 D15（Phase R5b-prep）: `adopt` の unit は子を作らないので数えない。
    let task_units = spec.units.iter().filter(|u| u.creates_child()).count();
    if task_units > tree.max_child_tasks_per_plan {
        errors.push(PlanValidationError::TooManyChildTasks {
            count: task_units,
            max: tree.max_child_tasks_per_plan,
        });
    }

    // kind task / leaf の欄。
    for u in &spec.units {
        let key = u.key.clone();
        if u.is_task() {
            if !crate::tree::can_have_child_tasks(ctx.depth, tree.max_depth) {
                errors.push(PlanValidationError::ChildTaskTooDeep {
                    key: key.clone(),
                    depth: ctx.depth,
                    max_depth: tree.max_depth,
                });
            }
            if u.acceptance.is_empty() {
                errors.push(PlanValidationError::TaskUnitNoAcceptance { key: key.clone() });
            } else if let Err(detail) =
                crate::model::validate_human_checks_have_deliverable(&u.acceptance)
            {
                errors.push(PlanValidationError::TaskUnitInvalidAcceptance {
                    key: key.clone(),
                    detail,
                });
            }
            for (field, present) in [
                ("checks", !u.checks.is_empty()),
                ("budget", u.budget.is_some()),
                ("harness", u.harness.is_some()),
                ("context.paths", !u.context.paths.is_empty()),
            ] {
                if present {
                    errors.push(PlanValidationError::TaskUnitFieldNotAllowed {
                        key: key.clone(),
                        field,
                    });
                }
            }
            // ADR-0079 R5b-fix1: planner の replan は、前の版から done の unit をそのまま写す（done の不変条件）
            // ので、その unit が持つ `adopt` も残ってよい（採用は済んでいて、replan は done の行に触れない）。
            // done の写しでない unit の新しい `adopt` は従来どおり人の計画だけ。
            if u.adopt.is_some()
                && ctx.origin != PlanOrigin::Human
                && !is_done_carry_over(u, done_work_units)
            {
                errors.push(PlanValidationError::AdoptNotAllowed { key: key.clone() });
            }
        } else {
            for (field, present) in [
                ("acceptance", !u.acceptance.is_empty()),
                ("genre", u.genre.is_some()),
                ("skills", !u.skills.is_empty()),
                ("repos", !u.repos.is_empty()),
                ("adopt", u.adopt.is_some()),
            ] {
                if present {
                    errors.push(PlanValidationError::LeafFieldNotAllowed {
                        key: key.clone(),
                        field,
                    });
                }
            }
            if u.checks.is_empty() {
                errors.push(PlanValidationError::LeafWithoutChecks { key: key.clone() });
            }
            let repo_count = u
                .context
                .repo
                .as_ref()
                .map(RepoSelector::count)
                .unwrap_or(0);
            if repo_count > 1 {
                errors.push(PlanValidationError::LeafMultipleRepos {
                    key: key.clone(),
                    count: repo_count,
                });
            }
            if let Some(budget) = u.budget {
                if let Some(turns) = budget.max_turns
                    && turns > limits.work_unit_max_turns
                {
                    errors.push(PlanValidationError::LeafBudgetOverLimit {
                        key: key.clone(),
                        detail: format!(
                            "budget.max_turns {turns} > {}",
                            limits.work_unit_max_turns
                        ),
                    });
                }
                if let Some(wall) = budget.max_wall_secs
                    && wall > limits.work_unit_max_wall_secs
                {
                    errors.push(PlanValidationError::LeafBudgetOverLimit {
                        key: key.clone(),
                        detail: format!(
                            "budget.max_wall_secs {wall} > {}",
                            limits.work_unit_max_wall_secs
                        ),
                    });
                }
            }
        }
    }

    // 依存（/2 の規則 + `child:` の拒否）。
    let known_keys: BTreeSet<&str> = spec.units.iter().map(|u| u.key.as_str()).collect();
    let unit_stage_of: BTreeMap<&str, usize> = spec
        .units
        .iter()
        .filter_map(|u| {
            stage_index
                .get(u.stage.as_str())
                .map(|&i| (u.key.as_str(), i))
        })
        .collect();
    for u in &spec.units {
        let mut intra_stage_deps = 0usize;
        for dep in &u.depends_on {
            if dep.starts_with(CHILD_DEP_PREFIX) {
                errors.push(PlanValidationError::ChildDependencyNotAllowed {
                    key: u.key.clone(),
                    depends_on: dep.clone(),
                });
                continue;
            }
            if !known_keys.contains(dep.as_str()) {
                errors.push(PlanValidationError::UnknownDependency {
                    key: u.key.clone(),
                    depends_on: dep.clone(),
                });
                continue;
            }
            let (Some(&own), Some(&other)) = (
                unit_stage_of.get(u.key.as_str()),
                unit_stage_of.get(dep.as_str()),
            ) else {
                continue;
            };
            match other.cmp(&own) {
                std::cmp::Ordering::Greater => {
                    errors.push(PlanValidationError::DependencyInLaterPhase {
                        key: u.key.clone(),
                        depends_on: dep.clone(),
                    })
                }
                std::cmp::Ordering::Equal => intra_stage_deps += 1,
                std::cmp::Ordering::Less => {}
            }
        }
        if intra_stage_deps > 1 {
            errors.push(PlanValidationError::TooManyIntraPhaseDependencies {
                key: u.key.clone(),
                phase: u.stage.clone(),
            });
        }
    }

    // 決定（D7）。
    let decisions = normalized_decisions(spec);
    let mut decision_keys: BTreeSet<&str> = BTreeSet::new();
    for d in &decisions {
        for e in crate::decision::validate_shape(d) {
            errors.push(PlanValidationError::InvalidDecision {
                detail: e.to_string(),
            });
        }
        if !decision_keys.insert(d.key.as_str()) {
            errors.push(PlanValidationError::DuplicateDecisionKey { key: d.key.clone() });
        }
        for target in &d.needed_before {
            let known = match target.strip_prefix(crate::decision::NEEDED_BEFORE_STAGE_PREFIX) {
                Some(stage) => stage_index.contains_key(stage),
                None => known_keys.contains(target.as_str()),
            };
            if !known {
                errors.push(PlanValidationError::UnknownNeededBefore {
                    key: d.key.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    if decisions.len() > tree.max_open_decisions_per_plan {
        errors.push(PlanValidationError::TooManyDecisions {
            count: decisions.len(),
            max: tree.max_open_decisions_per_plan,
        });
    }
    for u in &spec.units {
        for d in &u.needs_decisions {
            if !decision_keys.contains(d.as_str()) {
                errors.push(PlanValidationError::UnknownNeedsDecision {
                    key: u.key.clone(),
                    decision: d.clone(),
                });
            }
        }
    }

    // /2 と同じ検査（`features`・サイズ・重複・replan の不変条件）は /2 の形に写して行う。
    let internal = internal_view(spec);
    for wu in &internal.work_units {
        if let Some(features) = &wu.features
            && let Err(e) =
                serde_json::from_value::<crate::model_policy::TaskFeatureHints>(features.clone())
        {
            errors.push(PlanValidationError::InvalidFeatures {
                key: wu.key.clone(),
                detail: e.to_string(),
            });
        }
    }
    let rationale_len = spec.rationale.chars().count();
    if rationale_len > limits.max_rationale_chars {
        errors.push(PlanValidationError::RationaleTooLong {
            len: rationale_len,
            max: limits.max_rationale_chars,
        });
    }
    for wu in &internal.work_units {
        let title_len = wu.title.chars().count();
        if title_len > limits.max_title_chars {
            errors.push(PlanValidationError::TitleTooLong {
                key: wu.key.clone(),
                len: title_len,
                max: limits.max_title_chars,
            });
        }
        let objective_len = wu.objective.chars().count();
        if objective_len > limits.max_objective_chars {
            errors.push(PlanValidationError::ObjectiveTooLong {
                key: wu.key.clone(),
                len: objective_len,
                max: limits.max_objective_chars,
            });
        }
        if wu.done_when.len() > limits.max_done_when_items {
            errors.push(PlanValidationError::TooManyDoneWhen {
                key: wu.key.clone(),
                count: wu.done_when.len(),
                max: limits.max_done_when_items,
            });
        }
        for (index, item) in wu.done_when.iter().enumerate() {
            let len = item.chars().count();
            if len > limits.max_done_when_chars {
                errors.push(PlanValidationError::DoneWhenItemTooLong {
                    key: wu.key.clone(),
                    index,
                    len,
                    max: limits.max_done_when_chars,
                });
            }
        }
        if wu.checks.len() > limits.max_checks {
            errors.push(PlanValidationError::TooManyChecks {
                key: wu.key.clone(),
                count: wu.checks.len(),
                max: limits.max_checks,
            });
        }
    }
    let plan_bytes = serde_json::to_vec(spec).map(|v| v.len()).unwrap_or(0);
    if plan_bytes > limits.max_plan_json_bytes {
        errors.push(PlanValidationError::PlanTooLarge {
            bytes: plan_bytes,
            max: limits.max_plan_json_bytes,
        });
    }

    let mut topological_order = Vec::new();
    if errors.is_empty() {
        match topo_sort(&internal.work_units, &stage_index) {
            Ok(order) => topological_order = order,
            Err(cycle) => errors.push(PlanValidationError::CyclicDependency { cycle }),
        }
    }

    for i in 0..internal.work_units.len() {
        for j in (i + 1)..internal.work_units.len() {
            let a = &internal.work_units[i];
            let b = &internal.work_units[j];
            if normalize_title(&a.title) == normalize_title(&b.title) && !a.title.is_empty() {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: "identical normalized title".to_string(),
                });
                continue;
            }
            let sim = jaccard(&token_set(&a.objective), &token_set(&b.objective));
            if sim >= 0.9 {
                errors.push(PlanValidationError::DuplicateWorkUnit {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    reason: format!("objective token overlap {sim:.2}"),
                });
            }
        }
    }

    errors.extend(done_carry_over_errors(
        &internal.work_units,
        done_work_units,
        ctx.origin,
    ));

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ValidatedPlan {
        spec: spec.clone(),
        rounding_notes: Vec::new(),
        topological_order,
    })
}

/// ADR-0074 D3.7（Phase F4b (f)）: `newly_ready` の一般化。`external_done` は満たされた外部の依存
/// （`child:<key>` のうち子 Task が `done` のもの）。`child:` の依存は `external_done` にあれば満たす。
pub fn newly_ready_with(units: &[WorkUnitRow], external_done: &BTreeSet<String>) -> Vec<String> {
    let done: BTreeSet<&str> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done)
        .map(|u| u.key.as_str())
        .chain(external_done.iter().map(String::as_str))
        .collect();
    let ranks = phase_ranks(units);
    units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| u.depends_on.iter().all(|d| done.contains(d.as_str())))
        .filter(|u| earlier_phases_done(units, &ranks, u))
        .map(|u| u.id.clone())
        .collect()
}

/// Kahn's algorithm。`Ok` はトポロジカル順（`work_units` の index。同順位は
/// `(phase_rank, key)` 昇順で tie-break）、`Err` は見つかった循環（key の列）。
///
/// ADR-0074 D1.1（Phase F2）: `phase_rank` は工程の key → `phases` での出現順（v2）。v1 は
/// 常に空の map を渡す（すべて rank 0 のまま。挙動は従来と 1 バイトも変わらない）。v2 では
/// 工程をまたぐ依存は必ず前の工程 → 後の工程（`validate` が別途保証する）なので、この
/// tie-break は「依存の無い WU 同士がどちらの工程にいても後の工程が先に選ばれない」ことだけを
/// 保証する（表示・`seq` の並びのため。scheduler 自体は `phase` 列で絞り込む。D1.3）。
fn topo_sort(
    work_units: &[WorkUnitSpec],
    phase_rank: &BTreeMap<&str, usize>,
) -> Result<Vec<usize>, Vec<String>> {
    let index_of: BTreeMap<&str, usize> = work_units
        .iter()
        .enumerate()
        .map(|(i, w)| (w.key.as_str(), i))
        .collect();
    let mut in_degree: Vec<usize> = vec![0; work_units.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); work_units.len()];
    for (i, wu) in work_units.iter().enumerate() {
        for dep in &wu.depends_on {
            if let Some(&dep_i) = index_of.get(dep.as_str()) {
                dependents[dep_i].push(i);
                in_degree[i] += 1;
            }
        }
    }
    let rank_of = |wu: &WorkUnitSpec| -> usize {
        wu.phase
            .as_deref()
            .and_then(|p| phase_rank.get(p).copied())
            .unwrap_or(0)
    };
    // 決定的に (phase_rank, key) 昇順で並べた候補集合を都度取り出す。
    let mut ready: BTreeSet<(usize, &str, usize)> = work_units
        .iter()
        .enumerate()
        .filter(|(i, _)| in_degree[*i] == 0)
        .map(|(i, w)| (rank_of(w), w.key.as_str(), i))
        .collect();
    let mut order = Vec::new();
    while let Some((_, _, i)) = ready.iter().next().copied() {
        ready.remove(&(rank_of(&work_units[i]), work_units[i].key.as_str(), i));
        order.push(i);
        for &dep in &dependents[i] {
            in_degree[dep] -= 1;
            if in_degree[dep] == 0 {
                ready.insert((rank_of(&work_units[dep]), work_units[dep].key.as_str(), dep));
            }
        }
    }
    if order.len() == work_units.len() {
        Ok(order)
    } else {
        let cycle: Vec<String> = (0..work_units.len())
            .filter(|i| !order.contains(i))
            .map(|i| work_units[i].key.clone())
            .collect();
        Err(cycle)
    }
}

// ---------------------------------------------------------------------------
// D6: WorkUnit / Run の状態
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitStatus {
    Pending,
    Ready,
    NeedsContinuation,
    Running,
    Done,
    Failed,
    Blocked,
    Superseded,
    Cancelled,
}

impl WorkUnitStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkUnitStatus::Pending => "pending",
            WorkUnitStatus::Ready => "ready",
            WorkUnitStatus::NeedsContinuation => "needs_continuation",
            WorkUnitStatus::Running => "running",
            WorkUnitStatus::Done => "done",
            WorkUnitStatus::Failed => "failed",
            WorkUnitStatus::Blocked => "blocked",
            WorkUnitStatus::Superseded => "superseded",
            WorkUnitStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(WorkUnitStatus::Pending),
            "ready" => Some(WorkUnitStatus::Ready),
            "needs_continuation" => Some(WorkUnitStatus::NeedsContinuation),
            "running" => Some(WorkUnitStatus::Running),
            "done" => Some(WorkUnitStatus::Done),
            "failed" => Some(WorkUnitStatus::Failed),
            "blocked" => Some(WorkUnitStatus::Blocked),
            "superseded" => Some(WorkUnitStatus::Superseded),
            "cancelled" => Some(WorkUnitStatus::Cancelled),
            _ => None,
        }
    }

    /// この状態が「まだ計画の実行に関わる」か（superseded/cancelled は外れる）。
    pub fn is_active(self) -> bool {
        !matches!(self, WorkUnitStatus::Superseded | WorkUnitStatus::Cancelled)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkUnitStatus::Done | WorkUnitStatus::Superseded | WorkUnitStatus::Cancelled
        )
    }
}

/// D6: `work_units.blocked_reason`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitBlockedReason {
    Question,
    DependencyFailed,
    Limit,
    /// ADR-0072 D17 3.（Phase E4b 項目2）: worker の checkpoint/result.json が `plan_issue`
    /// （計画そのものが誤っているという 1 文の申告）を書いた。replan の余地があれば
    /// `Trigger::Continue{why: Replan}` で即座に Task を Ready へ戻す（Blocked のままにはしない）ので、
    /// この行が実際に `Task.status == Blocked` と一緒に残るのは replan の上限を使い切ったときだけ。
    PlanIssue,
    /// ADR-0079 D3 / D4 (3)（Phase R2a）: 人への決定の要求（`kind: limit` / `leaf_too_large`）を待つ。
    /// 木の上限を超える unit、子 task にできない深さの compound な leaf。同じ段階の他の unit・兄弟は
    /// 止めない（工程の失敗にも質問にも数えない）。回答で再開するのは R3a。
    Decision,
    /// ADR-0079 D9（Phase R2b）: kind task の unit の子が基盤の分類で失敗し、自動の作り直し
    /// （`MAX_CHILD_INFRA_RETRIES`）でも失敗した。障害通知を出し、人の再試行を待つ（質問でも決定でもない）。
    /// `Decision` と同じく工程の失敗に数えず、同じ段階の他の unit・兄弟は止めない（段階は完了しない）。
    Infra,
}

impl WorkUnitBlockedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkUnitBlockedReason::Question => "question",
            WorkUnitBlockedReason::DependencyFailed => "dependency_failed",
            WorkUnitBlockedReason::Limit => "limit",
            WorkUnitBlockedReason::PlanIssue => "plan_issue",
            WorkUnitBlockedReason::Decision => "decision",
            WorkUnitBlockedReason::Infra => "infra",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "question" => Some(WorkUnitBlockedReason::Question),
            "dependency_failed" => Some(WorkUnitBlockedReason::DependencyFailed),
            "limit" => Some(WorkUnitBlockedReason::Limit),
            "plan_issue" => Some(WorkUnitBlockedReason::PlanIssue),
            "decision" => Some(WorkUnitBlockedReason::Decision),
            "infra" => Some(WorkUnitBlockedReason::Infra),
            _ => None,
        }
    }
}

/// D5: `execution_plans.origin`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanOrigin {
    Planner,
    Human,
    Repair,
    Fixture,
}

impl PlanOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanOrigin::Planner => "planner",
            PlanOrigin::Human => "human",
            PlanOrigin::Repair => "repair",
            PlanOrigin::Fixture => "fixture",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "planner" => Some(PlanOrigin::Planner),
            "human" => Some(PlanOrigin::Human),
            "repair" => Some(PlanOrigin::Repair),
            "fixture" => Some(PlanOrigin::Fixture),
            _ => None,
        }
    }
}

/// D5: `execution_plans.status`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Active,
    Superseded,
    Completed,
    Abandoned,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Active => "active",
            PlanStatus::Superseded => "superseded",
            PlanStatus::Completed => "completed",
            PlanStatus::Abandoned => "abandoned",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(PlanStatus::Active),
            "superseded" => Some(PlanStatus::Superseded),
            "completed" => Some(PlanStatus::Completed),
            "abandoned" => Some(PlanStatus::Abandoned),
            _ => None,
        }
    }
}

/// D5: `runs.role`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunIndexRole {
    Worker,
    Reviewer,
    Planner,
    WrapUp,
}

impl RunIndexRole {
    pub fn as_str(self) -> &'static str {
        match self {
            RunIndexRole::Worker => "worker",
            RunIndexRole::Reviewer => "reviewer",
            RunIndexRole::Planner => "planner",
            RunIndexRole::WrapUp => "wrap_up",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "worker" => Some(RunIndexRole::Worker),
            "reviewer" => Some(RunIndexRole::Reviewer),
            "planner" => Some(RunIndexRole::Planner),
            "wrap_up" => Some(RunIndexRole::WrapUp),
            _ => None,
        }
    }
}

/// D5: `runs.status`（Run の終わり方。`RunEnd` とほぼ対応するが `running` を持つ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunIndexStatus {
    Running,
    Completed,
    Yielded,
    BudgetExhausted,
    Question,
    Failed,
    HarnessError,
    Cancelled,
}

impl RunIndexStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunIndexStatus::Running => "running",
            RunIndexStatus::Completed => "completed",
            RunIndexStatus::Yielded => "yielded",
            RunIndexStatus::BudgetExhausted => "budget_exhausted",
            RunIndexStatus::Question => "question",
            RunIndexStatus::Failed => "failed",
            RunIndexStatus::HarnessError => "harness_error",
            RunIndexStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(RunIndexStatus::Running),
            "completed" => Some(RunIndexStatus::Completed),
            "yielded" => Some(RunIndexStatus::Yielded),
            "budget_exhausted" => Some(RunIndexStatus::BudgetExhausted),
            "question" => Some(RunIndexStatus::Question),
            "failed" => Some(RunIndexStatus::Failed),
            "harness_error" => Some(RunIndexStatus::HarnessError),
            "cancelled" => Some(RunIndexStatus::Cancelled),
            _ => None,
        }
    }

    /// `RunEnd`（D7）から `runs.status` へ。
    pub fn from_run_end(end: crate::execution::RunEnd) -> Self {
        use crate::execution::RunEnd;
        match end {
            RunEnd::Completed => RunIndexStatus::Completed,
            RunEnd::Yielded => RunIndexStatus::Yielded,
            RunEnd::BudgetExhausted { .. } => RunIndexStatus::BudgetExhausted,
            RunEnd::Question => RunIndexStatus::Question,
            RunEnd::Failed { .. } => RunIndexStatus::Failed,
            RunEnd::HarnessError { .. } => RunIndexStatus::HarnessError,
            RunEnd::Cancelled => RunIndexStatus::Cancelled,
        }
    }
}

// ---------------------------------------------------------------------------
// 派生の索引の行（D5）。永続化そのものは `task_core::store` が行う。
// ---------------------------------------------------------------------------

/// `execution_plans` の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionPlanRow {
    pub id: String,
    pub task_id: String,
    pub version: u32,
    pub origin: PlanOrigin,
    pub planner_run_id: Option<String>,
    pub status: PlanStatus,
    pub spec: ExecutionPlanSpec,
    pub created_at: String,
    pub superseded_at: Option<String>,
}

/// `work_units` の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct WorkUnitRow {
    pub id: String,
    pub task_id: String,
    pub plan_id: String,
    pub key: String,
    pub seq: u32,
    pub kind: WorkUnitKind,
    pub status: WorkUnitStatus,
    pub blocked_reason: Option<WorkUnitBlockedReason>,
    pub depends_on: Vec<String>,
    pub runs: u32,
    pub continuations: u32,
    pub retries: u32,
    pub last_run_id: Option<String>,
    pub last_checkpoint_run_id: Option<String>,
    pub spec: WorkUnitSpec,
    pub created_at: String,
    pub updated_at: String,
    /// ADR-0074 D1（Phase F2 / migration 0027）: v2 の工程の key（v1・atomic は `None`）。
    /// 普通の WU は `spec.phase` の写し、統合 WU（`integrate-<phase>`）は自分の工程。
    pub phase: Option<String>,
    /// ADR-0074 D1.5: この WU を今実行している run（`acquire_work_unit_lease`）。揮発（replay で比べない）。
    pub lease_run_id: Option<String>,
    /// ADR-0074 D1.5: 上の lease の期限（RFC 3339）。揮発。
    pub lease_expires_at: Option<String>,
    /// ADR-0074 D1.2: `celeris-wu/<task_id>/<key>`（WU の worktree を切ったときだけ）。
    pub branch: Option<String>,
    /// ADR-0074 D1.2: WU の worktree を切った基点 sha。
    pub base_commit: Option<String>,
    /// ADR-0074 D1.2: `WorkUnitCommitted` の commit（run が done になったときの決定的な commit）。
    pub head_commit: Option<String>,
    /// ADR-0074 D1.4: `PhaseIntegrated` の Task ブランチの HEAD（統合 WU の行だけ）。
    pub integrated_commit: Option<String>,
    /// ADR-0079 D4 (4) / D15（migration 0031）: kind task の unit の子 task（`ChildTaskCreated` /
    /// `ChildAdopted` の写し。leaf・統合 WU は `None`）。
    pub child_task_id: Option<String>,
    /// ADR-0079 D7（migration 0031）: この unit が回答を待つ決定の key（/3 の
    /// [`effective_needs_decisions`]。/1・/2 は空）。
    pub needs_decisions: Vec<String>,
}

impl WorkUnitRow {
    pub fn new(
        id: String,
        task_id: String,
        plan_id: String,
        seq: u32,
        spec: WorkUnitSpec,
        status: WorkUnitStatus,
        created_at: String,
    ) -> Self {
        WorkUnitRow {
            id,
            task_id,
            plan_id,
            key: spec.key.clone(),
            seq,
            kind: spec.kind,
            status,
            blocked_reason: None,
            depends_on: spec.depends_on.clone(),
            runs: 0,
            continuations: 0,
            retries: 0,
            last_run_id: None,
            last_checkpoint_run_id: None,
            phase: spec.phase.clone(),
            spec,
            created_at: created_at.clone(),
            updated_at: created_at,
            lease_run_id: None,
            lease_expires_at: None,
            branch: None,
            base_commit: None,
            head_commit: None,
            integrated_commit: None,
            child_task_id: None,
            needs_decisions: Vec::new(),
        }
    }

    /// ADR-0074 D1.5: WU の lease を外す（`running` を離れる遷移で呼ぶ）。
    pub fn clear_lease(&mut self) {
        self.lease_run_id = None;
        self.lease_expires_at = None;
    }
}

/// `runs` の 1 行。
#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub run_id: String,
    pub task_id: String,
    pub work_unit_id: Option<String>,
    pub role: RunIndexRole,
    pub seq: u32,
    pub status: RunIndexStatus,
    pub adapter: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub session_id: Option<String>,
    pub checkpoint: Option<crate::execution::Checkpoint>,
    pub usage: Option<crate::model::Usage>,
    pub metrics: Option<crate::model::RunMetrics>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

// ---------------------------------------------------------------------------
// D15: scheduler（決定的。ready queue・依存の伝播）
// ---------------------------------------------------------------------------

/// D15: 次に何をすべきか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep {
    /// この WorkUnit（`work_units.id`）の Run を起こす。
    RunWorkUnit(String),
    /// Planner run を起こす（`replan` なら replan モード。E2 では発行しない）。
    RunPlanner {
        replan: bool,
    },
    AllDone,
    Stuck(String),
}

/// D15: `next_work_unit`。`needs_continuation` を優先し、次に `ready` を `seq` 順で選ぶ。
/// `units` は `superseded`/`cancelled` を含めてよい（無視する）。
pub fn next_work_unit(units: &[WorkUnitRow]) -> NextStep {
    let active: Vec<&WorkUnitRow> = units.iter().filter(|u| u.status.is_active()).collect();
    if active.is_empty() {
        return NextStep::Stuck("計画に有効な WorkUnit がありません".to_string());
    }
    if let Some(u) = active
        .iter()
        .filter(|u| u.status == WorkUnitStatus::NeedsContinuation)
        .min_by_key(|u| u.seq)
    {
        return NextStep::RunWorkUnit(u.id.clone());
    }
    if let Some(u) = active
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Ready)
        .min_by_key(|u| u.seq)
    {
        return NextStep::RunWorkUnit(u.id.clone());
    }
    if active.iter().all(|u| u.status == WorkUnitStatus::Done) {
        return NextStep::AllDone;
    }
    if active
        .iter()
        .any(|u| matches!(u.status, WorkUnitStatus::Running))
    {
        // 直列実行（D6）なので、走っている WU があれば「次」は無い（呼ばれない想定）。
        return NextStep::Stuck("既に実行中の WorkUnit があります".to_string());
    }
    NextStep::Stuck(
        "実行できる WorkUnit がありません（blocked/failed のみ残っています）".to_string(),
    )
}

/// ADR-0074「F5-fix8 実装時の明確化」: 計画に残っている仕事が無い（有効な WorkUnit がすべて `done`、または
/// 有効な WorkUnit が 1 つも無い）か。統合 WU・repair WU も含めて見る（統合が済んでいない工程は `false`）。
pub fn plan_work_finished(units: &[WorkUnitRow]) -> bool {
    units
        .iter()
        .filter(|u| u.status.is_active())
        .all(|u| u.status == WorkUnitStatus::Done)
}

/// ADR-0074「F5-fix8 実装時の明確化」: `active` な計画（`plan_id`）が採用されてから、最終レビューの判定
/// （`review_pass` / `review_fail` / `review_repair` で `reviewing` を出た遷移）がまだ 1 度も無いか。
///
/// 仕事の残っていない計画について dispatcher が「最終レビューへ進める（`Trigger::PlanComplete`）」か
/// 「replan を試す（D17 4.）」かを分ける。採用の後に判定が無い = この版はまだ審査されていない（replan で
/// 何も足さなかった版を含む）ので、審査に出す。判定の後（不合格で `ready` に戻った）なら従来どおり replan。
/// 採用の event（`ExecutionPlanned{plan_id}`）が見つからなければ `false`（従来どおり）。純粋関数（LLM なし）。
pub fn plan_awaits_final_review(events: &[(u64, crate::model::Event)], plan_id: &str) -> bool {
    use crate::model::{Event, Status};
    for (_, event) in events.iter().rev() {
        match event {
            Event::ExecutionPlanned { plan_id: id, .. } if id == plan_id => return true,
            Event::Transitioned {
                from: Status::Reviewing,
                reason,
                ..
            } if matches!(
                reason.as_str(),
                "review_pass" | "review_fail" | "review_repair"
            ) =>
            {
                return false;
            }
            _ => {}
        }
    }
    false
}

/// ADR-0074 D1.3（Phase F2）: `next_work_unit` の一般化。工程の中で並列に何本まで起こせるかを
/// 決める（純粋関数。実際に走らせる・lease を取るのは呼び出し側の責務）。
///
/// - 対象は**現在の工程**（有効〈`!is_terminal()`〉な WorkUnit が残っている工程のうち、
///   `seq` が最も小さいもの）だけ。v1（`spec.phase` が常に `None`）は工程が実質 1 つなので
///   全部が対象になる（`limit = 1` と組み合わせると `next_work_unit` と同じ 1 件を返す）。
/// - `needs_continuation` を先に、次に `ready` を `seq` 順で選ぶ（`next_work_unit` と同じ順序）。
/// - `limit` から `in_flight`（呼び出し側がこの Task について現在走らせている run の数）を引いた
///   件数まで。
/// - 現在の工程に `failed`/`blocked` の WorkUnit があれば、**新しい**（`ready` の）WorkUnit は
///   起こさない。ただし既に走ったことのある `needs_continuation` の WorkUnit は続ける
///   （D1.6「走っている WU とその continuation だけは続ける」）。
pub fn runnable_work_units(units: &[WorkUnitRow], in_flight: usize, limit: usize) -> Vec<String> {
    let slots = limit.saturating_sub(in_flight);
    if slots == 0 {
        return Vec::new();
    }
    let in_play: Vec<&WorkUnitRow> = units.iter().filter(|u| !u.status.is_terminal()).collect();
    let Some(current) = in_play.iter().min_by_key(|u| u.seq) else {
        return Vec::new();
    };
    let current_phase = current.spec.phase.as_deref();
    // ADR-0074 D1.4（Phase F2b）: 統合 WU は LLM run を起こさない（scheduler が直接走らせる）。
    let in_phase: Vec<&WorkUnitRow> = in_play
        .into_iter()
        .filter(|u| u.spec.phase.as_deref() == current_phase)
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        // ADR-0079（Phase R1a）: kind task の unit は子 task の代理で、LLM run を起こさない（R1b）。
        .filter(|u| u.kind != WorkUnitKind::Task)
        .collect();

    // ADR-0079 D5（Phase R2a）: 人への決定を待つ unit（`blocked(decision)`）は段階の完了を止めるが、同じ段階の
    // 他の unit は止めない（/1・/2 にこの理由は無いので従来どおり）。
    let has_failed_or_blocked = in_phase.iter().any(|u| {
        u.status == WorkUnitStatus::Failed
            || (u.status == WorkUnitStatus::Blocked
                && !matches!(
                    u.blocked_reason,
                    Some(WorkUnitBlockedReason::Decision | WorkUnitBlockedReason::Infra)
                ))
    });

    let mut candidates: Vec<&WorkUnitRow> = in_phase
        .iter()
        .copied()
        .filter(|u| u.status == WorkUnitStatus::NeedsContinuation)
        .collect();
    candidates.sort_by_key(|u| u.seq);

    if !has_failed_or_blocked {
        let mut ready: Vec<&WorkUnitRow> = in_phase
            .iter()
            .copied()
            .filter(|u| u.status == WorkUnitStatus::Ready)
            .collect();
        ready.sort_by_key(|u| u.seq);
        candidates.extend(ready);
    }

    candidates
        .into_iter()
        .take(slots)
        .map(|u| u.id.clone())
        .collect()
}

/// D15: 依存の解決。`depends_on` が全て `done` になった `pending` の WU を `ready` にする（`id` の集合を返す。
/// 呼び出し側が状態を書き換える）。
///
/// ADR-0074 D1.3（Phase F2b）: v2（`phase` のある行）では「前の工程の統合が done」を追加の条件にする
/// （工程の境は障壁。前の工程の有効な行〈統合 WU を含む〉がすべて `done` になるまで上げない）。
/// 統合 WU（`kind = integrate`）は `ready` にしない（scheduler が工程の完了を見て直接走らせる）。
/// v1（`phase` が無い行）は従来と同じ。
pub fn newly_ready(units: &[WorkUnitRow]) -> Vec<String> {
    let done: BTreeSet<&str> = units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Done)
        .map(|u| u.key.as_str())
        .collect();
    let ranks = phase_ranks(units);
    units
        .iter()
        .filter(|u| u.status == WorkUnitStatus::Pending)
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| u.depends_on.iter().all(|d| done.contains(d.as_str())))
        .filter(|u| earlier_phases_done(units, &ranks, u))
        .map(|u| u.id.clone())
        .collect()
}

/// ADR-0074 D1.3（Phase F2b）: 工程の key → 順位（その工程の行の `seq` の最小値。`topo_sort` が
/// 工程順を保証し、統合 WU は工程の末尾に置くので、`seq` の最小値で工程の順が引ける）。
pub fn phase_ranks(units: &[WorkUnitRow]) -> BTreeMap<String, u32> {
    let mut ranks: BTreeMap<String, u32> = BTreeMap::new();
    for u in units.iter().filter(|u| u.status.is_active()) {
        if let Some(p) = &u.phase {
            let e = ranks.entry(p.clone()).or_insert(u.seq);
            if u.seq < *e {
                *e = u.seq;
            }
        }
    }
    ranks
}

fn earlier_phases_done(
    units: &[WorkUnitRow],
    ranks: &BTreeMap<String, u32>,
    u: &WorkUnitRow,
) -> bool {
    let Some(rank) = u.phase.as_ref().and_then(|p| ranks.get(p)) else {
        return true;
    };
    units
        .iter()
        .filter(|o| o.status.is_active())
        .filter(|o| {
            o.phase
                .as_ref()
                .and_then(|p| ranks.get(p))
                .is_some_and(|r| r < rank)
        })
        .all(|o| o.status == WorkUnitStatus::Done)
}

/// ADR-0074 D1.4（Phase F2b）: 統合 WU の key の接頭辞（`integrate-<phase>`）。
pub const INTEGRATE_KEY_PREFIX: &str = "integrate-";

/// `integrate-<phase>`。
pub fn integrate_key(phase: &str) -> String {
    format!("{INTEGRATE_KEY_PREFIX}{phase}")
}

/// ADR-0074 D1.4（Phase F2b）: v2 の工程ごとの統合 WU の spec（`kind = integrate`、依存は
/// その工程のすべての WU）。計画の spec（`ExecutionPlanned.plan`）には入れない（daemon が足す
/// system WU。`max_work_units` にも数えない）。v1 は空。
pub fn integration_work_unit_specs(spec: &ExecutionPlanSpec) -> Vec<WorkUnitSpec> {
    if !is_phased_schema(&spec.schema) {
        return Vec::new();
    }
    // ADR-0079（Phase R1a）: /3 は段階を工程として同じ統合 WU を足す（`internal_view`）。
    let spec = internal_view(spec);
    spec.phases
        .iter()
        .map(|p| WorkUnitSpec {
            key: integrate_key(&p.key),
            kind: WorkUnitKind::Integrate,
            title: format!("工程 {} の統合", p.title),
            objective: format!(
                "工程 {} の WorkUnit のブランチを Task のブランチへ決定的に merge し、検査を再実行する（daemon が行う。LLM run は起こさない）",
                p.key
            ),
            depends_on: spec
                .work_units
                .iter()
                .filter(|w| w.phase.as_deref() == Some(p.key.as_str()))
                .map(|w| w.key.clone())
                .collect(),
            done_when: Vec::new(),
            checks: Vec::new(),
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: Vec::new(),
            phase: Some(p.key.clone()),
        })
        .collect()
}

/// ADR-0074 F5-fix: 検証エラーに添える「daemon が足した WU は書かなくてよい」の一文。
pub const DAEMON_ADDED_HINT: &str = "daemon が足した WU（kind = integrate の統合 WU・統合の repair WU）は書かなくてよい（daemon が旧版から持ち越す・補う）";

/// ADR-0074 D1.4（Phase F2b、F5-fix で共通化）: この行が daemon の足した WU（計画の spec に無い
/// system WU）か。`kind = integrate` の統合 WU、および v2 で `phase` を持ち `active` な計画の spec に
/// 無い WU（統合の repair WU）。planner の視野に無いので、replan の done の不変条件の対象にしない。
pub fn is_daemon_added_work_unit(active: &ExecutionPlanSpec, row: &WorkUnitRow) -> bool {
    row.kind == WorkUnitKind::Integrate
        || (is_phased_schema(&active.schema)
            && row.phase.is_some()
            && !internal_view(active)
                .work_units
                .iter()
                .any(|w| w.key == row.key))
}

/// ADR-0074 D5.3/D1.4（F5-fix）: replan の `validate` に渡す done の WU（`(key, spec)`）。
/// daemon が足した WU（[`is_daemon_added_work_unit`]）は除く（行は replan が触れずに持ち越す）。
/// dispatcher（planner run の検証）と `task_ops::execution::replan` の両方がこれを使う。
pub fn replan_done_work_units(
    active: &ExecutionPlanSpec,
    rows: &[WorkUnitRow],
) -> Vec<(String, WorkUnitSpec)> {
    rows.iter()
        .filter(|u| u.status == WorkUnitStatus::Done && !is_daemon_added_work_unit(active, u))
        .map(|u| (u.key.clone(), u.spec.clone()))
        .collect()
}

/// ADR-0079 D9（Phase R2b）: /3 の replan（差分 `execution-plan-delta/1` は /2 の形しか持たないので /3 は計画の全体を
/// 書く）で、`done` の unit を今の版（`active`）から持ち越す（純粋関数）。planner が done の unit を書かなかった・
/// 書き写し損ねた（unit の gate で上げ下げされた spec を知らない）ときも、採用した spec のまま新しい版に入る:
/// - `done_keys` の unit は `active` の spec で置き換える（無ければ足す。並びは新しい計画の段階の中の先頭）。
/// - その段階が新しい計画に無ければ、`active` の段階の順を保って挿入する。
/// - その unit が待つ決定（`needs_decisions`）が新しい計画に無ければ、`active` の決定を足す。
///
/// `active` か `new` が /3 でなければ何もしない。
pub fn carry_done_units_v3(
    active: &ExecutionPlanSpec,
    new: &mut ExecutionPlanSpec,
    done_keys: &BTreeSet<String>,
) {
    if active.schema != EXECUTION_PLAN_SCHEMA_V3 || new.schema != EXECUTION_PLAN_SCHEMA_V3 {
        return;
    }
    for done in active.units.iter().filter(|u| done_keys.contains(&u.key)) {
        // 段階（無ければ active の順を保って挿入）。
        if !new.stages.iter().any(|s| s.key == done.stage)
            && let Some(stage) = active.stages.iter().find(|s| s.key == done.stage)
        {
            let active_pos = |key: &str| active.stages.iter().position(|s| s.key == key);
            let own = active_pos(&stage.key).unwrap_or(0);
            let at = new
                .stages
                .iter()
                .position(|s| active_pos(&s.key).is_some_and(|p| p > own))
                .unwrap_or(new.stages.len());
            new.stages.insert(at, stage.clone());
        }
        match new.units.iter_mut().find(|u| u.key == done.key) {
            Some(existing) => *existing = done.clone(),
            None => {
                let at = new
                    .units
                    .iter()
                    .position(|u| u.stage == done.stage)
                    .unwrap_or(new.units.len());
                new.units.insert(at, done.clone());
            }
        }
        for d in &done.needs_decisions {
            if !new.decisions.iter().any(|x| &x.key == d)
                && !new
                    .units
                    .iter()
                    .any(|u| u.decisions.iter().any(|x| &x.key == d))
                && let Some(spec) = active.decisions.iter().find(|x| &x.key == d)
            {
                new.decisions.push(spec.clone());
            }
        }
    }
}

/// ADR-0074 D1.1/D1.4（Phase F2b）: 採用する計画の WU の並び（`seq` の順）。v1 はトポロジカル順
/// そのまま（従来どおり）。v2 は工程ごとに「その工程の WU（トポロジカル順）→ `integrate-<phase>`」。
pub fn materialized_order(
    spec: &ExecutionPlanSpec,
    topological_order: &[usize],
) -> Vec<WorkUnitSpec> {
    // ADR-0079（Phase R1a）: /3 は `units` を /2 の `work_units` の形に写してから並べる
    // （`topological_order` は `units` の index。写しても index は変わらない）。
    let integrations = integration_work_unit_specs(spec);
    let spec = internal_view(spec);
    let in_order: Vec<WorkUnitSpec> = topological_order
        .iter()
        .filter_map(|&i| spec.work_units.get(i).cloned())
        .collect();
    if !is_phased_schema(&spec.schema) {
        return in_order;
    }
    let mut out = Vec::with_capacity(in_order.len() + integrations.len());
    for (phase, integrate) in spec.phases.iter().zip(integrations) {
        out.extend(
            in_order
                .iter()
                .filter(|w| w.phase.as_deref() == Some(phase.key.as_str()))
                .cloned(),
        );
        out.push(integrate);
    }
    out
}

/// ADR-0074 D1.1/D1.4（Phase F2b）: 採用する計画の `work_units` の行を作る（純粋関数）。`id_of` は
/// 行の id を決める（`adopt_plan` は新しい ULID、replay は events から復元した id）。
/// v1 は「依存が無ければ ready、あれば pending」（従来どおり）。v2 は統合 WU を足し、
/// 工程の障壁つきの [`newly_ready`] で ready を決める（最初の工程の依存の無い WU だけが ready）。
pub fn materialize_work_units(
    task_id: &str,
    plan_id: &str,
    spec: &ExecutionPlanSpec,
    topological_order: &[usize],
    created_at: &str,
    id_of: &mut dyn FnMut(&WorkUnitSpec) -> String,
) -> Vec<WorkUnitRow> {
    let v2 = is_phased_schema(&spec.schema);
    let v3 = spec.schema == EXECUTION_PLAN_SCHEMA_V3;
    let mut rows: Vec<WorkUnitRow> = materialized_order(spec, topological_order)
        .into_iter()
        .enumerate()
        .map(|(seq, wu_spec)| {
            let status = if !v2 && wu_spec.depends_on.is_empty() {
                WorkUnitStatus::Ready
            } else {
                WorkUnitStatus::Pending
            };
            WorkUnitRow::new(
                id_of(&wu_spec),
                task_id.to_string(),
                plan_id.to_string(),
                seq as u32,
                wu_spec,
                status,
                created_at.to_string(),
            )
        })
        .collect();
    // ADR-0079 D7（Phase R1a）: /3 の unit が回答を待つ決定（`work_units.needs_decisions_json`）。
    if v3 {
        for r in rows.iter_mut() {
            r.needs_decisions = effective_needs_decisions(spec, &r.key);
        }
    }
    if v2 {
        let ready = newly_ready(&rows);
        for r in rows.iter_mut() {
            if ready.contains(&r.id) {
                r.status = WorkUnitStatus::Ready;
            }
        }
    }
    rows
}

/// ADR-0074 D1.4（Phase F2b）: 工程 `phase` の葉の WU（同じ工程の他の有効な WU に依存されていない、
/// 統合 WU 以外、ブランチを持つもの）を `seq` 順で。積み上げた依存先は葉に含まれる。
pub fn phase_leaves<'a>(units: &'a [WorkUnitRow], phase: &str) -> Vec<&'a WorkUnitRow> {
    let in_phase: Vec<&WorkUnitRow> = units
        .iter()
        .filter(|u| u.status.is_active())
        .filter(|u| u.kind != WorkUnitKind::Integrate)
        .filter(|u| u.phase.as_deref() == Some(phase))
        .collect();
    let mut leaves: Vec<&WorkUnitRow> = in_phase
        .iter()
        .copied()
        .filter(|u| u.branch.is_some())
        .filter(|u| {
            !in_phase
                .iter()
                .any(|o| o.id != u.id && o.depends_on.iter().any(|d| d == &u.key))
        })
        .collect();
    leaves.sort_by_key(|u| u.seq);
    leaves
}

/// D15: WU が `failed` になったとき、それに（直接・間接に）依存する未着手の WU を
/// `blocked(dependency_failed)` にする対象の `id` を返す（推移閉包）。
pub fn dependents_to_block(units: &[WorkUnitRow], failed_key: &str) -> Vec<String> {
    let mut blocked_keys: BTreeSet<String> = BTreeSet::new();
    blocked_keys.insert(failed_key.to_string());
    let mut changed = true;
    while changed {
        changed = false;
        for u in units {
            if blocked_keys.contains(&u.key) {
                continue;
            }
            if matches!(
                u.status,
                WorkUnitStatus::Pending | WorkUnitStatus::Ready | WorkUnitStatus::Blocked
            ) && u.depends_on.iter().any(|d| blocked_keys.contains(d))
            {
                blocked_keys.insert(u.key.clone());
                changed = true;
            }
        }
    }
    blocked_keys.remove(failed_key);
    units
        .iter()
        .filter(|u| blocked_keys.contains(&u.key))
        .map(|u| u.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str, depends_on: &[&str]) -> WorkUnitSpec {
        WorkUnitSpec {
            key: key.to_string(),
            kind: WorkUnitKind::Implement,
            title: format!("title {key}"),
            objective: format!("objective for {key} which is sufficiently distinct"),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            done_when: vec![],
            checks: vec![],
            context: WorkUnitContext::default(),
            harness: None,
            features: None,
            budget: None,
            outputs: vec![],
            phase: None,
        }
    }

    fn plan(work_units: Vec<WorkUnitSpec>) -> ExecutionPlanSpec {
        ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: EXECUTION_PLAN_SCHEMA.to_string(),
            rationale: "test".to_string(),
            work_units,
            phases: Vec::new(),
            children: Vec::new(),
        }
    }

    // ---- ADR-0074 D1.1（Phase F2）: v2（`phases`）のテスト用ヘルパー ----

    fn phase(key: &str) -> PhaseSpec {
        PhaseSpec {
            key: key.to_string(),
            kind: WorkUnitKind::Implement,
            title: format!("phase {key}"),
        }
    }

    fn spec_v2(key: &str, wu_phase: &str, depends_on: &[&str]) -> WorkUnitSpec {
        let mut s = spec(key, depends_on);
        s.phase = Some(wu_phase.to_string());
        s
    }

    fn plan_v2(phases: Vec<PhaseSpec>, work_units: Vec<WorkUnitSpec>) -> ExecutionPlanSpec {
        ExecutionPlanSpec {
            stages: Vec::new(),
            units: Vec::new(),
            decisions: Vec::new(),
            schema: EXECUTION_PLAN_SCHEMA_V2.to_string(),
            rationale: "test v2".to_string(),
            work_units,
            phases,
            children: Vec::new(),
        }
    }

    #[test]
    fn valid_three_step_plan_passes_and_orders_topologically() {
        let p = plan(vec![spec("a", &[]), spec("b", &["a"]), spec("c", &["b"])]);
        let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
        let order: Vec<&str> = validated
            .topological_order
            .iter()
            .map(|&i| validated.spec.work_units[i].key.as_str())
            .collect();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn rejects_cycles() {
        let p = plan(vec![spec("a", &["b"]), spec("b", &["a"])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::CyclicDependency { .. })),
            "{errs:?}"
        );
    }

    #[test]
    fn rejects_duplicate_keys() {
        let p = plan(vec![spec("a", &[]), spec("a", &[])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            PlanValidationError::DuplicateKey { key } if key == "a"
        )));
    }

    #[test]
    fn rejects_unknown_dependency() {
        let p = plan(vec![spec("a", &["ghost"])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::UnknownDependency { .. }))
        );
    }

    #[test]
    fn rejects_invalid_key_format() {
        let p = plan(vec![spec("Not Valid!", &[])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::InvalidKey { .. }))
        );
    }

    #[test]
    fn rejects_too_many_work_units() {
        let units: Vec<WorkUnitSpec> = (0..10).map(|i| spec(&format!("wu{i}"), &[])).collect();
        let p = plan(units);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::TooManyWorkUnits { .. }))
        );
    }

    #[test]
    fn rejects_near_duplicate_objectives() {
        let mut a = spec("a", &[]);
        a.objective = "investigate the current dispatcher and review pipeline in depth".into();
        let mut b = spec("b", &[]);
        b.objective = "investigate the current dispatcher and review pipeline in depth!".into();
        let p = plan(vec![a, b]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::DuplicateWorkUnit { .. }))
        );
    }

    #[test]
    fn rounds_budget_to_the_limits_and_records_it() {
        let mut a = spec("a", &[]);
        a.budget = Some(WorkUnitBudget {
            max_turns: Some(999),
            max_wall_secs: Some(99999),
        });
        let p = plan(vec![a]);
        let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
        assert_eq!(
            validated.spec.work_units[0].budget.unwrap().max_turns,
            Some(80)
        );
        assert_eq!(
            validated.spec.work_units[0].budget.unwrap().max_wall_secs,
            Some(3600)
        );
        assert_eq!(
            validated.rounding_notes.len(),
            2,
            "{:?}",
            validated.rounding_notes
        );
    }

    #[test]
    fn rejects_changed_done_work_unit_on_replan() {
        let done_spec = spec("a", &[]);
        let mut changed = done_spec.clone();
        changed.objective = "a completely different objective now".to_string();
        let p = plan(vec![changed]);
        let errs = validate(
            &p,
            ExecutionLimits::default(),
            &[("a".to_string(), done_spec)],
        )
        .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::DoneWorkUnitChanged { .. }))
        );
    }

    fn ctx(origin: PlanOrigin) -> PlanContext {
        PlanContext { origin, depth: 1 }
    }

    /// ADR-0079 R5b-fix1: 本番の task の形（done の `baseline` の check を人が直す）。
    fn baseline_override_fixture() -> (WorkUnitSpec, ExecutionPlanSpec) {
        let mut done_spec = spec("baseline", &[]);
        done_spec.checks = vec![WorkUnitCheck {
            cmd: "git diff --quiet 06e9a03cffe8 -- gui".to_string(),
            expect_exit: 0,
        }];
        let mut fixed = done_spec.clone();
        fixed.checks[0].cmd =
            "git diff --quiet 06e9a03cffe8 -- gui \":!gui/docs/adr/0002-frontend-stack.md\""
                .to_string();
        (done_spec, plan(vec![fixed, spec("next", &["baseline"])]))
    }

    /// ADR-0079 R5b-fix1: 人の replan は done の WU の spec（check）を上書きできる。
    #[test]
    fn human_replan_may_override_a_done_work_unit_spec() {
        let (done_spec, p) = baseline_override_fixture();
        let done = [("baseline".to_string(), done_spec)];
        validate_with(
            &p,
            ExecutionLimits::default(),
            &done,
            ctx(PlanOrigin::Human),
        )
        .expect("a human may correct the check of a done work unit");
        let overrides = done_work_unit_overrides(&p, &done);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].0, "baseline");
        assert_eq!(overrides[0].1, p.work_units[0]);
        assert_eq!(overrides[0].2, vec!["checks".to_string()]);
    }

    /// ADR-0079 R5b-fix1: planner（と repair）の replan は従来どおり done の spec を変えられない。
    #[test]
    fn planner_replan_still_rejects_a_changed_done_work_unit() {
        let (done_spec, p) = baseline_override_fixture();
        let done = [("baseline".to_string(), done_spec)];
        for origin in [PlanOrigin::Planner, PlanOrigin::Repair] {
            let errs =
                validate_with(&p, ExecutionLimits::default(), &done, ctx(origin)).unwrap_err();
            assert!(
                errs.contains(&PlanValidationError::DoneWorkUnitChanged {
                    key: "baseline".into()
                }),
                "{origin:?}: {errs:?}"
            );
        }
        let msg = PlanValidationError::DoneWorkUnitChanged {
            key: "baseline".into(),
        }
        .to_string();
        assert!(msg.contains("PUT /tasks/{id}/execution-plan"), "{msg}");
    }

    /// ADR-0079 R5b-fix1: 人でも done の WU は消せない・構造（kind / phase / depends_on）は変えられない。
    #[test]
    fn human_replan_still_rejects_a_removed_or_restructured_done_work_unit() {
        let (done_spec, p) = baseline_override_fixture();
        let done = [("baseline".to_string(), done_spec)];
        let mut removed = p.clone();
        removed.work_units.remove(0);
        removed.work_units[0].depends_on.clear();
        let errs = validate_with(
            &removed,
            ExecutionLimits::default(),
            &done,
            ctx(PlanOrigin::Human),
        )
        .unwrap_err();
        assert!(errs.contains(&PlanValidationError::DoneWorkUnitChanged {
            key: "baseline".into()
        }));

        let mut restructured = p.clone();
        restructured.work_units.push(spec("extra", &[]));
        restructured.work_units[0].depends_on = vec!["extra".into()];
        restructured.work_units[0].kind = WorkUnitKind::Test;
        let errs = validate_with(
            &restructured,
            ExecutionLimits::default(),
            &done,
            ctx(PlanOrigin::Human),
        )
        .unwrap_err();
        for field in ["kind", "depends_on"] {
            assert!(
                errs.contains(&PlanValidationError::DoneWorkUnitStructureChanged {
                    key: "baseline".into(),
                    field,
                }),
                "{field}: {errs:?}"
            );
        }
    }

    /// ADR-0074 D5.1（Phase F1）: `features` は `TaskFeatureHints` として読めなければ検証エラー
    /// （黙って `.ok()` で捨てない）。
    #[test]
    fn features_must_parse_as_task_feature_hints() {
        let mut a = spec("a", &[]);
        a.features = Some(serde_json::json!({"judgment": "low", "ambiguity": "low"}));
        let p = plan(vec![a]);
        validate(&p, ExecutionLimits::default(), &[]).expect("valid partial features");

        let mut b = spec("b", &[]);
        // `lane` は `TaskFeatureHints` に無い欄（`deny_unknown_fields`）。
        b.features = Some(serde_json::json!({"lane": "frontier"}));
        let p = plan(vec![b]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::InvalidFeatures { key, .. } if key == "b")
            ),
            "{errs:?}"
        );

        let mut c = spec("c", &[]);
        // 型が違う（配列であるべき欄が文字列）。
        c.features = Some(serde_json::json!({"judgment": "not-a-level"}));
        let p = plan(vec![c]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::InvalidFeatures { key, .. } if key == "c")
            ),
            "{errs:?}"
        );
    }

    /// ADR-0074 D5.3（Phase F1）: 計画のサイズ上限（§4）。
    #[test]
    fn plan_size_limits_reject_oversized_rationale() {
        let mut p = plan(vec![spec("a", &[])]);
        p.rationale = "x".repeat(1_501);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::RationaleTooLong { len, max } if *len == 1_501 && *max == 1_500)
            ),
            "{errs:?}"
        );
    }

    #[test]
    fn plan_size_limits_reject_oversized_work_unit_fields() {
        let mut a = spec("a", &[]);
        a.title = "x".repeat(121);
        a.objective = "y".repeat(2_001);
        a.done_when = (0..9).map(|i| format!("done {i}")).collect();
        a.checks = (0..7)
            .map(|i| WorkUnitCheck {
                cmd: format!("cmd {i}"),
                expect_exit: 0,
            })
            .collect();
        let p = plan(vec![a]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::TitleTooLong { key, .. } if key == "a")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::ObjectiveTooLong { key, .. } if key == "a")
            ),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::TooManyDoneWhen { key, .. } if key == "a")
            ),
            "{errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::TooManyChecks { key, .. } if key == "a")),
            "{errs:?}"
        );
    }

    #[test]
    fn plan_size_limits_reject_the_whole_json_being_too_large() {
        let limits = ExecutionLimits {
            max_plan_json_bytes: 200,
            ..ExecutionLimits::default()
        };
        let p = plan(vec![spec("a", &[]), spec("b", &["a"])]);
        let errs = validate(&p, limits, &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::PlanTooLarge { .. })),
            "{errs:?}"
        );
    }

    fn row(key: &str, seq: u32, status: WorkUnitStatus, depends_on: &[&str]) -> WorkUnitRow {
        WorkUnitRow::new(
            format!("wu-{key}"),
            "task".to_string(),
            "plan".to_string(),
            seq,
            spec(key, depends_on),
            status,
            "2026-09-24T00:00:00Z".to_string(),
        )
    }

    #[test]
    fn next_work_unit_prefers_needs_continuation_then_ready_by_seq() {
        let units = vec![
            row("a", 0, WorkUnitStatus::Done, &[]),
            row("c", 2, WorkUnitStatus::Ready, &[]),
            row("b", 1, WorkUnitStatus::NeedsContinuation, &[]),
        ];
        assert_eq!(next_work_unit(&units), NextStep::RunWorkUnit("wu-b".into()));

        let units2 = vec![
            row("a", 0, WorkUnitStatus::Done, &[]),
            row("c", 2, WorkUnitStatus::Ready, &[]),
            row("b", 1, WorkUnitStatus::Ready, &[]),
        ];
        assert_eq!(
            next_work_unit(&units2),
            NextStep::RunWorkUnit("wu-b".into())
        );
    }

    #[test]
    fn next_work_unit_all_done_when_everything_active_is_done() {
        let units = vec![
            row("a", 0, WorkUnitStatus::Done, &[]),
            row("b", 1, WorkUnitStatus::Superseded, &[]),
        ];
        assert_eq!(next_work_unit(&units), NextStep::AllDone);
    }

    // ---- ADR-0074 D1.3（Phase F2）: `runnable_work_units` ----

    fn row_v2(
        key: &str,
        wu_phase: &str,
        seq: u32,
        status: WorkUnitStatus,
        depends_on: &[&str],
    ) -> WorkUnitRow {
        WorkUnitRow::new(
            format!("wu-{key}"),
            "task".to_string(),
            "plan".to_string(),
            seq,
            spec_v2(key, wu_phase, depends_on),
            status,
            "2026-09-24T00:00:00Z".to_string(),
        )
    }

    /// v1（`phase` が常に `None`）で `limit = 1` なら `next_work_unit` と同じ 1 件を返す。
    #[test]
    fn runnable_work_units_matches_next_work_unit_for_v1_with_limit_one() {
        let units = vec![
            row("a", 0, WorkUnitStatus::Done, &[]),
            row("c", 2, WorkUnitStatus::Ready, &[]),
            row("b", 1, WorkUnitStatus::NeedsContinuation, &[]),
        ];
        assert_eq!(runnable_work_units(&units, 0, 1), vec!["wu-b".to_string()]);
    }

    #[test]
    fn runnable_work_units_respects_the_parallel_limit() {
        let units = vec![
            row_v2("a", "build", 0, WorkUnitStatus::Ready, &[]),
            row_v2("b", "build", 1, WorkUnitStatus::Ready, &[]),
            row_v2("c", "build", 2, WorkUnitStatus::Ready, &[]),
        ];
        assert_eq!(
            runnable_work_units(&units, 0, 2),
            vec!["wu-a".to_string(), "wu-b".to_string()],
            "2 本まで、seq 順"
        );
        assert_eq!(
            runnable_work_units(&units, 0, 3),
            vec!["wu-a".to_string(), "wu-b".to_string(), "wu-c".to_string()]
        );
        assert!(
            runnable_work_units(&units, 3, 3).is_empty(),
            "in_flight が limit に達していれば何も起こさない"
        );
        assert_eq!(
            runnable_work_units(&units, 1, 3).len(),
            2,
            "in_flight の分だけ枠が減る"
        );
    }

    #[test]
    fn runnable_work_units_prefers_needs_continuation_over_ready() {
        let units = vec![
            row_v2("a", "build", 0, WorkUnitStatus::Ready, &[]),
            row_v2("b", "build", 1, WorkUnitStatus::NeedsContinuation, &[]),
        ];
        assert_eq!(
            runnable_work_units(&units, 0, 1),
            vec!["wu-b".to_string()],
            "needs_continuation を先に選ぶ"
        );
    }

    /// D1.3: 対象は現在の工程だけ（前の工程がまだ終わっていなければ、後の工程の ready な WU は
    /// 対象にしない）。
    #[test]
    fn runnable_work_units_only_considers_the_current_phase() {
        let units = vec![
            row_v2("a", "build", 0, WorkUnitStatus::Ready, &[]),
            // `verify` 工程は `build` に依存していないが、工程の境が障壁になる。
            row_v2("z", "verify", 1, WorkUnitStatus::Ready, &[]),
        ];
        assert_eq!(
            runnable_work_units(&units, 0, 5),
            vec!["wu-a".to_string()],
            "build 工程がまだ終わっていないので verify の WU は対象外"
        );

        // build がすべて終われば（is_terminal）、verify が「現在の工程」になる。
        let mut done = units.clone();
        done[0].status = WorkUnitStatus::Done;
        assert_eq!(runnable_work_units(&done, 0, 5), vec!["wu-z".to_string()]);
    }

    /// D1.6: 兄弟が failed/blocked のとき、新しい（ready の）WU は起こさないが、既に走ったことの
    /// ある needs_continuation の WU は続ける。
    #[test]
    fn runnable_work_units_does_not_start_new_ones_when_a_sibling_failed_but_continues_in_flight() {
        let units = vec![
            row_v2("a", "build", 0, WorkUnitStatus::Failed, &[]),
            row_v2("b", "build", 1, WorkUnitStatus::Ready, &[]),
            row_v2("c", "build", 2, WorkUnitStatus::NeedsContinuation, &[]),
        ];
        assert_eq!(
            runnable_work_units(&units, 0, 5),
            vec!["wu-c".to_string()],
            "ready の b は起こさないが、needs_continuation の c は続ける"
        );
    }

    #[test]
    fn runnable_work_units_returns_empty_when_everything_is_terminal() {
        let units = vec![
            row("a", 0, WorkUnitStatus::Done, &[]),
            row("b", 1, WorkUnitStatus::Superseded, &[]),
        ];
        assert!(runnable_work_units(&units, 0, 3).is_empty());
    }

    #[test]
    fn newly_ready_promotes_pending_whose_dependencies_are_all_done() {
        let units = vec![
            row("a", 0, WorkUnitStatus::Done, &[]),
            row("b", 1, WorkUnitStatus::Pending, &["a"]),
            row("c", 2, WorkUnitStatus::Pending, &["b"]),
        ];
        assert_eq!(newly_ready(&units), vec!["wu-b".to_string()]);
    }

    #[test]
    fn dependents_to_block_finds_the_transitive_closure() {
        let units = vec![
            row("a", 0, WorkUnitStatus::Failed, &[]),
            row("b", 1, WorkUnitStatus::Pending, &["a"]),
            row("c", 2, WorkUnitStatus::Pending, &["b"]),
            row("d", 3, WorkUnitStatus::Done, &[]),
        ];
        let mut blocked = dependents_to_block(&units, "a");
        blocked.sort();
        assert_eq!(blocked, vec!["wu-b".to_string(), "wu-c".to_string()]);
    }

    /// ADR-0072 D8 / ADR-0003 D6: 生成スキーマとコミット済みファイルの一致。`UPDATE_SCHEMA=1` で再生成。
    #[test]
    fn committed_schema_matches_generated() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/protocol/execution-plan.schema.json"
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

    /// ADR-0074 D5.3（Phase F1）: `execution-plan-delta/1` の生成スキーマとコミット済みファイルの一致。
    #[test]
    fn delta_committed_schema_matches_generated() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/protocol/execution-plan-delta.schema.json"
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

    // ---- ADR-0074 D5.3（Phase F1）: replan の差分 (add/modify/remove) ----

    fn delta(
        base_version: u32,
        add: Vec<WorkUnitSpec>,
        modify: Vec<WorkUnitPatch>,
        remove: Vec<&str>,
    ) -> ExecutionPlanDelta {
        ExecutionPlanDelta {
            schema: EXECUTION_PLAN_DELTA_SCHEMA.to_string(),
            base_version,
            rationale: "delta test".to_string(),
            add,
            modify,
            remove: remove.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn apply_delta_adds_modifies_and_removes_without_restating_untouched_units() {
        let base = plan(vec![spec("a", &[]), spec("b", &["a"]), spec("c", &["b"])]);
        let d = delta(
            1,
            vec![spec("m", &[])],
            vec![WorkUnitPatch {
                key: "b".to_string(),
                depends_on: Some(vec!["a".to_string(), "m".to_string()]),
                ..WorkUnitPatch::default()
            }],
            vec!["c"],
        );
        let applied = apply_delta(&base, &d).expect("delta applies");
        let keys: Vec<&str> = applied.work_units.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b", "m"], "{keys:?}");
        let b = applied.work_units.iter().find(|w| w.key == "b").unwrap();
        assert_eq!(b.depends_on, vec!["a".to_string(), "m".to_string()]);
        // `a` は modify/remove の対象ではないので、`base` の spec のまま（書き写していない）。
        let a = applied.work_units.iter().find(|w| w.key == "a").unwrap();
        assert_eq!(a, &spec("a", &[]));
    }

    #[test]
    fn apply_delta_rejects_modifying_an_unknown_or_removed_key() {
        let base = plan(vec![spec("a", &[])]);
        let d = delta(
            1,
            vec![],
            vec![WorkUnitPatch {
                key: "ghost".to_string(),
                title: Some("x".to_string()),
                ..WorkUnitPatch::default()
            }],
            vec![],
        );
        assert!(apply_delta(&base, &d).is_err());

        let d2 = delta(
            1,
            vec![],
            vec![WorkUnitPatch {
                key: "a".to_string(),
                title: Some("x".to_string()),
                ..WorkUnitPatch::default()
            }],
            vec!["a"],
        );
        assert!(apply_delta(&base, &d2).is_err(), "removed then modified");
    }

    #[test]
    fn apply_delta_rejects_adding_a_key_that_still_exists() {
        let base = plan(vec![spec("a", &[])]);
        let d = delta(1, vec![spec("a", &[])], vec![], vec![]);
        assert!(apply_delta(&base, &d).is_err());
    }

    // -------------------------------------------------------------------
    // ADR-0074 D1.1（Phase F2 (a)）: `celeris.execution-plan/2` の検証
    // -------------------------------------------------------------------

    /// v1 の計画は 1 バイトも挙動が変わらない（`phases`/`work_units[].phase` を書かない、
    /// 既定の `ExecutionLimits` で通る）。既存の `valid_three_step_plan_passes_and_orders_topologically`
    /// と合わせて、v1 の後方互換を確かめる。
    #[test]
    fn v1_plan_without_phases_still_validates_exactly_as_before() {
        let p = plan(vec![spec("a", &[]), spec("b", &["a"])]);
        assert!(p.phases.is_empty());
        assert!(p.children.is_empty());
        let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
        assert_eq!(validated.spec.work_units[0].phase, None);
    }

    #[test]
    fn v1_rejects_phases_being_set() {
        let mut p = plan(vec![spec("a", &[])]);
        p.phases = vec![phase("build")];
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::PhasesNotAllowedInV1)),
            "{errs:?}"
        );
    }

    #[test]
    fn v1_rejects_a_work_unit_with_a_phase_set() {
        let p = plan(vec![spec_v2("a", "build", &[])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                PlanValidationError::WorkUnitPhaseNotAllowedInV1 { key } if key == "a"
            )),
            "{errs:?}"
        );
    }

    /// ADR-0074 D1.4（Phase F2b）: v2 の採用は工程ごとに統合 WU（`integrate-<phase>`、依存はその工程の
    /// すべての WU）を末尾に足し、工程の障壁つきで ready を決める（最初の工程の依存の無い WU だけ）。
    #[test]
    fn materialize_adds_integration_units_and_only_the_first_phase_is_ready() {
        let p = plan_v2(
            vec![phase("build"), phase("verify")],
            vec![
                spec_v2("a", "build", &[]),
                spec_v2("b", "build", &[]),
                spec_v2("c", "build", &["a"]),
                spec_v2("d", "verify", &[]),
                spec_v2("e", "verify", &["a", "b"]),
            ],
        );
        let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
        let rows = materialize_work_units(
            "t",
            "p",
            &validated.spec,
            &validated.topological_order,
            "2026-09-26T00:00:00Z",
            &mut |w| format!("id-{}", w.key),
        );
        let keys: Vec<(&str, u32, WorkUnitStatus)> = rows
            .iter()
            .map(|r| (r.key.as_str(), r.seq, r.status))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("a", 0, WorkUnitStatus::Ready),
                ("b", 1, WorkUnitStatus::Ready),
                ("c", 2, WorkUnitStatus::Pending),
                ("integrate-build", 3, WorkUnitStatus::Pending),
                ("d", 4, WorkUnitStatus::Pending),
                ("e", 5, WorkUnitStatus::Pending),
                ("integrate-verify", 6, WorkUnitStatus::Pending),
            ],
            "後の工程の依存の無い WU（d）も、前の工程の統合が済むまで pending"
        );
        let integ = rows.iter().find(|r| r.key == "integrate-build").unwrap();
        assert_eq!(integ.kind, WorkUnitKind::Integrate);
        assert_eq!(integ.phase.as_deref(), Some("build"));
        assert_eq!(integ.depends_on, vec!["a", "b", "c"]);
        // 統合 WU は `ready` にならない（scheduler が直接走らせる）・runnable にも出ない。
        let mut all_done: Vec<WorkUnitRow> = rows.clone();
        for r in all_done
            .iter_mut()
            .filter(|r| r.phase.as_deref() == Some("build"))
        {
            if r.kind != WorkUnitKind::Integrate {
                r.status = WorkUnitStatus::Done;
            }
        }
        assert!(
            newly_ready(&all_done).is_empty(),
            "統合が済むまで次の工程は上がらない"
        );
        assert!(runnable_work_units(&all_done, 0, 3).is_empty());
        // 統合が done になれば次の工程が上がる（依存が done の d と e）。
        for r in all_done.iter_mut() {
            if r.key == "integrate-build" {
                r.status = WorkUnitStatus::Done;
            }
        }
        let mut up = newly_ready(&all_done);
        up.sort();
        assert_eq!(up, vec!["id-d".to_string(), "id-e".to_string()]);
        // v1 は従来どおり（統合 WU なし、依存が無ければ ready）。
        let v1 = plan(vec![spec("a", &[]), spec("b", &["a"])]);
        let v1v = validate(&v1, ExecutionLimits::default(), &[]).unwrap();
        let v1_rows = materialize_work_units(
            "t",
            "p",
            &v1v.spec,
            &v1v.topological_order,
            "2026-09-26T00:00:00Z",
            &mut |w| w.key.clone(),
        );
        assert_eq!(
            v1_rows
                .iter()
                .map(|r| (r.key.as_str(), r.status))
                .collect::<Vec<_>>(),
            vec![("a", WorkUnitStatus::Ready), ("b", WorkUnitStatus::Pending)]
        );
    }

    #[test]
    fn phase_leaves_are_the_units_nobody_in_the_phase_depends_on() {
        let mut rows = vec![
            row_v2("a", "build", 0, WorkUnitStatus::Done, &[]),
            row_v2("b", "build", 1, WorkUnitStatus::Done, &["a"]),
            row_v2("c", "build", 2, WorkUnitStatus::Done, &[]),
            row_v2("d", "verify", 3, WorkUnitStatus::Pending, &["c"]),
        ];
        for r in rows.iter_mut() {
            r.branch = Some(format!("celeris-wu/t/{}", r.key));
        }
        let leaves: Vec<&str> = phase_leaves(&rows, "build")
            .iter()
            .map(|r| r.key.as_str())
            .collect();
        assert_eq!(
            leaves,
            vec!["b", "c"],
            "a は b に積み上げられている（b に含まれる）"
        );
    }

    #[test]
    fn v2_rejects_a_key_reserved_for_integration_units() {
        let p = plan_v2(
            vec![phase("build")],
            vec![spec_v2("integrate-build", "build", &[])],
        );
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::ReservedKey { .. })),
            "{errs:?}"
        );
    }

    #[test]
    fn v2_valid_plan_with_parallel_and_stacked_units_passes() {
        let p = plan_v2(
            vec![phase("build"), phase("verify")],
            vec![
                spec_v2("a", "build", &[]),
                spec_v2("b", "build", &[]),
                // 積み上げ（D1.2）: 同じ工程で a に依存。
                spec_v2("c", "build", &["a"]),
                // 前の工程への依存はいくつでもよい（規則 3）。
                spec_v2("d", "verify", &["a", "b", "c"]),
            ],
        );
        let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid v2 plan");
        assert_eq!(validated.spec.work_units.len(), 4);
    }

    #[test]
    fn v2_rejects_no_phases() {
        let p = plan_v2(vec![], vec![]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::NoPhases)),
            "{errs:?}"
        );
    }

    #[test]
    fn v2_rejects_too_many_phases() {
        let phases: Vec<PhaseSpec> = (0..6).map(|i| phase(&format!("p{i}"))).collect();
        let p = plan_v2(phases, vec![spec_v2("a", "p0", &[])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::TooManyPhases { count, max } if *count == 6 && *max == 5)),
            "{errs:?}"
        );
    }

    #[test]
    fn v2_rejects_duplicate_phase_keys() {
        let p = plan_v2(
            vec![phase("build"), phase("build")],
            vec![spec_v2("a", "build", &[])],
        );
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::DuplicatePhaseKey { key } if key == "build")
            ),
            "{errs:?}"
        );
    }

    #[test]
    fn v2_rejects_a_work_unit_missing_its_phase() {
        let p = plan_v2(vec![phase("build")], vec![spec("a", &[])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(
                |e| matches!(e, PlanValidationError::WorkUnitMissingPhase { key } if key == "a")
            ),
            "{errs:?}"
        );
    }

    #[test]
    fn v2_rejects_a_work_unit_with_an_unknown_phase() {
        let p = plan_v2(vec![phase("build")], vec![spec_v2("a", "ghost-phase", &[])]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                PlanValidationError::UnknownWorkUnitPhase { key, phase } if key == "a" && phase == "ghost-phase"
            )),
            "{errs:?}"
        );
    }

    /// ADR-0074 D1.1 の依存の規則 1: 後の工程への依存は拒否する。
    #[test]
    fn v2_rejects_a_dependency_on_a_later_phase() {
        let p = plan_v2(
            vec![phase("build"), phase("verify")],
            vec![spec_v2("a", "build", &["b"]), spec_v2("b", "verify", &[])],
        );
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                PlanValidationError::DependencyInLaterPhase { key, depends_on }
                    if key == "a" && depends_on == "b"
            )),
            "{errs:?}"
        );
    }

    /// ADR-0074 D1.1 の依存の規則 2: 同じ工程の中の依存は高々 1 つ（鎖か木のみ）。
    #[test]
    fn v2_rejects_two_intra_phase_dependencies() {
        let p = plan_v2(
            vec![phase("build")],
            vec![
                spec_v2("a", "build", &[]),
                spec_v2("b", "build", &[]),
                spec_v2("c", "build", &["a", "b"]),
            ],
        );
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                PlanValidationError::TooManyIntraPhaseDependencies { key, phase }
                    if key == "c" && phase == "build"
            )),
            "{errs:?}"
        );
    }

    /// 同じ工程で 1 つの WU に複数の WU が依存する「木」は許す（規則 2 は「自分の依存の数」を
    /// 数えるのであって、依存されている側の数ではない）。
    #[test]
    fn v2_allows_a_tree_of_intra_phase_dependencies() {
        let p = plan_v2(
            vec![phase("build")],
            vec![
                spec_v2("a", "build", &[]),
                spec_v2("b", "build", &["a"]),
                spec_v2("c", "build", &["a"]),
            ],
        );
        validate(&p, ExecutionLimits::default(), &[]).expect("tree of dependencies is valid");
    }

    #[test]
    fn v2_uses_the_v2_work_unit_limit_not_the_v1_one() {
        let limits = ExecutionLimits::default();
        assert_eq!(limits.max_work_units, 8);
        assert_eq!(limits.max_work_units_v2, 10);
        let units: Vec<WorkUnitSpec> = (0..9)
            .map(|i| spec_v2(&format!("wu{i}"), "build", &[]))
            .collect();
        let p = plan_v2(vec![phase("build")], units);
        validate(&p, limits, &[]).expect("9 work units fit under the v2 limit of 10");
    }

    fn child(key: &str, deps: &[&str]) -> ExecutionChildSpec {
        ExecutionChildSpec {
            key: key.into(),
            title: format!("child {key}"),
            objective: format!("do {key}"),
            acceptance: vec![crate::model::Criterion {
                text: "ok".into(),
                check: crate::model::Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            }],
            genre: None,
            skills: vec![],
            features: None,
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    /// ADR-0074 D3.7（Phase F4b (f)）: `children` は v1 では拒否、v2 では検証を通り、WU は
    /// `child:<key>` で既知の子だけを指せる。
    #[test]
    fn children_are_v2_only_and_child_dependencies_must_be_known() {
        let mut v1 = plan(vec![spec("a", &[])]);
        v1.children = vec![child("c", &[])];
        let errs = validate(&v1, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::NonEmptyChildren)),
            "{errs:?}"
        );

        let mut v2 = plan_v2(
            vec![phase("build")],
            vec![spec_v2("a", "build", &["child:c"])],
        );
        v2.children = vec![child("c", &[]), child("d", &["c"])];
        validate(&v2, ExecutionLimits::default(), &[]).expect("valid v2 with children");

        let mut bad = v2.clone();
        bad.work_units[0].depends_on = vec!["child:zzz".into()];
        bad.children.push(child("e", &["e"]));
        bad.children.push(child("c", &[]));
        let mut no_acc = child("f", &[]);
        no_acc.acceptance.clear();
        bad.children.push(no_acc);
        let errs = validate(&bad, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, PlanValidationError::UnknownDependency { depends_on, .. } if depends_on == "child:zzz")), "{errs:?}");
        assert!(errs.iter().any(|e| matches!(e, PlanValidationError::UnknownChildDependency { key, .. } if key == "e")), "{errs:?}");
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::DuplicateChildKey { key } if key == "c")),
            "{errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::ChildNoAcceptance { key } if key == "f")),
            "{errs:?}"
        );

        let mut cyclic = v2.clone();
        cyclic.children = vec![child("c", &["d"]), child("d", &["c"])];
        let errs = validate(&cyclic, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::CyclicChildDependency { .. })),
            "{errs:?}"
        );
    }

    #[test]
    fn rejects_a_work_unit_with_the_reserved_integrate_kind() {
        let mut a = spec("a", &[]);
        a.kind = WorkUnitKind::Integrate;
        let p = plan(vec![a]);
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::ReservedKind { key } if key == "a")),
            "{errs:?}"
        );
    }

    #[test]
    fn rejects_a_schema_that_is_neither_v1_nor_v2() {
        let mut p = plan(vec![spec("a", &[])]);
        p.schema = "celeris.execution-plan/99".to_string();
        let errs = validate(&p, ExecutionLimits::default(), &[]).unwrap_err();
        assert!(
            errs.iter().any(|e| matches!(
                e,
                PlanValidationError::WrongSchema { found } if found == "celeris.execution-plan/99"
            )),
            "{errs:?}"
        );
    }

    /// v2 の topological order は工程順を tie-break にする（依存の無い WU 同士が工程をまたいでも、
    /// 後の工程が先に選ばれない）。
    #[test]
    fn v2_topological_order_respects_phase_order_for_independent_units() {
        // `z`（verify 工程、依存なし）と `a`（build 工程、依存なし）は互いに依存が無いが、
        // key の昇順だけで tie-break すると `a` より `z` が先に来てしまう対象にした。
        let p = plan_v2(
            vec![phase("build"), phase("verify")],
            vec![spec_v2("z", "verify", &[]), spec_v2("a", "build", &[])],
        );
        let validated = validate(&p, ExecutionLimits::default(), &[]).expect("valid");
        let order: Vec<&str> = validated
            .topological_order
            .iter()
            .map(|&i| validated.spec.work_units[i].key.as_str())
            .collect();
        assert_eq!(order, vec!["a", "z"], "{order:?}");
    }

    /// ADR-0074 F5-fix（不具合 2 の再現）: v2・2 工程で `integrate-investigate`（daemon の統合 WU）と
    /// 統合の repair WU が done のとき、planner の差分（`modify: [impl-quota]` だけ）を当てて
    /// `validate` が通る。daemon 由来の WU は差分の結果に現れず（base の spec にも無い）、行は
    /// 不変条件の対象から外れる。外さなければ（F5-1 dogfood の挙動）拒否され、文言に
    /// 「daemon が足した WU は書かなくてよい」が出る。
    #[test]
    fn replan_delta_does_not_treat_daemon_added_done_units_as_changed() {
        let base = plan_v2(
            vec![phase("investigate"), phase("implement")],
            vec![
                spec_v2("inv-a", "investigate", &[]),
                spec_v2("inv-b", "investigate", &[]),
                spec_v2("impl-quota", "implement", &["inv-a"]),
                spec_v2("impl-api", "implement", &["inv-b"]),
                spec_v2("impl-ui", "implement", &["inv-a", "inv-b"]),
            ],
        );
        let validated = validate(&base, ExecutionLimits::default(), &[]).expect("valid base");
        let mut rows = materialize_work_units(
            "t",
            "p",
            &validated.spec,
            &validated.topological_order,
            "2026-09-27T00:00:00Z",
            &mut |w| format!("id-{}", w.key),
        );
        for r in rows.iter_mut() {
            if r.phase.as_deref() == Some("investigate") {
                r.status = WorkUnitStatus::Done;
            }
        }
        // 統合の repair WU（daemon が足す。計画の spec に無い、工程を持つ）も done。
        let mut repair = row_v2(
            "integ-repair-investigate-1",
            "investigate",
            2,
            WorkUnitStatus::Done,
            &[],
        );
        repair.kind = WorkUnitKind::Repair;
        rows.push(repair);
        let integ = rows
            .iter()
            .find(|r| r.key == "integrate-investigate")
            .expect("integration unit");
        assert!(is_daemon_added_work_unit(&validated.spec, integ));
        assert!(!is_daemon_added_work_unit(
            &validated.spec,
            rows.iter().find(|r| r.key == "inv-a").expect("inv-a")
        ));

        let d = delta(
            1,
            vec![],
            vec![WorkUnitPatch {
                key: "impl-quota".to_string(),
                objective: Some("quota を runs_by_role から数える（やり直し）".to_string()),
                ..Default::default()
            }],
            vec![],
        );
        let applied = apply_delta(&validated.spec, &d).expect("delta applies");
        assert!(
            applied
                .work_units
                .iter()
                .all(|w| w.kind != WorkUnitKind::Integrate
                    && !w.key.starts_with(INTEGRATE_KEY_PREFIX)
                    && w.key != "integ-repair-investigate-1"),
            "daemon 由来の WU は差分の結果（計画の spec）に入らない"
        );
        let done = replan_done_work_units(&validated.spec, &rows);
        let done_keys: Vec<&str> = done.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(done_keys, vec!["inv-a", "inv-b"]);
        validate(&applied, ExecutionLimits::default(), &done).expect("delta replan validates");

        // 全体形式でも、planner が統合 WU を書かなければ通る（daemon が採用時に補う）。
        let full = applied.clone();
        let revalidated = validate(&full, ExecutionLimits::default(), &done).expect("full replan");
        let integ_keys: Vec<String> = integration_work_unit_specs(&revalidated.spec)
            .into_iter()
            .map(|w| w.key)
            .collect();
        assert_eq!(
            integ_keys,
            vec!["integrate-investigate", "integrate-implement"]
        );

        // F5-1 dogfood の挙動（daemon 由来の WU も不変条件に入れる）は拒否され、文言が案内する。
        let naive: Vec<(String, WorkUnitSpec)> = rows
            .iter()
            .filter(|u| u.status == WorkUnitStatus::Done)
            .map(|u| (u.key.clone(), u.spec.clone()))
            .collect();
        let errs = validate(&applied, ExecutionLimits::default(), &naive).unwrap_err();
        let msg = errs
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            msg.contains("done work unit integrate-investigate must not change on replan"),
            "{msg}"
        );
        assert!(msg.contains("daemon が足した WU"), "{msg}");
    }

    // ---- ADR-0079 R1a (a): /1・/2 の fixture の検証結果と出力 JSON が変わらないこと ----

    /// fixture 1 本の「検証の結果と出力」を決定的な JSON にする（parse → 再直列化、検証の結果、
    /// 採用時の行）。スナップショットは R1a の変更の**前**のコードで作った（`UPDATE_PLAN_FIXTURES=1`）。
    fn fixture_outcome(text: &str) -> serde_json::Value {
        let spec: ExecutionPlanSpec = match serde_json::from_str(text) {
            Ok(s) => s,
            Err(e) => return serde_json::json!({ "parse_error": e.to_string() }),
        };
        let reserialized = serde_json::to_string(&spec).unwrap();
        match validate(&spec, ExecutionLimits::default(), &[]) {
            Err(errors) => serde_json::json!({
                "reserialized": reserialized,
                "errors": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            }),
            Ok(v) => {
                let rows = materialize_work_units(
                    "task",
                    "plan",
                    &v.spec,
                    &v.topological_order,
                    "T",
                    &mut |w| format!("id-{}", w.key),
                );
                let rows: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "id": r.id, "key": r.key, "seq": r.seq, "kind": r.kind.as_str(),
                            "status": r.status.as_str(), "phase": r.phase,
                            "depends_on": r.depends_on,
                            "spec": serde_json::to_string(&r.spec).unwrap(),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "reserialized": reserialized,
                    "validated": serde_json::to_string(&v.spec).unwrap(),
                    "topological_order": v.topological_order,
                    "rounding_notes": v.rounding_notes,
                    "rows": rows,
                })
            }
        }
    }

    #[test]
    fn plan_v1_and_v2_fixtures_are_byte_identical() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/execution-plan");
        for name in [
            "v1-basic",
            "v1-invalid",
            "v2-phases",
            "v2-children",
            "v2-invalid",
        ] {
            let input = std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap();
            let outcome = serde_json::to_string_pretty(&fixture_outcome(&input)).unwrap() + "\n";
            let snap_path = format!("{dir}/{name}.expected.json");
            if std::env::var_os("UPDATE_PLAN_FIXTURES").is_some() {
                std::fs::write(&snap_path, &outcome).unwrap();
            }
            let expected = std::fs::read_to_string(&snap_path)
                .unwrap_or_else(|e| panic!("read {snap_path}: {e}"));
            assert_eq!(expected, outcome, "fixture {name} changed");
        }
    }

    // ---- ADR-0079（Phase R1a）: `celeris.execution-plan/3` ----

    fn tree_on() -> ExecutionLimits {
        ExecutionLimits {
            tree: crate::tree::TreeLimits {
                enabled: true,
                ..crate::tree::TreeLimits::default()
            },
            ..ExecutionLimits::default()
        }
    }

    fn v3_fixture() -> ExecutionPlanSpec {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/execution-plan/v3-browser.json"
        ))
        .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn v3_errors(spec: &ExecutionPlanSpec) -> Vec<PlanValidationError> {
        validate(spec, tree_on(), &[]).expect_err("must be rejected")
    }

    fn leaf(key: &str, stage: &str) -> PlanUnitSpec {
        PlanUnitSpec {
            key: key.into(),
            stage: stage.into(),
            kind: WorkUnitKind::Implement,
            title: format!("leaf {key}"),
            objective: format!("objective of leaf {key} that is distinct"),
            depends_on: vec![],
            needs_decisions: vec![],
            decisions: vec![],
            acceptance: vec![],
            genre: None,
            skills: vec![],
            repos: vec![],
            adopt: None,
            done_when: vec![],
            checks: vec![WorkUnitCheck {
                cmd: "true".into(),
                expect_exit: 0,
            }],
            context: UnitContext::default(),
            harness: None,
            budget: None,
            outputs: vec![],
            features: None,
        }
    }

    fn task_unit(key: &str, stage: &str) -> PlanUnitSpec {
        PlanUnitSpec {
            kind: WorkUnitKind::Task,
            title: format!("task {key}"),
            objective: format!("objective of child task {key} which differs"),
            checks: vec![],
            acceptance: vec![crate::model::Criterion {
                text: "ok".into(),
                check: crate::model::Check::Reviewer,
            }],
            ..leaf(key, stage)
        }
    }

    /// R1a (a): /3 の fixture が通り、段階ごとに統合 WU が付き、kind task の unit は `task` の行になり、
    /// 回答を待つ決定（`needs_decisions` と `needed_before`）が行に写る。最初の段階だけが ready。
    #[test]
    fn v3_fixture_validates_and_materializes_stages_units_and_decisions() {
        let spec = v3_fixture();
        let v = validate(&spec, tree_on(), &[]).expect("valid /3");
        assert!(v.rounding_notes.is_empty());
        // JSON の往復（/3 は `stages`/`units`/`decisions` を出し、/2 の欄は空のまま）。
        let back: ExecutionPlanSpec =
            serde_json::from_str(&serde_json::to_string(&v.spec).unwrap()).unwrap();
        assert_eq!(back, spec);
        assert!(spec.work_units.is_empty() && spec.phases.is_empty());

        let rows =
            materialize_work_units("t", "plan", &v.spec, &v.topological_order, "T", &mut |w| {
                format!("id-{}", w.key)
            });
        type RowSummary<'a> = (String, &'a str, &'a str, Option<String>, Vec<String>);
        let summary: Vec<RowSummary> = rows
            .iter()
            .map(|r| {
                (
                    r.key.clone(),
                    r.kind.as_str(),
                    r.status.as_str(),
                    r.phase.clone(),
                    r.needs_decisions.clone(),
                )
            })
            .collect();
        let s = |x: &str| x.to_string();
        assert_eq!(
            summary,
            vec![
                (s("p1"), "task", "ready", Some(s("phase-1")), vec![]),
                (s("p1-note"), "design", "ready", Some(s("phase-1")), vec![]),
                (
                    s("integrate-phase-1"),
                    "integrate",
                    "pending",
                    Some(s("phase-1")),
                    vec![]
                ),
                (
                    s("p2-a"),
                    "task",
                    "pending",
                    Some(s("phase-2")),
                    vec![s("h2")]
                ),
                (
                    s("p2-b"),
                    "task",
                    "pending",
                    Some(s("phase-2")),
                    vec![s("h1"), s("h3")]
                ),
                (
                    s("integrate-phase-2"),
                    "integrate",
                    "pending",
                    Some(s("phase-2")),
                    vec![]
                ),
                (
                    s("p3"),
                    "implement",
                    "pending",
                    Some(s("phase-3")),
                    vec![s("h2")]
                ),
                (
                    s("integrate-phase-3"),
                    "integrate",
                    "pending",
                    Some(s("phase-3")),
                    vec![]
                ),
            ]
        );
        // 統合 WU は段階のすべての unit（子 task を含む）に依存する。
        let integ2 = rows.iter().find(|r| r.key == "integrate-phase-2").unwrap();
        assert_eq!(
            integ2.depends_on,
            vec!["p2-a".to_string(), "p2-b".to_string()]
        );
        // kind task の unit は LLM run を起こさない（scheduler の候補にならない。子の生成は R1b）。
        assert_eq!(
            runnable_work_units(&rows, 0, 3),
            vec!["id-p1-note".to_string()]
        );
        // unit の `decisions` は計画の決定に `needed_before: [<unit>]` を付けて読む。
        let all = normalized_decisions(&spec);
        assert_eq!(
            all.iter().map(|d| d.key.as_str()).collect::<Vec<_>>(),
            vec!["h1", "h2", "h3"]
        );
        assert_eq!(all[2].needed_before, vec!["p2-b".to_string()]);
    }

    /// R1a (c): `[execution.tree] enabled = false`（既定）なら /3 は `TreeDisabled` だけで拒否される。
    #[test]
    fn v3_is_rejected_when_tree_is_disabled() {
        let errs = validate(&v3_fixture(), ExecutionLimits::default(), &[]).unwrap_err();
        assert_eq!(errs, vec![PlanValidationError::TreeDisabled]);
        let msg = errs[0].to_string();
        assert!(msg.contains("[execution.tree] enabled = true"), "{msg}");
        assert!(msg.contains(EXECUTION_PLAN_SCHEMA_V3), "{msg}");
    }

    /// R1a (b): leaf に機械的な検査が無い。
    #[test]
    fn rejects_leaf_without_checks() {
        let mut p = v3_fixture();
        p.units[1].checks.clear();
        let errs = v3_errors(&p);
        assert!(
            errs.contains(&PlanValidationError::LeafWithoutChecks {
                key: "p1-note".into()
            }),
            "{errs:?}"
        );
        assert!(errs.iter().any(|e| e.to_string().contains("task")));
    }

    /// R1a (b): leaf の `context.repo` が 2 つ。
    #[test]
    fn rejects_leaf_with_two_repos() {
        let mut p = v3_fixture();
        p.units[1].context.repo = Some(RepoSelector::Many(vec!["a".into(), "b".into()]));
        assert!(
            v3_errors(&p).contains(&PlanValidationError::LeafMultipleRepos {
                key: "p1-note".into(),
                count: 2
            })
        );
        // 1 つなら文字列でも配列でも通る。
        p.units[1].context.repo = Some(RepoSelector::Many(vec!["a".into()]));
        validate(&p, tree_on(), &[]).expect("one repo is fine");
    }

    /// R1a (b): kind task の unit に `checks`（と `budget` / `harness` / `context.paths`）。
    #[test]
    fn rejects_task_unit_with_checks() {
        let mut p = v3_fixture();
        p.units[0].checks = vec![WorkUnitCheck {
            cmd: "true".into(),
            expect_exit: 0,
        }];
        p.units[0].budget = Some(WorkUnitBudget {
            max_turns: Some(10),
            max_wall_secs: None,
        });
        p.units[0].harness = Some("claude-code".into());
        p.units[0].context.paths = vec!["src/".into()];
        let errs = v3_errors(&p);
        for field in ["checks", "budget", "harness", "context.paths"] {
            assert!(
                errs.contains(&PlanValidationError::TaskUnitFieldNotAllowed {
                    key: "p1".into(),
                    field
                }),
                "{field}: {errs:?}"
            );
        }
    }

    /// R1a (b): kind task の unit に `acceptance` が無い。
    #[test]
    fn rejects_task_unit_without_acceptance() {
        let mut p = v3_fixture();
        p.units[0].acceptance.clear();
        assert!(
            v3_errors(&p).contains(&PlanValidationError::TaskUnitNoAcceptance { key: "p1".into() })
        );
        // human の受け入れ条件には成果物が要る（ADR-0067 D2。子の生成と同じ規則）。
        let mut p = v3_fixture();
        p.units[3].acceptance.truncate(1);
        assert!(v3_errors(&p).iter().any(|e| matches!(
            e,
            PlanValidationError::TaskUnitInvalidAcceptance { key, .. } if key == "p2-b"
        )));
    }

    /// R1a (b): /2 の `child:<key>` の依存は /3 では書けない。
    #[test]
    fn rejects_child_prefix_dependency_in_v3() {
        let mut p = v3_fixture();
        p.units[4].depends_on = vec!["child:p2-b".into()];
        assert!(
            v3_errors(&p).contains(&PlanValidationError::ChildDependencyNotAllowed {
                key: "p3".into(),
                depends_on: "child:p2-b".into()
            })
        );
    }

    /// R1a (b): /2 の `children` は /3 では書けない（`phases` / `work_units` も）。
    #[test]
    fn rejects_children_in_v3() {
        let mut p = v3_fixture();
        p.children = vec![child("c", &[])];
        p.phases = vec![phase("x")];
        p.work_units = vec![spec("w", &[])];
        let errs = v3_errors(&p);
        for field in ["children", "phases", "work_units"] {
            assert!(
                errs.contains(&PlanValidationError::V2FieldInV3 { field }),
                "{field}: {errs:?}"
            );
        }
        assert!(
            PlanValidationError::V2FieldInV3 { field: "children" }
                .to_string()
                .contains("kind \"task\"")
        );
    }

    /// R1a (b): 依存の循環（同じ段階の中で互いに依存）。
    #[test]
    fn rejects_cycle_in_v3() {
        let mut p = v3_fixture();
        // phase-2 の p2-a -> p2-b -> p2-a（どちらも同じ段階の依存は 1 つだけ）。
        p.units[2].depends_on = vec!["p2-b".into()];
        let errs = v3_errors(&p);
        assert!(
            errs.iter()
                .any(|e| matches!(e, PlanValidationError::CyclicDependency { .. })),
            "{errs:?}"
        );
    }

    /// R1a (b): `needed_before` が計画に無い unit / 段階を指す。
    #[test]
    fn rejects_unknown_needed_before() {
        let mut p = v3_fixture();
        p.decisions[0].needed_before = vec!["nope".into(), "stage:phase-9".into()];
        let errs = v3_errors(&p);
        for target in ["nope", "stage:phase-9"] {
            assert!(
                errs.contains(&PlanValidationError::UnknownNeededBefore {
                    key: "h1".into(),
                    target: target.into()
                }),
                "{target}: {errs:?}"
            );
        }
        // 空の `needed_before`（何を止めるか書いていない）も拒否。
        let mut p = v3_fixture();
        p.decisions[0].needed_before.clear();
        assert!(v3_errors(&p).iter().any(|e| matches!(
            e,
            PlanValidationError::InvalidDecision { detail } if detail.contains("needed_before")
        )));
    }

    /// R1a (b): `needs_decisions` が計画に無い決定を指す。
    #[test]
    fn rejects_unknown_needs_decision() {
        let mut p = v3_fixture();
        p.units[2].needs_decisions = vec!["h9".into()];
        assert!(
            v3_errors(&p).contains(&PlanValidationError::UnknownNeedsDecision {
                key: "p2-a".into(),
                decision: "h9".into()
            })
        );
    }

    /// R1a (b): 段階あたり 7 unit（既定の上限 6）。
    #[test]
    fn rejects_seven_units_in_a_stage() {
        let mut p = v3_fixture();
        for i in 0..5 {
            p.units.push(leaf(&format!("extra-{i}"), "phase-1"));
        }
        assert_eq!(p.units.iter().filter(|u| u.stage == "phase-1").count(), 7);
        assert!(
            v3_errors(&p).contains(&PlanValidationError::TooManyUnitsInStage {
                stage: "phase-1".into(),
                count: 7,
                max: 6
            })
        );
        p.units.pop();
        validate(&p, tree_on(), &[]).expect("6 units fit");
    }

    /// R1a (b): 決定 9 件（計画あたりの上限 8）。
    #[test]
    fn rejects_nine_decisions() {
        let mut p = v3_fixture();
        let template = p.decisions[0].clone();
        for i in 0..6 {
            let mut d = template.clone();
            d.key = format!("x{i}");
            p.decisions.push(d);
        }
        assert_eq!(normalized_decisions(&p).len(), 9);
        assert!(
            v3_errors(&p).contains(&PlanValidationError::TooManyDecisions { count: 9, max: 8 })
        );
        p.decisions.pop();
        validate(&p, tree_on(), &[]).expect("8 decisions fit");
    }

    /// D2: 決定の key の重複と形（選択肢 1 つ・推奨が選択肢に無い）。
    #[test]
    fn rejects_duplicate_and_malformed_decisions() {
        let mut p = v3_fixture();
        p.decisions[1].key = "h1".into();
        p.decisions[0].options.truncate(1);
        let errs = v3_errors(&p);
        assert!(errs.contains(&PlanValidationError::DuplicateDecisionKey { key: "h1".into() }));
        assert!(errs.iter().any(|e| matches!(
            e,
            PlanValidationError::InvalidDecision { detail } if detail.contains("options")
        )));
    }

    /// U-R1: kind task の unit は `depth < max_depth` の task の計画だけ（既定 3 層: root・子は可、孫は不可）。
    #[test]
    fn rejects_task_unit_at_max_depth() {
        let p = v3_fixture();
        for depth in [1, 2] {
            validate_with(
                &p,
                tree_on(),
                &[],
                PlanContext {
                    origin: PlanOrigin::Planner,
                    depth,
                },
            )
            .unwrap_or_else(|e| panic!("depth {depth}: {e:?}"));
        }
        let errs = validate_with(
            &p,
            tree_on(),
            &[],
            PlanContext {
                origin: PlanOrigin::Planner,
                depth: 3,
            },
        )
        .unwrap_err();
        assert!(errs.contains(&PlanValidationError::ChildTaskTooDeep {
            key: "p1".into(),
            depth: 3,
            max_depth: 3
        }));
        // leaf だけの計画は最下段（depth 3）でも書ける。
        let mut leaves_only = p.clone();
        leaves_only.units.retain(|u| !u.is_task());
        leaves_only
            .units
            .iter_mut()
            .for_each(|u| u.depends_on.clear());
        leaves_only.decisions.clear();
        validate_with(
            &leaves_only,
            tree_on(),
            &[],
            PlanContext {
                origin: PlanOrigin::Planner,
                depth: 3,
            },
        )
        .expect("leaves are fine at the bottom level");
    }

    /// ADR-0079 R5b-fix1（2 つ目の規則）: planner の replan は、前の版の done の unit をそのまま写すなら `adopt` を
    /// 残してよい（done の不変条件がそれを強いる）。done の写しでない unit の新しい `adopt` は従来どおり拒む。
    #[test]
    fn planner_replan_may_keep_adopt_on_a_verbatim_done_carry_over() {
        let mut p = v3_fixture();
        p.units[0].adopt = Some(crate::model::TaskId::new());
        let key = p.units[0].key.clone();
        let done = [(key.clone(), p.units[0].to_work_unit_spec())];
        validate_with(&p, tree_on(), &done, ctx(PlanOrigin::Planner))
            .expect("a verbatim done carry-over may keep its adopt");

        // done の写しでない（done が無い・spec が違う）unit の `adopt` は planner には許さない。
        let errs = validate_with(&p, tree_on(), &[], ctx(PlanOrigin::Planner)).unwrap_err();
        assert!(errs.contains(&PlanValidationError::AdoptNotAllowed { key: key.clone() }));
        let mut changed = p.clone();
        changed.units[0].title = "a different title for the adopted unit".into();
        let errs = validate_with(&changed, tree_on(), &done, ctx(PlanOrigin::Planner)).unwrap_err();
        assert!(errs.contains(&PlanValidationError::AdoptNotAllowed { key: key.clone() }));
        assert!(errs.contains(&PlanValidationError::DoneWorkUnitChanged { key: key.clone() }));
        let msg = PlanValidationError::AdoptNotAllowed { key }.to_string();
        assert!(msg.contains("copied verbatim"), "{msg}");
    }

    /// D2 / D15: `adopt` は人の計画（origin human）だけ。leaf には書けない。
    #[test]
    fn adopt_is_only_allowed_in_human_plans() {
        let mut p = v3_fixture();
        p.units[0].adopt = Some(crate::model::TaskId::new());
        assert!(v3_errors(&p).contains(&PlanValidationError::AdoptNotAllowed { key: "p1".into() }));
        validate_with(
            &p,
            tree_on(),
            &[],
            PlanContext {
                origin: PlanOrigin::Human,
                depth: 1,
            },
        )
        .expect("a human may adopt an existing task");
        let mut p = v3_fixture();
        p.units[1].adopt = Some(crate::model::TaskId::new());
        p.units[1].acceptance = p.units[0].acceptance.clone();
        let errs = validate_with(
            &p,
            tree_on(),
            &[],
            PlanContext {
                origin: PlanOrigin::Human,
                depth: 1,
            },
        )
        .unwrap_err();
        assert!(errs.contains(&PlanValidationError::LeafFieldNotAllowed {
            key: "p1-note".into(),
            field: "adopt"
        }));
        assert!(errs.contains(&PlanValidationError::LeafFieldNotAllowed {
            key: "p1-note".into(),
            field: "acceptance"
        }));
    }

    /// D4 (2) (a): /3 の leaf は予算を丸めずに拒否する（/1・/2 は従来どおり丸める）。
    #[test]
    fn v3_leaf_budget_over_the_limit_is_rejected_not_rounded() {
        let mut p = v3_fixture();
        p.units[1].budget = Some(WorkUnitBudget {
            max_turns: Some(81),
            max_wall_secs: Some(3601),
        });
        let errs = v3_errors(&p);
        assert_eq!(
            errs.iter()
                .filter(|e| matches!(e, PlanValidationError::LeafBudgetOverLimit { .. }))
                .count(),
            2,
            "{errs:?}"
        );
    }

    /// D2: 段階の形（空・上限・key・kind）と unit の段階・予約語。
    #[test]
    fn rejects_malformed_stages_and_unknown_unit_stage() {
        let mut p = v3_fixture();
        p.stages.push(StageSpec {
            key: "phase-1".into(),
            kind: WorkUnitKind::Task,
            title: "dup".into(),
            review: StageReview::None,
        });
        p.units[4].stage = "phase-9".into();
        let mut reserved = leaf("integrate-phase-1", "phase-1");
        reserved.kind = WorkUnitKind::Integrate;
        p.units.push(reserved);
        let errs = v3_errors(&p);
        assert!(errs.contains(&PlanValidationError::DuplicateStageKey {
            key: "phase-1".into()
        }));
        assert!(errs.contains(&PlanValidationError::InvalidStageKind {
            key: "phase-1".into()
        }));
        assert!(errs.contains(&PlanValidationError::UnknownUnitStage {
            key: "p3".into(),
            stage: "phase-9".into()
        }));
        assert!(errs.contains(&PlanValidationError::ReservedKey {
            key: "integrate-phase-1".into()
        }));
        assert!(errs.contains(&PlanValidationError::ReservedKind {
            key: "integrate-phase-1".into()
        }));

        let mut empty = v3_fixture();
        empty.stages.clear();
        empty.units.clear();
        empty.decisions.clear();
        let errs = v3_errors(&empty);
        assert!(errs.contains(&PlanValidationError::NoStages));
        assert!(errs.contains(&PlanValidationError::NoUnits));

        let mut many = v3_fixture();
        for i in 0..3 {
            many.stages.push(StageSpec {
                key: format!("more-{i}"),
                kind: WorkUnitKind::Test,
                title: format!("more {i}"),
                review: StageReview::None,
            });
        }
        assert!(
            v3_errors(&many).contains(&PlanValidationError::TooManyStages { count: 6, max: 5 })
        );
    }

    /// D3: 計画あたりの kind task の unit の上限（既定 6）。
    #[test]
    fn rejects_too_many_child_task_units() {
        let mut p = v3_fixture();
        p.decisions.clear();
        p.units.iter_mut().for_each(|u| {
            u.needs_decisions.clear();
            u.decisions.clear();
        });
        for i in 0..4 {
            p.units.push(task_unit(&format!("more-{i}"), "phase-3"));
        }
        assert!(
            v3_errors(&p).contains(&PlanValidationError::TooManyChildTasks { count: 7, max: 6 })
        );
    }

    /// /1・/2 に /3 の欄・語彙を書けば拒否（`kind = task`・`stages` / `units` / `decisions`）。
    #[test]
    fn v1_and_v2_reject_v3_fields_and_the_task_kind() {
        let mut a = spec("a", &[]);
        a.kind = WorkUnitKind::Task;
        let mut p = plan(vec![a]);
        p.stages = v3_fixture().stages;
        p.units = vec![leaf("u", "phase-1")];
        p.decisions = v3_fixture().decisions;
        let errs = validate(&p, tree_on(), &[]).unwrap_err();
        assert!(errs.contains(&PlanValidationError::TaskKindRequiresV3 { key: "a".into() }));
        for field in ["stages", "units", "decisions"] {
            assert!(
                errs.contains(&PlanValidationError::V3FieldNotAllowed { field }),
                "{field}"
            );
        }
        let mut b = spec_v2("b", "build", &[]);
        b.kind = WorkUnitKind::Task;
        let errs = validate(&plan_v2(vec![phase("build")], vec![b]), tree_on(), &[]).unwrap_err();
        assert!(errs.contains(&PlanValidationError::TaskKindRequiresV3 { key: "b".into() }));
        // /1・/2 の検証は `[execution.tree]` を見ない（enabled でも既定でも同じ結果）。
        let v1 = plan(vec![spec("a", &[]), spec("b", &["a"])]);
        assert_eq!(
            validate(&v1, tree_on(), &[]),
            validate(&v1, ExecutionLimits::default(), &[])
        );
    }

    /// `schema` の値で /3 に振り分け、未知の版は従来どおり `WrongSchema`（文言は /3 を含む）。
    #[test]
    fn wrong_schema_message_lists_all_three_versions() {
        let msg = PlanValidationError::WrongSchema { found: "x".into() }.to_string();
        assert!(msg.contains(EXECUTION_PLAN_SCHEMA_V3), "{msg}");
        assert!(is_phased_schema(EXECUTION_PLAN_SCHEMA_V3));
        assert!(is_phased_schema(EXECUTION_PLAN_SCHEMA_V2));
        assert!(!is_phased_schema(EXECUTION_PLAN_SCHEMA));
    }

    /// D2: `deny_unknown_fields`（`assignee` / `tier` / `lane` は書けない）と `review` の語彙。
    #[test]
    fn v3_json_rejects_unknown_fields_and_parses_review() {
        let mut v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/execution-plan/v3-browser.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let parsed: ExecutionPlanSpec = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(parsed.stages[1].review, StageReview::Human);
        assert_eq!(parsed.stages[0].review, StageReview::None);
        v["units"][0]["assignee"] = serde_json::json!("dev");
        assert!(serde_json::from_value::<ExecutionPlanSpec>(v.clone()).is_err());
        v["units"][0].as_object_mut().unwrap().remove("assignee");
        v["stages"][0]["review"] = serde_json::json!("sometimes");
        assert!(serde_json::from_value::<ExecutionPlanSpec>(v).is_err());
    }
}
