//! ADR-0072「Phase F6 実装時の決定」: 起票済みの Task を、後から人が分解の経路（ExecutionPlan）に
//! 入れる / atomic に戻す（`POST /tasks/{id}/execution/decompose`、MCP `task_decompose`、
//! `POST /tasks/{id}/retry` の `execution`）。
//!
//! - 書くのは `Task.routing.execution_hint = {mode, explicit: true}`（人の明示）と、前の gate の判定
//!   （`Task.routing.execution`）の消去だけ。`Event::ExecutionHintSet` を同じトランザクションに積む。
//!   gate の判定そのものは**書かない**: 次の dispatch で daemon の `execution_gate_if_needed` が
//!   `human/explicit` として判定し直し、新しい `Event::ExecutionGated` を残す（新しく作った「人が
//!   `execution: compound` を明示した Task」と同じ経路。`gate = "shadow"` でも人の明示の compound は
//!   planner run に進む。ADR-0074「Phase F3（途中確認）」）。
//! - 受け付ける状態は `draft` / `ready` / `blocked`（`blocked` は回答などで `ready` に戻った次の
//!   dispatch から効く）。`running` / `reviewing` は 409（走っている run を止めない。止めたいなら
//!   中止してから `retry` の `execution`）。終端（`done` / `failed` / `cancelled`）も 409（`retry` の
//!   `execution` を使う）。
//! - 計画を既に持つ Task への `compound` は replan の依頼（`replan = true`。daemon は次の dispatch で
//!   `max_replans` の範囲で replan の planner run を起こす）。計画を持つ Task を `atomic` に戻すことは
//!   しない（採用済みの WU を宙に浮かせない。409）。
//! - gate の対象外（`kind != execute`・対話・support-task・`routing` の無い旧タスク・固定パイプラインの
//!   harness・`workspace_mode = shared`）は 422（gate が常に atomic にするので、書いても効かない）。
//!
//! I/O はストアの読み書きだけで、LLM は呼ばない（DESIGN 原則 1）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{Event, ExecutionHintSpec, ExecutionMode, Status, Task, TaskId, TaskStore};
use time::OffsetDateTime;

use crate::error::OpsError;

/// `note` の上限（文字数）。replan の planner run の「起こした理由」にそのまま渡る。
pub const NOTE_MAX_CHARS: usize = 2000;

/// `POST /tasks/{id}/execution/decompose` の要求本文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecomposeRequest {
    /// `"compound"`（計画を作らせる）か `"atomic"`（1 つの run で直接実行する）。
    pub mode: ExecutionMode,
    /// 人の一言（任意、2,000 文字まで）。replan の依頼では planner run の「起こした理由」に渡る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /tasks/{id}/execution/decompose` の応答（200）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DecomposeResult {
    pub task: Task,
    pub mode: ExecutionMode,
    /// `true` なら計画を既に持つ Task への replan の依頼（次の dispatch で replan の planner run）。
    pub replan: bool,
    /// 消した前の gate の判定（無ければ `null`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_decision: Option<task_core::ExecutionGateDecision>,
}

/// gate の対象外なら理由を返す（`execution_gate::out_of_scope_rule` と同じ 7 条件。ADR-0131 付記
/// 2026-10-04 で knowledge-curation の cron task を追加）。
fn out_of_scope_reason(task: &Task) -> Option<String> {
    task_core::execution_gate::out_of_scope_rule(task).map(|_| {
        "this task is out of scope of the Complexity Gate (not kind=execute, a conversation, a \
         support task, a task without routing, a fixed-pipeline harness, a shared workspace, or a \
         knowledge-curation cron task); it always runs atomic"
            .to_string()
    })
}

/// 人（`source = "human"`）または MCP クライアント（`"mcp:<id>"`）が実行の形を決める。
pub fn set_execution_mode(
    store: &dyn TaskStore,
    id: TaskId,
    mode: ExecutionMode,
    source: &str,
    note: Option<String>,
    now: OffsetDateTime,
) -> Result<DecomposeResult, OpsError> {
    let (mut result, event) = plan_execution_mode(store, id, mode, source, note, now)?;
    result.task = store.update_task(&result.task, event)?;
    Ok(result)
}

