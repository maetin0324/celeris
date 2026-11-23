//! ADR 2026-11-22-parent-checkpoint-evaluator D1.1: 戦略の文書の参照の型（crates で 1 つ）。
//!
//! CoS の `/cos/operations` の欄 `strategy`（子 5 の gate）、`cos_operations.payload_json.strategy` の照会
//! （`strategy_reads::strategy_task_creates`）、親の途中評価の入力 `ParentCheckpointRequest.strategy` が同じ型を使う。
//! この module は task-core の他の module を use しない（serde・schemars だけ。D11 の deps.cycle）。
//! 文書の成果物の name の形（`artifact_name`）とその逆（`strategy_artifact`）もここに置く。`crate::chat` が読む
//! `crate::proposal_review_wire`（ADR 2026-11-23-project-observability-publish D2.4）が CoS の案の添付を見分けるのに使い、
//! `strategy`・`strategy_critic` を経て task-core の循環に入らないため（`strategy`・`strategy_critic` は再 export する）。

use serde::{Deserialize, Serialize};

/// `/cos/operations` の欄 `strategy`: 実行する戦略の文書（作った task・成果物の name・記録の sha256）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrategyRef {
    pub task_id: String,
    pub name: String,
    pub sha256: String,
    /// ADR 2026-11-22-strategy-review-critic D2.2: どの候補の task か（`[a-z0-9-]{1,40}`）。実験段階の版では
    /// open の候補、実行段階の版では省略か `selected` の候補。`cos_operations.payload_json.strategy.candidate` が記録になる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

/// 成果物の name（ADR 2026-11-21-strategy-evidence-contract D3.1）。
pub fn artifact_name(strategy_id: &str, version: u32) -> String {
    format!("strategy/{strategy_id}/v{version}.json")
}

/// name が戦略の成果物（`artifact_name(id, v)` の形）なら `(id, v)`。
pub fn strategy_artifact(name: &str) -> Option<(String, u32)> {
    let (id, version) = name.strip_prefix("strategy/")?.rsplit_once("/v")?;
    let version: u32 = version.strip_suffix(".json")?.parse().ok()?;
    if id.is_empty() || id.contains('/') || artifact_name(id, version) != name {
        return None;
    }
    Some((id.to_string(), version))
}
