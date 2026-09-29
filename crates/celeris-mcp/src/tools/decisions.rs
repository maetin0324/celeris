//! ADR-0079 D7（Phase R3a）: `decision_list` / `decision_answer`（scope `tasks:interact`。`task_answer` と同じ重さ）。
//!
//! HTTP の `GET /decisions` / `POST /decisions/{id}/answer` と**同じ** `task_ops::decision` の関数を呼ぶ（ロジックの
//! 二重実装をしない）。回答の主体（`DecisionAnswered.by`）は `mcp:<client_id>`。取り下げ・revise は MCP には
//! 出さない（人が GUI / API で行う。取り下げは節点の中止を伴いうるので `tasks:control` 相当の重さ）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use task_core::{McpScope, TaskId};
use time::OffsetDateTime;

use super::tasks::map_ops_err;
use super::{ToolDef, ToolError, ToolOutput, schema};
use crate::auth::AuthedClient;
use crate::state::McpState;

// ---- decision_list ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    /// `true`（既定）= 未回答だけ、`false` = 回答済み・取り下げ済みだけ。
    #[serde(default)]
    pub open: Option<bool>,
    /// 1 つの木（root task の id）に絞る。
    #[serde(default)]
    pub root_id: Option<String>,
    /// その task の subtree（その task か子孫が出した決定）に絞る。`root_id` と同時には使えない。
    #[serde(default)]
    pub task_id: Option<String>,
}

fn parse_task_id(raw: &str, field: &str) -> Result<TaskId, ToolError> {
    raw.parse::<TaskId>()
        .map_err(|_| ToolError::invalid_params(format!("{field}: {raw:?} is not a task id")))
}

async fn list_impl(
    state: &Arc<McpState>,
    _client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: ListArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    if args.root_id.is_some() && args.task_id.is_some() {
        return Err(ToolError::invalid_params(
            "root_id and task_id cannot be used together",
        ));
    }
    let open = Some(args.open.unwrap_or(true));
    let root_id = args
        .root_id
        .as_deref()
        .map(|r| parse_task_id(r, "root_id"))
        .transpose()?;
    let task_id = args
        .task_id
        .as_deref()
        .map(|r| parse_task_id(r, "task_id"))
        .transpose()?;
    let list = state
        .blocking(move |store| match task_id {
            Some(id) => task_ops::decision::for_subtree(store, id, open).map_err(map_ops_err),
            None => task_ops::decision::list(
                store,
                &task_ops::decision::DecisionFilter { open, root_id },
            )
            .map_err(map_ops_err),
        })
        .await?;
    ToolOutput::from_serialize(&list)
}

fn list_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(list_impl(state, client, args))
}

pub fn list_def() -> ToolDef {
    ToolDef {
        name: "decision_list",
        description: "List the decision requests of the recursive task tree (same as GET /decisions): by default the open ones, each with its id, path (root › stage › unit breadcrumb), question, options, recommended option, cost of reversal and what it blocks (needed_before). Filter with root_id (one tree) or task_id (a task's subtree); open=false lists answered/withdrawn ones.",
        scope: McpScope::TasksInteract,
        input_schema: schema::<ListArgs>,
        call: list_call,
    }
}

// ---- decision_answer ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnswerArgs {
    /// 決定の id（`decision_list` の `decision.id`）。
    pub id: String,
    /// 選択肢の key。`kind = choice` の決定だけ省いて `note` に自由記述で答えられる。
    #[serde(default)]
    pub option: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

async fn answer_impl(
    state: &Arc<McpState>,
    client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: AnswerArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    let by = format!("mcp:{}", client.id);
    let outcome = state
        .blocking(move |store| {
            task_ops::decision::answer(
                store,
                &args.id,
                args.option.as_deref(),
                args.note.as_deref(),
                &by,
                OffsetDateTime::now_utc(),
            )
            .map_err(map_ops_err)
        })
        .await?;
    ToolOutput::from_serialize(&outcome)
}

fn answer_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(answer_impl(state, client, args))
}

pub fn answer_def() -> ToolDef {
    ToolDef {
        name: "decision_answer",
        description: "Answer an open decision request (same as POST /decisions/{id}/answer): option must be one of the decision's option keys (a kind=choice decision also accepts a free-text answer in note with option omitted). Records decision_answered with by=mcp:<client_id> and applies the deterministic effect (resume the waiting units, raise-once / replan / withdraw for limits, replan / atomic / cancel for plan_invalid). Already answered or withdrawn decisions are refused.",
        scope: McpScope::TasksInteract,
        input_schema: schema::<AnswerArgs>,
        call: answer_call,
    }
}

// ---- task_plan_gate（ADR-0079 D8、Phase R3b）----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanGateArgs {
    /// root の計画の承認を待っている task の id。
    pub task_id: String,
    /// `approve` | `replan`（note 必須）| `withdraw`。
    #[serde(alias = "decision")]
    pub action: task_ops::plan_gate::PlanGateAction,
    #[serde(default)]
    pub note: Option<String>,
}

async fn plan_gate_impl(
    state: &Arc<McpState>,
    client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: PlanGateArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    let task_id = parse_task_id(&args.task_id, "task_id")?;
    let by = format!("mcp:{}", client.id);
    let result = state
        .blocking(move |store| {
            task_ops::plan_gate::plan_gate(store, task_id, args.action, args.note, &by)
                .map_err(map_ops_err)
        })
        .await?;
    ToolOutput::from_serialize(&result)
}

fn plan_gate_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(plan_gate_impl(state, client, args))
}

pub fn plan_gate_def() -> ToolDef {
    ToolDef {
        name: "task_plan_gate",
        description: "Respond to a root plan that is waiting for human approval (same as POST /tasks/{id}/execution/plan-gate; the task's execution phase is awaiting_plan_approval): action approve (run the plan as adopted), replan (note required: the instruction for the planner, which writes a new plan version) or withdraw (cancel the task and its subtree). Recorded with by=mcp:<client_id>. Tasks not awaiting a plan approval are refused. Answering the plan's decisions is separate (decision_answer).",
        scope: McpScope::TasksInteract,
        input_schema: schema::<PlanGateArgs>,
        call: plan_gate_call,
    }
}
