//! surface registry and shared-operation dispatch (ADR 2026-10-09 D3).

use super::super::operations::{DispatchEnv, Matched, OperationAudit};
use super::super::operations::{
    audited, decode_optional, decode_problem, effect_value, external_store, path_param,
    unprocessable,
};
use crate::problem::ApiProblem;
use serde_json::Value;
use task_core::chat::{
    self, ChatCreateThreadRequest, ChatError, ChatMessageResponse, ChatPatchThreadRequest,
    ChatPostMessageRequest, ChatResumeQueueRequest, ChatStopRequest, ChatThreadResponse,
    CosOperation,
};
use task_core::store::SqliteStore;

/// Registered `(method, path, action)` operations. Chat thread/message/run-queue writes and the
/// legacy new conversation are one transaction (A); attachment files, run starts (`org.message`),
/// docs promotion and browser operations are two-stage external operations (C).
pub(crate) const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/api/v1/chat/attachments/{id}/references",
        "attachment.reference",
    ),
    ("POST", "/api/v1/chat/threads", "chat.thread_create"),
    ("PATCH", "/api/v1/chat/threads/{t}", "chat.thread_update"),
    (
        "POST",
        "/api/v1/chat/threads/{t}/messages",
        "chat.message_post",
    ),
    (
        "DELETE",
        "/api/v1/chat/threads/{t}/messages/{m}",
        "chat.message_cancel",
    ),
    ("POST", "/api/v1/chat/threads/{t}/stop", "chat.run_stop"),
    (
        "POST",
        "/api/v1/chat/threads/{t}/resume-queue",
        "chat.queue_resume",
    ),
    (
        "POST",
        "/api/v1/chat/threads/{t}/attachments",
        "chat.attachment_upload",
    ),
    (
        "DELETE",
        "/api/v1/chat/attachments/{a}",
        "chat.attachment_delete",
    ),
    (
        "POST",
        "/api/v1/console/new-conversation",
        "console.new_conversation",
    ),
    ("POST", "/api/v1/org/{id}/messages", "org.message"),
    (
        "POST",
        "/api/v1/tasks/{id}/artifacts/promote",
        "artifact.promote",
    ),
    (
        "PUT",
        "/api/v1/browser/site-policies/{policy_id}",
        "browser.site_policy_put",
    ),
    (
        "DELETE",
        "/api/v1/browser/site-policies/{policy_id}",
        "browser.site_policy_delete",
    ),
    (
        "PUT",
        "/api/v1/tasks/{id}/browser/policy",
        "browser.task_policy_put",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/requests",
        "browser.request_open",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}",
        "browser.control",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/disconnect",
        "browser.control_disconnect",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/agent/begin",
        "browser.agent_begin",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/agent/end",
        "browser.agent_end",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/control/{run}/{session}/auth-section",
        "browser.auth_section",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/events",
        "browser.live_event",
    ),
];

