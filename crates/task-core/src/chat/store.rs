//! CoS chat store operations (ADR 2026-10-05 D1/D2): threads and messages.
//!
//! Every write takes the writer lock and runs one IMMEDIATE transaction; reference checks
//! (project, attachment, reply target) happen inside that transaction. Clocks are injected
//! (`now` arguments), and no method calls an LLM. Runs, events and retention live in
//! `run_store.rs`.

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use ulid::Ulid;

use super::*;
use crate::store::{SqliteStore, StoreError};

/// D2: message text limit (UTF-8 bytes).
pub const CHAT_MESSAGE_TEXT_MAX_BYTES: usize = 64 * 1024;
/// D2: attachments per message.
pub const CHAT_MESSAGE_ATTACHMENTS_MAX: usize = 10;
/// D2: waiting (queued) user messages per thread.
pub const CHAT_QUEUE_MAX: usize = 100;
/// D2: `GET /chat/threads` limit (default / max).
pub const CHAT_THREAD_PAGE_DEFAULT: u32 = 50;
pub const CHAT_THREAD_PAGE_MAX: u32 = 100;
/// D2: `GET /chat/threads/{t}/messages` limit (default / max).
pub const CHAT_MESSAGE_PAGE_DEFAULT: u32 = 50;
pub const CHAT_MESSAGE_PAGE_MAX: u32 = 200;
/// Title length limit in characters (the ADR does not fix one; keeps list rows bounded).
pub const CHAT_TITLE_MAX_CHARS: usize = 200;
/// Idempotency key length limit (client_thread_id / client_message_id).
pub const CHAT_CLIENT_KEY_MAX_BYTES: usize = 128;

/// Store-level chat error. Each variant maps to one D2 HTTP status (`http_status`).
#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Malformed query/cursor, or mutually exclusive parameters (400). Includes future cursors.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Well-formed but invalid content (422).
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("{kind} {id} not found")]
    NotFound { kind: &'static str, id: String },
    /// State or idempotency-key mismatch (409).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Size limit exceeded (413).
    #[error("too large: {0}")]
    TooLarge(String),
    /// Queue limit reached (429).
    #[error("queue full: {limit} queued messages")]
    QueueFull { limit: usize },
    /// The cursor points into events removed by retention (410, code chat-cursor-expired).
    #[error("chat cursor {0} expired")]
    CursorExpired(String),
}

impl From<rusqlite::Error> for ChatError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(StoreError::Sqlite(e))
    }
}
impl From<serde_json::Error> for ChatError {
    fn from(e: serde_json::Error) -> Self {
        Self::Store(StoreError::Serde(e))
    }
}

impl ChatError {
    /// D2 status for the API (`ApiProblem`).
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Store(_) => 500,
            Self::BadRequest(_) => 400,
            Self::Invalid(_) => 422,
            Self::NotFound { .. } => 404,
            Self::Conflict(_) => 409,
            Self::TooLarge(_) => 413,
            Self::QueueFull { .. } => 429,
            Self::CursorExpired(_) => 410,
        }
    }
    pub(crate) fn not_found(kind: &'static str, id: &str) -> Self {
        Self::NotFound {
            kind,
            id: id.to_string(),
        }
    }
}

/// `GET /chat/threads` query.
#[derive(Debug, Clone, Default)]
pub struct ChatThreadQuery {
    pub q: Option<String>,
    pub status: Option<ChatThreadStatus>,
    pub before: Option<String>,
    pub limit: Option<u32>,
}

/// `GET /chat/threads/{t}/messages` query (`before_seq` and `after_seq` are exclusive).
#[derive(Debug, Clone, Default)]
pub struct ChatMessageQuery {
    pub before_seq: Option<u64>,
    pub after_seq: Option<u64>,
    pub limit: Option<u32>,
}

/// Thread creation result: `created=false` means an idempotent replay (API 200 instead of 201).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatThreadCreated {
    pub thread: ChatThread,
    pub created: bool,
}

/// Message post result: `created=false` means a `client_message_id` replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessagePosted {
    pub response: ChatPostMessageResponse,
    pub created: bool,
}

// ---- shared helpers ----

