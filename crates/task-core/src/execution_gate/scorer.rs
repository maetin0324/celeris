//! ADR 2026-11-04-ts-execution-gate-domain: Complexity Gate の規則表（Rust の参照実装）と TS 制御層の領域 `execution_gate` の DTO。
//!
//! [`score`] は入力 DTO だけで決まる純粋関数で、`execution_gate::decide_at` が対象外の規則と人の明示の後に呼ぶ。
//! TS（`control-ts/src/domains/execution_gate.ts`）は同じ DTO を受け、同じ出力を返す。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ExecutionGateInputs, ExecutionMode, GateSignal};
use crate::model::{Check, Task};
use crate::model_policy::{Level, TaskFeatures};

/// TS が名乗ってよい `rule_id`（ADR D8。`human/explicit`・対象外の rule_id は Rust だけが書く）。
pub const EXECUTION_GATE_TS_RULES: [&str; 4] = [
    "compound/long-and-broad",
    "atomic/small",
    "compound/score",
    "atomic/score",
];

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

/// 領域 `execution_gate` の入力 DTO（ADR D5）。TS に渡す値で、記録には [`ExecutionGateRecordedInput`] を使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGateInput {
    pub features: TaskFeatures,
    pub title: String,
    pub objective: String,
    pub acceptance_total: u32,
    pub artifact_exists: u32,
    pub has_human: bool,
    pub has_command: bool,
    pub multi_environment: bool,
    pub recent_budget_exhausted_ratio: Option<f64>,
    pub cos_hint_compound: bool,
    pub max_turns: u32,
    pub tree_child: bool,
    pub threshold: i32,
}

/// 記録する入力（ADR D6）。`title`・`objective` の本文を持たない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGateRecordedInput {
    pub features: TaskFeatures,
    pub text_sha256: String,
    pub title_chars: u64,
    pub objective_chars: u64,
    pub acceptance_total: u32,
    pub artifact_exists: u32,
    pub has_human: bool,
    pub has_command: bool,
    pub multi_environment: bool,
    pub recent_budget_exhausted_ratio: Option<f64>,
    pub cos_hint_compound: bool,
    pub max_turns: u32,
    pub tree_child: bool,
    pub threshold: i32,
}

impl ExecutionGateInput {
    /// `task` から入力を作る（`decide_at` と dispatcher の比較が同じ値を使う）。
    pub fn from_task(
        task: &Task,
        features: &TaskFeatures,
        cos_hint_compound: bool,
        inputs: ExecutionGateInputs,
        threshold: i32,
    ) -> Self {
        let count = |pred: fn(&Check) -> bool| {
            let n = task.acceptance.iter().filter(|c| pred(&c.check)).count();
            u32::try_from(n).unwrap_or(u32::MAX)
        };
        Self {
            features: *features,
            title: task.title.clone(),
            objective: task.objective.clone(),
            acceptance_total: u32::try_from(task.acceptance.len()).unwrap_or(u32::MAX),
            artifact_exists: count(|c| matches!(c, Check::ArtifactExists { .. })),
            has_human: task.acceptance.iter().any(|c| c.check == Check::Human),
            has_command: task
                .acceptance
                .iter()
                .any(|c| matches!(c.check, Check::Command { .. })),
            multi_environment: inputs.multi_environment,
            recent_budget_exhausted_ratio: inputs.recent_budget_exhausted_ratio,
            cos_hint_compound,
            max_turns: task.budget.max_turns,
            tree_child: crate::tree::is_tree_child(task),
            threshold,
        }
    }

    /// 本文を除いた記録の値（ADR D6）。
    pub fn recorded(&self) -> ExecutionGateRecordedInput {
        let text = format!("{}\n{}", self.title, self.objective);
        ExecutionGateRecordedInput {
            features: self.features,
            text_sha256: crate::script_compare::sha256_digest(text.as_bytes()),
            title_chars: self.title.chars().count() as u64,
            objective_chars: self.objective.chars().count() as u64,
            acceptance_total: self.acceptance_total,
            artifact_exists: self.artifact_exists,
            has_human: self.has_human,
            has_command: self.has_command,
            multi_environment: self.multi_environment,
            recent_budget_exhausted_ratio: self.recent_budget_exhausted_ratio,
            cos_hint_compound: self.cos_hint_compound,
            max_turns: self.max_turns,
            tree_child: self.tree_child,
            threshold: self.threshold,
        }
    }
}

