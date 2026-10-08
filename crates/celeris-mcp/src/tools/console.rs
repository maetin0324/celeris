//! ADR-0056 D2 / CoS chat D6: MCP 入力を通常の CoS chat thread に渡す。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{ProjectId, TaskId};

use super::{ToolDef, ToolError, ToolOutput, schema};
use crate::auth::AuthedClient;
use crate::state::McpState;

fn chat_error(error: task_core::chat::ChatError) -> ToolError {
    if matches!(error, task_core::chat::ChatError::Store(_)) {
        ToolError::internal(error.to_string())
    } else {
        ToolError::invalid_params(error.to_string())
    }
}

// ---- console_instruct ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstructArgs {
    pub text: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InstructOutput {
    pub message_id: String,
    pub task_id: String,
    pub thread_id: String,
}

async fn instruct_impl(
    state: &Arc<McpState>,
    client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: InstructArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    if args.text.trim().is_empty() {
        return Err(ToolError::invalid_params("text must not be blank"));
    }
    let project_id = match args.project_id.as_deref() {
        Some(s) => Some(
            s.parse::<ProjectId>()
                .map_err(|_| ToolError::invalid_params(format!("{s:?} is not a project id")))?,
        ),
        None => None,
    };
    let client_id = client.id.clone();
    let posted = state
        .blocking(move |store| {
            store.chat_mcp_instruct(
                &client_id,
                args.thread_id.as_deref(),
                project_id.map(|p| p.to_string()).as_deref(),
                &args.text,
                time::OffsetDateTime::now_utc(),
            )
        })
        .await
        .map_err(chat_error)?;
    let message = posted.response.message;
    ToolOutput::from_serialize(&InstructOutput {
        task_id: message.id.clone(),
        message_id: message.id,
        thread_id: message.thread_id,
    })
}

fn instruct_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(instruct_impl(state, client, args))
}

pub fn instruct_def() -> ToolDef {
    ToolDef {
        name: "console_instruct",
        description: "新しい CoS 会話を作って発言を積む。thread_id 指定で続ける（author は mcp:<client_id>）。",
        scope: task_core::McpScope::ConsoleInstruct,
        input_schema: schema::<InstructArgs>,
        call: instruct_call,
    }
}

// ---- console_reply ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyArgs {
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub wait_secs: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ActionSummary {
    pub kind: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone_id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ReplyOutput {
    Pending,
    Done {
        reply: String,
        actions: Vec<ActionSummary>,
    },
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<String>,
    },
    Cancelled,
}

/// 上限 60 秒（ADR-0056 D2）。
const MAX_WAIT_SECS: u64 = 60;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

async fn reply_impl(
    state: &Arc<McpState>,
    _client: &AuthedClient,
    args: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    let args: ReplyArgs =
        serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))?;
    if args.task_id.is_some() == args.thread_id.is_some() {
        return Err(ToolError::invalid_params(
            "specify exactly one of task_id or thread_id",
        ));
    }
    if let Some(id) = &args.task_id {
        id.parse::<TaskId>()
            .map_err(|_| ToolError::invalid_params(format!("{id:?} is not a task id")))?;
    }
    let wait = Duration::from_secs(args.wait_secs.unwrap_or(0).min(MAX_WAIT_SECS));
    let out = state
        .blocking(move |store| -> Result<ReplyOutput, ToolError> {
            let deadline = Instant::now() + wait;
            loop {
                let reply = store
                    .chat_mcp_reply(args.task_id.as_deref(), args.thread_id.as_deref())
                    .map_err(chat_error)?;
                match reply {
                    Some((state, reply)) => match state.as_str() {
                        "completed" => {
                            return Ok(ReplyOutput::Done {
                                reply: reply.unwrap_or_default(),
                                actions: Vec::new(),
                            });
                        }
                        "failed" => return Ok(ReplyOutput::Failed { reply }),
                        "stopped" | "interrupted" | "cancelled" => {
                            return Ok(ReplyOutput::Cancelled);
                        }
                        _ => {}
                    },
                    None if args.task_id.is_some() => {
                        return Err(ToolError::not_found("MCP receipt was not found"));
                    }
                    None => {}
                }
                if Instant::now() >= deadline {
                    return Ok(ReplyOutput::Pending);
                }
                std::thread::sleep(
                    POLL_INTERVAL
                        .min(deadline.saturating_duration_since(Instant::now()))
                        .max(Duration::from_millis(1)),
                );
            }
        })
        .await?;
    ToolOutput::from_serialize(&out)
}

fn reply_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: serde_json::Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(reply_impl(state, client, args))
}

pub fn reply_def() -> ToolDef {
    ToolDef {
        name: "console_reply",
        description: "console_instruct の CoS chat run の返事を返す（wait_secs 上限 60）。",
        scope: task_core::McpScope::ConsoleInstruct,
        input_schema: schema::<ReplyArgs>,
        call: reply_call,
    }
}
