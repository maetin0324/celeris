//! ADR-0079（Phase R1a）: 再帰的な task 分解の木の純粋な型と関数。
//!
//! - [`TreeInfo`]: `Task.tree`（木の中の位置。root / 深さ / 親の unit / 基点）。
//! - [`TreeLimits`]: `[execution.tree]` の上限（D3。R0 の付記 U-R1 の数え方）。
//! - 深さの計算（`can_have_child_tasks` / `remaining_depth` / `gate_threshold`）。
//! - [`UnitDeclared`] / [`UnitGateAction`]: `Event::UnitGateOverridden` の語彙。
//! - Phase R2a: unit の gate（[`unit_gate`] / [`apply_unit_gates`]。D4 (3)）、木の上限の数え上げ
//!   （[`tree_counters`] / [`plan_limit_holds`] / [`run_limit_breach`]。D3）、daemon の決定の要求
//!   （[`limit_decision`] / [`leaf_too_large_decision`]。D7）。
//!
//! **深さの数え方**（ADR-0079 付記 U-R1。D3 の表の「root → 子 task → leaf」を置き換える）:
//! `max_depth` は **task の層数**で数える。root = 1、子 = 2、孫 = 3。leaf（WorkUnit）はどの層の task にも
//! ぶら下がり、深さに数えない。したがって深さ `d` の task が kind task の unit（子 task）を持てるのは
//! `d < max_depth` のとき（既定 3 なら root と子は子 task を持て、孫は持てない）。
//!
//! I/O・LLM 呼び出しはしない（ADR-0001 D2、ADR-0079 D16）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Task, TaskId};

/// D3 / U-R1: `max_depth` の既定（task の層数）。
pub const DEFAULT_MAX_DEPTH: u32 = 3;
/// D3: `max_depth` の上限（広げるには ADR）。設定は `1..=MAX_DEPTH_CAP` だけを受け付ける。
pub const MAX_DEPTH_CAP: u32 = 3;
/// D3: 深さ 1（root）の gate の閾値（ADR-0072 D13 の `EXECUTION_GATE_SCORE_THRESHOLD` と同じ 5）。
pub const ROOT_GATE_THRESHOLD: u32 = 5;

/// D4 (4): 子 task が親の計画のどの unit から作られたか（`Task.tree.parent_unit`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParentUnit {
    /// 親 task（`Task.parent_id` と同じ値。採用〈adopt、D15〉で `parent_id` を書き換えない子では
    /// こちらだけが木の親を指す）。
    pub task_id: TaskId,
    /// 親の計画の版（`execution_plans.id`）。
    pub plan_id: String,
    /// 親の計画の unit の key（`work_units.key`）。
    pub unit_key: String,
    /// その unit の段階の key（`work_units.phase`）。
    pub stage: String,
    /// ADR-0079 D9（Phase R2b）: 同じ unit から作った子の何回目か（1 始まり）。子の work の失敗で親の
    /// replan が同じ key の unit を残したとき・基盤の失敗で自動で 1 回作り直したときに増える。
    /// 1 は書かない（R2b より前の子の JSON は 1 バイトも変わらない）。
    #[serde(default = "first_attempt", skip_serializing_if = "is_first_attempt")]
    pub attempt: u32,
}

fn first_attempt() -> u32 {
    1
}

fn is_first_attempt(n: &u32) -> bool {
    *n <= 1
}

/// ADR-0079 D12（Phase R2b）: 人が名指しした段階（`Task.routing.stages_hint`）。planner への入力で、
/// 構造の強制ではない（段階の数・名前は planner が決める）。CoS の `create_task` から写すのは R5a。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StageHint {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
}

/// ADR-0079 D9（Phase R2b）: 子 task が基盤の分類（`classify_task_failure` の infra）で `failed` になったとき、
/// 同じ unit から自動で作り直す回数（`max_child_infra_retries`）。超えたら障害通知と unit `blocked(infra)`。
pub const MAX_CHILD_INFRA_RETRIES: u32 = 1;

/// D4 (4) / D15: `Task.tree`。木に属する task だけが持つ（`None` は木を持たない従来の task = 深さ 1 の
/// 節点として扱う。`root_id` 列も NULL のまま埋め戻さない。D15）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeInfo {
    /// 木の root task（root 自身なら自分の id）。`tasks.root_id` 列（索引）の正本。
    pub root_id: TaskId,
    /// task の層数で数えた深さ（root = 1、子 = 2、孫 = 3。U-R1）。
    pub depth: u32,
    /// 子 task なら、作られた元の親の unit（root は `None`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_unit: Option<ParentUnit>,
    /// D6: 子の worktree を切る基点（親の段階の基点、または同じ段階の依存先の HEAD。R1c で使う）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_commit: Option<String>,
}

impl TreeInfo {
    /// root task の `tree`（深さ 1、親の unit なし）。
    pub fn root(id: TaskId) -> Self {
        TreeInfo {
            root_id: id,
            depth: 1,
            parent_unit: None,
            base_commit: None,
        }
    }

    /// `parent`（木の節点）の unit から作る子の `tree`（深さ = 親 + 1）。
    pub fn child_of(parent: &Task, unit: ParentUnit, base_commit: Option<String>) -> Self {
        TreeInfo {
            root_id: root_id_of(parent),
            depth: depth_of(parent) + 1,
            parent_unit: Some(unit),
            base_commit,
        }
    }
}

/// task の深さ（`tree` の無い task は深さ 1。D15）。
pub fn depth_of(task: &Task) -> u32 {
    task.tree.as_ref().map(|t| t.depth).unwrap_or(1)
}

/// task の木の root（`tree` の無い task は自分自身）。
pub fn root_id_of(task: &Task) -> TaskId {
    task.tree.as_ref().map(|t| t.root_id).unwrap_or(task.id)
}

/// ADR-0079 D6（Phase R1c）: 親の計画の unit から作られた木の子 task か（`tree.parent_unit` を持つ）。
/// 子の成果は親のブランチに取り込まれ、main への取り込み（ADR-0051 の `deliveries`・ADR-0043 D5 の
/// 取り込み）と `TaskReady` の通知は持たない。root（`tree` を持たない task）は `false`。
pub fn is_tree_child(task: &Task) -> bool {
    tree_parent(task).is_some()
}

/// D6: 木の子 task の親（`tree.parent_unit.task_id`。採用〈adopt〉で `parent_id` を書き換えない子でも
/// 木の親を指す）。木の子でなければ `None`。
pub fn tree_parent(task: &Task) -> Option<TaskId> {
    task.tree
        .as_ref()
        .and_then(|t| t.parent_unit.as_ref())
        .map(|u| u.task_id)
}

/// D6: 木の子 task の取り込み先 = 親の task ブランチ（`<prefix><parent_id>`。`prefix` は設定の
/// `worktree_branch_prefix`、既定 `celeris/`）。木の子でなければ `None`（root は main と比べる）。
pub fn parent_branch(task: &Task, prefix: &str) -> Option<String> {
    tree_parent(task).map(|parent| format!("{prefix}{parent}"))
}

/// D6: 木の子 task の worktree を切る基点（`tree.base_commit`）。木の子でなければ `None`。
pub fn child_base_commit(task: &Task) -> Option<&str> {
    if !is_tree_child(task) {
        return None;
    }
    task.tree.as_ref().and_then(|t| t.base_commit.as_deref())
}

/// U-R1: 深さ `depth` の task の計画が kind task の unit（子 task）を持てるか（`depth < max_depth`）。
pub fn can_have_child_tasks(depth: u32, max_depth: u32) -> bool {
    depth < max_depth
}

/// U-R1: 深さ `depth` の task の下にまだ作れる task の層の数（`max_depth − depth`、負にはしない）。
/// 1 以上なら計画に kind task の unit を書ける（D4 (2) の planner の入力 `remaining_depth`）。
pub fn remaining_depth(depth: u32, max_depth: u32) -> u32 {
    max_depth.saturating_sub(depth)
}

/// D3: 深さ `depth` の gate の閾値（`5 + step × (depth − 1)`。depth 2・step 2 なら 7）。適用は R2a。
pub fn gate_threshold(depth: u32, step: u32) -> u32 {
    ROOT_GATE_THRESHOLD.saturating_add(step.saturating_mul(depth.saturating_sub(1)))
}

/// D3: `[execution.tree]` の上限（既定は D3 の表と U-R4 の決定どおり）。`ExecutionLimits.tree` として
/// plan/3 の検証だけに効く（/1・/2 は見ない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLimits {
    /// plan/3 と子 task の生成を有効にする（既定 `false`。R5b で人が `true` に）。`false` なら /3 の
    /// 計画は検証で拒否される（`PlanValidationError::TreeDisabled`）。
    pub enabled: bool,
    /// task の層数（1..=3、既定 3。U-R1）。
    pub max_depth: u32,
    /// 1 段階の unit 数（leaf + task。統合 WU と repair は数えない。既定 6）。
    pub max_units_per_stage: usize,
    /// 1 計画の段階の数（既定 5 = ADR-0074 の `max_phases`）。
    pub max_stages: usize,
    /// 1 計画の kind task の unit 数（既定 6）。
    pub max_child_tasks_per_plan: usize,
    /// 1 つの親で同時に非終端の子 task の数（既定 2。R1b で使う）。
    pub max_parallel_child_tasks: usize,
    /// 木の生涯で作る leaf（repair・統合を除く。既定 40。R2a で使う）。
    pub max_tree_leaves: u32,
    /// 木全体の worker / planner / repair の run（reviewer を除く。既定 120。R2a で使う）。
    pub max_tree_runs: u32,
    /// 木全体で採用した replan の版（既定 10。R2a で使う）。
    pub max_tree_replans: u32,
    /// 木全体の input + output トークン（設定したときだけ。R2a で使う）。
    pub max_tree_tokens: Option<u64>,
    /// 木あたりの未回答の決定（既定 12。R3a で使う）。
    pub max_open_decisions_per_tree: usize,
    /// 計画あたりの未回答の決定（既定 8。plan/3 の検証で使う）。
    pub max_open_decisions_per_plan: usize,
    /// 深さの gate の閾値の刻み（既定 2。R2a で使う）。
    pub gate_depth_step: u32,
    /// D8 の「上限に近い」の比（千分率。既定 800 = 0.8。R3b で使う。`ExecutionLimits` を `Eq` の
    /// まま保つため浮動小数にしない）。
    pub approval_near_limit_permille: u32,
}