/// Validate and construct the execution-mode update without writing. API operations use this
/// plan so the task update, event, and CoS audit record can share one SQLite transaction.
pub fn plan_execution_mode(
    store: &dyn TaskStore,
    id: TaskId,
    mode: ExecutionMode,
    source: &str,
    note: Option<String>,
    now: OffsetDateTime,
) -> Result<(DecomposeResult, Event), OpsError> {
    let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    if let Some(n) = &note
        && n.chars().count() > NOTE_MAX_CHARS
    {
        return Err(OpsError::Validation(format!(
            "note must be at most {NOTE_MAX_CHARS} characters"
        )));
    }
    let mut task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if let Some(reason) = out_of_scope_reason(&task) {
        return Err(OpsError::Validation(reason));
    }
    match task.status {
        Status::Draft | Status::Ready | Status::Blocked => {}
        Status::Running | Status::Reviewing => {
            return Err(OpsError::InvalidState {
                id,
                context: format!("status={:?}", task.status),
                action: "re-gated while a run is in flight; wait for it to finish, or cancel \
                         the task and retry it with `execution`"
                    .to_string(),
            });
        }
        Status::Done | Status::Failed | Status::Cancelled => {
            return Err(OpsError::InvalidState {
                id,
                context: format!("status={:?}", task.status),
                action: "re-gated; it is terminal (use POST /tasks/{id}/retry with `execution` \
                         to run a copy through the chosen path)"
                    .to_string(),
            });
        }
    }
    let has_plan = store.execution_plan_active(id)?.is_some();
    if has_plan && mode == ExecutionMode::Atomic {
        return Err(OpsError::InvalidState {
            id,
            context: "it has an adopted execution plan".to_string(),
            action: "switched back to atomic (cancel it and retry with execution=atomic)"
                .to_string(),
        });
    }
    let mut routing = task.routing.clone().unwrap_or_default();
    let previous = routing.execution_hint;
    // 計画を持つ Task（replan の依頼）は gate の判定を消さない（計画の由来の監査のため）。
    let previous_decision = if has_plan {
        None
    } else {
        // ADR-0124 D2: 経路の判定（routing.route）は gate の判定と同じ寿命。人が経路を明示
        // したら消し、次の dispatch で gate → evaluate の順に記録し直す。
        routing.route = None;
        routing.execution.take()
    };
    routing.execution_hint = Some(ExecutionHintSpec {
        mode,
        explicit: true,
    });
    task.routing = Some(routing);
    task.updated_at = now;
    let event = Event::ExecutionHintSet {
        mode,
        previous,
        previous_decision: previous_decision.clone().map(Box::new),
        source: source.to_string(),
        note,
        replan: has_plan,
    };
    Ok((
        DecomposeResult {
            task,
            mode,
            replan: has_plan,
            previous_decision,
        },
        event,
    ))
}

/// ADR-0079「R5b-fix3」(D3 (d)): 人が計画を採用した（`PUT/POST /tasks/{id}/execution-plan`、`celerisctl
/// execution plan set`）Task の gate の記録を「人の compound」にする。人が計画を書いた = 分けて進めると
/// 決めたので、前の `atomic/small` などの判定を `routing.execution` に残さない（GUI と監査が誤解する）。
///
/// - 書くのは `routing.execution_hint = {compound, explicit: true}` と `routing.execution` = `{mode: compound,
///   source: human, rule_id: "human/plan"}`（閾値・深さは前の判定、無ければ root の値）。`Event::ExecutionGated`
///   を同じトランザクションに積む（`execution_gate_if_needed` の記録と同じ形。`task_ops::regate` の再 gate と
///   `decide_at` の `human/explicit` と整合する: 次に gate をかけ直しても人の明示の compound になる）。
/// - 既に人の compound なら何もしない（`Ok(None)`）。gate の対象外（`routing` の無い旧タスクなど）も何もしない。
/// - 採用そのもの（`task_ops::execution::adopt_human_plan`）とは別のトランザクション。書けなくても採用は
///   失敗にしない（呼び出し側が警告を出す）。
pub fn record_human_plan_gate(
    store: &dyn TaskStore,
    id: TaskId,
    now: OffsetDateTime,
) -> Result<Option<Task>, OpsError> {
    let mut task = store.get(id)?.ok_or(OpsError::NotFound(id))?;
    if task_core::execution_gate::out_of_scope_rule(&task).is_some() {
        return Ok(None);
    }
    let mut routing = task.routing.clone().unwrap_or_default();
    if routing.execution.as_ref().is_some_and(|d| {
        d.mode == ExecutionMode::Compound && d.source == task_core::GateSource::Human
    }) {
        return Ok(None);
    }
    let (threshold, depth) = routing
        .execution
        .as_ref()
        .map(|d| (d.threshold, d.depth))
        .unwrap_or((task_core::EXECUTION_GATE_SCORE_THRESHOLD, None));
    let decision = task_core::ExecutionGateDecision {
        mode: ExecutionMode::Compound,
        source: task_core::GateSource::Human,
        score: 0,
        threshold,
        rule_id: HUMAN_PLAN_RULE_ID.to_string(),
        signals: Vec::new(),
        policy_version: task_core::EXECUTION_GATE_POLICY_VERSION.to_string(),
        shadow: false,
        depth,
    };
    routing.execution_hint = Some(ExecutionHintSpec {
        mode: ExecutionMode::Compound,
        explicit: true,
    });
    routing.execution = Some(decision.clone());
    task.routing = Some(routing);
    task.updated_at = now;
    let task = store.update_task(
        &task,
        Event::ExecutionGated {
            decision: Box::new(decision),
        },
    )?;
    Ok(Some(task))
}

/// R5b-fix3: 人が計画を採用した Task の gate の `rule_id`。
pub const HUMAN_PLAN_RULE_ID: &str = "human/plan";

/// ADR-0072「Phase F6 実装時の決定」: 計画を持つ Task に、まだ消費されていない人の replan の依頼
/// （`ExecutionHintSet{replan: true}`）があれば、その `note`（無ければ空文字列）を返す。
/// 依頼の後に計画が採用された（`ExecutionPlanned`）か、run が始まった（`Transitioned{to: running}`）
/// なら消費済み（`None`）。daemon の `wu_dispatch_gate` が決定的に読む。
pub fn pending_replan_request(events: &[(u64, Event)]) -> Option<String> {
    for (_, event) in events.iter().rev() {
        match event {
            Event::ExecutionHintSet {
                replan: true, note, ..
            } => return Some(note.clone().unwrap_or_default()),
            Event::ExecutionPlanned { .. } => return None,
            Event::Transitioned {
                to: Status::Running,
                ..
            } => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests;
