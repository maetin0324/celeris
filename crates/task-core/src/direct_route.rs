//! ADR-0124: atomic coding task の直行経路を選ぶ純粋な規則。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::execution_gate::{ExecutionGateDecision, ExecutionMode, GateSource, out_of_scope_rule};
use crate::model::{Check, Task};

pub const DIRECT_ROUTE_POLICY_VERSION: &str = "direct-route/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Direct,
    Planned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteReason {
    pub rule_id: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteDecision {
    pub route: Route,
    pub reasons: Vec<RouteReason>,
    pub gate_rule_id: String,
    pub overrode_gate: bool,
    pub shadow: bool,
    pub policy_version: String,
}

/// 呼び出し側が store と担当 profile から決定的に計算する信号。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectRouteInputs {
    pub coding_harness: bool,
    pub cross_department: bool,
    pub pending_approval: bool,
}

/// 全条件を記録し、すべて満たすときだけ直行させる。
/// 計画を既に持つ Task には呼び出し側で適用しない。
pub fn evaluate(
    task: &Task,
    gate: &ExecutionGateDecision,
    inputs: DirectRouteInputs,
) -> RouteDecision {
    let mut reasons = Vec::with_capacity(8);
    let mut add = |rule: &str, ok: bool, detail: String| {
        reasons.push(RouteReason {
            rule_id: rule.to_string(),
            ok,
            detail,
        });
    };

    let out_of_scope = out_of_scope_rule(task);
    add(
        "direct/in-scope",
        out_of_scope.is_none(),
        out_of_scope.unwrap_or("in scope").to_string(),
    );
    add(
        "direct/coding",
        inputs.coding_harness,
        format!("coding_harness={}", inputs.coding_harness),
    );
    // The gate's S4 signal records a mixed execution environment. RepoRef itself
    // carries only an ID and a name, so environment cannot be recovered from it.
    let mixed_environment = gate.signals.iter().any(|signal| signal.name == "S4");
    add(
        "direct/single-repo",
        task.repos.len() == 1 && !mixed_environment,
        format!(
            "repos={}, multi_environment={mixed_environment}",
            task.repos.len()
        ),
    );
    add(
        "direct/single-department",
        task.assignee.is_some() && !inputs.cross_department,
        format!(
            "assignee={}, cross_department={}",
            task.assignee.as_deref().unwrap_or("none"),
            inputs.cross_department
        ),
    );
    let human_check = task
        .acceptance
        .iter()
        .any(|criterion| matches!(criterion.check, Check::Human));
    add(
        "direct/no-human-approval",
        !human_check && !inputs.pending_approval,
        format!(
            "human_check={human_check}, pending_approval={}",
            inputs.pending_approval
        ),
    );
    let command_checks = task
        .acceptance
        .iter()
        .filter(|criterion| matches!(criterion.check, Check::Command { .. }))
        .count();
    add(
        "direct/command-check",
        command_checks > 0,
        format!("command_checks={command_checks}"),
    );
    add(
        "direct/no-plan",
        !task.aggregate,
        format!("aggregate={}", task.aggregate),
    );

    let override_score = gate.mode == ExecutionMode::Compound
        && gate.source == GateSource::Policy
        && gate.rule_id == "compound/score"
        && gate.depth.is_none()
        && !crate::tree::is_tree_child(task);
    let gate_ok = gate.mode == ExecutionMode::Atomic || override_score;
    add(
        "direct/gate",
        gate_ok,
        format!(
            "mode={}, source={:?}, rule_id={}, depth={:?}",
            gate.mode.as_str(),
            gate.source,
            gate.rule_id,
            gate.depth
        ),
    );

    let direct = reasons.iter().all(|reason| reason.ok);
    RouteDecision {
        route: if direct {
            Route::Direct
        } else {
            Route::Planned
        },
        reasons,
        gate_rule_id: gate.rule_id.clone(),
        overrode_gate: direct && override_score,
        shadow: gate.shadow,
        policy_version: DIRECT_ROUTE_POLICY_VERSION.to_string(),
    }
}

#[cfg(test)]
#[path = "direct_route/tests.rs"]
mod tests;
