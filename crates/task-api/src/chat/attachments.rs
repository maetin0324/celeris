//! Authenticated, bounded multipart attachment routes.
use std::io::{Read, Seek, SeekFrom};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::routing::{get, post};
use rusqlite::{OptionalExtension, params};
use task_core::chat::attachments::{AttachmentError, ChatAttachmentStore, StoredAttachment};
use task_core::chat::{
    ChatAttachment, ChatAttachmentResponse, ChatAttachmentState, ChatReferenceOwnerKind,
    ChatReferenceRequest, ChatReferenceResponse,
};
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;

use crate::chat::{authorize, chat_json, chat_problem};
use crate::handlers::{ApiResult, Params, json_response, no_query};
use crate::problem::ApiProblem;
use crate::state::ApiState;

pub(crate) fn routes() -> axum::Router<ApiState> {
    axum::Router::new()
        .route(
            "/api/v1/chat/threads/{t}/attachments",
            post(upload).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/api/v1/chat/attachments/{a}",
            get(get_metadata).delete(delete),
        )
        .route("/api/v1/chat/attachments/{a}/content", get(content))
        .route("/api/v1/chat/attachments/{a}/preview", get(preview))
        .route(
            "/api/v1/chat/attachments/{a}/references",
            post(add_reference),
        )
}

fn error(err: AttachmentError) -> ApiProblem {
    let (status, code) = match err {
        AttachmentError::Limit => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
        AttachmentError::Conflict => (StatusCode::CONFLICT, "chat_conflict"),
        AttachmentError::NotFound => (StatusCode::NOT_FOUND, "chat_not_found"),
        AttachmentError::InvalidPath => (StatusCode::BAD_REQUEST, "bad_request"),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "attachment_store_error"),
    };
    ApiProblem::new(status, code, err.to_string())
}

fn multipart_error(err: axum::extract::multipart::MultipartError) -> ApiProblem {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        error(AttachmentError::Limit)
    } else {
        ApiProblem::bad_request(err.to_string())
    }
}

fn open(state: &ApiState) -> Result<ChatAttachmentStore, ApiProblem> {
    let dir = state.chat.attachment_data_dir.as_ref().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments_unavailable",
            "attachment storage is not configured",
        )
    })?;
    let db = state.chat.attachment_db_path.as_ref().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments_unavailable",
            "attachment storage is not configured",
        )
    })?;
    ChatAttachmentStore::open(dir, db, state.chat.attachment_limits).map_err(error)
}

fn preview_bytes(store: &ChatAttachmentStore, row: &StoredAttachment) -> Option<Vec<u8>> {
    if !matches!(
        row.media_type.as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        return None;
    }
    let file = store.read_verified(&row.id).ok()?;
    let reader = image::ImageReader::new(std::io::BufReader::new(file))
        .with_guessed_format()
        .ok()?;
    let (width, height) = reader.into_dimensions().ok()?;
    if u64::from(width) * u64::from(height) > 40_000_000 {
        return None;
    }
    let file = store.read_verified(&row.id).ok()?;
    let image = image::ImageReader::new(std::io::BufReader::new(file))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    // GIF decode takes the first frame; re-encoding produces raster bytes only.
    let mut output = std::io::Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).ok()?;
    Some(output.into_inner())
}

fn representation(
    state: &ApiState,
    store: &ChatAttachmentStore,
    row: StoredAttachment,
) -> Result<ChatAttachment, ApiProblem> {
    let db = state.chat.attachment_db_path.as_ref().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments_unavailable",
            "attachment storage is not configured",
        )
    })?;
    let conn = rusqlite::Connection::open(db).map_err(|e| error(AttachmentError::Db(e)))?;
    let expires_at: Option<String> = conn
        .query_row(
            "SELECT expires_at FROM chat_attachments WHERE id=?1",
            [&row.id],
            |r| r.get(0),
        )
        .map_err(|e| error(AttachmentError::Db(e)))?;
    let preview_url =
        preview_bytes(store, &row).map(|_| format!("/api/v1/chat/attachments/{}/preview", row.id));
    Ok(ChatAttachment {
        id: row.id.clone(),
        thread_id: row.thread_id,
        name: row.original_name,
        media_type: row.media_type,
        size_bytes: row.size_bytes,
        sha256: row.sha256,
        state: ChatAttachmentState::Ready,
        preview_url,
        download_url: format!("/api/v1/chat/attachments/{}/content", row.id),
        expires_at,
    })
}