/// Fixed-width RFC3339 UTC with milliseconds, so string order equals time order.
pub(crate) fn chat_ts(now: OffsetDateTime) -> String {
    let t = now.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

pub(crate) fn enum_str<T: Serialize>(v: &T) -> Result<String, ChatError> {
    match serde_json::to_value(v)? {
        Value::String(s) => Ok(s),
        other => Err(ChatError::Store(StoreError::Invalid(format!(
            "enum did not encode as string: {other}"
        )))),
    }
}

pub(crate) fn enum_from<T: DeserializeOwned>(s: &str) -> Result<T, ChatError> {
    serde_json::from_value(Value::String(s.to_string()))
        .map_err(|e| ChatError::Store(StoreError::Invalid(format!("stored enum {s:?}: {e}"))))
}

fn hash_json(v: &Value) -> String {
    let mut h = Sha256::new();
    h.update(v.to_string().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn check_key(name: &str, key: &str) -> Result<(), ChatError> {
    if key.trim().is_empty() || key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
        return Err(ChatError::Invalid(format!(
            "{name} must be 1..={CHAT_CLIENT_KEY_MAX_BYTES} bytes and not blank"
        )));
    }
    Ok(())
}

fn check_title(title: &str) -> Result<(), ChatError> {
    if title.trim().is_empty() || title.chars().count() > CHAT_TITLE_MAX_CHARS {
        return Err(ChatError::Invalid(format!(
            "title must be 1..={CHAT_TITLE_MAX_CHARS} characters and not blank"
        )));
    }
    Ok(())
}

pub(crate) fn page_limit(limit: Option<u32>, default: u32, max: u32) -> Result<u32, ChatError> {
    match limit {
        None => Ok(default),
        Some(n) if (1..=max).contains(&n) => Ok(n),
        Some(n) => Err(ChatError::BadRequest(format!(
            "limit {n} must be 1..={max}"
        ))),
    }
}

/// D1: the query is matched literally. Each whitespace-separated word becomes a quoted FTS5
/// string (embedded `"` doubled), so operators (`OR`, `NEAR`, `-`, `*`, `^`, `col:`) are words.
pub fn chat_fts_literal(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

pub(crate) const THREAD_SELECT: &str = "SELECT t.id,t.kind,t.title,t.project_id,t.status,t.queue_paused,\
 t.revision,t.created_at,t.updated_at,\
 (SELECT r.run_id FROM chat_runs r WHERE r.thread_id=t.id AND r.state IN ('running','stopping')),\
 (SELECT COUNT(*) FROM chat_messages m WHERE m.thread_id=t.id AND m.role='user' AND m.state='queued') \
 FROM chat_threads t";

struct ThreadRaw {
    id: String,
    kind: String,
    title: String,
    project_id: Option<String>,
    status: String,
    queue_paused: bool,
    revision: i64,
    created_at: String,
    updated_at: String,
    active_run_id: Option<String>,
    queued_count: i64,
}

fn thread_raw(row: &Row<'_>) -> rusqlite::Result<ThreadRaw> {
    Ok(ThreadRaw {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        project_id: row.get(3)?,
        status: row.get(4)?,
        queue_paused: row.get(5)?,
        revision: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        active_run_id: row.get(9)?,
        queued_count: row.get(10)?,
    })
}

fn thread_from_raw(r: ThreadRaw) -> Result<ChatThread, ChatError> {
    Ok(ChatThread {
        kind: enum_from(&r.kind)?,
        status: enum_from(&r.status)?,
        id: r.id,
        title: r.title,
        project_id: r.project_id,
        queue_paused: r.queue_paused,
        active_run_id: r.active_run_id,
        queued_count: u32::try_from(r.queued_count).unwrap_or(u32::MAX),
        revision: u64::try_from(r.revision).unwrap_or(0),
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

pub(crate) fn thread_get_conn(
    conn: &Connection,
    id: &str,
) -> Result<Option<ChatThread>, ChatError> {
    let raw = conn
        .query_row(
            &format!("{THREAD_SELECT} WHERE t.id=?1"),
            params![id],
            thread_raw,
        )
        .optional()?;
    raw.map(thread_from_raw).transpose()
}

pub(crate) fn thread_require(conn: &Connection, id: &str) -> Result<ChatThread, ChatError> {
    thread_get_conn(conn, id)?.ok_or_else(|| ChatError::not_found("thread", id))
}

pub(crate) const MESSAGE_SELECT: &str = "SELECT id,thread_id,seq,role,text,state,client_message_id,\
 reply_to_id,run_id,metadata_json,created_at,updated_at FROM chat_messages";

pub(crate) struct MessageRaw {
    id: String,
    thread_id: String,
    seq: i64,
    role: String,
    text: String,
    state: String,
    client_message_id: Option<String>,
    reply_to_id: Option<String>,
    run_id: Option<String>,
    metadata_json: String,
    created_at: String,
    updated_at: String,
}

pub(crate) fn message_raw(row: &Row<'_>) -> rusqlite::Result<MessageRaw> {
    Ok(MessageRaw {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        seq: row.get(2)?,
        role: row.get(3)?,
        text: row.get(4)?,
        state: row.get(5)?,
        client_message_id: row.get(6)?,
        reply_to_id: row.get(7)?,
        run_id: row.get(8)?,
        metadata_json: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

/// metadata_json keys used here: `mode`, `interrupt`, `attachment_ids` (ordered), `cards`.
/// Other keys (legacy provenance etc.) are preserved and ignored.
pub(crate) fn message_from_raw(conn: &Connection, r: MessageRaw) -> Result<ChatMessage, ChatError> {
    let meta: Value = serde_json::from_str(&r.metadata_json).unwrap_or(Value::Null);
    let attachment_ids: Vec<String> = match meta.get("attachment_ids") {
        Some(v) => serde_json::from_value(v.clone())?,
        None => {
            let mut stmt = conn.prepare_cached(
                "SELECT attachment_id FROM chat_attachment_refs WHERE owner_kind='message' \
                 AND owner_id=?1 ORDER BY created_at,attachment_id",
            )?;
            stmt.query_map(params![r.id], |row| row.get(0))?
                .collect::<Result<_, _>>()?
        }
    };
    let cards: Vec<ChatCard> = match meta.get("cards") {
        Some(v) => serde_json::from_value(v.clone())?,
        None => Vec::new(),
    };
    Ok(ChatMessage {
        role: enum_from(&r.role)?,
        state: enum_from(&r.state)?,
        id: r.id,
        thread_id: r.thread_id,
        seq: u64::try_from(r.seq).unwrap_or(0),
        text: r.text,
        client_message_id: r.client_message_id,
        reply_to_id: r.reply_to_id,
        run_id: r.run_id,
        attachment_ids,
        cards,
        created_at: r.created_at,
        updated_at: r.updated_at,
    })
}

pub(crate) fn message_get_conn(
    conn: &Connection,
    thread_id: &str,
    id: &str,
) -> Result<Option<ChatMessage>, ChatError> {
    let raw = conn
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE id=?1 AND thread_id=?2"),
            params![id, thread_id],
            message_raw,
        )
        .optional()?;
    raw.map(|r| message_from_raw(conn, r)).transpose()
}

pub(crate) fn message_require(
    conn: &Connection,
    thread_id: &str,
    id: &str,
) -> Result<ChatMessage, ChatError> {
    message_get_conn(conn, thread_id, id)?.ok_or_else(|| ChatError::not_found("message", id))
}

/// Queued user messages in claim order: interrupt messages first, then seq (D2).
pub(crate) fn queue_order(conn: &Connection, thread_id: &str) -> Result<Vec<String>, ChatError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM chat_messages WHERE thread_id=?1 AND role='user' AND state='queued' \
         ORDER BY CASE WHEN json_extract(metadata_json,'$.interrupt')=1 THEN 0 ELSE 1 END, seq",
    )?;
    let ids = stmt
        .query_map(params![thread_id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(ids)
}

/// Appends one chat_events row (the SSE source of truth) and returns its id.
pub(crate) fn append_event(
    conn: &Connection,
    thread_id: &str,
    run_id: Option<&str>,
    message_id: Option<&str>,
    event_type: ChatEventType,
    data: &ChatEventData,
    now: &str,
) -> Result<i64, ChatError> {
    conn.execute(
        "INSERT INTO chat_events(thread_id,run_id,message_id,type,payload_json,created_at) \
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            thread_id,
            run_id,
            message_id,
            enum_str(&event_type)?,
            serde_json::to_string(data)?,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn emit_message(
    conn: &Connection,
    m: &ChatMessage,
    now: &str,
) -> Result<i64, ChatError> {
    append_event(
        conn,
        &m.thread_id,
        m.run_id.as_deref(),
        Some(&m.id),
        ChatEventType::Message,
        &ChatEventData::Message(ChatMessageData { message: m.clone() }),
        now,
    )
}

pub(crate) fn emit_thread(conn: &Connection, thread_id: &str, now: &str) -> Result<i64, ChatError> {
    let thread = thread_require(conn, thread_id)?;
    append_event(
        conn,
        thread_id,
        None,
        None,
        ChatEventType::Thread,
        &ChatEventData::Thread(ChatThreadData { thread }),
        now,
    )
}

pub(crate) fn emit_queue(conn: &Connection, thread_id: &str, now: &str) -> Result<i64, ChatError> {
    let paused: bool = conn.query_row(
        "SELECT queue_paused FROM chat_threads WHERE id=?1",
        params![thread_id],
        |row| row.get(0),
    )?;
    let message_ids = queue_order(conn, thread_id)?;
    append_event(
        conn,
        thread_id,
        None,
        None,
        ChatEventType::Queue,
        &ChatEventData::Queue(ChatQueueData {
            message_ids,
            paused,
        }),
        now,
    )
}

pub(crate) fn touch_thread(conn: &Connection, thread_id: &str, now: &str) -> Result<(), ChatError> {
    conn.execute(
        "UPDATE chat_threads SET updated_at=?2 WHERE id=?1",
        params![thread_id, now],
    )?;
    Ok(())
}

pub(crate) fn writer(
    store: &SqliteStore,
) -> Result<std::sync::MutexGuard<'_, Connection>, ChatError> {
    Ok(store.lock()?)
}

pub(crate) fn immediate(conn: &mut Connection) -> Result<rusqlite::Transaction<'_>, ChatError> {
    Ok(conn.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

fn parse_thread_cursor(s: &str) -> Result<(String, String), ChatError> {
    match s.split_once('|') {
        Some((at, id)) if !at.is_empty() && !id.is_empty() => Ok((at.to_string(), id.to_string())),
        _ => Err(ChatError::BadRequest(format!(
            "invalid thread cursor {s:?}"
        ))),
    }
}

impl SqliteStore {
    /// D2 `POST /chat/threads`: idempotent on `(scope_id, client_thread_id)` via
    /// `chat_client_requests`. The same key with the same title/project returns the same thread;
    /// different content is a conflict. kind is always `human`.
    pub fn chat_thread_create(
        &self,
        scope_id: &str,
        req: &ChatCreateThreadRequest,
        now: OffsetDateTime,
    ) -> Result<ChatThreadCreated, ChatError> {
        check_key("client_thread_id", &req.client_thread_id)?;
        check_title(&req.title)?;
        let hash = hash_json(&json!({"title": req.title, "project_id": req.project_id}));
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT request_hash,result_id FROM chat_client_requests \
                 WHERE kind='thread' AND scope_id=?1 AND key=?2",
                params![scope_id, req.client_thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((prev_hash, thread_id)) = existing {
            if prev_hash != hash {
                return Err(ChatError::Conflict(format!(
                    "client_thread_id {} was used with different content",
                    req.client_thread_id
                )));
            }
            let thread = thread_require(&tx, &thread_id)?;
            return Ok(ChatThreadCreated {
                thread,
                created: false,
            });
        }
        if let Some(project_id) = &req.project_id {
            let found: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                params![project_id],
                |row| row.get(0),
            )?;
            if !found {
                return Err(ChatError::Invalid(format!(
                    "project {project_id} not found"
                )));
            }
        }
        let id = Ulid::new().to_string();
        tx.execute(
            "INSERT INTO chat_threads(id,kind,title,project_id,status,created_at,updated_at) \
             VALUES(?1,'human',?2,?3,'open',?4,?4)",
            params![id, req.title, req.project_id, at],
        )?;
        tx.execute(
            "INSERT INTO chat_client_requests(kind,scope_id,key,request_hash,result_id,created_at) \
             VALUES('thread',?1,?2,?3,?4,?5)",
            params![scope_id, req.client_thread_id, hash, id, at],
        )?;
        emit_thread(&tx, &id, &at)?;
        let thread = thread_require(&tx, &id)?;
        tx.commit()?;
        Ok(ChatThreadCreated {
            thread,
            created: true,
        })
    }

    pub fn chat_thread_get(&self, id: &str) -> Result<Option<ChatThread>, ChatError> {
        read_tx(self, |conn| thread_get_conn(conn, id))
    }

    /// D2 `GET /chat/threads/{t}`: thread, active run and the last event id in one read tx.
    pub fn chat_thread_detail(&self, id: &str) -> Result<ChatThreadDetailResponse, ChatError> {
        read_tx(self, |conn| {
            let thread = thread_require(conn, id)?;
            let active_run = match &thread.active_run_id {
                Some(run_id) => super::run_store::run_get_conn(conn, id, run_id)?,
                None => None,
            };
            let last: i64 = conn.query_row(
                "SELECT COALESCE(MAX(id),0) FROM chat_events WHERE thread_id=?1",
                params![id],
                |row| row.get(0),
            )?;
            Ok(ChatThreadDetailResponse {
                thread,
                active_run,
                last_event_id: last.to_string(),
            })
        })
    }

    /// Summary written by a CoS worker checkpoint. The delivery cursor remains
    /// independent: the caller must still claim queued inputs through `chat_run_claim_next`.
    pub fn chat_thread_summary(&self, id: &str) -> Result<(String, u64), ChatError> {
        read_tx(self, |conn| {
            thread_require(conn, id)?;
            let (summary, through): (String, i64) = conn.query_row(
                "SELECT summary,summary_through_seq FROM chat_threads WHERE id=?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((summary, u64::try_from(through).unwrap_or(0)))
        })
    }

    /// D1/D2 `GET /chat/threads`: `(updated_at, id)` descending keyset pages. A non-blank `q`
    /// searches titles and message text through FTS5 with the query taken literally; hits are
    /// deduplicated per thread. A blank `q` is the plain list.
    pub fn chat_thread_list(
        &self,
        query: &ChatThreadQuery,
    ) -> Result<ChatThreadListResponse, ChatError> {
        let limit = page_limit(query.limit, CHAT_THREAD_PAGE_DEFAULT, CHAT_THREAD_PAGE_MAX)?;
        let cursor = query
            .before
            .as_deref()
            .map(parse_thread_cursor)
            .transpose()?;
        let status = query.status.as_ref().map(enum_str).transpose()?;
        let fts = query.q.as_deref().and_then(chat_fts_literal);
        read_tx(self, |conn| {
            let mut sql = format!("{THREAD_SELECT} WHERE 1=1");
            let mut args: Vec<Value> = Vec::new();
            if let Some(s) = &status {
                args.push(Value::String(s.clone()));
                sql.push_str(&format!(" AND t.status=?{}", args.len()));
            }
            if let Some((at, id)) = &cursor {
                args.push(Value::String(at.clone()));
                let a = args.len();
                args.push(Value::String(id.clone()));
                let b = args.len();
                sql.push_str(&format!(
                    " AND (t.updated_at<?{a} OR (t.updated_at=?{a} AND t.id<?{b}))"
                ));
            }
            if let Some(m) = &fts {
                args.push(Value::String(m.clone()));
                sql.push_str(&format!(
                    " AND t.id IN (SELECT thread_id FROM chat_search WHERE chat_search MATCH ?{})",
                    args.len()
                ));
            }
            args.push(json!(i64::from(limit) + 1));
            sql.push_str(&format!(
                " ORDER BY t.updated_at DESC, t.id DESC LIMIT ?{}",
                args.len()
            ));
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<rusqlite::types::Value> = args
                .into_iter()
                .map(|v| match v {
                    Value::String(s) => rusqlite::types::Value::Text(s),
                    Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0)),
                    _ => rusqlite::types::Value::Null,
                })
                .collect();
            let raws = stmt
                .query_map(rusqlite::params_from_iter(params), thread_raw)?
                .collect::<Result<Vec<_>, _>>()?;
            let mut items = raws
                .into_iter()
                .map(thread_from_raw)
                .collect::<Result<Vec<_>, _>>()?;
            let next_cursor = if items.len() > limit as usize {
                items.truncate(limit as usize);
                items.last().map(|t| format!("{}|{}", t.updated_at, t.id))
            } else {
                None
            };
            Ok(ChatThreadListResponse { items, next_cursor })
        })
    }

    /// D2 `PATCH /chat/threads/{t}`: rename and/or archive/unarchive under `expected_revision`.
    /// Archiving the inbox thread, or a thread with an active run or queued input, is a conflict.
    pub fn chat_thread_patch(
        &self,
        id: &str,
        req: &ChatPatchThreadRequest,
        now: OffsetDateTime,
    ) -> Result<ChatThread, ChatError> {
        if req.title.is_none() && req.status.is_none() {
            return Err(ChatError::Invalid("title or status is required".into()));
        }
        if let Some(title) = &req.title {
            check_title(title)?;
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let thread = thread_require(&tx, id)?;
        if thread.revision != req.expected_revision {
            return Err(ChatError::Conflict(format!(
                "thread {id} revision is {}, expected {}",
                thread.revision, req.expected_revision
            )));
        }
        if let Some(title) = &req.title {
            tx.execute(
                "UPDATE chat_threads SET title=?2 WHERE id=?1",
                params![id, title],
            )?;
        }
        match req.status {
            Some(ChatThreadStatus::Archived) if thread.status != ChatThreadStatus::Archived => {
                if thread.kind == ChatThreadKind::Inbox {
                    return Err(ChatError::Conflict(
                        "the inbox thread cannot be archived".into(),
                    ));
                }
                if thread.active_run_id.is_some() || thread.queued_count > 0 {
                    return Err(ChatError::Conflict(format!(
                        "thread {id} still has a run or queued messages"
                    )));
                }
                tx.execute(
                    "UPDATE chat_threads SET status='archived',archived_at=?2 WHERE id=?1",
                    params![id, at],
                )?;
            }
            Some(ChatThreadStatus::Open) if thread.status != ChatThreadStatus::Open => {
                tx.execute(
                    "UPDATE chat_threads SET status='open',archived_at=NULL WHERE id=?1",
                    params![id],
                )?;
            }
            _ => {}
        }
        tx.execute(
            "UPDATE chat_threads SET revision=revision+1,updated_at=?2 WHERE id=?1",
            params![id, at],
        )?;
        emit_thread(&tx, id, &at)?;
        let thread = thread_require(&tx, id)?;
        tx.commit()?;
        Ok(thread)
    }

    /// D2 `POST /chat/threads/{t}/resume-queue`: clears `queue_paused` under `expected_revision`.
    pub fn chat_thread_resume_queue(
        &self,
        id: &str,
        expected_revision: u64,
        now: OffsetDateTime,
    ) -> Result<ChatThread, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let thread = thread_require(&tx, id)?;
        if thread.revision != expected_revision {
            return Err(ChatError::Conflict(format!(
                "thread {id} revision is {}, expected {expected_revision}",
                thread.revision
            )));
        }
        if thread.queue_paused {
            set_paused(&tx, id, false, &at)?;
        }
        let thread = thread_require(&tx, id)?;
        tx.commit()?;
        Ok(thread)
    }

    /// D2 `POST /chat/threads/{t}/messages`. Validation, seq allocation, queue insertion and the
    /// chat_events rows are one transaction. A `client_message_id` replay with the same text,
    /// attachments, mode and reply target returns the current state of the same message; any
    /// difference is a conflict. `mode=interrupt` asks the active run to stop and puts this
    /// message ahead of the queue without touching `queue_paused`.
    pub fn chat_message_post(
        &self,
        thread_id: &str,
        req: &ChatPostMessageRequest,
        now: OffsetDateTime,
    ) -> Result<ChatMessagePosted, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let posted = message_post_conn(&tx, thread_id, req, now)?;
        tx.commit()?;
        Ok(posted)
    }

    /// D2 `GET /chat/threads/{t}/messages`: items in ascending seq, with `snapshot_event_id` read
    /// in the same read transaction (so SSE can continue from it without a gap).
    pub fn chat_message_list(
        &self,
        thread_id: &str,
        query: &ChatMessageQuery,
    ) -> Result<ChatMessageListResponse, ChatError> {
        if query.before_seq.is_some() && query.after_seq.is_some() {
            return Err(ChatError::BadRequest(
                "before_seq and after_seq are exclusive".into(),
            ));
        }
        let limit = page_limit(
            query.limit,
            CHAT_MESSAGE_PAGE_DEFAULT,
            CHAT_MESSAGE_PAGE_MAX,
        )?;
        let fetch = i64::from(limit) + 1;
        read_tx(self, |conn| {
            thread_require(conn, thread_id)?;
            let snapshot: i64 = conn.query_row(
                "SELECT COALESCE(MAX(id),0) FROM chat_events WHERE thread_id=?1",
                params![thread_id],
                |row| row.get(0),
            )?;
            let (sql, bound) = match (query.before_seq, query.after_seq) {
                (_, Some(after)) => (
                    format!(
                        "{MESSAGE_SELECT} WHERE thread_id=?1 AND seq>?2 ORDER BY seq ASC LIMIT ?3"
                    ),
                    after as i64,
                ),
                (Some(before), None) => (
                    format!(
                        "{MESSAGE_SELECT} WHERE thread_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3"
                    ),
                    before as i64,
                ),
                (None, None) => (
                    format!(
                        "{MESSAGE_SELECT} WHERE thread_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT ?3"
                    ),
                    i64::MAX,
                ),
            };
            let mut stmt = conn.prepare(&sql)?;
            let raws = stmt
                .query_map(params![thread_id, bound, fetch], message_raw)?
                .collect::<Result<Vec<_>, _>>()?;
            let more = raws.len() > limit as usize;
            let mut items = raws
                .into_iter()
                .take(limit as usize)
                .map(|r| message_from_raw(conn, r))
                .collect::<Result<Vec<_>, _>>()?;
            let (mut next_before_seq, mut next_after_seq) = (None, None);
            if query.after_seq.is_some() {
                if more {
                    next_after_seq = items.last().map(|m| m.seq);
                }
            } else {
                items.reverse();
                if more {
                    next_before_seq = items.first().map(|m| m.seq);
                }
            }
            Ok(ChatMessageListResponse {
                items,
                next_before_seq,
                next_after_seq,
                snapshot_event_id: snapshot.to_string(),
            })
        })
    }

    /// D2 `DELETE /chat/threads/{t}/messages/{m}`: only a queued user input can be cancelled.
    /// Cancelling an already cancelled message returns it unchanged.
    pub fn chat_message_cancel(
        &self,
        thread_id: &str,
        message_id: &str,
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        thread_require(&tx, thread_id)?;
        let message = message_require(&tx, thread_id, message_id)?;
        if message.state == ChatMessageState::Cancelled {
            return Ok(message);
        }
        if message.role != ChatMessageRole::User || message.state != ChatMessageState::Queued {
            return Err(ChatError::Conflict(format!(
                "message {message_id} is not a queued user input"
            )));
        }
        tx.execute(
            "UPDATE chat_messages SET state='cancelled',updated_at=?2 WHERE id=?1",
            params![message_id, at],
        )?;
        touch_thread(&tx, thread_id, &at)?;
        let message = message_require(&tx, thread_id, message_id)?;
        emit_message(&tx, &message, &at)?;
        emit_queue(&tx, thread_id, &at)?;
        tx.commit()?;
        Ok(message)
    }

    /// One message of a thread by id (404 when the thread or the message is missing).
    pub fn chat_message_get(
        &self,
        thread_id: &str,
        message_id: &str,
    ) -> Result<ChatMessage, ChatError> {
        let conn = writer(self)?;
        thread_require(&conn, thread_id)?;
        message_require(&conn, thread_id, message_id)
    }

    /// Adds a completed system message (cards such as task/operation notices) to a thread.
    pub fn chat_system_message_add(
        &self,
        thread_id: &str,
        text: &str,
        cards: &[ChatCard],
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        self.chat_system_message_add_keyed(thread_id, None, text, cards, now)
    }

    /// A system notice whose stable key prevents duplicate cards when daemon
    /// recovery is retried after a crash.
    pub fn chat_system_message_add_once(
        &self,
        thread_id: &str,
        key: &str,
        text: &str,
        cards: &[ChatCard],
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        if key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
            return Err(ChatError::TooLarge("system message key".into()));
        }
        self.chat_system_message_add_keyed(thread_id, Some(key), text, cards, now)
    }

    /// ADR 2026-10-07-cos-inbox-thread-conversation D1: a completed assistant message written by
    /// the daemon (the deterministic digest of one triage run). The stable key makes the write
    /// idempotent across ticks and restarts; `run_id` records which run the digest describes.
    pub fn chat_assistant_message_add_once(
        &self,
        thread_id: &str,
        key: &str,
        text: &str,
        cards: &[ChatCard],
        run_id: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        if key.len() > CHAT_CLIENT_KEY_MAX_BYTES {
            return Err(ChatError::TooLarge("assistant message key".into()));
        }
        self.chat_daemon_message_add_keyed(
            thread_id,
            ChatMessageRole::Assistant,
            Some(key),
            text,
            cards,
            run_id,
            now,
        )
    }

    fn chat_system_message_add_keyed(
        &self,
        thread_id: &str,
        key: Option<&str>,
        text: &str,
        cards: &[ChatCard],
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        self.chat_daemon_message_add_keyed(
            thread_id,
            ChatMessageRole::System,
            key,
            text,
            cards,
            None,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn chat_daemon_message_add_keyed(
        &self,
        thread_id: &str,
        role: ChatMessageRole,
        key: Option<&str>,
        text: &str,
        cards: &[ChatCard],
        run_id: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        if text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES {
            return Err(ChatError::TooLarge("system message text".into()));
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        thread_require(&tx, thread_id)?;
        if let Some(key) = key {
            let existing = tx
                .query_row(
                    &format!("{MESSAGE_SELECT} WHERE thread_id=?1 AND client_message_id=?2"),
                    params![thread_id, key],
                    message_raw,
                )
                .optional()?;
            if let Some(raw) = existing {
                let message = message_from_raw(&tx, raw)?;
                if message.role == role && message.text == text && message.cards == cards {
                    return Ok(message);
                }
                return Err(ChatError::Conflict(format!(
                    "system message key {key} reused"
                )));
            }
        }
        let meta = json!({ "cards": cards });
        let id = insert_message(
            &tx,
            NewMessage {
                thread_id,
                role,
                text,
                state: ChatMessageState::Completed,
                client_message_id: key,
                reply_to_id: None,
                run_id,
                metadata: &meta,
            },
            &at,
        )?;
        let message = message_require(&tx, thread_id, &id)?;
        emit_message(&tx, &message, &at)?;
        tx.commit()?;
        Ok(message)
    }
}

/// `chat_message_post` の本体（呼び出し側の write transaction の中で走る）。ADR 2026-10-07-cos-live-fixes D4 の
/// takeover は、run の終端の確認・続きの message の投入・run の interrupted 化を 1 つの transaction で行うために使う。
pub(crate) fn message_post_conn(
    tx: &Connection,
    thread_id: &str,
    req: &ChatPostMessageRequest,
    now: OffsetDateTime,
) -> Result<ChatMessagePosted, ChatError> {
    check_key("client_message_id", &req.client_message_id)?;
    if req.text.len() > CHAT_MESSAGE_TEXT_MAX_BYTES {
        return Err(ChatError::TooLarge(format!(
            "text is {} bytes (max {CHAT_MESSAGE_TEXT_MAX_BYTES})",
            req.text.len()
        )));
    }
    if req.attachment_ids.len() > CHAT_MESSAGE_ATTACHMENTS_MAX {
        return Err(ChatError::TooLarge(format!(
            "{} attachments (max {CHAT_MESSAGE_ATTACHMENTS_MAX})",
            req.attachment_ids.len()
        )));
    }
    if req.text.trim().is_empty() && req.attachment_ids.is_empty() {
        return Err(ChatError::Invalid("blank text needs an attachment".into()));
    }
    let mut seen = std::collections::BTreeSet::new();
    if !req.attachment_ids.iter().all(|a| seen.insert(a.as_str())) {
        return Err(ChatError::Invalid("duplicate attachment id".into()));
    }
    let interrupt = req.mode == ChatSendMode::Interrupt;
    let mode = enum_str(&req.mode)?;
    let at = chat_ts(now);
    let thread = thread_require(tx, thread_id)?;
    let existing = tx
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE thread_id=?1 AND client_message_id=?2"),
            params![thread_id, req.client_message_id],
            message_raw,
        )
        .optional()?;
    if let Some(raw) = existing {
        let meta: Value = serde_json::from_str(&raw.metadata_json).unwrap_or(Value::Null);
        let same_mode = meta.get("mode").and_then(Value::as_str) == Some(mode.as_str());
        let message = message_from_raw(tx, raw)?;
        if message.text != req.text
            || message.attachment_ids != req.attachment_ids
            || message.reply_to_id != req.reply_to_id
            || !same_mode
        {
            return Err(ChatError::Conflict(format!(
                "client_message_id {} was used with different content",
                req.client_message_id
            )));
        }
        let response = post_response(tx, message)?;
        return Ok(ChatMessagePosted {
            response,
            created: false,
        });
    }
    if thread.status == ChatThreadStatus::Archived {
        return Err(ChatError::Conflict(format!(
            "thread {thread_id} is archived"
        )));
    }
    if thread.queued_count as usize >= CHAT_QUEUE_MAX {
        return Err(ChatError::QueueFull {
            limit: CHAT_QUEUE_MAX,
        });
    }
    if let Some(reply) = &req.reply_to_id
        && message_get_conn(tx, thread_id, reply)?.is_none()
    {
        return Err(ChatError::Invalid(format!(
            "reply_to_id {reply} is not a message of this thread"
        )));
    }
    for a in &req.attachment_ids {
        let ok: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_attachments WHERE id=?1 AND thread_id=?2 \
             AND state='ready')",
            params![a, thread_id],
            |row| row.get(0),
        )?;
        if !ok {
            return Err(ChatError::Invalid(format!(
                "attachment {a} is not a ready attachment of this thread"
            )));
        }
    }
    let meta = json!({"mode": mode, "interrupt": interrupt, "attachment_ids": req.attachment_ids});
    let id = insert_message(
        tx,
        NewMessage {
            thread_id,
            role: ChatMessageRole::User,
            text: &req.text,
            state: ChatMessageState::Queued,
            client_message_id: Some(&req.client_message_id),
            reply_to_id: req.reply_to_id.as_deref(),
            run_id: None,
            metadata: &meta,
        },
        &at,
    )?;
    for a in &req.attachment_ids {
        tx.execute(
            "INSERT INTO chat_attachment_refs(attachment_id,owner_kind,owner_id,created_at) \
             VALUES(?1,'message',?2,?3)",
            params![a, id, at],
        )?;
    }
    let message = message_require(tx, thread_id, &id)?;
    emit_message(tx, &message, &at)?;
    if req.resume_queue && thread.queue_paused {
        set_paused(tx, thread_id, false, &at)?;
    }
    if interrupt && let Some(run_id) = &thread.active_run_id {
        super::run_store::request_stop(tx, thread_id, run_id, "interrupt", &at)?;
    }
    emit_queue(tx, thread_id, &at)?;
    let response = post_response(tx, message)?;
    Ok(ChatMessagePosted {
        response,
        created: true,
    })
}

fn post_response(
    conn: &Connection,
    message: ChatMessage,
) -> Result<ChatPostMessageResponse, ChatError> {
    let order = queue_order(conn, &message.thread_id)?;
    let queue_position = order
        .iter()
        .position(|id| *id == message.id)
        .map(|p| u32::try_from(p + 1).unwrap_or(u32::MAX))
        .unwrap_or(0);
    Ok(ChatPostMessageResponse {
        run_id: message.run_id.clone(),
        message,
        queue_position,
    })
}

pub(crate) fn set_paused(
    conn: &Connection,
    thread_id: &str,
    paused: bool,
    now: &str,
) -> Result<(), ChatError> {
    conn.execute(
        "UPDATE chat_threads SET queue_paused=?2,revision=revision+1,updated_at=?3 WHERE id=?1",
        params![thread_id, paused, now],
    )?;
    emit_thread(conn, thread_id, now)?;
    Ok(())
}

pub(crate) struct NewMessage<'a> {
    pub thread_id: &'a str,
    pub role: ChatMessageRole,
    pub text: &'a str,
    pub state: ChatMessageState,
    pub client_message_id: Option<&'a str>,
    pub reply_to_id: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub metadata: &'a Value,
}

/// Allocates the next seq (thread `next_seq`) and inserts a message row; returns its id.
pub(crate) fn insert_message(
    conn: &Connection,
    m: NewMessage<'_>,
    now: &str,
) -> Result<String, ChatError> {
    let seq: i64 = conn.query_row(
        "SELECT next_seq FROM chat_threads WHERE id=?1",
        params![m.thread_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "UPDATE chat_threads SET next_seq=next_seq+1,updated_at=?2 WHERE id=?1",
        params![m.thread_id, now],
    )?;
    let id = Ulid::new().to_string();
    conn.execute(
        "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,client_message_id,reply_to_id,\
         run_id,metadata_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)",
        params![
            id,
            m.thread_id,
            seq,
            enum_str(&m.role)?,
            m.text,
            enum_str(&m.state)?,
            m.client_message_id,
            m.reply_to_id,
            m.run_id,
            m.metadata.to_string(),
            now
        ],
    )?;
    Ok(id)
}

/// Runs `f` on a read connection inside one deferred transaction (a consistent snapshot).
pub(crate) fn read_tx<T>(
    store: &SqliteStore,
    f: impl FnOnce(&Connection) -> Result<T, ChatError>,
) -> Result<T, ChatError> {
    let mut out: Option<Result<T, ChatError>> = None;
    store.with_read_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let r = f(&tx);
        tx.finish()?;
        out = Some(r);
        Ok(())
    })?;
    out.unwrap_or_else(|| {
        Err(ChatError::Store(StoreError::Invalid(
            "read tx did not run".into(),
        )))
    })
}