impl Default for TreeLimits {
    fn default() -> Self {
        TreeLimits {
            enabled: false,
            max_depth: DEFAULT_MAX_DEPTH,
            max_units_per_stage: 6,
            max_stages: 5,
            max_child_tasks_per_plan: 6,
            max_parallel_child_tasks: 2,
            max_tree_leaves: 40,
            max_tree_runs: 120,
            max_tree_replans: 10,
            max_tree_tokens: None,
            max_open_decisions_per_tree: 12,
            max_open_decisions_per_plan: 8,
            gate_depth_step: 2,
            approval_near_limit_permille: 800,
        }
    }
}

impl TreeLimits {
    /// replay（採用済みの計画の並びの復元）用: 形の検査だけをし、上限では拒否しない。
    pub fn permissive() -> Self {
        TreeLimits {
            enabled: true,
            max_depth: u32::MAX,
            max_units_per_stage: usize::MAX,
            max_stages: usize::MAX,
            max_child_tasks_per_plan: usize::MAX,
            max_parallel_child_tasks: usize::MAX,
            max_tree_leaves: u32::MAX,
            max_tree_runs: u32::MAX,
            max_tree_replans: u32::MAX,
            max_tree_tokens: None,
            max_open_decisions_per_tree: usize::MAX,
            max_open_decisions_per_plan: usize::MAX,
            gate_depth_step: 0,
            approval_near_limit_permille: 1000,
        }
    }
}

/// D4 (3): planner が unit に宣言した種類（`Event::UnitGateOverridden.declared`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitDeclared {
    Leaf,
    Task,
}

/// D4 (3): unit の gate が planner の宣言と食い違ったときに daemon が取った行動。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitGateAction {
    /// leaf と宣言されたが compound、子 task を持てる深さ → task に上げた。
    Promoted,
    /// leaf と宣言されたが compound、子 task を持てない深さ → 決定の要求（`leaf_too_large`）。
    Decision,
    /// task と宣言され atomic だが、構造上の理由があるので task のまま。
    KeptTask,
    /// task と宣言され atomic、構造上の理由なし・leaf の基準を満たす → leaf に下げた。
    Demoted,
}

// ---------------------------------------------------------------------------
// ADR-0079 D4 (3)（Phase R2a）: unit の gate（計画の採用のときの最終判断）
// ---------------------------------------------------------------------------

/// D4 (3): unit の gate の入力のうち、計画と親から決まるもの。
#[derive(Debug, Clone, Copy)]
pub struct UnitGateContext<'a> {
    /// 計画を持つ task（親）。
    pub parent: &'a Task,
    /// 親の深さ（task の層数。unit の深さは `parent_depth + 1`）。
    pub parent_depth: u32,
    /// 親の実効の repos の名前（親が持たなければ案件の primary。`task_ops::tree::parent_repo_names`）。
    pub parent_repo_names: &'a [String],
    pub limits: &'a TreeLimits,
    /// leaf の 1 run の上限（`ExecutionLimits.work_unit_max_turns` / `work_unit_max_wall_secs`）。
    pub work_unit_max_turns: u32,
    pub work_unit_max_wall_secs: u64,
}

/// D4 (3): 1 つの unit の gate の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitGate {
    pub unit_key: String,
    pub declared: UnitDeclared,
    /// unit の view にかけた gate（深さ `parent_depth + 1` の閾値。`shadow = false`）。
    pub decision: crate::execution_gate::ExecutionGateDecision,
    /// unit の深さ（`parent_depth + 1`）。
    pub depth: u32,
    pub threshold: u32,
    /// 宣言と食い違ったときの行動（一致なら `None`）。
    pub action: Option<UnitGateAction>,
    /// 判断の理由（人が読む 1 行。`UnitGateOverridden.reason`）。
    pub reason: String,
}

/// D4 (3) / ADR-0072 D21: unit の view（親の Task を複製し、題名・目的・受け入れ・予算・genre を unit の
/// spec に差し替えたもの）。gate の特徴量の推定と規則表にだけ使う（保存しない）。
///
/// - leaf: 受け入れは `done_when`（reviewer）と `checks`（command）、予算は unit の `budget`（無ければ
///   ADR-0072 D18 の既定 `max(親, 30 turns / 1,800 秒)`）、genre は `harness`（無ければ親）。
/// - kind task: 受け入れは unit の `acceptance`、予算は親（子 task は親の予算を継ぐ。D4 (4)）、genre は unit
///   （無ければ親）。
/// - `routing` は unit の `features` のヒントだけ（親のヒント・人の明示・CoS のヒントは継がない）。
///   印（labels）は持たない（対話・support-task の判定に当たらないように）。
pub fn unit_view(parent: &Task, unit: &crate::execution_plan::PlanUnitSpec) -> Task {
    use crate::model::{Check, Criterion};
    let mut view = parent.clone();
    view.title = unit.title.clone();
    view.objective = unit.objective.clone();
    view.labels = Vec::new();
    view.kind = crate::model::TaskKind::Execute;
    view.tree = None;
    let hints = unit
        .features
        .clone()
        .and_then(|v| serde_json::from_value::<crate::model_policy::TaskFeatureHints>(v).ok())
        .filter(|h| !h.is_empty());
    view.routing = Some(crate::model::TaskRouting {
        features: hints,
        ..Default::default()
    });
    if unit.is_task() {
        view.acceptance = unit.acceptance.clone();
        view.genre = unit.genre.clone().or_else(|| parent.genre.clone());
    } else {
        let mut acceptance: Vec<Criterion> = unit
            .done_when
            .iter()
            .map(|t| Criterion {
                text: t.clone(),
                check: Check::Reviewer,
            })
            .collect();
        acceptance.extend(unit.checks.iter().map(|c| Criterion {
            text: c.cmd.clone(),
            check: Check::Command {
                cmd: c.cmd.clone(),
                expect_exit: c.expect_exit,
            },
        }));
        view.acceptance = acceptance;
        view.genre = unit.harness.clone().or_else(|| parent.genre.clone());
        let budget = unit.budget.unwrap_or_default();
        view.budget.max_turns = budget
            .max_turns
            .unwrap_or_else(|| parent.budget.max_turns.max(30));
        view.budget.max_wall_secs = budget
            .max_wall_secs
            .unwrap_or_else(|| parent.budget.max_wall_secs.max(1800));
    }
    view
}

/// D4 (3) の表の 5 行目: kind task の unit を task のまま残す構造上の理由（人の acceptance・決定を待つ・
/// 既存の task の採用・親と違う genre / repos / 部署〈親の skills に無い skill〉）。空なら理由なし。
/// `needs_decisions` は実効の値（`effective_needs_decisions`）。
pub fn structural_reasons(
    parent: &Task,
    parent_repo_names: &[String],
    unit: &crate::execution_plan::PlanUnitSpec,
    needs_decisions: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    if unit
        .acceptance
        .iter()
        .any(|c| c.check == crate::model::Check::Human)
    {
        out.push("human acceptance".to_string());
    }
    if !needs_decisions.is_empty() {
        out.push(format!("needs decisions [{}]", needs_decisions.join(", ")));
    }
    if let Some(adopt) = unit.adopt {
        out.push(format!("adopts task {adopt}"));
    }
    if let Some(genre) = unit.genre.as_deref()
        && parent.genre.as_deref() != Some(genre)
    {
        out.push(format!(
            "genre {genre} differs from the parent's {}",
            parent.genre.as_deref().unwrap_or("(none)")
        ));
    }
    if !unit.repos.is_empty() {
        let unit_set: std::collections::BTreeSet<&str> =
            unit.repos.iter().map(String::as_str).collect();
        let parent_set: std::collections::BTreeSet<&str> =
            parent_repo_names.iter().map(String::as_str).collect();
        if unit_set != parent_set {
            out.push(format!(
                "repos [{}] differ from the parent's [{}]",
                unit.repos.join(", "),
                parent_repo_names.join(", ")
            ));
        }
    }
    let foreign: Vec<&str> = unit
        .skills
        .iter()
        .filter(|s| !parent.skills.iter().any(|p| p == *s))
        .map(String::as_str)
        .collect();
    if !foreign.is_empty() {
        out.push(format!(
            "skills [{}] beyond the parent's (another department)",
            foreign.join(", ")
        ));
    }
    out
}

/// D4 (2) の leaf の基準のうち、kind task の unit が満たさないもの（空なら leaf に下げられる）。
/// (a) 1 run: 子が継ぐ予算（親の予算）が leaf の上限以内、(b) リポジトリは高々 1、(c) 機械的な検査
/// （`Check::Command` の acceptance）が 1 本以上。gate の atomic は呼び出し側が見る。
pub fn task_unit_leaf_shortfalls(
    parent: &Task,
    unit: &crate::execution_plan::PlanUnitSpec,
    work_unit_max_turns: u32,
    work_unit_max_wall_secs: u64,
) -> Vec<String> {
    let mut out = Vec::new();
    if parent.budget.max_turns > work_unit_max_turns
        || parent.budget.max_wall_secs > work_unit_max_wall_secs
    {
        out.push(format!(
            "the inherited budget ({} turns / {} s) exceeds one run ({work_unit_max_turns} / {work_unit_max_wall_secs})",
            parent.budget.max_turns, parent.budget.max_wall_secs
        ));
    }
    if unit.repos.len() > 1 {
        out.push(format!("{} repos", unit.repos.len()));
    }
    if !unit
        .acceptance
        .iter()
        .any(|c| matches!(c.check, crate::model::Check::Command { .. }))
    {
        out.push("no command check".to_string());
    }
    out
}