async fn upload(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(thread): Params<String>,
    RawQuery(raw): RawQuery,
    mut multipart: Multipart,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let store = open(&state)?;
    let limit = state.chat.attachment_limits.max_file_bytes;
    let mut key = None;
    let mut name = None;
    let mut file = None;
    let mut size = 0u64;
    while let Some(mut field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name() {
            Some("client_upload_id") if key.is_none() => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                    if bytes.len().saturating_add(chunk.len()) > 256 {
                        return Err(ApiProblem::bad_request("invalid client_upload_id"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let value = String::from_utf8(bytes)
                    .map_err(|_| ApiProblem::bad_request("invalid client_upload_id"))?;
                if value.is_empty() {
                    return Err(ApiProblem::bad_request("invalid client_upload_id"));
                }
                key = Some(value);
            }
            Some("file") if file.is_none() => {
                let filename = field
                    .file_name()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ApiProblem::bad_request("file name is required"))?
                    .to_owned();
                let staging = state
                    .chat
                    .attachment_data_dir
                    .as_ref()
                    .ok_or_else(|| {
                        ApiProblem::new(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "attachments_unavailable",
                            "attachment storage is not configured",
                        )
                    })?
                    .join("chat/staging");
                let temporary =
                    tempfile::tempfile_in(staging).map_err(|e| error(AttachmentError::Io(e)))?;
                let mut output = tokio::fs::File::from_std(temporary);
                while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
                    size = size
                        .checked_add(chunk.len() as u64)
                        .ok_or_else(|| error(AttachmentError::Limit))?;
                    if size > limit {
                        return Err(error(AttachmentError::Limit));
                    }
                    output
                        .write_all(&chunk)
                        .await
                        .map_err(|e| error(AttachmentError::Io(e)))?;
                }
                output
                    .flush()
                    .await
                    .map_err(|e| error(AttachmentError::Io(e)))?;
                file = Some(output.into_std().await);
                name = Some(filename);
            }
            _ => {
                return Err(ApiProblem::bad_request(
                    "expected exactly one file and client_upload_id",
                ));
            }
        }
    }
    let (Some(mut file), Some(name), Some(key)) = (file, name, key) else {
        return Err(ApiProblem::bad_request(
            "file and client_upload_id are required",
        ));
    };
    file.seek(SeekFrom::Start(0))
        .map_err(|e| error(AttachmentError::Io(e)))?;
    let thread_check = thread.clone();
    state
        .blocking(move |db| {
            db.chat_thread_detail(&thread_check)
                .map(|_| ())
                .map_err(chat_problem)
        })
        .await?;
    let db_path = state.chat.attachment_db_path.as_ref().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments_unavailable",
            "attachment storage is not configured",
        )
    })?;
    let conn = rusqlite::Connection::open(db_path).map_err(|e| error(AttachmentError::Db(e)))?;
    let replay: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chat_client_requests WHERE kind='upload' AND scope_id=?1 AND key=?2)",
        params![thread, key], |r| r.get(0),
    ).map_err(|e| error(AttachmentError::Db(e)))?;
    let (store, created) = tokio::task::spawn_blocking(move || {
        let row = store
            .upload(
                &thread,
                &key,
                &name,
                Some(size),
                &mut file,
                OffsetDateTime::now_utc(),
            )
            .map_err(error)?;
        Ok::<_, ApiProblem>((store, row))
    })
    .await
    .map_err(|e| ApiProblem::internal(format!("upload task failed: {e}")))??;
    Ok(json_response(
        if replay {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        &ChatAttachmentResponse {
            attachment: representation(&state, &store, created)?,
        },
    ))
}

async fn get_metadata(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let store = open(&state)?;
    let row = store.get(&id).map_err(error)?;
    Ok(json_response(
        StatusCode::OK,
        &ChatAttachmentResponse {
            attachment: representation(&state, &store, row)?,
        },
    ))
}

