//! `[execution]`（ADR-0072 D18）・`[execution.tree]`（ADR-0079 D3）・`[execution.planner]`（ADR-0072 D14）。

use serde::Deserialize;
use task_core::Tier;

use super::ConfigError;

/// `[execution]`（ADR-0072 D18, Phase E1）: continuation（予算切れ・yield の続き）の可否と上限。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTomlConfig {
    /// `false` で E1 の continuation を無効にする（予算切れ・yield は従来どおり
    /// `WorkerError{retryable:true}` に戻る。ADR-0072 §6 (f)）。
    #[serde(default = "default_execution_continuation")]
    pub continuation: bool,
    /// 1 つの WorkUnit（E1 は暗黙の WorkUnit）が continuation できる回数の上限。
    #[serde(default = "default_max_continuations_per_work_unit")]
    pub max_continuations_per_work_unit: u32,
    /// 進捗なしの continuation が連続この回数で `blocked` にする。
    #[serde(default = "default_no_progress_limit")]
    pub no_progress_limit: u32,
    /// ADR-0072 D13（Phase E3）: Complexity Gate の運用モード。`"off" | "shadow" | "on"`。
    /// 既定は `"shadow"`（判定と記録だけをして、計画は作らない）。
    #[serde(default = "default_execution_gate")]
    pub gate: String,
    /// ADR-0072 D14（Phase E3）: `[execution.planner]`。
    #[serde(default)]
    pub planner: ExecutionPlannerTomlConfig,
    /// ADR-0072 D16/D18（Phase E4）: Task ごとの reviewer repair の上限（既定 3）。
    #[serde(default = "default_max_repairs")]
    pub max_repairs: u32,
    /// ADR-0072 D16/D18（Phase E4）: 同じ class の repair の上限（既定 2）。
    #[serde(default = "default_max_repairs_per_class")]
    pub max_repairs_per_class: u32,
    /// ADR-0072 D17/D18（Phase E4）: Task ごとの replan（計画の版の更新）の上限（既定 5。ADR-0079「R6-2」で 3 から）。
    #[serde(default = "default_max_replans")]
    pub max_replans: u32,
    /// ADR-0074 D5.2（Phase F1）: WU の lane の上限を Task の lane に合わせるか
    /// （`"task"` | `"none"`。既定 `"task"`）。
    #[serde(default = "default_work_unit_lane_cap")]
    pub work_unit_lane_cap: String,
    /// ADR-0074 D1.1/§4（Phase F2b）: `true` で planner に v2（工程と並列 WU）を出させる（既定 `false`）。
    #[serde(default)]
    pub parallel: bool,
    /// ADR-0074 D1.3/§4（Phase F2b）: Task ごとの同時 WU 数の上限（既定 3、1..=6）。
    #[serde(default = "default_max_parallel_work_units")]
    pub max_parallel_work_units: usize,
    /// ADR-0089（Phase R6-5）: CoS の対話 run（Console の一言）の同時数の絶対上限（既定 2、0..=8）。
    /// CoS の対話 run は `max_concurrency` とアカウントプールのプロバイダの `concurrency` に数えず、
    /// この上限だけで待つ。`0` で例外を無効にする（通常の run と同じ規則に戻る）。
    #[serde(default = "default_max_cos_runs")]
    pub max_cos_runs: usize,
    /// ADR-0132 付記 L7: cheap lane の worker run で、アカウントプールの順位付けより先にローカルの行
    /// （Qwen 等。付記 L1）を試すか（既定 `true`）。`false` で付記の前の選び方に戻す。
    #[serde(default = "default_cheap_local_first")]
    pub cheap_local_first: bool,
    /// ADR-0079 D3（Phase R1a）: `[execution.tree]`（再帰的な task 分解の上限。既定 `enabled = false`）。
    #[serde(default)]
    pub tree: ExecutionTreeTomlConfig,
}

