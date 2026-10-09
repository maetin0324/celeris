//! CoS chat store operations (ADR 2026-10-05 D2): runs, stop/interrupt, event cursor and
//! retention. Run *processes* are cos-run's; these are the DB state transitions it drives.
//!
//! Event retention keeps a per-thread watermark (the largest removed chat_events id) in the
//! existing `feed_cursor` table under `chat_events_purged:<thread_id>`. A cursor below it is
//! expired (API 410); a cursor above the largest id ever allocated is a future cursor (API 400).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Value, json};
use time::OffsetDateTime;

use super::store::{
    ChatError, chat_ts, emit_message, emit_queue, enum_from, enum_str, immediate, message_require,
    page_limit, queue_order, read_tx, thread_require, touch_thread, writer,
};
use super::*;
use crate::store::SqliteStore;

/// D2: run events page limit (default / max).
pub const CHAT_EVENT_PAGE_DEFAULT: u32 = 100;
pub const CHAT_EVENT_PAGE_MAX: u32 = 500;
/// D2: retention of text/tool detail events of finished runs (days).
pub const CHAT_EVENT_RETENTION_DAYS: i64 = 30;
/// D2: tool detail limit after redaction (bytes).
pub const CHAT_TOOL_DETAIL_MAX_BYTES: usize = 4 * 1024;

const PURGED_PREFIX: &str = "chat_events_purged:";

/// `GET /chat/threads/{t}/runs/{r}/events` and SSE replay query.
#[derive(Debug, Clone, Default)]
pub struct ChatEventQuery {
    pub after: Option<String>,
    pub run_id: Option<String>,
    pub limit: Option<u32>,
}

/// Stop result: `accepted=false` is a replay against a finished run (API 200 instead of 202).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatStopOutcome {
    pub response: ChatStopResponse,
    pub accepted: bool,
}

const RUN_SELECT: &str = "SELECT run_id,thread_id,input_message_id,output_message_id,state,reason,\
 resolved_config_json,started_at,finished_at,usage_json,skill_reads,first_output_at FROM chat_runs";

fn run_row(row: &Row<'_>) -> rusqlite::Result<(ChatRunRaw, String)> {
    Ok((
        ChatRunRaw {
            run_id: row.get(0)?,
            thread_id: row.get(1)?,
            input_message_id: row.get(2)?,
            output_message_id: row.get(3)?,
            state: row.get(4)?,
            reason: row.get(5)?,
            started_at: row.get(7)?,
            finished_at: row.get(8)?,
            usage_json: row.get(9)?,
            skill_reads: row.get(10)?,
            first_output_at: row.get(11)?,
        },
        row.get(6)?,
    ))
}

struct ChatRunRaw {
    run_id: String,
    thread_id: String,
    input_message_id: String,
    output_message_id: Option<String>,
    state: String,
    reason: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    usage_json: Option<String>,
    skill_reads: Option<u64>,
    first_output_at: Option<String>,
}

fn run_from_raw((r, config): (ChatRunRaw, String)) -> Result<ChatRun, ChatError> {
    let cfg: Value = serde_json::from_str(&config).unwrap_or(Value::Null);
    let s = |k: &str| cfg.get(k).and_then(Value::as_str).map(str::to_string);
    let session_mode = match cfg.get("session_mode").and_then(Value::as_str) {
        Some(m) => Some(enum_from(m)?),
        None => None,
    };
    Ok(ChatRun {
        state: enum_from(&r.state)?,
        id: r.run_id,
        thread_id: r.thread_id,
        input_message_id: r.input_message_id,
        output_message_id: r.output_message_id,
        reason: r.reason,
        harness: s("harness"),
        llm_source: s("llm_source"),
        provider: s("provider"),
        account_id: s("account_id"),
        model: s("model"),
        tier: s("tier"),
        session_mode,
        usage: r
            .usage_json
            .as_deref()
            .map(serde_json::from_str::<crate::Usage>)
            .transpose()
            .map_err(|e| ChatError::Invalid(format!("invalid chat usage: {e}")))?
            .map(Box::new),
        skill_reads: r.skill_reads,
        latency_ms: elapsed_ms(r.started_at.as_deref(), r.finished_at.as_deref()),
        time_to_first_output_ms: elapsed_ms(r.started_at.as_deref(), r.first_output_at.as_deref()),
        first_output_at: r.first_output_at,
        started_at: r.started_at,
        finished_at: r.finished_at,
    })
}

