//! Resolve human instruction evidence and enforce relay scope.
use super::{DispatchEnv, Matched};
use serde_json::Value;
use task_core::{TaskStore, chat::AuditContext, store::SqliteStore};

/// The task an operation acts on, for the scope check of a relayed instruction: the task of a
/// `/api/v1/tasks/{id}/…` route, or the task behind an inbox item (`inbox.answer`). Other
/// targets (a project, a release, …) carry no task; a task-scoped relay then does not cover them.
pub(super) fn operation_target(
    matched: &Matched,
    path: &str,
    env: &DispatchEnv,
) -> Option<OperationTarget> {
    let id = matched.id.as_deref()?;
    if matched.action == "inbox.answer" {
        let task = env
            .inbox_feed
            .as_ref()?
            .iter()
            .find(|item| item.id == id)?
            .task
            .as_ref()
            .map(|task| task.id.to_string());
        return Some(OperationTarget {
            kind: "inbox",
            id: id.to_owned(),
            task_id: task,
        });
    }
    let task_id = path.starts_with("/api/v1/tasks/").then(|| id.to_owned());
    Some(OperationTarget {
        kind: if task_id.is_some() {
            "task"
        } else if path.starts_with("/api/v1/projects/") {
            "project"
        } else {
            "other"
        },
        id: id.to_owned(),
        task_id,
    })
}

pub(super) struct OperationTarget {
    kind: &'static str,
    id: String,
    task_id: Option<String>,
}

/// The instruction `instructed_by` resolved to (D3), as recorded in the payload and the reason.
pub(super) struct Instruction {
    message: task_core::chat::ChatMessage,
    /// `cos_general` / `cos_project` of the caller's thread (human-ops D2 vocabulary).
    executor_kind: &'static str,
    thread_id: String,
}

impl Instruction {
    pub(super) fn reason_prefix(&self) -> String {
        match &self.message.relay {
            Some(relay) => format!(
                "人の指示（seq {}、{} の取り次ぎ）",
                self.message.seq, relay.relayed_by
            ),
            None => format!("人の指示（seq {}）", self.message.seq),
        }
    }

    /// ADR 2026-10-27-relayed-instructions D3: `decided_by` is the person (relayed or not),
    /// `executed_by` is the CoS of this thread.
    pub(super) fn payload(&self) -> Value {
        let mut decided_by = serde_json::json!({"kind": "human", "id": Value::Null});
        let mut out = serde_json::json!({
            "message_id": self.message.id,
            "seq": self.message.seq,
        });
        if let Some(relay) = &self.message.relay {
            let (kind, id) = relayer_actor(&relay.relayed_by);
            decided_by["relayed_by"] = serde_json::json!({"kind": kind, "id": id});
            // ADR 2026-10-28-single-human-window-general-cos D3: a forward knows the person.
            if let Some(name) = relay
                .source
                .as_ref()
                .and_then(|s| s.author.as_deref())
                .and_then(|a| a.strip_prefix("human:"))
            {
                decided_by["id"] = Value::String(name.to_owned());
            }
            out["relay"] = serde_json::to_value(relay).unwrap_or(Value::Null);
        }
        out["decided_by"] = decided_by;
        out["executed_by"] = serde_json::json!({"kind": self.executor_kind, "id": self.thread_id});
        out
    }
}

/// `operator:<name>` → (`external_operator`, name); `mcp:<id>` → (`mcp_client`, id);
/// `cos:<general thread>` → (`cos_general`, thread) (human-ops D2 vocabulary; ADR
/// 2026-10-28-single-human-window-general-cos D3). Any other author is recorded as `unknown`.
fn relayer_actor(relayed_by: &str) -> (&'static str, &str) {
    match relayed_by.split_once(':') {
        Some(("operator", name)) => ("external_operator", name),
        Some(("mcp", id)) => ("mcp_client", id),
        Some(("cos", thread)) => ("cos_general", thread),
        _ => ("unknown", relayed_by),
    }
}