/// `[execution.tree]`（ADR-0079 D3、Phase R1a）: plan/3 と子 task の上限。すべて任意で、既定は D3 の表
/// （U-R4 の決定どおり）。`max_depth` は **task の層数**（root 1 / 子 2 / 孫 3。付記 U-R1）で 1..=3。
/// `enabled = false`（既定。R5b で人が `true` に）なら `celeris.execution-plan/3` は検証で拒否される。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTreeTomlConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_tree_max_depth")]
    pub max_depth: u32,
    #[serde(default = "default_tree_auto_leaf")]
    pub auto_leaf: bool,
    #[serde(default = "default_tree_auto_leaf_max_compactions")]
    pub auto_leaf_max_compactions: u32,
    #[serde(default = "default_tree_auto_leaf_max_continuations")]
    pub auto_leaf_max_continuations: u32,
    #[serde(default = "default_tree_max_units_per_stage")]
    pub max_units_per_stage: usize,
    #[serde(default = "default_tree_max_stages")]
    pub max_stages: usize,
    #[serde(default = "default_tree_max_child_tasks_per_plan")]
    pub max_child_tasks_per_plan: usize,
    #[serde(default = "default_tree_max_parallel_child_tasks")]
    pub max_parallel_child_tasks: usize,
    #[serde(default = "default_tree_max_tree_leaves")]
    pub max_tree_leaves: u32,
    #[serde(default = "default_tree_max_tree_runs")]
    pub max_tree_runs: u32,
    #[serde(default = "default_tree_max_tree_replans")]
    pub max_tree_replans: u32,
    /// 木全体の input + output トークンの上限（設定したときだけ）。
    #[serde(default)]
    pub max_tree_tokens: Option<u64>,
    /// 未回答の決定の上限（木あたり）。
    #[serde(default = "default_tree_max_open_decisions")]
    pub max_open_decisions: usize,
    /// 未回答の決定の上限（計画あたり）。
    #[serde(default = "default_tree_max_open_decisions_per_plan")]
    pub max_open_decisions_per_plan: usize,
    /// Phase R2a: 深さ d の子 task・unit の gate の閾値 = `5 + gate_depth_step × (d − 1)`（既定 2、0..=10）。
    /// root（深さ 1）の閾値は 5 のまま、root の gate は `[execution] gate` に従う（U-R5）。
    #[serde(default = "default_tree_gate_depth_step")]
    pub gate_depth_step: u32,
    /// D8 の「上限に近い」の比（0 < r <= 1、既定 0.8）。
    #[serde(default = "default_tree_approval_near_limit_ratio")]
    pub approval_near_limit_ratio: f64,
    /// Phase R3b（D10 の生存確認）: 木の節点が理由なく止まっているとみなすまでの秒数（既定 600、60 以上）。
    #[serde(default = "default_tree_liveness_timeout_secs")]
    pub liveness_timeout_secs: u64,
}

impl Default for ExecutionTreeTomlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_depth: default_tree_max_depth(),
            auto_leaf: default_tree_auto_leaf(),
            auto_leaf_max_compactions: default_tree_auto_leaf_max_compactions(),
            auto_leaf_max_continuations: default_tree_auto_leaf_max_continuations(),
            max_units_per_stage: default_tree_max_units_per_stage(),
            max_stages: default_tree_max_stages(),
            max_child_tasks_per_plan: default_tree_max_child_tasks_per_plan(),
            max_parallel_child_tasks: default_tree_max_parallel_child_tasks(),
            max_tree_leaves: default_tree_max_tree_leaves(),
            max_tree_runs: default_tree_max_tree_runs(),
            max_tree_replans: default_tree_max_tree_replans(),
            max_tree_tokens: None,
            max_open_decisions: default_tree_max_open_decisions(),
            max_open_decisions_per_plan: default_tree_max_open_decisions_per_plan(),
            gate_depth_step: default_tree_gate_depth_step(),
            approval_near_limit_ratio: default_tree_approval_near_limit_ratio(),
            liveness_timeout_secs: default_tree_liveness_timeout_secs(),
        }
    }
}