/// D4 (3): unit の gate（純粋関数）。view に深さ `parent_depth + 1` の閾値で gate をかけ、planner の
/// 宣言と照らして D4 (3) の表の行を決める（表の 7 行目〈子 task を持てない深さの task〉は計画の検証の
/// `ChildTaskTooDeep` が先に拾う）。
pub fn unit_gate(
    ctx: &UnitGateContext<'_>,
    unit: &crate::execution_plan::PlanUnitSpec,
    needs_decisions: &[String],
) -> UnitGate {
    use crate::execution_gate::{ExecutionMode, GateThreshold};
    let depth = ctx.parent_depth.saturating_add(1);
    let at = GateThreshold::at_depth(depth, ctx.limits.gate_depth_step);
    let threshold = gate_threshold(depth, ctx.limits.gate_depth_step);
    let view = unit_view(ctx.parent, unit);
    let hints = view.routing.as_ref().and_then(|r| r.features);
    let (features, _) = crate::model_policy::TaskFeatures::infer_with_hints(&view, hints.as_ref());
    let decision = crate::execution_gate::decide_at(
        &view,
        &features,
        None,
        false,
        crate::execution_gate::ExecutionGateInputs::default(),
        false,
        at,
    );
    let gate_note = format!(
        "{} (score {} / threshold {threshold})",
        decision.rule_id, decision.score
    );
    let (declared, action, reason) = if unit.is_task() {
        match decision.mode {
            ExecutionMode::Compound => (UnitDeclared::Task, None, gate_note),
            ExecutionMode::Atomic => {
                let reasons =
                    structural_reasons(ctx.parent, ctx.parent_repo_names, unit, needs_decisions);
                if !reasons.is_empty() {
                    (
                        UnitDeclared::Task,
                        Some(UnitGateAction::KeptTask),
                        format!("{gate_note}; kept as a task: {}", reasons.join("; ")),
                    )
                } else {
                    let shortfalls = task_unit_leaf_shortfalls(
                        ctx.parent,
                        unit,
                        ctx.work_unit_max_turns,
                        ctx.work_unit_max_wall_secs,
                    );
                    if shortfalls.is_empty() {
                        (
                            UnitDeclared::Task,
                            Some(UnitGateAction::Demoted),
                            format!("{gate_note}; meets the leaf criteria: demoted to a leaf"),
                        )
                    } else {
                        (
                            UnitDeclared::Task,
                            Some(UnitGateAction::KeptTask),
                            format!(
                                "{gate_note}; kept as a task: leaf criteria not met ({})",
                                shortfalls.join("; ")
                            ),
                        )
                    }
                }
            }
        }
    } else {
        match decision.mode {
            ExecutionMode::Atomic => (UnitDeclared::Leaf, None, gate_note),
            ExecutionMode::Compound => {
                if can_have_child_tasks(ctx.parent_depth, ctx.limits.max_depth) {
                    (
                        UnitDeclared::Leaf,
                        Some(UnitGateAction::Promoted),
                        format!("{gate_note}; promoted to a child task"),
                    )
                } else {
                    (
                        UnitDeclared::Leaf,
                        Some(UnitGateAction::Decision),
                        format!(
                            "{gate_note}; a task at depth {} cannot have child tasks (max_depth {}): asking a human",
                            ctx.parent_depth, ctx.limits.max_depth
                        ),
                    )
                }
            }
        }
    };
    UnitGate {
        unit_key: unit.key.clone(),
        declared,
        decision,
        depth,
        threshold,
        action,
        reason,
    }
}

/// D4 (3) の「task に上げる」: leaf の spec を kind task の spec にする。`checks` は command の acceptance
/// （leaf は `checks` を 1 本以上持つ）、`done_when` と `context.paths` は目的の末尾に固定の書式で移し
/// （仕事を落とさない。reviewer の acceptance にはしない — 子の最終レビューに reviewer run を足さない。U-R7）、
/// `context.repo` は `repos`。leaf 専用の欄（`checks` / `budget` / `harness` / `context.paths`）は消す。
pub fn promote_to_task(
    unit: &crate::execution_plan::PlanUnitSpec,
) -> crate::execution_plan::PlanUnitSpec {
    use crate::execution_plan::RepoSelector;
    use crate::model::{Check, Criterion};
    let mut out = unit.clone();
    out.kind = crate::execution_plan::WorkUnitKind::Task;
    let mut acceptance: Vec<Criterion> = unit
        .checks
        .iter()
        .map(|c| Criterion {
            text: format!("`{}` exits {}", c.cmd, c.expect_exit),
            check: Check::Command {
                cmd: c.cmd.clone(),
                expect_exit: c.expect_exit,
            },
        })
        .collect();
    if acceptance.is_empty() {
        acceptance = unit
            .done_when
            .iter()
            .map(|t| Criterion {
                text: t.clone(),
                check: Check::Reviewer,
            })
            .collect();
    }
    if acceptance.is_empty() {
        acceptance.push(Criterion {
            text: unit.title.clone(),
            check: Check::Reviewer,
        });
    }
    out.acceptance = acceptance;
    let mut objective = unit.objective.trim_end().to_string();
    if !unit.done_when.is_empty() {
        objective.push_str(&format!(
            "\n\n{PROMOTED_DONE_WHEN_HEADING}\n{}",
            unit.done_when
                .iter()
                .map(|d| format!("- {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !unit.context.paths.is_empty() {
        objective.push_str(&format!(
            "\n\n{PROMOTED_PATHS_HEADING} {}",
            unit.context.paths.join(", ")
        ));
    }
    out.objective = objective;
    out.done_when = Vec::new();
    out.checks = Vec::new();
    out.budget = None;
    out.harness = None;
    out.repos = match &unit.context.repo {
        Some(RepoSelector::One(r)) => vec![r.clone()],
        Some(RepoSelector::Many(v)) => v.clone(),
        None => Vec::new(),
    };
    out.context.paths = Vec::new();
    out.context.repo = None;
    out
}

/// [`promote_to_task`] が目的の末尾に足す見出し（固定）。
pub const PROMOTED_DONE_WHEN_HEADING: &str =
    "## 完了の条件（計画の leaf から引き継ぎ。ADR-0079 D4 (3)）";
/// [`promote_to_task`] が目的の末尾に足す対象のパスの前置き（固定）。
pub const PROMOTED_PATHS_HEADING: &str = "対象のパス（計画の leaf から引き継ぎ）:";

/// D4 (3) の「leaf に下げる」: kind task の spec を leaf の spec（kind = 段階の kind）にする。acceptance の
/// 文は `done_when`、command の検査は `checks`、`repos`（高々 1）は `context.repo`。kind task 専用の欄
/// （`acceptance` / `genre` / `skills` / `repos` / `adopt`）は消す（構造上の理由が無いことを確かめてから
/// 呼ぶので、genre は親と同じか無く、skills は親の部分集合）。
pub fn demote_to_leaf(
    unit: &crate::execution_plan::PlanUnitSpec,
    stage_kind: crate::execution_plan::WorkUnitKind,
) -> crate::execution_plan::PlanUnitSpec {
    use crate::execution_plan::{RepoSelector, WorkUnitCheck};
    let mut out = unit.clone();
    out.kind = stage_kind;
    let mut done_when = unit.done_when.clone();
    done_when.extend(unit.acceptance.iter().map(|c| c.text.clone()));
    out.done_when = done_when;
    out.checks = unit
        .acceptance
        .iter()
        .filter_map(|c| match &c.check {
            crate::model::Check::Command { cmd, expect_exit } => Some(WorkUnitCheck {
                cmd: cmd.clone(),
                expect_exit: *expect_exit,
            }),
            _ => None,
        })
        .collect();
    out.context.repo = unit.repos.first().map(|r| RepoSelector::One(r.clone()));
    out.acceptance = Vec::new();
    out.genre = None;
    out.skills = Vec::new();
    out.repos = Vec::new();
    out.adopt = None;
    out
}

/// D4 (3): 計画のすべての unit に gate をかけた結果（純粋関数）。`spec` は上げる・下げるを当てた計画、
/// `gates` は unit ごとの結果（計画の順）、`leaf_too_large` は決定の要求にする unit（`spec` では leaf の
/// まま）。/3 以外は何もしない（`gates` は空）。
#[derive(Debug, Clone)]
pub struct UnitGateReport {
    pub spec: crate::execution_plan::ExecutionPlanSpec,
    pub gates: Vec<UnitGate>,
    pub leaf_too_large: Vec<String>,
}

impl UnitGateReport {
    /// 宣言と食い違った unit（`UnitGateOverridden` を出すもの）。
    pub fn overridden(&self) -> impl Iterator<Item = &UnitGate> {
        self.gates.iter().filter(|g| g.action.is_some())
    }
}

/// D4 (3)（Phase R2a）: 計画の各 unit に [`unit_gate`] をかけ、上げる・下げるを spec に当てる（純粋関数）。
/// `skip` の unit（replan で持ち越す done の unit）は gate をかけず spec も変えない。
pub fn apply_unit_gates(
    ctx: &UnitGateContext<'_>,
    spec: &crate::execution_plan::ExecutionPlanSpec,
    skip: &std::collections::BTreeSet<String>,
) -> UnitGateReport {
    let mut out = spec.clone();
    let mut gates = Vec::new();
    let mut leaf_too_large = Vec::new();
    if spec.schema != crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3 {
        return UnitGateReport {
            spec: out,
            gates,
            leaf_too_large,
        };
    }
    for (i, unit) in spec.units.iter().enumerate() {
        if skip.contains(&unit.key) {
            continue;
        }
        let needs = crate::execution_plan::effective_needs_decisions(spec, &unit.key);
        let gate = unit_gate(ctx, unit, &needs);
        match gate.action {
            Some(UnitGateAction::Promoted) => out.units[i] = promote_to_task(unit),
            Some(UnitGateAction::Demoted) => {
                let stage_kind = spec
                    .stages
                    .iter()
                    .find(|s| s.key == unit.stage)
                    .map(|s| s.kind)
                    .unwrap_or(crate::execution_plan::WorkUnitKind::Implement);
                out.units[i] = demote_to_leaf(unit, stage_kind);
            }
            Some(UnitGateAction::Decision) => leaf_too_large.push(unit.key.clone()),
            Some(UnitGateAction::KeptTask) | None => {}
        }
        gates.push(gate);
    }
    UnitGateReport {
        spec: out,
        gates,
        leaf_too_large,
    }
}

// ---------------------------------------------------------------------------
// ADR-0079 D3（Phase R2a）: 木の上限の決定的な数え上げと、超過の決定の要求
// ---------------------------------------------------------------------------

/// D3: 上限の種類（`kind: limit` の決定の key は [`TreeLimitKind::decision_key`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TreeLimitKind {
    /// 1 計画の段階の数（`max_stages`）。
    Stages,
    /// 1 段階の unit の数（`max_units_per_stage`）。
    UnitsPerStage,
    /// 1 計画の kind task の unit の数（`max_child_tasks_per_plan`）。
    ChildTasks,
    /// 子 task を持てない深さの kind task の unit（`max_depth`）。
    MaxDepth,
    /// 木の生涯の leaf の数（`max_tree_leaves`）。
    TreeLeaves,
    /// 木全体の run（reviewer を除く。`max_tree_runs`）。
    TreeRuns,
    /// 木全体で採用した replan の版（`max_tree_replans`）。
    TreeReplans,
    /// 木全体の input + output トークン（`max_tree_tokens`）。
    TreeTokens,
    /// ADR-0079 D9（Phase R2b）: 節点の replan の版（`[execution] max_replans`。節点ごとの上限）。子の失敗を
    /// 吸収する replan が節点の上限に達したら、黙って止まらずに決定の要求にする。
    NodeReplans,
}

impl TreeLimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TreeLimitKind::Stages => "max_stages",
            TreeLimitKind::UnitsPerStage => "max_units_per_stage",
            TreeLimitKind::ChildTasks => "max_child_tasks_per_plan",
            TreeLimitKind::MaxDepth => "max_depth",
            TreeLimitKind::TreeLeaves => "max_tree_leaves",
            TreeLimitKind::TreeRuns => "max_tree_runs",
            TreeLimitKind::TreeReplans => "max_tree_replans",
            TreeLimitKind::TreeTokens => "max_tree_tokens",
            TreeLimitKind::NodeReplans => "max_replans",
        }
    }

    /// `kind: limit` の決定の key（daemon が振る。`limit:<設定名>`、段階ごとなら `:<段階>` を足す）。
    pub fn decision_key(self, scope: Option<&str>) -> String {
        match scope {
            Some(s) => format!("limit:{}:{s}", self.as_str()),
            None => format!("limit:{}", self.as_str()),
        }
    }

    /// 木全体の上限（決定は木に 1 件だけ開く）か。
    pub fn is_tree_wide(self) -> bool {
        matches!(
            self,
            TreeLimitKind::TreeLeaves
                | TreeLimitKind::TreeRuns
                | TreeLimitKind::TreeReplans
                | TreeLimitKind::TreeTokens
        )
    }

    /// ADR-0079 D7（Phase R3a）: `kind: limit` の決定の key（`limit:<設定名>[:<段階>]`）から種類を引く。
    pub fn from_decision_key(key: &str) -> Option<Self> {
        let name = key.strip_prefix("limit:")?;
        let name = name.split(':').next().unwrap_or(name);
        [
            TreeLimitKind::Stages,
            TreeLimitKind::UnitsPerStage,
            TreeLimitKind::ChildTasks,
            TreeLimitKind::MaxDepth,
            TreeLimitKind::TreeLeaves,
            TreeLimitKind::TreeRuns,
            TreeLimitKind::TreeReplans,
            TreeLimitKind::TreeTokens,
            TreeLimitKind::NodeReplans,
        ]
        .into_iter()
        .find(|k| k.as_str() == name)
    }

    /// ADR-0079 D7（Phase R3a）: run を起こす時に数える上限（`tree_run_limit_hold` / 節点の replan）。
    /// これらの `raise-once` / `replan` の回答は、上限に余裕を足す（[`limit_allowance_step`]）。計画の採用時の
    /// 上限（段階・段階あたり・子 task・深さ・木の leaf）は止めた unit を進めるだけ。
    pub fn is_run_time(self) -> bool {
        matches!(
            self,
            TreeLimitKind::TreeRuns
                | TreeLimitKind::TreeReplans
                | TreeLimitKind::TreeTokens
                | TreeLimitKind::NodeReplans
        )
    }
}