/// D3: the message `instructed_by` names, when it is the `role=user` input of the caller's run
/// in a CoS thread, or a relayed instruction of that thread in scope at `now`
/// (ADR 2026-10-27-relayed-instructions D3). Otherwise the reason it is refused
/// (`cos_instruction_invalid`).
pub(super) fn instructed_message(
    store: &SqliteStore,
    ctx: &AuditContext,
    message_id: &str,
    target: Option<&OperationTarget>,
    now: time::OffsetDateTime,
) -> Result<Instruction, String> {
    let thread_id = &ctx.thread_id;
    let Ok(message) = store.chat_message_get(thread_id, message_id) else {
        return Err(format!(
            "instructed_by {message_id} is not a message of thread {thread_id}"
        ));
    };
    if message.role != task_core::chat::ChatMessageRole::User
        || message.state == task_core::chat::ChatMessageState::Cancelled
    {
        return Err(format!("instructed_by {message_id} is not a human message"));
    }
    let thread = match store.chat_thread_get(thread_id) {
        Ok(Some(thread)) => thread,
        _ => return Err(format!("thread {thread_id} is not readable")),
    };
    // ADR 2026-10-22-cos-fixed-threads D7: any CoS thread (general or project), not legacy.
    let executor_kind = match thread.kind {
        task_core::chat::ChatThreadKind::General => "cos_general",
        task_core::chat::ChatThreadKind::Project => "cos_project",
        task_core::chat::ChatThreadKind::Legacy => {
            return Err(format!(
                "instructed_by is accepted only in a CoS thread (thread {thread_id} is {:?})",
                thread.kind
            ));
        }
    };
    // ADR 2026-10-28-single-human-window-general-cos D4: the general CoS forwards a project's
    // matter (chat.relay_forward); only the project CoS answers it, so nothing is answered twice.
    if executor_kind == "cos_general"
        && let Some(project) = target_project(store, target)
    {
        return Err(format!(
            "the general CoS does not act on project {project} itself: forward the person's \
             message to the project CoS (POST /api/v1/chat/threads/{{t}}/forwards)"
        ));
    }
    let Ok(run) = store.chat_run_get(thread_id, &ctx.run_id) else {
        return Err(format!(
            "run {} is not a run of thread {thread_id}",
            ctx.run_id
        ));
    };
    if let Some(relay) = &message.relay {
        // A relayed instruction grounds any run of its thread while in scope.
        if let Some(project) = &relay.scope.project_id
            && target_project(store, target).as_ref() != Some(project)
        {
            return Err(format!(
                "instructed_by {message_id} applies to project {project}, not this target"
            ));
        }

        if task_core::chat::relay_expired(relay, now) {
            return Err(format!(
                "instructed_by {message_id} is a relayed instruction that expired at {}",
                relay.scope.expires_at.as_deref().unwrap_or("?")
            ));
        }
        if !relay.scope.task_ids.is_empty() {
            let covered = target
                .and_then(|t| t.task_id.as_deref())
                .is_some_and(|task| relay.scope.task_ids.iter().any(|t| t == task));
            if !covered {
                return Err(format!(
                    "instructed_by {message_id} applies to tasks {} but the operation targets {}",
                    relay.scope.task_ids.join(", "),
                    target
                        .map(|t| format!("{} {}", t.kind, t.id))
                        .unwrap_or_else(|| "nothing of that kind".into())
                ));
            }
        }
    } else if run.input_message_id != message.id {
        return Err(format!(
            "instructed_by {message_id} is not the human message run {} answers (its input is {})",
            ctx.run_id, run.input_message_id
        ));
    }
    Ok(Instruction {
        message,
        executor_kind,
        thread_id: thread_id.clone(),
    })
}

/// The project an operation's target belongs to: the project itself, or the project of the
/// target task (`None` for targets with neither).
fn target_project(store: &SqliteStore, target: Option<&OperationTarget>) -> Option<String> {
    match target {
        Some(t) if t.kind == "project" => Some(t.id.clone()),
        Some(t) => t
            .task_id
            .as_ref()
            .and_then(|id| id.parse().ok())
            .and_then(|id| store.get(id).ok().flatten())
            .and_then(|task| task.project_id.map(|id| id.to_string())),
        None => None,
    }
}