impl ExecutionTreeTomlConfig {
    /// `ExecutionLimits.tree`（plan/3 の検証だけが見る）。`validate()` を通った値を前提にする。
    pub fn limits(&self) -> task_core::TreeLimits {
        task_core::TreeLimits {
            enabled: self.enabled,
            max_depth: self.max_depth,
            auto_leaf: self.auto_leaf,
            auto_leaf_max_compactions: self.auto_leaf_max_compactions,
            auto_leaf_max_continuations: self.auto_leaf_max_continuations,
            max_units_per_stage: self.max_units_per_stage,
            max_stages: self.max_stages,
            max_child_tasks_per_plan: self.max_child_tasks_per_plan,
            max_parallel_child_tasks: self.max_parallel_child_tasks,
            max_tree_leaves: self.max_tree_leaves,
            max_tree_runs: self.max_tree_runs,
            max_tree_replans: self.max_tree_replans,
            max_tree_tokens: self.max_tree_tokens,
            max_open_decisions_per_tree: self.max_open_decisions,
            max_open_decisions_per_plan: self.max_open_decisions_per_plan,
            gate_depth_step: self.gate_depth_step,
            approval_near_limit_permille: (self.approval_near_limit_ratio * 1000.0).round() as u32,
            liveness_timeout_secs: self.liveness_timeout_secs,
        }
    }

    /// 設定の綴り・範囲（`Config::validate` から呼ぶ）。
    fn validate(&self) -> Result<(), String> {
        if !(1..=task_core::tree::MAX_DEPTH_CAP).contains(&self.max_depth) {
            return Err(format!(
                "max_depth must be between 1 and {} task levels (root = 1, child = 2, grandchild = 3; got {})",
                task_core::tree::MAX_DEPTH_CAP,
                self.max_depth
            ));
        }
        for (name, v) in [
            ("max_units_per_stage", self.max_units_per_stage),
            ("max_stages", self.max_stages),
            ("max_child_tasks_per_plan", self.max_child_tasks_per_plan),
            ("max_parallel_child_tasks", self.max_parallel_child_tasks),
            ("max_open_decisions", self.max_open_decisions),
            (
                "max_open_decisions_per_plan",
                self.max_open_decisions_per_plan,
            ),
        ] {
            if v == 0 {
                return Err(format!("{name} must be >= 1"));
            }
        }
        for (name, v) in [
            ("max_tree_leaves", self.max_tree_leaves),
            ("max_tree_runs", self.max_tree_runs),
            ("max_tree_replans", self.max_tree_replans),
        ] {
            if v == 0 {
                return Err(format!("{name} must be >= 1"));
            }
        }
        if self.gate_depth_step > MAX_TREE_GATE_DEPTH_STEP {
            return Err(format!(
                "gate_depth_step must be <= {MAX_TREE_GATE_DEPTH_STEP} (got {})",
                self.gate_depth_step
            ));
        }
        if self.max_tree_tokens == Some(0) {
            return Err("max_tree_tokens must be >= 1 when set".to_string());
        }
        if self.max_open_decisions_per_plan > self.max_open_decisions {
            return Err(format!(
                "max_open_decisions_per_plan ({}) must not exceed max_open_decisions ({})",
                self.max_open_decisions_per_plan, self.max_open_decisions
            ));
        }
        if self.liveness_timeout_secs < MIN_TREE_LIVENESS_TIMEOUT_SECS {
            return Err(format!(
                "liveness_timeout_secs must be >= {MIN_TREE_LIVENESS_TIMEOUT_SECS} (got {})",
                self.liveness_timeout_secs
            ));
        }
        let r = self.approval_near_limit_ratio;
        if !(r.is_finite() && r > 0.0 && r <= 1.0) {
            return Err(format!(
                "approval_near_limit_ratio must be in (0, 1] (got {r})"
            ));
        }
        Ok(())
    }
}

/// Phase R3b: `liveness_timeout_secs` の下限（tick の間隔より十分に長く、通知を乱発しない）。
const MIN_TREE_LIVENESS_TIMEOUT_SECS: u64 = 60;

fn default_tree_liveness_timeout_secs() -> u64 {
    task_core::tree::DEFAULT_LIVENESS_TIMEOUT_SECS
}

/// Phase R2a: `gate_depth_step` の上限（深さ 3 で閾値 25。規則表のスコアの最大を十分に超える）。
const MAX_TREE_GATE_DEPTH_STEP: u32 = 10;