/// ADR-0079 D7（Phase R3a）: run 時の上限に対する `raise-once`（と `replan`）の回答 1 件が足す余裕。
/// 設定の上限の半分（切り上げ、最低 1）。節点の replan（`max_replans`）は 1 回ずつ。
pub fn limit_allowance_step(limit: TreeLimitKind, configured_max: u64) -> u64 {
    if limit == TreeLimitKind::NodeReplans {
        return 1;
    }
    configured_max.div_ceil(2).max(1)
}

/// ADR-0079 D7（Phase R3a）: 回答で足した余裕を当てた木の上限（run 時の上限だけ。`allowances` は種類ごとの
/// 回答の件数）。
pub fn limits_with_allowances(
    limits: &TreeLimits,
    allowances: &std::collections::BTreeMap<TreeLimitKind, u32>,
) -> TreeLimits {
    let mut out = *limits;
    let grant = |kind: TreeLimitKind, max: u64| -> u64 {
        let n = u64::from(allowances.get(&kind).copied().unwrap_or(0));
        max.saturating_add(n.saturating_mul(limit_allowance_step(kind, max)))
    };
    out.max_tree_runs = u32::try_from(grant(
        TreeLimitKind::TreeRuns,
        u64::from(limits.max_tree_runs),
    ))
    .unwrap_or(u32::MAX);
    out.max_tree_replans = u32::try_from(grant(
        TreeLimitKind::TreeReplans,
        u64::from(limits.max_tree_replans),
    ))
    .unwrap_or(u32::MAX);
    out.max_tree_tokens = limits
        .max_tree_tokens
        .map(|max| grant(TreeLimitKind::TreeTokens, max));
    out
}

/// D3: 計画の採用のときに止める unit の束（1 束 = 1 件の `kind: limit` の決定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitHold {
    pub limit: TreeLimitKind,
    /// 段階ごとの上限なら段階の key。
    pub scope: Option<String>,
    /// 止める unit の key（計画の順）。決定の `needed_before`。
    pub units: Vec<String>,
    /// 上限に照らした数（計画・木の値）と上限。
    pub count: u64,
    pub max: u64,
}

/// D3（Phase R2a）: 計画の上限（段階の数・段階あたりの unit・計画あたりの子 task・`max_depth`）と木の
/// leaf の上限に照らし、超える分の unit を決定的に選ぶ（純粋関数）。上限の内に収まる unit は止めない
/// （兄弟は進む）。選び方はどれも「計画の順で上限の内に入るものを残し、残りを止める」:
///
/// - 段階の数: `max_stages` 番目より後の段階の unit すべて。
/// - 段階あたり: 段階ごとに、先頭から `max_units_per_stage` 個より後の unit。
/// - 子 task: kind task の unit の先頭から `max_child_tasks_per_plan` 個より後。
/// - `max_depth`: `depth` の task が子 task を持てなければ、kind task の unit すべて。
/// - 木の leaf: この計画で新しく作る leaf（`existing_keys` に無い leaf）のうち、木の残り
///   （`max_tree_leaves − tree_leaves`）より後。
///
/// 前の束で止めた unit は後の束に入れない（1 つの unit は 1 件の決定で止まる）。`extra_held` は unit の
/// gate が既に止めた unit（`leaf_too_large`。数えるが止める束には入れない）。
pub fn plan_limit_holds(
    spec: &crate::execution_plan::ExecutionPlanSpec,
    limits: &TreeLimits,
    depth: u32,
    tree_leaves: u32,
    existing_keys: &std::collections::BTreeSet<String>,
    extra_held: &std::collections::BTreeSet<String>,
) -> Vec<LimitHold> {
    let mut holds: Vec<LimitHold> = Vec::new();
    if spec.schema != crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3 {
        return holds;
    }
    let mut held: std::collections::BTreeSet<String> = extra_held.clone();
    let push = |holds: &mut Vec<LimitHold>,
                held: &mut std::collections::BTreeSet<String>,
                limit: TreeLimitKind,
                scope: Option<String>,
                candidates: Vec<String>,
                count: u64,
                max: u64| {
        let units: Vec<String> = candidates
            .into_iter()
            .filter(|k| !held.contains(k))
            .collect();
        if units.is_empty() {
            return;
        }
        held.extend(units.iter().cloned());
        holds.push(LimitHold {
            limit,
            scope,
            units,
            count,
            max,
        });
    };
    // 段階の数。
    if spec.stages.len() > limits.max_stages {
        let late: std::collections::BTreeSet<&str> = spec
            .stages
            .iter()
            .skip(limits.max_stages)
            .map(|s| s.key.as_str())
            .collect();
        let units = spec
            .units
            .iter()
            .filter(|u| late.contains(u.stage.as_str()))
            .map(|u| u.key.clone())
            .collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::Stages,
            None,
            units,
            spec.stages.len() as u64,
            limits.max_stages as u64,
        );
    }
    // 段階あたりの unit。
    for stage in &spec.stages {
        let in_stage: Vec<&crate::execution_plan::PlanUnitSpec> =
            spec.units.iter().filter(|u| u.stage == stage.key).collect();
        if in_stage.len() > limits.max_units_per_stage {
            let units = in_stage
                .iter()
                .skip(limits.max_units_per_stage)
                .map(|u| u.key.clone())
                .collect();
            push(
                &mut holds,
                &mut held,
                TreeLimitKind::UnitsPerStage,
                Some(stage.key.clone()),
                units,
                in_stage.len() as u64,
                limits.max_units_per_stage as u64,
            );
        }
    }
    // 子 task の数と深さ。
    let task_units: Vec<&crate::execution_plan::PlanUnitSpec> =
        spec.units.iter().filter(|u| u.is_task()).collect();
    if task_units.len() > limits.max_child_tasks_per_plan {
        let units = task_units
            .iter()
            .skip(limits.max_child_tasks_per_plan)
            .map(|u| u.key.clone())
            .collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::ChildTasks,
            None,
            units,
            task_units.len() as u64,
            limits.max_child_tasks_per_plan as u64,
        );
    }
    if !task_units.is_empty() && !can_have_child_tasks(depth, limits.max_depth) {
        let units = task_units.iter().map(|u| u.key.clone()).collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::MaxDepth,
            None,
            units,
            u64::from(depth) + 1,
            u64::from(limits.max_depth),
        );
    }
    // 木の leaf（この計画で新しく作るもの。既に止めた unit は数えない）。
    let new_leaves: Vec<String> = spec
        .units
        .iter()
        .filter(|u| !u.is_task() && !existing_keys.contains(&u.key) && !held.contains(&u.key))
        .map(|u| u.key.clone())
        .collect();
    let room = limits.max_tree_leaves.saturating_sub(tree_leaves) as usize;
    if new_leaves.len() > room {
        let total = u64::from(tree_leaves) + new_leaves.len() as u64;
        let units = new_leaves.into_iter().skip(room).collect();
        push(
            &mut holds,
            &mut held,
            TreeLimitKind::TreeLeaves,
            None,
            units,
            total,
            u64::from(limits.max_tree_leaves),
        );
    }
    holds
}

/// D3 / U-R7（Phase R2a）: 深さごとの数（木の節点の run・トークン・定価と、reviewer の run と定価）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DepthCounters {
    pub depth: u32,
    /// この深さの task の数。
    pub nodes: u32,
    /// reviewer を除く run（worker・planner・wrap-up。repair の run は worker に数える）。
    pub runs: u32,
    /// role ごとの run（reviewer を含む。`RunIndexRole::as_str`）。
    pub runs_by_role: std::collections::BTreeMap<String, u32>,
    /// U-R7: reviewer の run と、その定価（USD）。
    pub reviewer_runs: u32,
    pub reviewer_cost_usd: f64,
    /// input + output トークン（reviewer を含むすべての run）。
    pub tokens: u64,
    /// 定価（USD。reviewer を含む）。`cost_usd_complete` が `false` なら下限。
    pub cost_usd: f64,
    pub cost_usd_complete: bool,
}