/// ADR D2 exclusions: `(method, path, reason code and detail)`.
pub(crate) const EXCLUDED: &[(&str, &str, &str)] = &[
    (
        "DELETE",
        "/api/v1/browser/credentials/{id}",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/identities",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "DELETE",
        "/api/v1/browser/identities/{id}",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/identities/{id}/restore",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/identities/{id}/revoke",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/trusted-devices",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/browser/trusted-devices/verify",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "DELETE",
        "/api/v1/browser/trusted-devices/{id}",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/console/instruct",
        "console_instruction_chain: 人の決定 daemon-wide=no-console。別の指示経路へ入り監査の鎖が二重になる。",
    ),
    (
        "POST",
        "/api/v1/cos/inbox/{i}/resolve",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/cos/operations",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/cos/operations/{o}/override",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/cos/threads/{t}/checkpoint",
        "recursive_cos: 既存の再帰禁止。操作 envelope 内から CoS 制御 API を呼ばない。override は人の取消・差し戻し専用。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/check",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/grant",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/read",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/live/{run}/{session}/frames",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/credential",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/decision",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/registered",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
    (
        "POST",
        "/api/v1/tasks/{id}/browser/waits/{wait_id}/revoke",
        "browser_credential_attestation: 人の決定 secrets=exclude。credential・封緘 state・receipt・owner attestation／信頼端末を扱う系列。値を含まない失効・削除も除外。",
    ),
];

const THREAD: &str = "/api/v1/chat/threads/{t}";
const MESSAGE: &str = "/api/v1/chat/threads/{t}/messages/{m}";
const ATTACHMENT: &str = "/api/v1/chat/attachments/{a}";
const SITE_POLICY: &str = "/api/v1/browser/site-policies/{policy_id}";
const CONTROL: &str = "/api/v1/tasks/{id}/browser/control/{run}/{session}";
const LIVE: &str = "/api/v1/tasks/{id}/browser/live/{run}/{session}/events";

/// The first segment after `/api/v1/chat/threads/`, for the thread sub-routes.
fn thread_of(path: &str) -> String {
    path.split('/').nth(5).unwrap_or_default().to_string()
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ApiProblem> {
    serde_json::to_value(value).map_err(|e| ApiProblem::internal(e.to_string()))
}

/// An A operation on the chat store: `write` runs in the operation transaction and a chat error
/// is the route's own problem (status and code), recorded as a rejected row.
fn chat_apply<T: serde::Serialize>(
    store: &SqliteStore,
    audit: &OperationAudit,
    target: (&str, &str),
    action: &str,
    write: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, ChatError>,
) -> Result<CosOperation, ApiProblem> {
    audit.apply_checked(store, target.0, target.1, action, |tx| {
        write(tx)
            .map_err(crate::chat::chat_problem)
            .and_then(|value| to_value(&value))
    })
}

/// The CoS form of a route without a body: `null` or `{}`.
fn require_empty(
    store: &SqliteStore,
    audit: &OperationAudit,
    path: &str,
    body: &Value,
) -> Result<(), ApiProblem> {
    if body.is_null() || *body == serde_json::json!({}) {
        return Ok(());
    }
    Err(audit.reject(
        store,
        "api",
        path,
        unprocessable("validation", "request.body must be empty"),
    ))
}

/// `{"scope": "all" | "project:<id>" | "node:cos"}`: the CoS form of the route's `?scope=`.
#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ScopeBody {
    #[serde(default)]
    scope: Option<String>,
}

pub(crate) fn dispatch(
    store: &SqliteStore,
    env: &DispatchEnv,
    audit: &OperationAudit,
    matched: Matched,
    path: &str,
    body: Value,
) -> Result<CosOperation, ApiProblem> {
    let decode = |error| decode_problem(store, audit, path, error);
    let now = time::OffsetDateTime::now_utc();
    let api = &env.api;
    match matched.action {
        "attachment.reference" => {
            let raw_id = matched.id.unwrap_or_default();
            let input = serde_json::from_value(body).map_err(decode)?;
            crate::chat::attachments::add_reference_op(
                store,
                env.kb_root.clone(),
                raw_id,
                input,
                audit,
            )
        }
        "chat.thread_create" => {
            let req: ChatCreateThreadRequest = serde_json::from_value(body).map_err(decode)?;
            chat_apply(
                store,
                audit,
                ("chat_thread", &req.client_thread_id),
                matched.action,
                |tx| {
                    let created = chat::chat_thread_create_tx(tx, "admin", &req, now)?;
                    Ok(serde_json::json!({"thread": created.thread, "created": created.created}))
                },
            )
        }
        "chat.thread_update" => {
            let thread = path_param(THREAD, path, "{t}");
            let req: ChatPatchThreadRequest = serde_json::from_value(body).map_err(decode)?;
            chat_apply(
                store,
                audit,
                ("chat_thread", &thread),
                matched.action,
                |tx| {
                    chat::chat_thread_patch_tx(tx, &thread, &req, now)
                        .map(|thread| ChatThreadResponse { thread })
                },
            )
        }
        "chat.message_post" => {
            let thread = thread_of(path);
            let req: ChatPostMessageRequest = serde_json::from_value(body).map_err(decode)?;
            let operation_id = audit.ctx.operation_id.clone();
            chat_apply(
                store,
                audit,
                ("chat_thread", &thread),
                matched.action,
                |tx| {
                    chat::chat_message_post_cos_tx(tx, &thread, &req, &operation_id, now)
                        .map(|message| ChatMessageResponse { message })
                },
            )
        }
        "chat.message_cancel" => {
            let thread = path_param(MESSAGE, path, "{t}");
            let message = path_param(MESSAGE, path, "{m}");
            require_empty(store, audit, path, &body)?;
            chat_apply(
                store,
                audit,
                ("chat_message", &message),
                matched.action,
                |tx| {
                    chat::chat_message_cancel_tx(tx, &thread, &message, now)
                        .map(|message| ChatMessageResponse { message })
                },
            )
        }
        "chat.run_stop" => {
            let thread = thread_of(path);
            let req: ChatStopRequest = serde_json::from_value(body).map_err(decode)?;
            chat_apply(
                store,
                audit,
                ("chat_run", &req.run_id),
                matched.action,
                |tx| {
                    let outcome = chat::chat_run_stop_tx(tx, &thread, &req.run_id, now)?;
                    Ok(
                        serde_json::json!({"accepted": outcome.accepted, "response": outcome.response}),
                    )
                },
            )
        }
        "chat.queue_resume" => {
            let thread = thread_of(path);
            let req: ChatResumeQueueRequest = serde_json::from_value(body).map_err(decode)?;
            chat_apply(
                store,
                audit,
                ("chat_thread", &thread),
                matched.action,
                |tx| {
                    chat::chat_thread_resume_queue_tx(tx, &thread, req.expected_revision, now)
                        .map(|thread| ChatThreadResponse { thread })
                },
            )
        }
        "chat.attachment_upload" => {
            let thread = thread_of(path);
            let input: crate::chat::attachments::CosUploadBody =
                serde_json::from_value(body).map_err(decode)?;
            let mut recorded = audit.clone();
            // The file bytes stay out of the audit record; its size and name remain.
            recorded.payload["body"]["content_base64"] =
                serde_json::json!({"redacted": true, "bytes": input.content_base64.len()});
            let bytes = crate::chat::attachments::decode_upload(api, &input)
                .map_err(|p| recorded.reject(store, "chat_thread", &thread, p))?;
            recorded.external(store, "chat_thread", &thread, matched.action, now, || {
                effect_value(crate::chat::attachments::upload_effect(
                    api, store, &thread, &input, bytes,
                ))
            })
        }
        "chat.attachment_delete" => {
            let id = path_param(ATTACHMENT, path, "{a}");
            require_empty(store, audit, path, &body)?;
            audit.external(store, "attachment", &id, matched.action, now, || {
                effect_value(
                    crate::chat::attachments::delete_effect(api, &id)
                        .map(|r| r.map(|()| serde_json::json!({"id": id, "deleted": true}))),
                )
            })
        }
        "console.new_conversation" => {
            let input: ScopeBody = decode_optional(body).map_err(decode)?;
            let project = crate::console::new_conversation_project(input.scope.as_deref())
                .map_err(|p| audit.reject(store, "chat_thread", path, p))?;
            let target = project.map_or_else(|| "cos".to_string(), |p| p.to_string());
            chat_apply(
                store,
                audit,
                ("chat_thread", &target),
                matched.action,
                |tx| {
                    let thread_id =
                        task_core::store::chat_legacy_new_conversation_tx(tx, project, now)?;
                    Ok(serde_json::json!({"thread_id": thread_id}))
                },
            )
        }
        "org.message" => {
            let id = matched.id.unwrap_or_default();
            let input: crate::conversation::MessagePostBody =
                serde_json::from_value(body).map_err(decode)?;
            let reject = |p| audit.reject(store, "org", &id, p);
            if id == task_core::COS_ID {
                // A CoS message to CoS would queue another CoS run: no self-chain.
                return Err(reject(unprocessable(
                    "cos_self_chain",
                    "a CoS run cannot message the CoS node (it would start another CoS run); reply in the thread",
                )));
            }
            crate::conversation::check_node(store, &id).map_err(reject)?;
            if input.text.trim().is_empty() {
                return Err(reject(unprocessable(
                    "validation",
                    "text must not be blank",
                )));
            }
            external_store(store, audit, ("org", &id), matched.action, || {
                crate::conversation::start_conversation(
                    store,
                    &id,
                    &input,
                    &env.roles,
                    &env.genres,
                    &api.inner.conversation_genre,
                )
                .map(|started| crate::conversation::MessageAccepted {
                    message_id: started.message.id.to_string(),
                    task_id: started.task.id,
                })
            })
        }
        "artifact.promote" => {
            let id = matched.id.unwrap_or_default();
            let task_id = crate::query::parse_task_id(&id)
                .map_err(|p| audit.reject(store, "docs", &id, p))?;
            let input = serde_json::from_value(body).map_err(decode)?;
            let docs = crate::docs::DocsEnv::of(api);
            audited(crate::docs::promote_op(
                store,
                &docs,
                &api.inner.documentation_state_dir,
                task_id,
                input,
                Some(audit),
            )?)
        }
        "browser.site_policy_put" => {
            let policy_id = path_param(SITE_POLICY, path, "{policy_id}");
            let input = serde_json::from_value(body).map_err(decode)?;
            let policy = crate::browser_site_policies::validated_policy(policy_id.clone(), input)
                .map_err(|p| audit.reject(store, "browser_site_policy", &policy_id, p))?;
            external_store(
                store,
                audit,
                ("browser_site_policy", &policy_id),
                matched.action,
                || crate::browser_site_policies::upsert_policy(store, &policy, "cos"),
            )
        }
        "browser.site_policy_delete" => {
            let policy_id = path_param(SITE_POLICY, path, "{policy_id}");
            require_empty(store, audit, path, &body)?;
            external_store(
                store,
                audit,
                ("browser_site_policy", &policy_id),
                matched.action,
                || {
                    crate::browser_site_policies::delete_policy(store, &policy_id, "cos")
                        .map(|()| serde_json::json!({"policy_id": policy_id, "deleted": true}))
                },
            )
        }
        "browser.task_policy_put" | "browser.request_open" => {
            let id = matched.id.unwrap_or_default();
            let reject = |p| audit.reject(store, "browser_task", &id, p);
            let task_id = crate::query::parse_task_id(&id).map_err(reject)?;
            if matched.action == "browser.task_policy_put" {
                let policy =
                    crate::browser::parse_task_policy(&body.to_string()).map_err(reject)?;
                external_store(store, audit, ("browser_task", &id), matched.action, || {
                    crate::browser::set_task_policy(store, task_id, &policy)
                        .map(|()| serde_json::json!({"updated": true}))
                })
            } else {
                let request = crate::browser::decode_wait(body).map_err(reject)?;
                if request.credential.is_some()
                    || request.credential_policy_id.is_some()
                    || request.trusted_login.is_some()
                {
                    // ADR D2 (human decision secrets=exclude): credential-use waits stay human.
                    return Err(reject(unprocessable(
                        "browser_credential_attestation",
                        "a browser wait naming a credential is a credential/attestation operation (excluded for CoS)",
                    )));
                }
                external_store(store, audit, ("browser_task", &id), matched.action, || {
                    crate::browser::open_wait(store, task_id, &request)
                })
            }
        }
        "browser.control"
        | "browser.control_disconnect"
        | "browser.agent_begin"
        | "browser.agent_end"
        | "browser.auth_section" => {
            let session = (
                path_param(CONTROL, path, "{id}"),
                path_param(CONTROL, path, "{run}"),
                path_param(CONTROL, path, "{session}"),
            );
            crate::browser_control::control_op(store, api, session, body, audit, matched.action)
        }
        "browser.live_event" => {
            let session = (
                path_param(LIVE, path, "{id}"),
                path_param(LIVE, path, "{run}"),
                path_param(LIVE, path, "{session}"),
            );
            crate::browser_live::event_op(store, api, session, body, audit)
        }
        other => Err(ApiProblem::internal(format!(
            "registered CoS operation {other} has no implementation in surface"
        ))),
    }
}
