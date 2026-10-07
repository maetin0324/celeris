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
    /// ADR-0074 付記 2026-10-05（`WorkUnitCheck.scope`）: 範囲 check（WU 自身の変更が許可範囲に収まるかを見る
    /// 検査）か。`true` の check は WU の作業時（`spawn_work_unit_checks`）だけで流し、段の統合の検査
    /// （D1.4 の 4）と子 task の acceptance（`promote_to_task`）には入れない。既定 `false`（JSON に書かない）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub scope: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// ADR-0074 付記 2026-10-05 D3: WU の checks と WU の run の環境に入る、WU の `base_commit`（範囲 check の基点）。
pub const WU_BASE_ENV: &str = "CELERIS_WU_BASE";
/// ADR-0074 付記 2026-10-05 D3: WU の checks と WU の run の環境に入る、統合先（Task のブランチ `celeris/<task_id>`）。
pub const WU_TARGET_ENV: &str = "CELERIS_WU_TARGET";

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
    /// Browser origins for a child task. An inherited browser skill still needs an explicit set.
    #[serde(
        default,
        skip_serializing_if = "crate::model::TaskRequirements::is_empty"
    )]
    pub requirements: crate::model::TaskRequirements,
    /// Optional repository-relative prefixes inherited by child tasks or leaf WUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_write_paths: Option<Vec<String>>,
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
    /// ADR-0079「R6-2」: 子 task の Complexity Gate の明示（`compound` = 子は自分の計画を持つ、`atomic` = 子は計画を
    /// 持たず 1 つの節点として走る）。子の `routing.execution_hint = {<gate>, explicit: true}`。省いたら
    /// `compound`（kind task を選んだこと自体が「自分の計画が要る」の意味。`tree::task_unit_execution_hint`）。
    /// 人の計画・planner の計画のどちらも書ける。leaf には書けない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<crate::execution_gate::ExecutionMode>,
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
    #[serde(
        default,
        skip_serializing_if = "crate::model::TaskRequirements::is_empty"
    )]
    pub requirements: crate::model::TaskRequirements,
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

mod scheduling;
mod validation;

pub use scheduling::*;
pub use validation::*;

#[cfg(test)]
#[path = "execution_plan/tests.rs"]
mod tests;
