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

/// ADR-0079「R5b-fix3」: 木の子 task の予算の下限（1 run の turns）。ADR-0072 D18 の WU の既定
/// （planner が `budget` を書かなかった leaf の run）と同じ値。子 task は「leaf 1 本より少ない予算」で
/// 走らない。
pub const TREE_CHILD_MIN_MAX_TURNS: u32 = 30;
/// ADR-0079「R5b-fix3」: 木の子 task の予算の下限（1 run の壁時計秒）。ADR-0072 D18 の WU の既定と同じ。
pub const TREE_CHILD_MIN_MAX_WALL_SECS: u64 = 1800;

/// ADR-0079「R5b-fix3」: 木の子 task（と、その unit の view）の予算 = `max(親の予算, leaf 1 run の既定
/// 30 turns / 1,800 秒)`。`max_retries` は親のまま。人が案件の下に直接作った root task の予算は変えない
/// （これは `build_child_task` と `unit_view` だけが使う）。
pub fn tree_child_budget(parent: &crate::model::Budget) -> crate::model::Budget {
    crate::model::Budget {
        max_turns: parent.max_turns.max(TREE_CHILD_MIN_MAX_TURNS),
        max_wall_secs: parent.max_wall_secs.max(TREE_CHILD_MIN_MAX_WALL_SECS),
        max_retries: parent.max_retries,
    }
}

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
    /// 木の生涯で作る leaf（repair・統合を除く。既定 120〈R6-2 で 40 から〉。R2a で使う）。
    pub max_tree_leaves: u32,
    /// 木全体の worker / planner / repair の run（reviewer を除く。既定 400〈R6-2 で 120 から〉。R2a で使う）。
    pub max_tree_runs: u32,
    /// 木全体で採用した replan の版（既定 30〈R6-2 で 10 から〉。R2a で使う）。
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
    /// D10 の生存確認（Phase R3b）: 木の節点が「走っている / 走れる / 名指しの待ち」のどれでもないまま
    /// この秒数続いたら `StallDetected` と障害通知（D10 の `stall_secs`。既定 600）。
    pub liveness_timeout_secs: u64,
}

/// D10（Phase R3b）: `liveness_timeout_secs` の既定。
pub const DEFAULT_LIVENESS_TIMEOUT_SECS: u64 = 600;

impl Default for TreeLimits {
    fn default() -> Self {
        TreeLimits {
            enabled: false,
            max_depth: DEFAULT_MAX_DEPTH,
            max_units_per_stage: 6,
            max_stages: 5,
            max_child_tasks_per_plan: 6,
            max_parallel_child_tasks: 2,
            // ADR-0079「R6-2」: 本番の木（web GUI は leaf 約 55、browser は run 100）が上限に当たったので上げた。
            max_tree_leaves: 120,
            max_tree_runs: 400,
            max_tree_replans: 30,
            max_tree_tokens: None,
            max_open_decisions_per_tree: 12,
            max_open_decisions_per_plan: 8,
            gate_depth_step: 2,
            approval_near_limit_permille: 800,
            liveness_timeout_secs: DEFAULT_LIVENESS_TIMEOUT_SECS,
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
            liveness_timeout_secs: u64::MAX,
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
    /// task と宣言され atomic だが、構造上の理由があるので task のまま。R6-2 からは出さない（kind task の
    /// unit の gate は明示。過去の event の読み取りのために残す）。
    KeptTask,
    /// task と宣言され atomic、構造上の理由なし・leaf の基準を満たす → leaf に下げた。R6-2 からは出さない
    /// （`KeptTask` と同じ）。
    Demoted,
}

mod approval;
mod gate;
mod limits;
mod liveness;

pub use approval::*;
pub use gate::*;
pub use limits::*;
pub use liveness::*;

#[cfg(test)]
#[path = "tree/r3b_tests.rs"]
mod r3b_tests;
#[cfg(test)]
#[path = "tree/tests.rs"]
mod tests;
