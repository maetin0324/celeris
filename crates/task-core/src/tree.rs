//! ADR-0079（Phase R1a）: 再帰的な task 分解の木の純粋な型と関数。
//!
//! - [`TreeInfo`]: `Task.tree`（木の中の位置。root / 深さ / 親の unit / 基点）。
//! - [`TreeLimits`]: `[execution.tree]` の上限（D3。R0 の付記 U-R1 の数え方）。
//! - 深さの計算（`can_have_child_tasks` / `remaining_depth` / `gate_threshold`）。
//! - [`UnitDeclared`] / [`UnitGateAction`]: `Event::UnitGateOverridden` の語彙（発行は R2a）。
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
}