async fn content(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let store = open(&state)?;
    let row = store.get(&id).map_err(error)?;
    let mut file = store.read_verified(&id).map_err(error)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| error(AttachmentError::Io(e)))?;
    let mut response = axum::http::Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    let encoded: String = form_urlencoded::byte_serialize(row.original_name.as_bytes()).collect();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}"))
            .map_err(|_| ApiProblem::bad_request("invalid file name"))?,
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn preview(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let store = open(&state)?;
    let row = store.get(&id).map_err(error)?;
    let bytes = preview_bytes(&store, &row).ok_or_else(|| error(AttachmentError::NotFound))?;
    let mut response = axum::http::Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

async fn delete(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    open(&state)?.delete_unreferenced(&id).map_err(error)?;
    let mut response = axum::http::Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    Ok(response)
}

async fn add_reference(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    authorize(&state, &headers)?;
    let req: ChatReferenceRequest = chat_json(body).await?;
    if req.owner_id.is_empty() || req.idempotency_key.is_empty() {
        return Err(ApiProblem::bad_request(
            "owner_id and idempotency_key are required",
        ));
    }
    let kind = match req.owner_kind {
        ChatReferenceOwnerKind::Task => "task",
        ChatReferenceOwnerKind::KnowledgeInbox => "knowledge_inbox",
    };
    let db = state.chat.attachment_db_path.as_ref().ok_or_else(|| {
        ApiProblem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "attachments_unavailable",
            "attachment storage is not configured",
        )
    })?;
    let conn = rusqlite::Connection::open(db).map_err(|e| error(AttachmentError::Db(e)))?;
    let request_hash = format!("{kind}:{}", req.owner_id);
    let prior: Option<String> = conn.query_row(
        "SELECT request_hash FROM chat_client_requests WHERE kind='attachment_ref' AND scope_id=?1 AND key=?2",
        params![id, req.idempotency_key], |r| r.get(0),
    ).optional().map_err(|e| error(AttachmentError::Db(e)))?;
    if prior.is_some_and(|hash| hash != request_hash) {
        return Err(error(AttachmentError::Conflict));
    }
    let kb_root = crate::knowledge::root_of(&state);
    let (owner_kind, owner_id) = (req.owner_kind, req.owner_id.clone());
    state
        .blocking(move |store| owner_checked(store, kb_root, owner_kind, &owner_id))
        .await?;
    open(&state)?
        .add_ref(&id, kind, &req.owner_id, OffsetDateTime::now_utc())
        .map_err(error)?;
    conn.execute(
        "INSERT OR IGNORE INTO chat_client_requests(kind,scope_id,key,request_hash,result_id,created_at) VALUES('attachment_ref',?1,?2,?3,?4,?5)",
        params![id, req.idempotency_key, request_hash, req.owner_id, OffsetDateTime::now_utc().to_string()],
    ).map_err(|e| error(AttachmentError::Db(e)))?;
    Ok(json_response(
        StatusCode::OK,
        &ChatReferenceResponse {
            attachment_id: id,
            owner_kind: req.owner_kind,
            owner_id: req.owner_id,
        },
    ))
}

/// The pin's owner must already exist (ADR 2026-10-05 cos-chat-home D4): a task that is not in the
/// store is 404 `task_not_found`, a KB inbox candidate without `_inbox/<id>.md` is 404
/// `candidate_not_found`. The REST route and the CoS operation share this check.
fn owner_checked(
    store: &task_core::store::SqliteStore,
    kb_root: Result<std::path::PathBuf, ApiProblem>,
    owner_kind: ChatReferenceOwnerKind,
    owner_id: &str,
) -> Result<&'static str, ApiProblem> {
    match owner_kind {
        ChatReferenceOwnerKind::Task => {
            let task = crate::query::parse_task_id(owner_id)?;
            if task_core::TaskStore::get(store, task)
                .map_err(|e| ApiProblem::internal(e.to_string()))?
                .is_none()
            {
                return Err(ApiProblem::new(
                    StatusCode::NOT_FOUND,
                    "task_not_found",
                    format!("no task {owner_id}"),
                ));
            }
            Ok("task")
        }
        ChatReferenceOwnerKind::KnowledgeInbox => {
            let root = kb_root?;
            if task_ops::knowledge::inbox_get(&root, owner_id).is_none() {
                return Err(ApiProblem::new(
                    StatusCode::NOT_FOUND,
                    "candidate_not_found",
                    format!("knowledge candidate not found: {owner_id}"),
                ));
            }
            Ok("knowledge_inbox")
        }
    }
}

/// The audited CoS operation `attachment.reference` (ADR 2026-10-05 cos-chat-home D4): a CoS run
/// pins a chat attachment to the task or KB inbox candidate it just created. The CoS credential
/// cannot POST the references route directly (422 `cos_audit_context_required`), so the same body
/// goes through `/cos/operations`. The owner must already exist; the pin is written in the audit
/// transaction.
pub(crate) fn add_reference_op(
    store: &task_core::store::SqliteStore,
    kb_root: Result<std::path::PathBuf, ApiProblem>,
    id: String,
    req: ChatReferenceRequest,
    audit: &crate::cos::operations::OperationAudit,
) -> Result<task_core::chat::CosOperation, ApiProblem> {
    let reject = |problem: ApiProblem| audit.reject(store, "attachment", &id, problem);
    if req.owner_id.is_empty() || req.idempotency_key.is_empty() {
        return Err(reject(ApiProblem::bad_request(
            "owner_id and idempotency_key are required",
        )));
    }
    let kind = owner_checked(store, kb_root, req.owner_kind, &req.owner_id).map_err(reject)?;
    let mut failure = None;
    let outcome = audit.apply(store, "attachment", &id, "attachment.reference", |tx| {
        match task_core::chat::attachments::add_ref_tx(
            tx,
            &id,
            kind,
            &req.owner_id,
            OffsetDateTime::now_utc(),
        ) {
            Ok(()) => Ok(serde_json::json!(ChatReferenceResponse {
                attachment_id: id.clone(),
                owner_kind: req.owner_kind,
                owner_id: req.owner_id.clone(),
            })),
            Err(err) => {
                let detail = err.to_string();
                failure = Some(error(err));
                Err(task_core::chat::ChatError::Invalid(detail))
            }
        }
    });
    outcome.map_err(|problem| failure.unwrap_or(problem))
}