fn elapsed_ms(start: Option<&str>, end: Option<&str>) -> Option<u64> {
    let parse = |s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok();
    let elapsed = parse(end?)? - parse(start?)?;
    u64::try_from(elapsed.whole_milliseconds()).ok()
}

pub(crate) fn run_get_conn(
    conn: &Connection,
    thread_id: &str,
    run_id: &str,
) -> Result<Option<ChatRun>, ChatError> {
    conn.query_row(
        &format!("{RUN_SELECT} WHERE run_id=?1 AND thread_id=?2"),
        params![run_id, thread_id],
        run_row,
    )
    .optional()?
    .map(run_from_raw)
    .transpose()
}

fn run_by_id(conn: &Connection, run_id: &str) -> Result<ChatRun, ChatError> {
    conn.query_row(
        &format!("{RUN_SELECT} WHERE run_id=?1"),
        params![run_id],
        run_row,
    )
    .optional()?
    .map(run_from_raw)
    .transpose()?
    .ok_or_else(|| ChatError::not_found("run", run_id))
}

pub fn chat_run_state_is_terminal(state: ChatRunState) -> bool {
    matches!(
        state,
        ChatRunState::Completed
            | ChatRunState::Stopped
            | ChatRunState::Failed
            | ChatRunState::Interrupted
    )
}

fn emit_run(conn: &Connection, run: &ChatRun, now: &str) -> Result<i64, ChatError> {
    super::store::append_event(
        conn,
        &run.thread_id,
        Some(&run.id),
        None,
        ChatEventType::Run,
        &ChatEventData::Run(ChatRunData { run: run.clone() }),
        now,
    )
}

/// running → stopping (no-op for any other state). Shared by stop and interrupt.
pub(crate) fn request_stop(
    conn: &Connection,
    thread_id: &str,
    run_id: &str,
    reason: &str,
    now: &str,
) -> Result<(), ChatError> {
    let changed = conn.execute(
        "UPDATE chat_runs SET state='stopping',reason=?3 WHERE run_id=?1 AND thread_id=?2 \
         AND state='running'",
        params![run_id, thread_id, reason],
    )?;
    if changed > 0 {
        let run = run_by_id(conn, run_id)?;
        emit_run(conn, &run, now)?;
    }
    Ok(())
}

/// The run must be live (running/stopping) to accept progress events.
fn live_run(conn: &Connection, run_id: &str) -> Result<ChatRun, ChatError> {
    let run = run_by_id(conn, run_id)?;
    if chat_run_state_is_terminal(run.state) {
        return Err(ChatError::Conflict(format!(
            "run {run_id} is already finished"
        )));
    }
    Ok(run)
}

fn parse_cursor(s: &str) -> Result<i64, ChatError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ChatError::BadRequest(format!("invalid event cursor {s:?}")));
    }
    s.parse::<i64>()
        .map_err(|_| ChatError::BadRequest(format!("invalid event cursor {s:?}")))
}