/// D3（Phase R2a）: 木（root とその子孫）の上限に照らす数。store から集めた [`TreeNodeFacts`] から
/// [`tree_counters`] が決定的に作る。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TreeCounters {
    pub root_id: Option<TaskId>,
    pub nodes: u32,
    /// 木の生涯で作った leaf（repair・統合・kind task の行を除く work_units の行。決定を待って止めている
    /// 行〈`blocked(decision)`〉は作ったうちに数えない）。
    pub leaves: u32,
    /// reviewer を除く run（`max_tree_runs` に照らす値）。
    pub runs: u32,
    pub reviewer_runs: u32,
    /// 木全体で採用した replan の版（節点ごとの `計画の版の数 − 1` の和）。
    pub replans: u32,
    /// input + output トークン（`max_tree_tokens` に照らす値。reviewer を含む）。
    pub tokens: u64,
    pub cost_usd: f64,
    pub cost_usd_complete: bool,
    /// 深さの昇順。
    pub by_depth: Vec<DepthCounters>,
}

/// [`tree_counters`] の入力: 木の 1 節点の事実（store の読み取りだけで集める。`task_ops::tree::tree_counters`）。
#[derive(Debug, Clone)]
pub struct TreeNodeFacts {
    pub task_id: TaskId,
    pub depth: u32,
    pub runs: Vec<crate::execution_plan::RunRow>,
    pub work_units: Vec<crate::execution_plan::WorkUnitRow>,
    /// 計画の版の数（`execution_plans` の行数。replan は `版 − 1`）。
    pub plan_versions: u32,
}

/// D3 / U-R7（Phase R2a）: 木の数（純粋関数）。
pub fn tree_counters(root_id: Option<TaskId>, nodes: &[TreeNodeFacts]) -> TreeCounters {
    use crate::execution_plan::{
        RunIndexRole, WorkUnitBlockedReason, WorkUnitKind, WorkUnitStatus,
    };
    let mut out = TreeCounters {
        root_id,
        cost_usd_complete: true,
        ..Default::default()
    };
    let mut by_depth: std::collections::BTreeMap<u32, DepthCounters> =
        std::collections::BTreeMap::new();
    for node in nodes {
        out.nodes += 1;
        out.replans += node.plan_versions.saturating_sub(1);
        out.leaves += node
            .work_units
            .iter()
            .filter(|u| {
                !matches!(
                    u.kind,
                    WorkUnitKind::Task | WorkUnitKind::Integrate | WorkUnitKind::Repair
                ) && !(u.status == WorkUnitStatus::Blocked
                    && u.blocked_reason == Some(WorkUnitBlockedReason::Decision))
            })
            .count() as u32;
        let d = by_depth.entry(node.depth).or_insert_with(|| DepthCounters {
            depth: node.depth,
            cost_usd_complete: true,
            ..Default::default()
        });
        d.nodes += 1;
        for run in &node.runs {
            *d.runs_by_role
                .entry(run.role.as_str().to_string())
                .or_default() += 1;
            let reviewer = run.role == RunIndexRole::Reviewer;
            if reviewer {
                d.reviewer_runs += 1;
                out.reviewer_runs += 1;
            } else {
                d.runs += 1;
                out.runs += 1;
            }
            let Some(usage) = run.usage.as_ref() else {
                continue;
            };
            let tokens = usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
            d.tokens += tokens;
            out.tokens += tokens;
            match usage.cost_usd {
                Some(c) => {
                    d.cost_usd += c;
                    out.cost_usd += c;
                    if reviewer {
                        d.reviewer_cost_usd += c;
                    }
                }
                None if tokens > 0 => {
                    d.cost_usd_complete = false;
                    out.cost_usd_complete = false;
                }
                None => {}
            }
        }
    }
    out.by_depth = by_depth.into_values().collect();
    out
}

/// D3（Phase R2a）: 次に起こそうとしている run の種類（`max_tree_replans` は replan の planner run だけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextRun {
    Worker,
    Planner { replan: bool },
}

/// D3: run の上限の超過（どれか 1 つ。runs → tokens → replans の順で先に当たったもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimitBreach {
    pub limit: TreeLimitKind,
    pub count: u64,
    pub max: u64,
}

/// D3（Phase R2a）: 木の数に照らし、次の run を起こすと上限を超えるか（純粋関数）。超えるなら
/// その上限。`runs >= max_tree_runs`（もう 1 本起こすと超える）、`tokens >= max_tree_tokens`（設定した
/// ときだけ）、replan の planner run で `replans >= max_tree_replans`。
pub fn run_limit_breach(
    limits: &TreeLimits,
    counters: &TreeCounters,
    next: NextRun,
) -> Option<RunLimitBreach> {
    if counters.runs >= limits.max_tree_runs {
        return Some(RunLimitBreach {
            limit: TreeLimitKind::TreeRuns,
            count: u64::from(counters.runs),
            max: u64::from(limits.max_tree_runs),
        });
    }
    if let Some(max) = limits.max_tree_tokens
        && counters.tokens >= max
    {
        return Some(RunLimitBreach {
            limit: TreeLimitKind::TreeTokens,
            count: counters.tokens,
            max,
        });
    }
    if next == (NextRun::Planner { replan: true }) && counters.replans >= limits.max_tree_replans {
        return Some(RunLimitBreach {
            limit: TreeLimitKind::TreeReplans,
            count: u64::from(counters.replans),
            max: u64::from(limits.max_tree_replans),
        });
    }
    None
}

