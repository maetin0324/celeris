//! CoS chat REST API. Run scheduling belongs to cos-run; writes here only change durable state.

pub mod attachments;
pub mod stream;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde::de::DeserializeOwned;
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::{
    ChatCreateThreadRequest, ChatError, ChatMessageQuery, ChatMessageResponse,
    ChatPatchThreadRequest, ChatPostMessageRequest, ChatResumeQueueRequest, ChatRunResponse,
    ChatStopRequest, ChatThreadQuery, ChatThreadResponse, ChatThreadStatus,
};
use time::OffsetDateTime;
use tokio::sync::Notify;

use crate::handlers::{ApiResult, Params, json_response, no_query};
use crate::middleware::require_admin;
use crate::problem::ApiProblem;
use crate::query::QueryParams;
use crate::state::ApiState;

/// Shared configuration reserved for the sibling attachment and SSE route modules.
#[derive(Clone)]
pub struct ChatState {
    pub attachment_data_dir: Option<PathBuf>,
    /// The migrated SQLite file used by the attachment store's separate connection.
    pub attachment_db_path: Option<PathBuf>,
    pub attachment_limits: ChatAttachmentLimits,
    pub events: Arc<Notify>,
    /// Single decision point for cos-run to replace when `[cos]` is wired.
    pub enabled: bool,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            attachment_data_dir: None,
            attachment_db_path: None,
            attachment_limits: ChatAttachmentLimits::default(),
            events: Arc::new(Notify::new()),
            enabled: true,
        }
    }
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/chat/threads",
            get(list_threads).post(create_thread),
        )
        .route(
            "/api/v1/chat/threads/{t}",
            get(get_thread).patch(patch_thread),
        )
        .route(
            "/api/v1/chat/threads/{t}/messages",
            get(list_messages).post(post_message),
        )
        .route(
            "/api/v1/chat/threads/{t}/messages/{m}",
            axum::routing::delete(cancel_message),
        )
        .route("/api/v1/chat/threads/{t}/stop", post(stop_run))
        .route("/api/v1/chat/threads/{t}/resume-queue", post(resume_queue))
        .route("/api/v1/chat/threads/{t}/runs/{r}", get(get_run))
        .merge(stream::routes())
        .merge(attachments::routes())
}

fn chat_problem(error: ChatError) -> ApiProblem {
    let status =
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let code = match &error {
        ChatError::Store(_) => "chat_store_error",
        ChatError::BadRequest(_) => "bad_request",
        ChatError::Invalid(_) => "validation",
        ChatError::NotFound { .. } => "chat_not_found",
        ChatError::Conflict(_) => "chat_conflict",
        ChatError::TooLarge(_) => "payload_too_large",
        ChatError::QueueFull { .. } => "chat_queue_full",
        ChatError::CursorExpired(_) => "chat_cursor_expired",
    };
    ApiProblem::new(status, code, error.to_string())
}

async fn chat_json<T: DeserializeOwned>(body: Body) -> Result<T, ApiProblem> {
    // Keep malformed JSON/unknown fields distinct from well-formed but invalid field values.
    let value: serde_json::Value = crate::handlers::read_json(body, false).await?;
    serde_json::from_value(value).map_err(|error| {
        let detail = error.to_string();
        if detail.starts_with("unknown field") {
            ApiProblem::bad_request(detail)
        } else {
            ApiProblem::new(StatusCode::UNPROCESSABLE_ENTITY, "validation", detail)
        }
    })
}

fn authorize(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiProblem> {
    require_admin(state, headers)
}

fn changed(state: &ApiState) {
    state.chat.events.notify_waiters();
}

async fn list_threads(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    authorize(&state, &headers)?;
    let params = QueryParams::parse(raw.as_deref(), &["q", "status", "before", "limit"])?;
    let status = match params.single("status")? {
        None => None,
        Some("open") => Some(ChatThreadStatus::Open),
        Some("archived") => Some(ChatThreadStatus::Archived),
        Some(_) => return Err(ApiProblem::bad_request("invalid thread status")),
    };
    let query = ChatThreadQuery {
        q: params.single("q")?.map(str::to_owned),
        status,
        before: params.single("before")?.map(str::to_owned),
        limit: Some(params.limit("limit", 50, 100)? as u32),
    };
    let result = state
        .blocking(move |store| store.chat_thread_list(&query).map_err(chat_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

async fn create_thread(
    State(state): State<ApiState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let req: ChatCreateThreadRequest = chat_json(body).await?;
    let result = state
        .blocking(move |store| {
            store
                .chat_thread_create("admin", &req, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        if result.created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        &ChatThreadResponse {
            thread: result.thread,
        },
    ))
}

async fn get_thread(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let result = state
        .blocking(move |store| store.chat_thread_detail(&t).map_err(chat_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

async fn patch_thread(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let req: ChatPatchThreadRequest = chat_json(body).await?;
    let thread = state
        .blocking(move |store| {
            store
                .chat_thread_patch(&t, &req, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        StatusCode::OK,
        &ChatThreadResponse { thread },
    ))
}

async fn list_messages(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    authorize(&state, &headers)?;
    let params = QueryParams::parse(raw.as_deref(), &["before_seq", "after_seq", "limit"])?;
    let query = ChatMessageQuery {
        before_seq: params.u64("before_seq")?,
        after_seq: params.u64("after_seq")?,
        limit: Some(params.limit("limit", 50, 200)? as u32),
    };
    let result = state
        .blocking(move |store| store.chat_message_list(&t, &query).map_err(chat_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &result))
}

async fn post_message(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    if !state.chat.enabled {
        return Err(ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "cos_unavailable",
            "CoS is disabled",
        ));
    }
    let req: ChatPostMessageRequest = chat_json(body).await?;
    let result = state
        .blocking(move |store| {
            store
                .chat_message_post(&t, &req, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(StatusCode::ACCEPTED, &result.response))
}

async fn cancel_message(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((t, m)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let message = state
        .blocking(move |store| {
            store
                .chat_message_cancel(&t, &m, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        StatusCode::OK,
        &ChatMessageResponse { message },
    ))
}

async fn stop_run(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let req: ChatStopRequest = chat_json(body).await?;
    let result = state
        .blocking(move |store| {
            store
                .chat_run_stop(&t, &req.run_id, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        if result.accepted {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        &result.response,
    ))
}

async fn resume_queue(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(t): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let req: ChatResumeQueueRequest = chat_json(body).await?;
    let thread = state
        .blocking(move |store| {
            store
                .chat_thread_resume_queue(&t, req.expected_revision, OffsetDateTime::now_utc())
                .map_err(chat_problem)
        })
        .await?;
    changed(&state);
    Ok(json_response(
        StatusCode::OK,
        &ChatThreadResponse { thread },
    ))
}

async fn get_run(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params((t, r)): Params<(String, String)>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let run = state
        .blocking(move |store| store.chat_run_get(&t, &r).map_err(chat_problem))
        .await?;
    Ok(json_response(StatusCode::OK, &ChatRunResponse { run }))
}