/// D2 cursor rules: future (beyond the largest allocated id) → BadRequest; below the thread's
/// retention watermark → CursorExpired. `0` is never expired.
fn check_cursor(conn: &Connection, thread_id: &str, after: i64) -> Result<(), ChatError> {
    let allocated: i64 = conn
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='chat_events'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if after > allocated {
        return Err(ChatError::BadRequest(format!(
            "event cursor {after} is in the future"
        )));
    }
    let purged: i64 = conn
        .query_row(
            "SELECT value FROM feed_cursor WHERE name=?1",
            params![format!("{PURGED_PREFIX}{thread_id}")],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if after > 0 && after < purged {
        return Err(ChatError::CursorExpired(after.to_string()));
    }
    Ok(())
}

type EventRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

fn event_from_row(row: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn decode_event(
    (id, thread_id, run_id, message_id, ty, payload, at): EventRow,
) -> Result<ChatEvent, ChatError> {
    let event_type: ChatEventType = enum_from(&ty)?;
    let data = match event_type {
        ChatEventType::Message => ChatEventData::Message(serde_json::from_str(&payload)?),
        ChatEventType::TextDelta => ChatEventData::TextDelta(serde_json::from_str(&payload)?),
        ChatEventType::Status => ChatEventData::Status(serde_json::from_str(&payload)?),
        ChatEventType::Tool => ChatEventData::Tool(serde_json::from_str(&payload)?),
        ChatEventType::Run => ChatEventData::Run(serde_json::from_str(&payload)?),
        ChatEventType::Queue => ChatEventData::Queue(serde_json::from_str(&payload)?),
        ChatEventType::Card => ChatEventData::Card(serde_json::from_str(&payload)?),
        ChatEventType::Thread => ChatEventData::Thread(serde_json::from_str(&payload)?),
    };
    Ok(ChatEvent {
        id: id.to_string(),
        event_type,
        thread_id,
        run_id,
        message_id,
        at,
        data,
    })
}

fn message_state_for(run_state: ChatRunState) -> ChatMessageState {
    match run_state {
        ChatRunState::Completed => ChatMessageState::Completed,
        ChatRunState::Failed => ChatMessageState::Failed,
        _ => ChatMessageState::Interrupted,
    }
}

impl SqliteStore {
    /// D2 FIFO: claims the next queued user input of `thread_id` for `run_id` (the runs row id
    /// cos-run created). Interrupt messages go first and ignore `queue_paused`; other messages
    /// wait while the queue is paused. Returns `None` when the thread already has a live run or
    /// nothing is claimable. The partial UNIQUE index keeps one live run per thread even across
    /// connections.
    pub fn chat_run_claim_next(
        &self,
        thread_id: &str,
        run_id: &str,
        resolved_config: &Value,
        now: OffsetDateTime,
    ) -> Result<Option<ChatRun>, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let thread = thread_require(&tx, thread_id)?;
        if thread.active_run_id.is_some() {
            return Ok(None);
        }
        // An interrupted run with an unresolved external effect needs a
        // human's review. Interrupt priority must not bypass that pause.
        // The review card remains historical after an explicit resume; a
        // still-pending operation blocks even if the queue was unpaused.
        let review_blocked: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM cos_operations WHERE thread_id=?1 AND state='pending') \
             OR EXISTS(SELECT 1 FROM chat_runs WHERE thread_id=?1 \
             AND state='interrupted' AND reason='operation outcome unknown; human review required' \
             AND (SELECT queue_paused FROM chat_threads WHERE id=?1)=1)",
            params![thread_id],
            |row| row.get(0),
        )?;
        if review_blocked {
            return Ok(None);
        }
        let mut candidates = queue_order(&tx, thread_id)?.into_iter();
        let Some(next) = candidates.next() else {
            return Ok(None);
        };
        let input = message_require(&tx, thread_id, &next)?;
        let is_interrupt: bool = tx.query_row(
            "SELECT COALESCE(json_extract(metadata_json,'$.interrupt'),0)=1 FROM chat_messages \
             WHERE id=?1",
            params![input.id],
            |row| row.get(0),
        )?;
        if thread.queue_paused && !is_interrupt {
            return Ok(None);
        }
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_runs WHERE run_id=?1)",
            params![run_id],
            |row| row.get(0),
        )?;
        if taken {
            return Err(ChatError::Conflict(format!("run {run_id} already exists")));
        }
        let output_id = super::store::insert_message(
            &tx,
            super::store::NewMessage {
                thread_id,
                role: ChatMessageRole::Assistant,
                text: "",
                state: ChatMessageState::Running,
                client_message_id: None,
                reply_to_id: Some(&input.id),
                run_id: Some(run_id),
                metadata: &json!({}),
            },
            &at,
        )?;
        tx.execute(
            "INSERT INTO chat_runs(run_id,thread_id,input_message_id,output_message_id,state,\
             resolved_config_json,started_at) VALUES(?1,?2,?3,?4,'running',?5,?6)",
            params![
                run_id,
                thread_id,
                input.id,
                output_id,
                resolved_config.to_string(),
                at
            ],
        )?;
        tx.execute(
            "UPDATE chat_messages SET state='running',run_id=?2,updated_at=?3 WHERE id=?1",
            params![input.id, run_id, at],
        )?;
        let input = message_require(&tx, thread_id, &input.id)?;
        let output = message_require(&tx, thread_id, &output_id)?;
        emit_message(&tx, &input, &at)?;
        emit_message(&tx, &output, &at)?;
        let run = run_by_id(&tx, run_id)?;
        emit_run(&tx, &run, &at)?;
        emit_queue(&tx, thread_id, &at)?;
        tx.commit()?;
        Ok(Some(run))
    }

    /// D2 `GET /chat/threads/{t}/runs/{r}`.
    pub fn chat_run_get(&self, thread_id: &str, run_id: &str) -> Result<ChatRun, ChatError> {
        read_tx(self, |conn| {
            thread_require(conn, thread_id)?;
            run_get_conn(conn, thread_id, run_id)?
                .ok_or_else(|| ChatError::not_found("run", run_id))
        })
    }

    /// Newest runs first; run_id is a stable opaque cursor (ordered by SQLite rowid).
    pub fn chat_run_list(
        &self,
        thread_id: &str,
        before: Option<&str>,
        limit: Option<u32>,
    ) -> Result<ChatRunListResponse, ChatError> {
        read_tx(self, |conn| {
            thread_require(conn, thread_id)?;
            let before_row: Option<i64> = before
                .map(|id| {
                    conn.query_row(
                        "SELECT rowid FROM chat_runs WHERE thread_id=?1 AND run_id=?2",
                        params![thread_id, id],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| ChatError::BadRequest("unknown run cursor".into()))
                })
                .transpose()?;
            let limit = page_limit(limit, 50, 200)?;
            let mut stmt = conn.prepare(&format!("{RUN_SELECT} WHERE thread_id=?1 AND (?2 IS NULL OR rowid<?2) ORDER BY rowid DESC LIMIT ?3"))?;
            let mut items = stmt
                .query_map(params![thread_id, before_row, limit + 1], run_row)?
                .map(|r| run_from_raw(r?))
                .collect::<Result<Vec<_>, ChatError>>()?;
            let more = items.len() > limit as usize;
            items.truncate(limit as usize);
            let next_before = more.then(|| items.last().map(|r| r.id.clone())).flatten();
            Ok(ChatRunListResponse { items, next_before })
        })
    }

    /// Atomically stores telemetry and emits the terminal run event containing it.
    #[allow(clippy::too_many_arguments)]
    pub fn chat_run_finish_with_telemetry(
        &self,
        run_id: &str,
        state: ChatRunState,
        final_text: Option<&str>,
        reason: Option<&str>,
        now: OffsetDateTime,
        usage: Option<&crate::Usage>,
        skill_reads: u64,
        first_output_at: Option<OffsetDateTime>,
    ) -> Result<ChatRun, ChatError> {
        if !chat_run_state_is_terminal(state) {
            return Err(ChatError::Invalid("not a terminal run state".into()));
        }
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let old = run_by_id(&tx, run_id)?;
        if !chat_run_state_is_terminal(old.state) {
            let usage_json = usage
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| ChatError::Invalid(e.to_string()))?;
            tx.execute("UPDATE chat_runs SET usage_json=?2,skill_reads=?3,first_output_at=?4 WHERE run_id=?1",
                params![run_id, usage_json, skill_reads, first_output_at.map(chat_ts)])?;
        }
        let run = finish_conn(&tx, run_id, state, final_text, reason, &chat_ts(now))?;
        tx.commit()?;
        Ok(run)
    }

    /// Appends assistant text to the run's output message and records a `text_delta` whose
    /// offset is the UTF-8 byte length before the append. Returns the event id.
    pub fn chat_run_append_text(
        &self,
        run_id: &str,
        text: &str,
        now: OffsetDateTime,
    ) -> Result<i64, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let run = live_run(&tx, run_id)?;
        let output_id = run
            .output_message_id
            .clone()
            .ok_or_else(|| ChatError::Conflict(format!("run {run_id} has no output message")))?;
        let current = message_require(&tx, &run.thread_id, &output_id)?;
        let mut next = current.text.clone();
        next.push_str(text);
        if next.len() > super::store::CHAT_MESSAGE_TEXT_MAX_BYTES * 16 {
            return Err(ChatError::TooLarge("assistant text".into()));
        }
        tx.execute(
            "UPDATE chat_messages SET text=?2,updated_at=?3 WHERE id=?1",
            params![output_id, next, at],
        )?;
        let id = super::store::append_event(
            &tx,
            &run.thread_id,
            Some(run_id),
            Some(&output_id),
            ChatEventType::TextDelta,
            &ChatEventData::TextDelta(ChatTextDeltaData {
                offset: current.text.len() as u64,
                text: text.to_string(),
            }),
            &at,
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Records the selected route without changing the run/input/output identity.
    /// The session fields set by choose_session are retained.
    pub fn chat_run_route(
        &self,
        run_id: &str,
        route: &Value,
        now: OffsetDateTime,
    ) -> Result<(), ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let run = live_run(&tx, run_id)?;
        if run.state != ChatRunState::Running {
            return Err(ChatError::Conflict("run is stopping".into()));
        }
        let raw: String = tx.query_row(
            "SELECT resolved_config_json FROM chat_runs WHERE run_id=?1",
            [run_id],
            |r| r.get(0),
        )?;
        let mut config: Value = serde_json::from_str(&raw)?;
        let fields = route
            .as_object()
            .ok_or_else(|| ChatError::Invalid("route must be an object".into()))?;
        for (key, value) in fields {
            config[key] = value.clone();
        }
        tx.execute(
            "UPDATE chat_runs SET resolved_config_json=?2 WHERE run_id=?1",
            params![run_id, config.to_string()],
        )?;
        super::store::append_event(
            &tx,
            &run.thread_id,
            Some(run_id),
            run.output_message_id.as_deref(),
            ChatEventType::Status,
            &ChatEventData::Status(ChatStatusData {
                phase: ChatStatusPhase::Working,
                summary: format!("CoS route: {}", route),
            }),
            &chat_ts(now),
        )?;
        emit_run(&tx, &run_by_id(&tx, run_id)?, &chat_ts(now))?;
        tx.commit()?;
        Ok(())
    }

    /// Records a public `status` phase for a live run. Returns the event id.
    pub fn chat_run_status(
        &self,
        run_id: &str,
        phase: ChatStatusPhase,
        summary: &str,
        now: OffsetDateTime,
    ) -> Result<i64, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let run = live_run(&tx, run_id)?;
        let id = super::store::append_event(
            &tx,
            &run.thread_id,
            Some(run_id),
            run.output_message_id.as_deref(),
            ChatEventType::Status,
            &ChatEventData::Status(ChatStatusData {
                phase,
                summary: summary.to_string(),
            }),
            &at,
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Records a `tool` event for a live run; `detail` is cut to 4 KiB on a char boundary
    /// (setting `truncated`). The caller redacts before calling. Returns the event id.
    pub fn chat_run_tool(
        &self,
        run_id: &str,
        tool: &ChatToolData,
        now: OffsetDateTime,
    ) -> Result<i64, ChatError> {
        let mut tool = tool.clone();
        if let Some(detail) = &tool.detail
            && detail.len() > CHAT_TOOL_DETAIL_MAX_BYTES
        {
            let mut end = CHAT_TOOL_DETAIL_MAX_BYTES;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            tool.detail = Some(detail[..end].to_string());
            tool.truncated = true;
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let run = live_run(&tx, run_id)?;
        let id = super::store::append_event(
            &tx,
            &run.thread_id,
            Some(run_id),
            run.output_message_id.as_deref(),
            ChatEventType::Tool,
            &ChatEventData::Tool(tool),
            &at,
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// ADR 2026-10-08-cos-workspace-files-in-chat D1: pins workspace files (already uploaded as
    /// attachments of the same thread) to an assistant reply and records their workspace paths in
    /// `metadata_json.workspace_files`, in one transaction. Repeating the same files is idempotent.
    /// No event is emitted here: the run's terminal `message` event carries the result.
    pub fn chat_message_attach_workspace_files(
        &self,
        thread_id: &str,
        message_id: &str,
        files: &[ChatWorkspaceFile],
        now: OffsetDateTime,
    ) -> Result<ChatMessage, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let message = message_require(&tx, thread_id, message_id)?;
        if message.role != ChatMessageRole::Assistant {
            return Err(ChatError::Invalid(format!(
                "message {message_id} is not an assistant reply"
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for file in files {
            if file.path.is_empty() || !seen.insert(file.attachment_id.as_str()) {
                return Err(ChatError::Invalid(
                    "duplicate or empty workspace file".into(),
                ));
            }
            let owner: Option<String> = tx
                .query_row(
                    "SELECT thread_id FROM chat_attachments WHERE id=?1 AND state='ready'",
                    [&file.attachment_id],
                    |r| r.get(0),
                )
                .optional()?;
            match owner {
                Some(owner) if owner == thread_id => {}
                Some(_) => {
                    return Err(ChatError::Conflict(format!(
                        "attachment {} belongs to another thread",
                        file.attachment_id
                    )));
                }
                None => return Err(ChatError::not_found("attachment", &file.attachment_id)),
            }
            super::attachments::add_ref_tx(&tx, &file.attachment_id, "message", message_id, now)
                .map_err(|e| ChatError::Invalid(e.to_string()))?;
        }
        let raw: String = tx.query_row(
            "SELECT metadata_json FROM chat_messages WHERE id=?1",
            [message_id],
            |r| r.get(0),
        )?;
        let mut meta: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
        if !meta.is_object() {
            meta = json!({});
        }
        meta["workspace_files"] = serde_json::to_value(files)?;
        tx.execute(
            "UPDATE chat_messages SET metadata_json=?2,updated_at=?3 WHERE id=?1",
            params![message_id, serde_json::to_string(&meta)?, chat_ts(now)],
        )?;
        let message = message_require(&tx, thread_id, message_id)?;
        tx.commit()?;
        Ok(message)
    }

    /// Ends a live run with a terminal `state`. `final_text` (if any) replaces the output text.
    /// Input/output messages become completed, failed or interrupted. Repeating the same terminal
    /// state is idempotent; a different terminal state is a conflict.
    pub fn chat_run_finish(
        &self,
        run_id: &str,
        state: ChatRunState,
        final_text: Option<&str>,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ChatRun, ChatError> {
        if !chat_run_state_is_terminal(state) {
            return Err(ChatError::Invalid(format!(
                "{} is not a terminal run state",
                enum_str(&state)?
            )));
        }
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let run = finish_conn(&tx, run_id, state, final_text, reason, &at)?;
        tx.commit()?;
        Ok(run)
    }

    /// ADR 2026-10-07-cos-live-fixes D4: 持ち主の居ない run の引き継ぎ。run が非終端であることの確認、
    /// 続きの message（`continuation`。通常 `mode=interrupt`）の投入、run の `Interrupted` 化を
    /// **1 つの write transaction**で行う。run が既に終端（completed / failed / stopped / interrupted）なら
    /// 何も書かずに `Ok(None)` を返す（終わった run から続きの run を起こさない）。
    pub fn chat_run_takeover(
        &self,
        run_id: &str,
        continuation: &ChatPostMessageRequest,
        reason: &str,
        now: OffsetDateTime,
    ) -> Result<Option<ChatRun>, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let run = run_by_id(&tx, run_id)?;
        if chat_run_state_is_terminal(run.state) {
            return Ok(None);
        }
        super::store::message_post_conn(&tx, &run.thread_id, continuation, now)?;
        let run = finish_conn(
            &tx,
            run_id,
            ChatRunState::Interrupted,
            None,
            Some(reason),
            &at,
        )?;
        tx.commit()?;
        Ok(Some(run))
    }

    /// D2 `POST /chat/threads/{t}/stop`: only the named run. A running run becomes stopping and
    /// the queue is paused (accepted). A run that is already stopping is accepted again and
    /// pauses the queue if an interrupt had left it unpaused. A finished run (including an old run_id arriving late) changes nothing and does
    /// not pause the queue or touch the current run (`accepted=false`, API 200).
    pub fn chat_run_stop(
        &self,
        thread_id: &str,
        run_id: &str,
        now: OffsetDateTime,
    ) -> Result<ChatStopOutcome, ChatError> {
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let outcome = chat_run_stop_tx(&tx, thread_id, run_id, now)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// Validates an SSE/event cursor for `thread_id` (404 thread, 400 malformed/future, 410
    /// expired) and returns it as a number.
    pub fn chat_event_cursor_check(&self, thread_id: &str, after: &str) -> Result<i64, ChatError> {
        let after = parse_cursor(after)?;
        read_tx(self, |conn| {
            thread_require(conn, thread_id)?;
            check_cursor(conn, thread_id, after)?;
            Ok(after)
        })
    }

    /// Events of a thread (optionally one run) after the cursor, ascending id. Used by
    /// `GET .../runs/{r}/events` and by SSE replay.
    pub fn chat_events_page(
        &self,
        thread_id: &str,
        query: &ChatEventQuery,
    ) -> Result<ChatEventListResponse, ChatError> {
        let limit = page_limit(query.limit, CHAT_EVENT_PAGE_DEFAULT, CHAT_EVENT_PAGE_MAX)?;
        let after = query
            .after
            .as_deref()
            .map(parse_cursor)
            .transpose()?
            .unwrap_or(0);
        read_tx(self, |conn| {
            thread_require(conn, thread_id)?;
            if let Some(run_id) = &query.run_id
                && run_get_conn(conn, thread_id, run_id)?.is_none()
            {
                return Err(ChatError::not_found("run", run_id));
            }
            check_cursor(conn, thread_id, after)?;
            let mut stmt = conn.prepare_cached(
                "SELECT id,thread_id,run_id,message_id,type,payload_json,created_at FROM chat_events \
                 WHERE thread_id=?1 AND id>?2 AND (?3 IS NULL OR run_id=?3) ORDER BY id LIMIT ?4",
            )?;
            let rows = stmt
                .query_map(
                    params![thread_id, after, query.run_id, i64::from(limit) + 1],
                    event_from_row,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let more = rows.len() > limit as usize;
            let items = rows
                .into_iter()
                .take(limit as usize)
                .map(decode_event)
                .collect::<Result<Vec<_>, _>>()?;
            let next_cursor = if more {
                items.last().map(|e| e.id.clone())
            } else {
                None
            };
            Ok(ChatEventListResponse { items, next_cursor })
        })
    }

    /// D2 retention: removes `text_delta`/`tool` events of runs that finished at or before
    /// `now - retention` and raises each affected thread's watermark. Messages, runs and other
    /// events stay. Returns the number of removed events.
    pub fn chat_events_retention(
        &self,
        now: OffsetDateTime,
        retention: time::Duration,
    ) -> Result<usize, ChatError> {
        let cutoff = chat_ts(now - retention);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        const TARGET: &str = "type IN ('text_delta','tool') AND run_id IN \
             (SELECT run_id FROM chat_runs WHERE finished_at IS NOT NULL AND finished_at<=?1 \
              AND state IN ('completed','stopped','failed','interrupted'))";
        let marks: Vec<(String, i64)> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT thread_id,MAX(id) FROM chat_events WHERE {TARGET} GROUP BY thread_id"
            ))?;
            stmt.query_map(params![cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let removed = tx.execute(
            &format!("DELETE FROM chat_events WHERE {TARGET}"),
            params![cutoff],
        )?;
        for (thread_id, max_id) in marks {
            let name = format!("{PURGED_PREFIX}{thread_id}");
            let prev: i64 = tx
                .query_row(
                    "SELECT value FROM feed_cursor WHERE name=?1",
                    params![name],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            tx.execute(
                "INSERT INTO feed_cursor(name,value) VALUES(?1,?2) \
                 ON CONFLICT(name) DO UPDATE SET value=excluded.value",
                params![name, prev.max(max_id).to_string()],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }
}

/// `chat_run_finish` の本体（呼び出し側の write transaction の中で走る）。
fn finish_conn(
    tx: &Connection,
    run_id: &str,
    state: ChatRunState,
    final_text: Option<&str>,
    reason: Option<&str>,
    at: &str,
) -> Result<ChatRun, ChatError> {
    let run = run_by_id(tx, run_id)?;
    if chat_run_state_is_terminal(run.state) {
        if run.state == state {
            return Ok(run);
        }
        return Err(ChatError::Conflict(format!(
            "run {run_id} already finished"
        )));
    }
    let reason = reason.map(str::to_string).or(run.reason.clone());
    tx.execute(
        "UPDATE chat_runs SET state=?2,reason=?3,finished_at=?4 WHERE run_id=?1",
        params![run_id, enum_str(&state)?, reason, at],
    )?;
    super::credential::revoke_conn(tx, run_id, at)?;
    let msg_state = enum_str(&message_state_for(state))?;
    tx.execute(
        "UPDATE chat_messages SET state=?2,updated_at=?3 WHERE id=?1",
        params![run.input_message_id, msg_state, at],
    )?;
    if let Some(output) = &run.output_message_id {
        match final_text {
            Some(text) => tx.execute(
                "UPDATE chat_messages SET state=?2,text=?3,updated_at=?4 WHERE id=?1",
                params![output, msg_state, text, at],
            )?,
            None => tx.execute(
                "UPDATE chat_messages SET state=?2,updated_at=?3 WHERE id=?1",
                params![output, msg_state, at],
            )?,
        };
    }
    touch_thread(tx, &run.thread_id, at)?;
    let input = message_require(tx, &run.thread_id, &run.input_message_id)?;
    emit_message(tx, &input, at)?;
    if let Some(output) = &run.output_message_id {
        let output = message_require(tx, &run.thread_id, output)?;
        emit_message(tx, &output, at)?;
    }
    let run = run_by_id(tx, run_id)?;
    emit_run(tx, &run, at)?;
    emit_queue(tx, &run.thread_id, at)?;
    Ok(run)
}

/// [`SqliteStore::chat_run_stop`] inside the caller's transaction (the CoS operation transaction).
pub fn chat_run_stop_tx(
    tx: &Connection,
    thread_id: &str,
    run_id: &str,
    now: OffsetDateTime,
) -> Result<ChatStopOutcome, ChatError> {
    let at = chat_ts(now);
    let thread = thread_require(tx, thread_id)?;
    let run =
        run_get_conn(tx, thread_id, run_id)?.ok_or_else(|| ChatError::not_found("run", run_id))?;
    let accepted = match run.state {
        s if chat_run_state_is_terminal(s) => false,
        ChatRunState::Stopping => {
            if !thread.queue_paused {
                super::store::set_paused(tx, thread_id, true, &at)?;
                emit_queue(tx, thread_id, &at)?;
            }
            true
        }
        ChatRunState::Running => {
            request_stop(tx, thread_id, run_id, "stopped by human", &at)?;
            if !thread.queue_paused {
                super::store::set_paused(tx, thread_id, true, &at)?;
            }
            emit_queue(tx, thread_id, &at)?;
            true
        }
        _ => {
            return Err(ChatError::Conflict(format!(
                "run {run_id} is not the current run of thread {thread_id}"
            )));
        }
    };
    let run = run_by_id(tx, run_id)?;
    let queue_paused = thread_require(tx, thread_id)?.queue_paused;
    Ok(ChatStopOutcome {
        response: ChatStopResponse { run, queue_paused },
        accepted,
    })
}