/// D3 / D7（Phase R2a）: `kind: limit` の決定の要求（daemon が出す。選択肢は D3 の「今回だけ上限を上げて
/// 続ける / この subtree を replan で小さくする / この subtree を取り下げる」、推奨は replan）。
#[allow(clippy::too_many_arguments)]
pub fn limit_decision(
    limit: TreeLimitKind,
    scope: Option<&str>,
    count: u64,
    max: u64,
    needed_before: Vec<String>,
    path: Vec<crate::decision::DecisionPathEntry>,
    raised_by: crate::decision::DecisionRaisedBy,
) -> crate::decision::DecisionRequest {
    use crate::decision::{CostOfReversal, DecisionKind, DecisionOption, DecisionRequest};
    let what = match scope {
        Some(s) => format!("{}（段階 {s}）", limit.as_str()),
        None => limit.as_str().to_string(),
    };
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: limit.decision_key(scope),
        kind: DecisionKind::Limit,
        question: format!(
            "木の上限 {what} を超えます（{count} > 上限 {max}）。止めた仕事をどうしますか"
        ),
        options: vec![
            DecisionOption {
                key: "raise-once".to_string(),
                label: "今回だけ上限を上げて続ける".to_string(),
                consequence: Some("止めた unit / run をそのまま進める".to_string()),
            },
            DecisionOption {
                key: "replan".to_string(),
                label: "この subtree を replan で小さくする".to_string(),
                consequence: Some("planner が上限に収まる計画を出し直す".to_string()),
            },
            DecisionOption {
                key: "withdraw".to_string(),
                label: "この subtree を取り下げる".to_string(),
                consequence: Some("止めた仕事を行わない".to_string()),
            },
        ],
        recommended: "replan".to_string(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: Some(format!("[execution.tree] {} = {max}", limit.as_str())),
        needed_before,
        path,
        raised_by,
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// D4 (3)（Phase R2a）: `kind: leaf_too_large` の決定の要求（compound な leaf を子 task にできない深さ）。
pub fn leaf_too_large_decision(
    gate: &UnitGate,
    unit_title: &str,
    path: Vec<crate::decision::DecisionPathEntry>,
    raised_by: crate::decision::DecisionRaisedBy,
) -> crate::decision::DecisionRequest {
    use crate::decision::{CostOfReversal, DecisionKind, DecisionOption, DecisionRequest};
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: format!("leaf_too_large:{}", gate.unit_key),
        kind: DecisionKind::LeafTooLarge,
        question: format!(
            "unit {}「{unit_title}」は 1 run に収まらない見込み（gate {}、score {} ≥ 閾値 {}）ですが、この深さでは子 task にできません。どうしますか",
            gate.unit_key, gate.decision.rule_id, gate.decision.score, gate.threshold
        ),
        options: vec![
            DecisionOption {
                key: "run-as-leaf".to_string(),
                label: "leaf のまま 1 run で試す".to_string(),
                consequence: Some("continuation に頼る可能性がある".to_string()),
            },
            DecisionOption {
                key: "replan".to_string(),
                label: "replan で小さな leaf に分ける".to_string(),
                consequence: None,
            },
            DecisionOption {
                key: "withdraw".to_string(),
                label: "この unit を取り下げる".to_string(),
                consequence: None,
            },
        ],
        recommended: "replan".to_string(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: Some(gate.reason.clone()),
        needed_before: vec![gate.unit_key.clone()],
        path,
        raised_by,
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

/// ADR-0079 D9（Phase R2b）: `kind: plan_invalid` の決定の key（daemon が振る。節点ごとに 1 件だけ開く）。
pub const PLAN_INVALID_DECISION_KEY: &str = "plan_invalid";

/// ADR-0079 D9（Phase R2b / R3a）: /3 の計画が 2 回不正だったときの決定の要求（atomic に倒さない）。選択肢は
/// 「note を添えて replan / atomic（1 run、初回の計画のときだけ）で試す / 取り下げる（cancel）」、推奨は replan。`errors` は検証の理由（最後の試行まで、古い順）。`needed_before: ["self"]`（この節点の run を止める）。
pub fn plan_invalid_decision(
    task_id: TaskId,
    replan: bool,
    errors: &[String],
    path: Vec<crate::decision::DecisionPathEntry>,
    raised_by: crate::decision::DecisionRaisedBy,
) -> crate::decision::DecisionRequest {
    use crate::decision::{CostOfReversal, DecisionKind, DecisionOption, DecisionRequest};
    let what = if replan {
        "計画の見直し（replan）"
    } else {
        "計画"
    };
    let mut note = errors
        .iter()
        .enumerate()
        .map(|(i, e)| format!("試行 {}: {e}", i + 1))
        .collect::<Vec<_>>()
        .join(" / ");
    if note.chars().count() > 2000 {
        note = note.chars().take(2000).collect::<String>() + "…";
    }
    DecisionRequest {
        id: ulid::Ulid::new().to_string(),
        key: PLAN_INVALID_DECISION_KEY.to_string(),
        kind: DecisionKind::PlanInvalid,
        question: format!(
            "planner が出した{what}（celeris.execution-plan/3）が 2 回とも検証に通りませんでした。この task をどう進めますか"
        ),
        // Phase R3a: 選択肢は「note を添えて planner にもう一度計画させる（replan）/ atomic / 取り下げる（cancel）」。
        // atomic は計画がまだ無いとき（初回の計画）だけ（採用済みの計画の WU を宙に浮かせない。
        // `task_ops::regate` の「計画を持つ Task を atomic に戻さない」と同じ規則）。
        options: {
            let mut options = vec![DecisionOption {
                key: "replan".to_string(),
                label: "note の指示を添えて planner にもう一度計画させる".to_string(),
                consequence: Some(format!(
                    "人の note が planner に渡る（人が計画を書くなら PUT /tasks/{task_id}/execution-plan）"
                )),
            }];
            if !replan {
                options.push(DecisionOption {
                    key: "atomic".to_string(),
                    label: "分けずに 1 run（atomic）で試す".to_string(),
                    consequence: Some("分けると決めた仕事を 1 run に収める".to_string()),
                });
            }
            options.push(DecisionOption {
                key: "cancel".to_string(),
                label: "この task を取り下げる".to_string(),
                consequence: None,
            });
            options
        },
        recommended: "replan".to_string(),
        cost_of_reversal: CostOfReversal::Low,
        cost_note: if note.is_empty() { None } else { Some(note) },
        needed_before: vec![crate::decision::NEEDED_BEFORE_SELF.to_string()],
        path,
        raised_by,
        status: crate::decision::DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// U-R1: task の層数で数える（root 1 / 子 2 / 孫 3）。既定 3 では root と子が子 task を持て、
    /// 孫は持てない。`max_depth = 1` なら root も持てない。
    #[test]
    fn depth_counts_task_levels_not_leaves() {
        assert!(can_have_child_tasks(1, 3));
        assert!(can_have_child_tasks(2, 3));
        assert!(!can_have_child_tasks(3, 3));
        assert!(!can_have_child_tasks(1, 1));
        assert!(can_have_child_tasks(1, 2));
        assert!(!can_have_child_tasks(2, 2));
        assert_eq!(remaining_depth(1, 3), 2);
        assert_eq!(remaining_depth(2, 3), 1);
        assert_eq!(remaining_depth(3, 3), 0);
        assert_eq!(remaining_depth(4, 3), 0);
    }

    /// D3: 閾値は `5 + step × (d − 1)`（depth 2・step 2 は 7）。
    #[test]
    fn gate_threshold_rises_with_depth_formula() {
        assert_eq!(gate_threshold(1, 2), 5);
        assert_eq!(gate_threshold(2, 2), 7);
        assert_eq!(gate_threshold(3, 2), 9);
        assert_eq!(gate_threshold(3, 0), 5);
    }

    #[test]
    fn tree_limits_defaults_follow_d3_and_u_r4() {
        let l = TreeLimits::default();
        assert!(!l.enabled);
        assert_eq!(l.max_depth, 3);
        assert_eq!(l.max_units_per_stage, 6);
        assert_eq!(l.max_stages, 5);
        assert_eq!(l.max_child_tasks_per_plan, 6);
        assert_eq!(l.max_parallel_child_tasks, 2);
        assert_eq!(l.max_tree_leaves, 40);
        assert_eq!(l.max_tree_runs, 120);
        assert_eq!(l.max_tree_replans, 10);
        assert_eq!(l.max_tree_tokens, None);
        assert_eq!(l.max_open_decisions_per_tree, 12);
        assert_eq!(l.max_open_decisions_per_plan, 8);
        assert_eq!(l.gate_depth_step, 2);
        assert_eq!(l.approval_near_limit_permille, 800);
    }

    /// `tree` の JSON は無ければ省略でき、あれば round-trip する（`Task.tree` は serde(default)）。
    #[test]
    fn tree_info_round_trips_and_child_depth_is_parent_plus_one() {
        let root_id = TaskId::new();
        let root = TreeInfo::root(root_id);
        let json = serde_json::to_string(&root).unwrap();
        assert_eq!(
            json,
            format!("{{\"root_id\":\"{root_id}\",\"depth\":1}}"),
            "optional fields are omitted"
        );
        let back: TreeInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, root);
    }

    // ---- Phase R2a: unit の gate・木の上限 ----

    use crate::execution_gate::ExecutionMode;
    use crate::execution_plan::{
        ExecutionLimits, ExecutionPlanSpec, PlanContext, PlanOrigin, PlanUnitSpec,
        PlanValidationError, RunIndexRole, RunIndexStatus, RunRow, WorkUnitBlockedReason,
        WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus, validate_with,
    };
    use crate::model::{
        ArtifactRef, Budget, Check, Status, TaskCategory, TaskKind, TaskMode, TaskRouting, Tier,
        Usage, WorkerHint, WorkspaceSpec,
    };

    fn parent_task(max_turns: u32) -> Task {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        Task {
            tree: None,
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "親".to_string(),
            objective: "親の目的".to_string(),
            acceptance: vec![],
            inputs: Vec::<ArtifactRef>::new(),
            depends_on: vec![],
            status: Status::Running,
            priority: 10,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::local("/tmp/x"),
            repos: vec![],
            budget: Budget {
                max_turns,
                max_wall_secs: 1800,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: Some("coding".into()),
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            labels: vec![],
            category: TaskCategory::Other,
            skills: vec!["rust".into()],
            mode: TaskMode::Production,
            conversation: None,
            routing: Some(TaskRouting::default()),
        }
    }

    /// 規則表の強制規則 `compound/long-and-broad`（深さに関わらず compound）に当てるヒント。
    fn broad() -> serde_json::Value {
        serde_json::json!({"expected_length": "high", "cross_cutting": "high"})
    }

    fn leaf_spec(key: &str, stage: &str, compound: bool) -> PlanUnitSpec {
        let mut v = serde_json::json!({
            "key": key, "stage": stage, "kind": "implement",
            "title": format!("Leaf {key}"),
            "objective": format!("Implement the {key} piece"),
            "done_when": [format!("{key} works")],
            "checks": [{"cmd": format!("test -f {key}.txt"), "expect_exit": 0}],
            "context": {"paths": ["src/"], "repo": "app"},
        });
        if compound {
            v["features"] = broad();
        }
        serde_json::from_value(v).unwrap()
    }

    fn task_spec(key: &str, stage: &str, compound: bool) -> PlanUnitSpec {
        let mut v = serde_json::json!({
            "key": key, "stage": stage, "kind": "task",
            "title": format!("Child {key}"),
            "objective": format!("Deliver the {key} part"),
            "acceptance": [{"text": format!("{key} passes"), "check": {"type": "command", "cmd": format!("make {key}"), "expect_exit": 0}}],
        });
        if compound {
            v["features"] = broad();
        }
        serde_json::from_value(v).unwrap()
    }

    fn plan(stages: &[(&str, &str)], units: Vec<PlanUnitSpec>) -> ExecutionPlanSpec {
        serde_json::from_value(serde_json::json!({
            "schema": crate::execution_plan::EXECUTION_PLAN_SCHEMA_V3,
            "rationale": "r",
            "stages": stages.iter().map(|(k, kind)| serde_json::json!({"key": k, "kind": kind, "title": k})).collect::<Vec<_>>(),
            "units": units,
        }))
        .unwrap()
    }

    fn enabled() -> TreeLimits {
        TreeLimits {
            enabled: true,
            ..TreeLimits::default()
        }
    }

    fn gate_of(parent: &Task, depth: u32, unit: &PlanUnitSpec, needs: &[String]) -> UnitGate {
        let limits = enabled();
        let names = vec!["app".to_string()];
        let ctx = UnitGateContext {
            parent,
            parent_depth: depth,
            parent_repo_names: &names,
            limits: &limits,
            work_unit_max_turns: 80,
            work_unit_max_wall_secs: 3600,
        };
        unit_gate(&ctx, unit, needs)
    }

    fn v3_limits() -> ExecutionLimits {
        ExecutionLimits {
            tree: enabled(),
            ..ExecutionLimits::default()
        }
    }

    /// ADR-0079 §7 R2a (c) `unit_gate_table`: D4 (3) の表の 7 行。
    #[test]
    fn unit_gate_table() {
        let parent = parent_task(30);
        // 1. leaf + atomic → leaf（一致、記録なし）。
        let g = gate_of(&parent, 1, &leaf_spec("a", "s1", false), &[]);
        assert_eq!(g.declared, UnitDeclared::Leaf);
        assert_eq!(g.decision.mode, ExecutionMode::Atomic);
        assert_eq!((g.depth, g.threshold), (2, 7));
        assert_eq!(g.decision.depth, Some(2));
        assert_eq!(g.action, None);

        // 2. leaf + compound、子 task を持てる深さ（親 1 / 2）→ task に上げる。
        for depth in [1, 2] {
            let g = gate_of(&parent, depth, &leaf_spec("a", "s1", true), &[]);
            assert_eq!(g.decision.mode, ExecutionMode::Compound);
            assert_eq!(g.action, Some(UnitGateAction::Promoted), "depth {depth}");
            assert!(g.reason.contains("promoted"), "{}", g.reason);
        }
        // 3. leaf + compound、子 task を持てない深さ（親 3 = max_depth）→ 決定の要求。
        let g = gate_of(&parent, 3, &leaf_spec("a", "s1", true), &[]);
        assert_eq!(g.action, Some(UnitGateAction::Decision));
        assert_eq!((g.depth, g.threshold), (4, 11));
        assert!(g.reason.contains("max_depth 3"), "{}", g.reason);

        // 4. task + compound → task（一致）。
        let g = gate_of(&parent, 1, &task_spec("c", "s1", true), &[]);
        assert_eq!(g.declared, UnitDeclared::Task);
        assert_eq!(g.decision.mode, ExecutionMode::Compound);
        assert_eq!(g.action, None);

        // 5. task + atomic、構造上の理由（人の acceptance・決定・別の skill・genre・repos）→ task のまま。
        let mut human = task_spec("c", "s1", false);
        human.acceptance.push(crate::model::Criterion {
            text: "人が確かめる".into(),
            check: Check::Human,
        });
        let mut skills = task_spec("c", "s1", false);
        skills.skills = vec!["gpu".into()];
        let mut genre = task_spec("c", "s1", false);
        genre.genre = Some("research".into());
        let mut repos = task_spec("c", "s1", false);
        repos.repos = vec!["docs".into()];
        for (unit, needs, word) in [
            (&human, vec![], "human acceptance"),
            (
                &task_spec("c", "s1", false),
                vec!["h1".to_string()],
                "needs decisions [h1]",
            ),
            (&skills, vec![], "skills [gpu]"),
            (&genre, vec![], "genre research"),
            (&repos, vec![], "repos [docs]"),
        ] {
            let g = gate_of(&parent, 1, unit, &needs);
            assert_eq!(g.decision.mode, ExecutionMode::Atomic, "{word}");
            assert_eq!(g.action, Some(UnitGateAction::KeptTask), "{word}");
            assert!(g.reason.contains(word), "{word}: {}", g.reason);
        }
        // 6. task + atomic、理由なし、leaf の基準を満たす → leaf に下げる。
        let g = gate_of(&parent, 1, &task_spec("c", "s1", false), &[]);
        assert_eq!(g.action, Some(UnitGateAction::Demoted));
        // 同じ unit でも、継ぐ予算が 1 run を超える（leaf の基準 (a) を満たさない）なら task のまま。
        let big = parent_task(200);
        let g = gate_of(&big, 1, &task_spec("c", "s1", false), &[]);
        assert_eq!(g.action, Some(UnitGateAction::KeptTask));
        assert!(g.reason.contains("leaf criteria not met"), "{}", g.reason);
        // command の検査が無い（基準 (c)）なら task のまま。
        let mut reviewer_only = task_spec("c", "s1", false);
        reviewer_only.acceptance[0].check = Check::Reviewer;
        let g = gate_of(&parent, 1, &reviewer_only, &[]);
        assert_eq!(g.action, Some(UnitGateAction::KeptTask));
        assert!(g.reason.contains("no command check"), "{}", g.reason);

        // 7. task を子 task を持てない深さ（3）の計画に書く → 検証で拒否（ChildTaskTooDeep）、最後の試行では
        //    `plan_limit_holds` の `MaxDepth`（決定の要求）で止める（下の `plan_limit_holds_*`）。
        let p = plan(&[("s1", "implement")], vec![task_spec("c", "s1", false)]);
        let errs = validate_with(
            &p,
            v3_limits(),
            &[],
            PlanContext {
                origin: PlanOrigin::Planner,
                depth: 3,
            },
        )
        .unwrap_err();
        assert!(errs.contains(&PlanValidationError::ChildTaskTooDeep {
            key: "c".into(),
            depth: 3,
            max_depth: 3
        }));
    }

    /// D4 (3): 上げた / 下げた unit は仕事を落とさずに形を変え、計画の検証を通る。
    #[test]
    fn promoted_and_demoted_units_keep_their_work_and_validate() {
        let parent = parent_task(30);
        let p = plan(
            &[("s1", "implement"), ("s2", "test")],
            vec![
                leaf_spec("a", "s1", true),
                task_spec("c", "s2", false),
                leaf_spec("b", "s2", false),
            ],
        );
        let limits = enabled();
        let names = vec!["app".to_string()];
        let ctx = UnitGateContext {
            parent: &parent,
            parent_depth: 1,
            parent_repo_names: &names,
            limits: &limits,
            work_unit_max_turns: 80,
            work_unit_max_wall_secs: 3600,
        };
        let report = apply_unit_gates(&ctx, &p, &Default::default());
        let actions: Vec<(String, Option<UnitGateAction>)> = report
            .gates
            .iter()
            .map(|g| (g.unit_key.clone(), g.action))
            .collect();
        assert_eq!(
            actions,
            vec![
                ("a".into(), Some(UnitGateAction::Promoted)),
                ("c".into(), Some(UnitGateAction::Demoted)),
                ("b".into(), None),
            ]
        );
        assert_eq!(report.overridden().count(), 2);
        assert!(report.leaf_too_large.is_empty());
        let a = &report.spec.units[0];
        assert!(a.is_task());
        assert_eq!(a.repos, vec!["app".to_string()]);
        assert!(a.checks.is_empty() && a.budget.is_none() && a.context.paths.is_empty());
        assert_eq!(
            a.acceptance.len(),
            1,
            "checks become the command acceptance"
        );
        assert!(matches!(a.acceptance[0].check, Check::Command { .. }));
        assert!(a.objective.contains("src/"), "{}", a.objective);
        assert!(
            a.objective.contains(PROMOTED_DONE_WHEN_HEADING),
            "{}",
            a.objective
        );
        assert!(a.objective.contains("- a works"), "{}", a.objective);
        assert!(a.done_when.is_empty());
        let c = &report.spec.units[1];
        assert_eq!(c.kind, WorkUnitKind::Test, "the stage's kind");
        assert_eq!(c.checks.len(), 1);
        assert_eq!(c.checks[0].cmd, "make c");
        assert_eq!(c.done_when, vec!["c passes".to_string()]);
        assert!(c.acceptance.is_empty());
        validate_with(&report.spec, v3_limits(), &[], PlanContext::default())
            .expect("the gated plan validates");
        // skip（replan の done）は gate をかけない。
        let skip: std::collections::BTreeSet<String> = ["a".to_string()].into();
        let report = apply_unit_gates(&ctx, &p, &skip);
        assert_eq!(report.gates.len(), 2);
        assert!(!report.spec.units[0].is_task());
        // 深さ 3 の計画の compound な leaf は上げずに決定の要求へ。
        let ctx3 = UnitGateContext {
            parent_depth: 3,
            ..ctx
        };
        let report = apply_unit_gates(&ctx3, &p, &Default::default());
        assert_eq!(report.leaf_too_large, vec!["a".to_string()]);
        assert!(!report.spec.units[0].is_task(), "not forced into a task");
    }

    fn many_leaves(stage: &str, n: usize) -> Vec<PlanUnitSpec> {
        (0..n)
            .map(|i| leaf_spec(&format!("{stage}-l{i}"), stage, false))
            .collect()
    }

    /// D3（Phase R2a）: 計画の上限（段階の数・段階あたり・子 task・max_depth）と木の leaf を超える unit を
    /// 決定的に選ぶ。上限の内の unit は止めない。
    #[test]
    fn plan_limit_holds_select_only_the_excess() {
        let limits = enabled();
        let none = std::collections::BTreeSet::new();
        // 段階あたり 7 → 7 つ目だけ。
        let p = plan(&[("s1", "implement")], many_leaves("s1", 7));
        let holds = plan_limit_holds(&p, &limits, 1, 0, &none, &none);
        assert_eq!(
            holds,
            vec![LimitHold {
                limit: TreeLimitKind::UnitsPerStage,
                scope: Some("s1".into()),
                units: vec!["s1-l6".into()],
                count: 7,
                max: 6
            }]
        );
        assert_eq!(
            TreeLimitKind::UnitsPerStage.decision_key(Some("s1")),
            "limit:max_units_per_stage:s1"
        );
        // 段階 6 つ → 6 つ目の段階の unit。
        let stages: Vec<(String, &str)> = (1..=6).map(|i| (format!("s{i}"), "implement")).collect();
        let stage_refs: Vec<(&str, &str)> = stages.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let units: Vec<PlanUnitSpec> = (1..=6)
            .map(|i| leaf_spec(&format!("u{i}"), &format!("s{i}"), false))
            .collect();
        let holds = plan_limit_holds(&plan(&stage_refs, units), &limits, 1, 0, &none, &none);
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].limit, TreeLimitKind::Stages);
        assert_eq!(holds[0].units, vec!["u6".to_string()]);
        assert_eq!((holds[0].count, holds[0].max), (6, 5));
        // 子 task 7 つ（段階 2 つに分ける）→ 7 つ目。
        let mut units: Vec<PlanUnitSpec> = (0..4)
            .map(|i| task_spec(&format!("c{i}"), "s1", false))
            .collect();
        units.extend((4..7).map(|i| task_spec(&format!("c{i}"), "s2", false)));
        let p = plan(&[("s1", "implement"), ("s2", "implement")], units);
        let holds = plan_limit_holds(&p, &limits, 1, 0, &none, &none);
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].limit, TreeLimitKind::ChildTasks);
        assert_eq!(holds[0].units, vec!["c6".to_string()]);
        // max_depth: 深さ 3 の計画の kind task はすべて。leaf は止めない。
        let p = plan(
            &[("s1", "implement")],
            vec![leaf_spec("a", "s1", false), task_spec("c", "s1", false)],
        );
        let holds = plan_limit_holds(&p, &limits, 3, 0, &none, &none);
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].limit, TreeLimitKind::MaxDepth);
        assert_eq!(holds[0].units, vec!["c".to_string()]);
        assert!(plan_limit_holds(&p, &limits, 2, 0, &none, &none).is_empty());
        // 木の leaf: 既に 38、この計画の新しい leaf 3 つ（うち 1 つは既存の key）→ 残り 2 に収まる。
        let p = plan(&[("s1", "implement")], many_leaves("s1", 3));
        let existing: std::collections::BTreeSet<String> = ["s1-l0".to_string()].into();
        assert!(plan_limit_holds(&p, &limits, 1, 38, &existing, &none).is_empty());
        // 既に 39 なら 1 つ目の新しい leaf だけ、残りを止める。
        let holds = plan_limit_holds(&p, &limits, 1, 39, &existing, &none);
        assert_eq!(holds.len(), 1);
        assert_eq!(holds[0].limit, TreeLimitKind::TreeLeaves);
        assert_eq!(holds[0].units, vec!["s1-l2".to_string()]);
        assert_eq!((holds[0].count, holds[0].max), (41, 40));
        // unit の gate が止めた leaf（extra_held）は束に入れず、leaf の数にも数えない。
        let extra: std::collections::BTreeSet<String> = ["s1-l1".to_string()].into();
        assert!(plan_limit_holds(&p, &limits, 1, 39, &existing, &extra).is_empty());
        // 上限の内なら何も止めない・/2 は対象外。
        let p = plan(&[("s1", "implement")], many_leaves("s1", 6));
        assert!(plan_limit_holds(&p, &limits, 1, 0, &none, &none).is_empty());
        let mut v2 = p.clone();
        v2.schema = crate::execution_plan::EXECUTION_PLAN_SCHEMA_V2.to_string();
        assert!(
            plan_limit_holds(
                &v2,
                &TreeLimits {
                    max_units_per_stage: 1,
                    ..limits
                },
                1,
                0,
                &none,
                &none
            )
            .is_empty()
        );
    }

    fn run(task: TaskId, role: RunIndexRole, tokens: (u64, u64), cost: Option<f64>) -> RunRow {
        RunRow {
            run_id: TaskId::new().to_string(),
            task_id: task.to_string(),
            work_unit_id: None,
            role,
            seq: 1,
            status: RunIndexStatus::Completed,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: Some(Usage {
                input_tokens: Some(tokens.0),
                output_tokens: Some(tokens.1),
                cache_read_tokens: Some(999),
                cache_creation_tokens: None,
                cost_usd: cost,
            }),
            metrics: None,
            started_at: String::new(),
            finished_at: None,
        }
    }

    fn wu(task: TaskId, key: &str, kind: WorkUnitKind, status: WorkUnitStatus) -> WorkUnitRow {
        let spec: WorkUnitSpec = serde_json::from_value(serde_json::json!({
            "key": key, "kind": kind.as_str(), "title": key, "objective": key,
        }))
        .unwrap();
        let mut row = WorkUnitRow::new(
            key.into(),
            task.to_string(),
            "p".into(),
            0,
            spec,
            status,
            String::new(),
        );
        if status == WorkUnitStatus::Blocked {
            row.blocked_reason = Some(WorkUnitBlockedReason::Decision);
        }
        row
    }

    /// D3 / U-R7（Phase R2a）: 3 段の木（root → 子 → 孫）の数。run は reviewer を除き、reviewer の run と
    /// 定価は深さごとに出る。leaf は repair・統合・kind task・決定待ちの行を数えない。replan は版 − 1 の和。
    #[test]
    fn tree_counters_across_a_three_level_tree() {
        let (root, child, grandchild) = (TaskId::new(), TaskId::new(), TaskId::new());
        let nodes = vec![
            TreeNodeFacts {
                task_id: root,
                depth: 1,
                runs: vec![
                    run(root, RunIndexRole::Planner, (100, 10), Some(0.5)),
                    run(root, RunIndexRole::Worker, (200, 20), Some(1.0)),
                    run(root, RunIndexRole::Reviewer, (50, 5), Some(0.25)),
                ],
                work_units: vec![
                    wu(root, "a", WorkUnitKind::Implement, WorkUnitStatus::Done),
                    wu(root, "c", WorkUnitKind::Task, WorkUnitStatus::Running),
                    wu(
                        root,
                        "integrate-s1",
                        WorkUnitKind::Integrate,
                        WorkUnitStatus::Pending,
                    ),
                    wu(
                        root,
                        "held",
                        WorkUnitKind::Implement,
                        WorkUnitStatus::Blocked,
                    ),
                ],
                plan_versions: 2,
            },
            TreeNodeFacts {
                task_id: child,
                depth: 2,
                runs: vec![
                    run(child, RunIndexRole::Planner, (10, 1), Some(0.1)),
                    run(child, RunIndexRole::Worker, (20, 2), None),
                    run(child, RunIndexRole::Reviewer, (5, 1), Some(0.05)),
                ],
                work_units: vec![
                    wu(child, "g", WorkUnitKind::Task, WorkUnitStatus::Done),
                    wu(child, "l1", WorkUnitKind::Test, WorkUnitStatus::Ready),
                    wu(child, "fix", WorkUnitKind::Repair, WorkUnitStatus::Done),
                ],
                plan_versions: 1,
            },
            TreeNodeFacts {
                task_id: grandchild,
                depth: 3,
                runs: vec![
                    run(grandchild, RunIndexRole::WrapUp, (1, 1), Some(0.01)),
                    run(grandchild, RunIndexRole::Reviewer, (2, 2), Some(0.02)),
                ],
                work_units: vec![],
                plan_versions: 0,
            },
        ];
        let c = tree_counters(Some(root), &nodes);
        assert_eq!(c.root_id, Some(root));
        assert_eq!(c.nodes, 3);
        assert_eq!(c.leaves, 2, "a and l1");
        assert_eq!(c.runs, 5);
        assert_eq!(c.reviewer_runs, 3);
        assert_eq!(c.replans, 1);
        assert_eq!(
            c.tokens,
            110 + 220 + 55 + 11 + 22 + 6 + 2 + 4,
            "input + output, no cache"
        );
        assert!(
            !c.cost_usd_complete,
            "the child's worker has tokens but no cost"
        );
        assert!((c.cost_usd - (0.5 + 1.0 + 0.25 + 0.1 + 0.05 + 0.01 + 0.02)).abs() < 1e-9);
        let depths: Vec<u32> = c.by_depth.iter().map(|d| d.depth).collect();
        assert_eq!(depths, vec![1, 2, 3]);
        let d1 = &c.by_depth[0];
        assert_eq!((d1.nodes, d1.runs, d1.reviewer_runs), (1, 2, 1));
        assert!((d1.reviewer_cost_usd - 0.25).abs() < 1e-9);
        assert!(d1.cost_usd_complete);
        assert_eq!(d1.runs_by_role.get("planner"), Some(&1));
        let d2 = &c.by_depth[1];
        assert_eq!((d2.runs, d2.reviewer_runs, d2.tokens), (2, 1, 11 + 22 + 6));
        assert!(!d2.cost_usd_complete);
        assert!((d2.reviewer_cost_usd - 0.05).abs() < 1e-9);
        let d3 = &c.by_depth[2];
        assert_eq!((d3.runs, d3.reviewer_runs), (1, 1));
        assert_eq!(
            d3.runs_by_role
                .get("wrap_up")
                .or(d3.runs_by_role.get("wrapup")),
            Some(&1)
        );

        // 上限: run 5 本は max 5 で超過（もう 1 本起こすと 6）、4 本の上限でも。6 なら通る。
        let limits = TreeLimits {
            max_tree_runs: 5,
            ..enabled()
        };
        assert_eq!(
            run_limit_breach(&limits, &c, NextRun::Worker),
            Some(RunLimitBreach {
                limit: TreeLimitKind::TreeRuns,
                count: 5,
                max: 5
            })
        );
        let limits = TreeLimits {
            max_tree_runs: 6,
            ..enabled()
        };
        assert_eq!(run_limit_breach(&limits, &c, NextRun::Worker), None);
        // replan は replan の planner run だけが見る。
        let limits = TreeLimits {
            max_tree_replans: 1,
            ..enabled()
        };
        assert_eq!(run_limit_breach(&limits, &c, NextRun::Worker), None);
        assert_eq!(
            run_limit_breach(&limits, &c, NextRun::Planner { replan: false }),
            None
        );
        assert_eq!(
            run_limit_breach(&limits, &c, NextRun::Planner { replan: true }).map(|b| b.limit),
            Some(TreeLimitKind::TreeReplans)
        );
        // トークンは設定したときだけ。
        let limits = TreeLimits {
            max_tree_tokens: Some(c.tokens),
            ..enabled()
        };
        assert_eq!(
            run_limit_breach(&limits, &c, NextRun::Worker).map(|b| b.limit),
            Some(TreeLimitKind::TreeTokens)
        );
        assert_eq!(run_limit_breach(&enabled(), &c, NextRun::Worker), None);
    }

    /// D7: daemon の決定の要求の形（選択肢・推奨・key・needed_before）。
    #[test]
    fn daemon_decisions_have_the_d7_shape() {
        let root = TaskId::new();
        let raised_by = crate::decision::DecisionRaisedBy {
            task_id: root,
            run_id: None,
            origin: crate::decision::DecisionOrigin::Daemon,
        };
        let d = limit_decision(
            TreeLimitKind::TreeRuns,
            None,
            120,
            120,
            vec!["self".into()],
            vec![],
            raised_by.clone(),
        );
        assert_eq!(d.kind, crate::decision::DecisionKind::Limit);
        assert_eq!(d.key, "limit:max_tree_runs");
        assert_eq!(d.recommended, "replan");
        let keys: Vec<&str> = d.options.iter().map(|o| o.key.as_str()).collect();
        assert_eq!(keys, vec!["raise-once", "replan", "withdraw"]);
        assert_eq!(d.status, crate::decision::DecisionStatus::Open);
        assert_eq!(d.root_id(), root);
        assert_eq!(d.id.len(), 26, "a ULID");
        // 計画の決定と同じ形の検査（key の書式以外）を通る。
        let spec = crate::decision::DecisionSpec {
            key: "x".into(),
            question: d.question.clone(),
            options: d.options.clone(),
            recommended: d.recommended.clone(),
            cost_of_reversal: d.cost_of_reversal,
            cost_note: None,
            needed_before: d.needed_before.clone(),
        };
        assert!(crate::decision::validate_shape(&spec).is_empty());
        let parent = parent_task(30);
        let g = gate_of(&parent, 3, &leaf_spec("a", "s1", true), &[]);
        let d = leaf_too_large_decision(&g, "Leaf a", vec![], raised_by);
        assert_eq!(d.kind, crate::decision::DecisionKind::LeafTooLarge);
        assert_eq!(d.key, "leaf_too_large:a");
        assert_eq!(d.needed_before, vec!["a".to_string()]);
        assert!(
            d.question.contains("compound/long-and-broad"),
            "{}",
            d.question
        );
    }

    /// R3a: limit の key の読み戻し、run 時の上限、`raise-once` の余裕、plan_invalid の選択肢。
    #[test]
    fn limit_keys_allowances_and_plan_invalid_options() {
        assert_eq!(
            TreeLimitKind::from_decision_key("limit:max_tree_runs"),
            Some(TreeLimitKind::TreeRuns)
        );
        assert_eq!(
            TreeLimitKind::from_decision_key("limit:max_units_per_stage:s1"),
            Some(TreeLimitKind::UnitsPerStage)
        );
        assert_eq!(TreeLimitKind::from_decision_key("h1"), None);
        assert!(TreeLimitKind::TreeRuns.is_run_time());
        assert!(TreeLimitKind::NodeReplans.is_run_time());
        assert!(!TreeLimitKind::TreeLeaves.is_run_time());
        assert_eq!(limit_allowance_step(TreeLimitKind::TreeRuns, 120), 60);
        assert_eq!(limit_allowance_step(TreeLimitKind::TreeRuns, 1), 1);
        assert_eq!(limit_allowance_step(TreeLimitKind::NodeReplans, 3), 1);
        let mut allowances = std::collections::BTreeMap::new();
        allowances.insert(TreeLimitKind::TreeRuns, 2);
        allowances.insert(TreeLimitKind::TreeTokens, 1);
        let base = TreeLimits {
            max_tree_tokens: Some(1000),
            ..TreeLimits::default()
        };
        let raised = limits_with_allowances(&base, &allowances);
        assert_eq!(raised.max_tree_runs, 240);
        assert_eq!(raised.max_tree_tokens, Some(1500));
        assert_eq!(raised.max_tree_replans, base.max_tree_replans);

        let raised_by = crate::decision::DecisionRaisedBy {
            task_id: TaskId::new(),
            run_id: None,
            origin: crate::decision::DecisionOrigin::Daemon,
        };
        let keys = |replan: bool| -> Vec<String> {
            plan_invalid_decision(TaskId::new(), replan, &[], vec![], raised_by.clone())
                .options
                .into_iter()
                .map(|o| o.key)
                .collect()
        };
        assert_eq!(keys(false), vec!["replan", "atomic", "cancel"]);
        assert_eq!(
            keys(true),
            vec!["replan", "cancel"],
            "no atomic once a plan exists"
        );
    }
}
