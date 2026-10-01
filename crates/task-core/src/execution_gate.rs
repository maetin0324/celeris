//! ADR-0072 D13（Phase E3）: Complexity Gate（atomic / compound の決定的な判定）。
//!
//! 純粋なデータ定義と純粋関数だけを置く（I/O・LLM 呼び出しはしない。ADR-0001 D2）。gate 自体は
//! LLM を使わない（決定的な規則表。ADR-0072 D4 / D13）。`TaskFeatures` は
//! `task_core::model_policy::TaskFeatures::infer_with_hints` の結果をそのまま渡してもらう
//! （既存の 9 軸を再利用し、足りない信号だけ [`ExecutionGateInputs`] で補う）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Check, Task, TaskKind, WorkspaceMode};
use crate::model_policy::{Level, TaskFeatures};

/// D13: gate の policy 版（監査記録に残す）。
pub const EXECUTION_GATE_POLICY_VERSION: &str = "exec-gate/1";
/// D13: `score >= threshold` なら compound。
pub const EXECUTION_GATE_SCORE_THRESHOLD: i32 = 5;

/// 固定パイプラインの harness（`literature` = paperqa、`web-research` = LDR、`knowledge` = langmem）。
/// D13 の対象外規則。
const FIXED_PIPELINE_GENRES: &[&str] = &["literature", "web-research", "knowledge"];

/// D13: atomic / compound の判定結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Atomic,
    Compound,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ExecutionMode::Atomic => "atomic",
            ExecutionMode::Compound => "compound",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "atomic" => Some(ExecutionMode::Atomic),
            "compound" => Some(ExecutionMode::Compound),
            _ => None,
        }
    }
}

/// D13: `ExecutionGateDecision.source`。人の明示 > 規則表（CoS のヒントが効いたかどうかは
/// `Hint` として残す。`Hint` でも実際に mode を決めるのは規則表のスコアである点に注意
/// — 人の明示だけが規則表そのものをバイパスする）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateSource {
    Policy,
    Human,
    Hint,
}

/// D13: 当たった信号 1 件（規則表の行、または強制規則・対象外規則）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GateSignal {
    pub name: String,
    pub weight: i32,
    pub detail: String,
}

/// D13: `Event::ExecutionGated` の中身、および `Task.routing.execution`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionGateDecision {
    pub mode: ExecutionMode,
    pub source: GateSource,
    pub score: i32,
    pub threshold: i32,
    pub rule_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<GateSignal>,
    pub policy_version: String,
    /// `[execution] gate = "shadow"` のときの判定なら `true`（記録だけで実行には使わない。D13）。
    /// ADR-0079 D4 (1)（Phase R2a）: 木の子（`depth` が `Some`）は `shadow` の設定でも判定を採用するので
    /// 常に `false`。
    pub shadow: bool,
    /// ADR-0079 D4 (1)（Phase R2a）: 木の子 task（depth ≥ 2）の判定なら、その深さ（task の層数）。
    /// 閾値は `threshold`（`5 + gate_depth_step × (depth − 1)`）。木の子の判定は `[execution] gate` が
    /// `shadow` / `off` でも採用される（`shadow = false`）。root・木でない task は `None`（出力しない。
    /// 既存の JSON は 1 バイトも変わらない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

/// ADR-0079 D3 / D4 (1)（Phase R2a）: gate の閾値と、木の子なら深さ。root・木でない task は
/// [`GateThreshold::ROOT`]（ADR-0072 D13 の 5、深さは記録しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateThreshold {
    pub threshold: i32,
    pub depth: Option<u32>,
}

impl GateThreshold {
    /// root（深さ 1）・木でない task: 閾値 5、深さは記録しない。
    pub const ROOT: GateThreshold = GateThreshold {
        threshold: EXECUTION_GATE_SCORE_THRESHOLD,
        depth: None,
    };

    /// 深さ `depth`（task の層数）の閾値（`task_core::tree::gate_threshold`）。`depth <= 1` は
    /// [`GateThreshold::ROOT`] と同じ閾値だが、深さを記録する。
    pub fn at_depth(depth: u32, step: u32) -> Self {
        let t = crate::tree::gate_threshold(depth, step);
        GateThreshold {
            threshold: i32::try_from(t).unwrap_or(i32::MAX),
            depth: Some(depth),
        }
    }
}

impl Default for GateThreshold {
    fn default() -> Self {
        GateThreshold::ROOT
    }
}

/// `NewTaskSpec.execution` / CoS の `create_task.execution` から `Task.routing.execution_hint` に運ぶ値。
/// `explicit = true` は人（API/CLI）の明示（gate をバイパスする）、`false` は CoS のヒント
/// （signal `H` として +2 されるだけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionHintSpec {
    pub mode: ExecutionMode,
    pub explicit: bool,
}