fn default_tree_max_depth() -> u32 {
    task_core::tree::DEFAULT_MAX_DEPTH
}
fn default_tree_max_units_per_stage() -> usize {
    task_core::TreeLimits::default().max_units_per_stage
}
fn default_tree_max_stages() -> usize {
    task_core::TreeLimits::default().max_stages
}
fn default_tree_max_child_tasks_per_plan() -> usize {
    task_core::TreeLimits::default().max_child_tasks_per_plan
}
fn default_tree_max_parallel_child_tasks() -> usize {
    task_core::TreeLimits::default().max_parallel_child_tasks
}
fn default_tree_max_tree_leaves() -> u32 {
    task_core::TreeLimits::default().max_tree_leaves
}
fn default_tree_max_tree_runs() -> u32 {
    task_core::TreeLimits::default().max_tree_runs
}
fn default_tree_max_tree_replans() -> u32 {
    task_core::TreeLimits::default().max_tree_replans
}
fn default_tree_max_open_decisions() -> usize {
    task_core::TreeLimits::default().max_open_decisions_per_tree
}
fn default_tree_max_open_decisions_per_plan() -> usize {
    task_core::TreeLimits::default().max_open_decisions_per_plan
}
fn default_tree_gate_depth_step() -> u32 {
    task_core::TreeLimits::default().gate_depth_step
}
fn default_tree_approval_near_limit_ratio() -> f64 {
    0.8
}

impl Default for ExecutionTomlConfig {
    fn default() -> Self {
        Self {
            continuation: default_execution_continuation(),
            max_continuations_per_work_unit: default_max_continuations_per_work_unit(),
            no_progress_limit: default_no_progress_limit(),
            gate: default_execution_gate(),
            planner: ExecutionPlannerTomlConfig::default(),
            max_repairs: default_max_repairs(),
            max_repairs_per_class: default_max_repairs_per_class(),
            max_replans: default_max_replans(),
            work_unit_lane_cap: default_work_unit_lane_cap(),
            parallel: false,
            max_parallel_work_units: default_max_parallel_work_units(),
            max_cos_runs: default_max_cos_runs(),
            cheap_local_first: default_cheap_local_first(),
            tree: ExecutionTreeTomlConfig::default(),
        }
    }
}

fn default_cheap_local_first() -> bool {
    true
}
fn default_execution_continuation() -> bool {
    true
}
fn default_max_continuations_per_work_unit() -> u32 {
    3
}
fn default_no_progress_limit() -> u32 {
    2
}
fn default_execution_gate() -> String {
    "shadow".to_string()
}
fn default_max_repairs() -> u32 {
    3
}
fn default_max_repairs_per_class() -> u32 {
    2
}
fn default_max_replans() -> u32 {
    // ADR-0079「R6-2」: 木の節点ごとの replan の余地を 3 → 5（子の失敗の replan で使い切っていた）。
    5
}
fn default_work_unit_lane_cap() -> String {
    "task".to_string()
}
fn default_max_parallel_work_units() -> usize {
    3
}
fn default_max_cos_runs() -> usize {
    task_dispatch::capacity::DEFAULT_MAX_COS_RUNS
}
/// ADR-0089: `[execution] max_cos_runs` の上限（CoS の対話 run が積み上がらないための設定値の天井）。
const MAX_COS_RUNS_CAP: usize = 8;