/// 領域 `execution_gate` の出力の signal 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGateSignalOutput {
    pub name: String,
    pub weight: i32,
    pub detail: String,
}

/// 領域 `execution_gate` の出力 DTO（ADR D7）。欄の順は宣言順。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionGateOutput {
    pub mode: ExecutionMode,
    pub score: i32,
    pub rule_id: String,
    pub signals: Vec<ExecutionGateSignalOutput>,
}

impl ExecutionGateOutput {
    /// `ExecutionGateDecision` の `signals` の型へ移す。
    pub fn gate_signals(&self) -> Vec<GateSignal> {
        self.signals
            .iter()
            .map(|s| GateSignal {
                name: s.name.clone(),
                weight: s.weight,
                detail: s.detail.clone(),
            })
            .collect()
    }
}

struct Scorer {
    score: i32,
    signals: Vec<ExecutionGateSignalOutput>,
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
            self.signals.push(ExecutionGateSignalOutput {
                name: name.to_string(),
                weight,
                detail,
            });
        }
    }
}

/// D13 の規則表と強制規則。`score >= input.threshold` なら compound。
pub fn score(input: &ExecutionGateInput) -> ExecutionGateOutput {
    let features = &input.features;
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
    let text = format!("{}\n{}", input.title, input.objective);
    let stages = count_process_stages(&text);
    match stages {
        n if n >= 3 => s.add("S1", 2, format!("{n} process stages mentioned")),
        2 => s.add("S1", 1, "2 process stages mentioned".to_string()),
        _ => {}
    }
    if input.acceptance_total >= 6 || input.artifact_exists >= 3 {
        s.add(
            "S2",
            1,
            format!(
                "acceptance={}, artifact_exists={}",
                input.acceptance_total, input.artifact_exists
            ),
        );
    }
    if input.has_human && input.has_command {
        s.add(
            "S3",
            1,
            "has both Check::Human and Check::Command".to_string(),
        );
    }
    if input.multi_environment {
        s.add("S4", 1, "multiple execution environments".to_string());
    }
    let objective_len = input.objective.chars().count();
    if objective_len > 2000 {
        s.add("S5", 1, format!("objective length {objective_len} > 2000"));
    }
    if let Some(ratio) = input.recent_budget_exhausted_ratio
        && ratio >= 0.3
    {
        s.add(
            "S6",
            2,
            format!("recent budget_exhausted ratio {ratio:.2} >= 0.3"),
        );
    }
    if input.cos_hint_compound {
        s.add("H", 2, "CoS hinted execution=compound".to_string());
    }

    // 強制規則（D13）: `expected_length=high` かつ `cross_cutting=high` なら compound。
    let (mode, rule_id) =
        if features.expected_length == Level::High && features.cross_cutting == Level::High {
            (ExecutionMode::Compound, "compound/long-and-broad")
        // 強制規則: `max_turns <= 10` かつ目的が 400 文字未満なら atomic。
        // ADR-0079「R5b-fix3」: 木の子 task（`tree.parent_unit` を持つ）には当てない。親の計画の kind task の
        // unit は作られた時点で「小さくない」（目的は unit の 1 行と木の中の位置なので短い）。
        } else if input.max_turns <= 10 && objective_len < 400 && !input.tree_child {
            (ExecutionMode::Atomic, "atomic/small")
        } else if s.score >= input.threshold {
            (ExecutionMode::Compound, "compound/score")
        } else {
            (ExecutionMode::Atomic, "atomic/score")
        };
    ExecutionGateOutput {
        mode,
        score: s.score,
        rule_id: rule_id.to_string(),
        signals: s.signals,
    }
}