/// gate が dispatcher/store から読む必要がある信号（S4/S6）。純粋関数のままにするため、
/// 呼び出し側が決定的に計算して渡す。
#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutionGateInputs {
    /// S4: 複数の実行環境（remote workspace と local repos の混在、または skills が 2 部署以上にまたがる）。
    pub multi_environment: bool,
    /// S6: 同じ担当ノード・同じ genre の直近終端タスク（14 日以内、最大 20 件）のうち、
    /// `budget_exhausted` の run/continuation を持ったものの割合（0.0..=1.0）。無ければ `None`。
    pub recent_budget_exhausted_ratio: Option<f64>,
}

/// D13: gate の対象外なら理由（rule_id）を返す。`kind != Execute` / 対話 / support-task /
/// `routing` を持たない旧タスク / 固定パイプラインの harness / `workspace_mode = Shared` の内部タスク。
pub fn out_of_scope_rule(task: &Task) -> Option<&'static str> {
    if task.kind != TaskKind::Execute {
        return Some("atomic/out-of-scope");
    }
    if crate::message::is_conversation(task) {
        return Some("atomic/out-of-scope");
    }
    if crate::report::support_kind(task).is_some() {
        return Some("atomic/out-of-scope");
    }
    if task.routing.is_none() {
        return Some("atomic/out-of-scope");
    }
    if task
        .genre
        .as_deref()
        .is_some_and(|g| FIXED_PIPELINE_GENRES.contains(&g))
    {
        return Some("atomic/out-of-scope");
    }
    if task.workspace.local_mode() == WorkspaceMode::Shared
        || task.workspace.remote_mode() == WorkspaceMode::Shared
    {
        return Some("atomic/out-of-scope");
    }
    None
}

/// 題名・目的に出てくる工程語の種類（S1）。
const PROCESS_WORD_GROUPS: &[&[&str]] = &[
    &["調査", "investigate", "survey"],
    &["設計", "design"],
    &["実装", "implement"],
    &["テスト", "test"],
    &["review", "レビュー"],
    &["release", "リリース", "deploy", "配送"],
];

fn count_process_stages(text: &str) -> usize {
    let lower = text.to_lowercase();
    PROCESS_WORD_GROUPS
        .iter()
        .filter(|group| group.iter().any(|w| lower.contains(&w.to_lowercase())))
        .count()
}

struct Scorer {
    score: i32,
    signals: Vec<GateSignal>,
}

impl Scorer {
    fn new() -> Self {
        Scorer {
            score: 0,
            signals: Vec::new(),
        }
    }

    fn add(&mut self, name: &str, weight: i32, detail: String) {
        if weight != 0 {
            self.score += weight;
            self.signals.push(GateSignal {
                name: name.to_string(),
                weight,
                detail,
            });
        }
    }
}

/// D13: Complexity Gate。`out_of_scope_rule` に当たれば常に atomic。次に人の明示
/// （`human_execution`）があればそれに従う（規則表を評価しない）。それ以外は規則表のスコアで決める。
/// `shadow` はそのまま `ExecutionGateDecision.shadow` に写す（判定のロジックそのものは変えない。
/// `gate = "shadow"` でも同じ判定をし、採用するかどうかは呼び出し側が決める）。
pub fn decide(
    task: &Task,
    features: &TaskFeatures,
    human_execution: Option<ExecutionMode>,
    cos_hint_compound: bool,
    inputs: ExecutionGateInputs,
    shadow: bool,
) -> ExecutionGateDecision {
    decide_at(
        task,
        features,
        human_execution,
        cos_hint_compound,
        inputs,
        shadow,
        GateThreshold::ROOT,
    )
}