/// `[execution.planner]`（ADR-0072 D14, Phase E3; ADR-0074 D5.3, Phase F1）: task-local な計画 run の
/// harness と上限。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlannerTomlConfig {
    #[serde(default = "default_planner_adapter")]
    pub adapter: String,
    /// planner run の `--permission-mode`。既定 `"bypassPermissions"`（アダプタ既定と同じ）。
    /// 2026-09-27 の F5-1 dogfood で `"plan"` だと claude-code が Plan Mode に入り、`Write` が plan
    /// ファイル以外へ書けず `ExitPlanMode` も非対話では使えないため、`execution-plan.json` /
    /// `result.json` を書けずに planner が 2 回失敗して atomic に倒れた。planner は成果物を
    /// **書く**役なので Plan Mode は使わない（ADR-0074「Phase F5 実装時の逸脱・明確化」）。
    #[serde(default = "default_planner_permission_mode")]
    pub permission_mode: String,
    /// ADR-0074 D5.3（Phase F1）: planner run の lane。既定 `standard`（E3〜E6 の固定 `frontier` から
    /// 変更。E6-4 の時間の多くが探索に費やされた分析を受けての判断）。人が Task に `tier:frontier` を
    /// 明示していれば、それが優先される（`dispatcher.rs` の配線）。
    #[serde(default = "default_planner_tier")]
    pub tier: Tier,
    #[serde(default = "default_planner_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_planner_max_wall_secs")]
    pub max_wall_secs: u64,
}

impl Default for ExecutionPlannerTomlConfig {
    fn default() -> Self {
        Self {
            adapter: default_planner_adapter(),
            permission_mode: default_planner_permission_mode(),
            tier: default_planner_tier(),
            max_turns: default_planner_max_turns(),
            max_wall_secs: default_planner_max_wall_secs(),
        }
    }
}

fn default_planner_adapter() -> String {
    "claude-code".to_string()
}
fn default_planner_permission_mode() -> String {
    "bypassPermissions".to_string()
}
fn default_planner_tier() -> Tier {
    Tier::Standard
}
/// ADR-0074 D5.3（Phase F1）: 40 -> 24（既定）。
fn default_planner_max_turns() -> u32 {
    24
}
/// ADR-0074 D5.3（Phase F1）: 1,200 -> 900 秒（既定）。
fn default_planner_max_wall_secs() -> u64 {
    900
}

impl ExecutionTomlConfig {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        // ADR-0072 D13（Phase E3）: gate は 3 つだけ（綴り間違いで黙って shadow/off に倒れないように）。
        if task_core::GateMode::parse(&self.gate).is_none() {
            return Err(ConfigError::Invalid(format!(
                "[execution] gate must be one of [\"off\", \"shadow\", \"on\"] (got {:?})",
                self.gate
            )));
        }
        // ADR-0074 D5.2（Phase F1）: work_unit_lane_cap は 2 つだけ。
        if task_core::WorkUnitLaneCap::parse(&self.work_unit_lane_cap).is_none() {
            return Err(ConfigError::Invalid(format!(
                "[execution] work_unit_lane_cap must be one of [\"task\", \"none\"] (got {:?})",
                self.work_unit_lane_cap
            )));
        }
        // ADR-0074 §4（Phase F2b）: max_parallel_work_units は 1..=6。
        if !(1..=task_dispatch::dispatcher::MAX_PARALLEL_WORK_UNITS_CAP)
            .contains(&self.max_parallel_work_units)
        {
            return Err(ConfigError::Invalid(format!(
                "[execution] max_parallel_work_units must be between 1 and {} (got {})",
                task_dispatch::dispatcher::MAX_PARALLEL_WORK_UNITS_CAP,
                self.max_parallel_work_units
            )));
        }
        // ADR-0089（Phase R6-5）: max_cos_runs は 0..=8。
        if self.max_cos_runs > MAX_COS_RUNS_CAP {
            return Err(ConfigError::Invalid(format!(
                "[execution] max_cos_runs must be between 0 and {MAX_COS_RUNS_CAP} (got {})",
                self.max_cos_runs
            )));
        }
        // ADR-0079 D3（Phase R1a）: `[execution.tree]` の範囲（max_depth は task の層数で 1..=3）。
        if let Err(why) = self.tree.validate() {
            return Err(ConfigError::Invalid(format!("[execution.tree] {why}")));
        }
        Ok(())
    }
}

fn default_tree_auto_leaf() -> bool {
    task_core::TreeLimits::default().auto_leaf
}
fn default_tree_auto_leaf_max_compactions() -> u32 {
    task_core::TreeLimits::default().auto_leaf_max_compactions
}
fn default_tree_auto_leaf_max_continuations() -> u32 {
    task_core::TreeLimits::default().auto_leaf_max_continuations
}