/// ADR-0079 D3 / D4 (1)（Phase R2a）: [`decide`] の閾値を深さで上げた版（規則表・強制規則・対象外規則は
/// 同じ。`score >= at.threshold` なら compound）。`at.depth` は `ExecutionGateDecision.depth` に写す。
pub fn decide_at(
    task: &Task,
    features: &TaskFeatures,
    human_execution: Option<ExecutionMode>,
    cos_hint_compound: bool,
    inputs: ExecutionGateInputs,
    shadow: bool,
    at: GateThreshold,
) -> ExecutionGateDecision {
    let threshold = at.threshold;
    let depth = at.depth;
    if let Some(rule_id) = out_of_scope_rule(task) {
        return ExecutionGateDecision {
            mode: ExecutionMode::Atomic,
            source: GateSource::Policy,
            score: 0,
            threshold,
            rule_id: rule_id.to_string(),
            signals: Vec::new(),
            policy_version: EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow,
            depth,
        };
    }
    if let Some(mode) = human_execution {
        return ExecutionGateDecision {
            mode,
            source: GateSource::Human,
            score: 0,
            threshold,
            rule_id: "human/explicit".to_string(),
            signals: Vec::new(),
            policy_version: EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow,
            depth,
        };
    }

    let mut s = Scorer::new();
    match features.context_size {
        Level::High => s.add("F1", 2, "context_size=high".to_string()),
        Level::Medium => s.add("F1", 1, "context_size=medium".to_string()),
        Level::Low => {}
    }
    if features.expected_length == Level::High {
        s.add("F2", 2, "expected_length=high".to_string());
    }
    if features.tool_intensity == Level::High {
        s.add("F3", 1, "tool_intensity=high".to_string());
    }
    match features.cross_cutting {
        Level::High => s.add("F4", 2, "cross_cutting=high".to_string()),
        Level::Medium => s.add("F4", 1, "cross_cutting=medium".to_string()),
        Level::Low => {}
    }
    if features.judgment == Level::High && features.tool_intensity >= Level::Medium {
        s.add(
            "F5",
            1,
            "judgment=high and tool_intensity>=medium".to_string(),
        );
    }
    let text = format!("{}\n{}", task.title, task.objective);
    let stages = count_process_stages(&text);
    match stages {
        n if n >= 3 => s.add("S1", 2, format!("{n} process stages mentioned")),
        2 => s.add("S1", 1, "2 process stages mentioned".to_string()),
        _ => {}
    }
    let artifact_exists = task
        .acceptance
        .iter()
        .filter(|c| matches!(c.check, Check::ArtifactExists { .. }))
        .count();
    if task.acceptance.len() >= 6 || artifact_exists >= 3 {
        s.add(
            "S2",
            1,
            format!(
                "acceptance={}, artifact_exists={artifact_exists}",
                task.acceptance.len()
            ),
        );
    }
    let has_human = task.acceptance.iter().any(|c| c.check == Check::Human);
    let has_command = task
        .acceptance
        .iter()
        .any(|c| matches!(c.check, Check::Command { .. }));
    if has_human && has_command {
        s.add(
            "S3",
            1,
            "has both Check::Human and Check::Command".to_string(),
        );
    }
    if inputs.multi_environment {
        s.add("S4", 1, "multiple execution environments".to_string());
    }
    let objective_len = task.objective.chars().count();
    if objective_len > 2000 {
        s.add("S5", 1, format!("objective length {objective_len} > 2000"));
    }
    if let Some(ratio) = inputs.recent_budget_exhausted_ratio
        && ratio >= 0.3
    {
        s.add(
            "S6",
            2,
            format!("recent budget_exhausted ratio {ratio:.2} >= 0.3"),
        );
    }
    if cos_hint_compound {
        s.add("H", 2, "CoS hinted execution=compound".to_string());
    }

    let source = if cos_hint_compound {
        GateSource::Hint
    } else {
        GateSource::Policy
    };

    // 強制規則（D13）: `expected_length=high` かつ `cross_cutting=high` なら compound。
    if features.expected_length == Level::High && features.cross_cutting == Level::High {
        return ExecutionGateDecision {
            mode: ExecutionMode::Compound,
            source,
            score: s.score,
            threshold,
            rule_id: "compound/long-and-broad".to_string(),
            signals: s.signals,
            policy_version: EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow,
            depth,
        };
    }
    // 強制規則: `max_turns <= 10` かつ目的が 400 文字未満なら atomic。
    // ADR-0079「R5b-fix3」: 木の子 task（`tree.parent_unit` を持つ）には当てない。親の計画の kind task の
    // unit は作られた時点で「小さくない」（目的は unit の 1 行と木の中の位置なので短い）。
    if task.budget.max_turns <= 10 && objective_len < 400 && !crate::tree::is_tree_child(task) {
        return ExecutionGateDecision {
            mode: ExecutionMode::Atomic,
            source,
            score: s.score,
            threshold,
            rule_id: "atomic/small".to_string(),
            signals: s.signals,
            policy_version: EXECUTION_GATE_POLICY_VERSION.to_string(),
            shadow,
            depth,
        };
    }

    let (mode, rule_id) = if s.score >= threshold {
        (ExecutionMode::Compound, "compound/score")
    } else {
        (ExecutionMode::Atomic, "atomic/score")
    };
    ExecutionGateDecision {
        mode,
        source,
        score: s.score,
        threshold,
        rule_id: rule_id.to_string(),
        signals: s.signals,
        policy_version: EXECUTION_GATE_POLICY_VERSION.to_string(),
        shadow,
        depth,
    }
}

/// `[execution] gate`。既定は `shadow`（ADR-0072 D13）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GateMode {
    Off,
    #[default]
    Shadow,
    On,
}

impl GateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            GateMode::Off => "off",
            GateMode::Shadow => "shadow",
            GateMode::On => "on",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(GateMode::Off),
            "shadow" => Some(GateMode::Shadow),
            "on" => Some(GateMode::On),
            _ => None,
        }
    }
}

/// `[execution.planner]`（D14, ADR-0074 D5.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerConfig {
    pub adapter: String,
    pub permission_mode: String,
    /// ADR-0074 D5.3（Phase F1）: planner run の lane（既定 `standard`。E3〜E6 は固定 `frontier` だった）。
    pub tier: crate::model::Tier,
    pub max_turns: u32,
    pub max_wall_secs: u64,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        PlannerConfig {
            adapter: "claude-code".to_string(),
            permission_mode: "plan".to_string(),
            tier: crate::model::Tier::Standard,
            // ADR-0074 D5.3（Phase F1）: 40 -> 24 turns, 1,200 -> 900 秒。
            max_turns: 24,
            max_wall_secs: 900,
        }
    }
}

#[cfg(test)]
#[path = "execution_gate/tests.rs"]
mod tests;
