//! CoS chat thread sessions (ADR 2026-10-05 D1/D2, on ADR-0054 `node_sessions`).
//!
//! One live `kind='cos_chat'` row per thread (partial UNIQUE index of migration 0050). The match
//! key is `(thread_id, harness, provider, llm_source, account_id, cwd, model)`; `harness` is the
//! `adapter` column. This module is SQL only: whether to resume or retire is decided by the
//! dispatcher's pure function (`task_dispatch::sessions::cos_chat`). No LLM call here.
//!
//! The run's session choice is recorded on `chat_runs`: `session_row_id` (column) and, inside
//! `resolved_config_json`, the wire `session_mode` (`new`/`resumed`/`fresh`, read back as
//! `R.session_mode`) plus the detailed `session_detail` and `session_reason`. No migration.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::store::{ChatError, chat_ts, enum_str, immediate, read_tx, writer};
use super::*;
use crate::store::{SqliteStore, StoreError, parse_rfc3339};

/// `node_sessions.kind` of CoS chat thread sessions.
pub const COS_CHAT_SESSION_KIND: &str = "cos_chat";
/// `node_sessions.node_id` of CoS chat thread sessions.
pub const COS_CHAT_SESSION_NODE: &str = "cos";

/// The D2 match key. `None` means "not set" and only equals `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatSessionKey {
    pub thread_id: String,
    pub harness: String,
    pub provider: Option<String>,
    pub llm_source: Option<String>,
    pub account_id: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

/// A `node_sessions` row of `kind='cos_chat'`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSession {
    pub id: String,
    pub key: ChatSessionKey,
    /// Harness session id (claude-code: UUID decided by celeris before the run; codex/acp: `''`
    /// until the harness reports it).
    pub session_id: String,
    pub turns: i64,
    pub approx_tokens: i64,
    /// Messages up to this thread seq are covered by the summary the worker wrote.
    pub summary_through_seq: i64,
    pub created_at: OffsetDateTime,
    pub last_used_at: OffsetDateTime,
    pub retired_at: Option<OffsetDateTime>,
}

impl ChatSession {
    pub fn new(key: ChatSessionKey, session_id: impl Into<String>, now: OffsetDateTime) -> Self {
        Self {
            id: ulid::Ulid::new().to_string(),
            key,
            session_id: session_id.into(),
            turns: 0,
            approx_tokens: 0,
            summary_through_seq: 0,
            created_at: now,
            last_used_at: now,
            retired_at: None,
        }
    }
}

/// How a chat run got its session (detailed; the wire `session_mode` is coarser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRunSessionMode {
    /// The thread had no live session.
    New,
    /// The live session matched and was resumed.
    Resumed,
    /// The live session was retired (key change, cache gone, context limit) and a fresh one made.
    Fresh,
    /// The single fresh retry after the harness refused to resume.
    FreshAfterRefusal,
}

impl ChatRunSessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRunSessionMode::New => "new",
            ChatRunSessionMode::Resumed => "resumed",
            ChatRunSessionMode::Fresh => "fresh",
            ChatRunSessionMode::FreshAfterRefusal => "fresh_after_refusal",
        }
    }

    /// The `R.session_mode` value.
    pub fn wire(self) -> ChatSessionMode {
        match self {
            ChatRunSessionMode::New => ChatSessionMode::New,
            ChatRunSessionMode::Resumed => ChatSessionMode::Resumed,
            ChatRunSessionMode::Fresh | ChatRunSessionMode::FreshAfterRefusal => {
                ChatSessionMode::Fresh
            }
        }
    }
}

/// What [`SqliteStore::chat_run_record_session`] wrote on a run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatRunSessionRecord {
    pub session_row_id: Option<String>,
    /// [`ChatRunSessionMode::as_str`].
    pub detail: Option<String>,
    /// The retire reason, if the run started fresh after retiring a session.
    pub reason: Option<String>,
}

const SELECT: &str = "SELECT id,thread_id,adapter,provider,llm_source,account_id,cwd,model,\
 session_id,turns,approx_tokens,summary_through_seq,created_at,last_used_at,retired_at \
 FROM node_sessions WHERE kind='cos_chat'";

type RawRow = (ChatSession, String, String, Option<String>);

fn raw(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        ChatSession {
            id: row.get(0)?,
            key: ChatSessionKey {
                thread_id: row.get(1)?,
                harness: row.get(2)?,
                provider: row.get(3)?,
                llm_source: row.get(4)?,
                account_id: row.get(5)?,
                cwd: row.get(6)?,
                model: row.get(7)?,
            },
            session_id: row.get(8)?,
            turns: row.get(9)?,
            approx_tokens: row.get(10)?,
            summary_through_seq: row.get(11)?,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_used_at: OffsetDateTime::UNIX_EPOCH,
            retired_at: None,
        },
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

fn decode((mut s, created, used, retired): RawRow) -> Result<ChatSession, ChatError> {
    s.created_at = parse_rfc3339(&created)?;
    s.last_used_at = parse_rfc3339(&used)?;
    s.retired_at = retired.as_deref().map(parse_rfc3339).transpose()?;
    Ok(s)
}

fn active_conn(conn: &Connection, thread_id: &str) -> Result<Option<ChatSession>, ChatError> {
    conn.query_row(
        &format!("{SELECT} AND thread_id=?1 AND retired_at IS NULL"),
        params![thread_id],
        raw,
    )
    .optional()?
    .map(decode)
    .transpose()
}

fn retire_conn(conn: &Connection, thread_id: &str, at: &str) -> Result<bool, ChatError> {
    Ok(conn.execute(
        "UPDATE node_sessions SET retired_at=?2 WHERE kind='cos_chat' AND thread_id=?1 \
         AND retired_at IS NULL",
        params![thread_id, at],
    )? > 0)
}

impl SqliteStore {
    /// The thread's live CoS chat session, if any.
    pub fn chat_session_active(&self, thread_id: &str) -> Result<Option<ChatSession>, ChatError> {
        read_tx(self, |conn| active_conn(conn, thread_id))
    }

    /// A CoS chat session row by id (retired rows included).
    pub fn chat_session_get(&self, row_id: &str) -> Result<Option<ChatSession>, ChatError> {
        read_tx(self, |conn| {
            conn.query_row(&format!("{SELECT} AND id=?1"), params![row_id], raw)
                .optional()?
                .map(decode)
                .transpose()
        })
    }

    /// Retires the thread's live session (if any) and inserts `session` in one transaction
    /// (D1: retire→create is atomic, so the partial UNIQUE index never sees two live rows).
    /// `session.key.thread_id` must name an existing thread. Returns whether a row was retired.
    pub fn chat_session_rotate(
        &self,
        session: &ChatSession,
        now: OffsetDateTime,
    ) -> Result<bool, ChatError> {
        let at = chat_ts(now);
        let k = &session.key;
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        super::store::thread_require(&tx, &k.thread_id)?;
        let retired = retire_conn(&tx, &k.thread_id, &at)?;
        tx.execute(
            "INSERT INTO node_sessions(id,node_id,kind,project_id,adapter,account_id,session_id,\
             turns,approx_tokens,created_at,last_used_at,retired_at,thread_id,llm_source,model,\
             summary_through_seq,provider,cwd) \
             VALUES(?1,?2,'cos_chat',NULL,?3,?4,?5,?6,?7,?8,?9,NULL,?10,?11,?12,?13,?14,?15)",
            params![
                session.id,
                COS_CHAT_SESSION_NODE,
                k.harness,
                k.account_id,
                session.session_id,
                session.turns,
                session.approx_tokens,
                chat_ts(session.created_at),
                chat_ts(session.last_used_at),
                k.thread_id,
                k.llm_source,
                k.model,
                session.summary_through_seq,
                k.provider,
                k.cwd,
            ],
        )?;
        tx.commit()?;
        Ok(retired)
    }

    /// Retires the thread's live session without creating a new one (e.g. cache found missing
    /// before a replacement exists). Returns whether a row was retired.
    pub fn chat_session_retire(
        &self,
        thread_id: &str,
        now: OffsetDateTime,
    ) -> Result<bool, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let retired = retire_conn(&tx, thread_id, &at)?;
        tx.commit()?;
        Ok(retired)
    }

    /// After a run: `turns += 1`, `approx_tokens += add_tokens`, `last_used_at = now` on the live
    /// row `row_id`. A retired or unknown row is left alone (`false`).
    pub fn chat_session_touch(
        &self,
        row_id: &str,
        add_tokens: i64,
        now: OffsetDateTime,
    ) -> Result<bool, ChatError> {
        let conn = writer(self)?;
        Ok(conn.execute(
            "UPDATE node_sessions SET turns=turns+1,approx_tokens=approx_tokens+?2,\
             last_used_at=?3 WHERE id=?1 AND kind='cos_chat' AND retired_at IS NULL",
            params![row_id, add_tokens.max(0), chat_ts(now)],
        )? > 0)
    }

    /// Records the harness-decided session id (codex/acp) on the live row `row_id`.
    pub fn chat_session_set_id(&self, row_id: &str, session_id: &str) -> Result<bool, ChatError> {
        let conn = writer(self)?;
        Ok(conn.execute(
            "UPDATE node_sessions SET session_id=?2 WHERE id=?1 AND kind='cos_chat' \
             AND retired_at IS NULL",
            params![row_id, session_id],
        )? > 0)
    }

    /// Raises the summary watermark of row `row_id` (never lowers it).
    pub fn chat_session_set_summary_through(
        &self,
        row_id: &str,
        seq: i64,
    ) -> Result<bool, ChatError> {
        let conn = writer(self)?;
        Ok(conn.execute(
            "UPDATE node_sessions SET summary_through_seq=MAX(summary_through_seq,?2) \
             WHERE id=?1 AND kind='cos_chat'",
            params![row_id, seq.max(0)],
        )? > 0)
    }

    /// Records the run's session choice: `chat_runs.session_row_id`, and in
    /// `resolved_config_json` the wire `session_mode`, `session_detail` and `session_reason`
    /// (the retire reason, or null). Emits a `run` event so SSE shows `R.session_mode`.
    /// The row must belong to the run's thread. Only a live run accepts it.
    pub fn chat_run_record_session(
        &self,
        run_id: &str,
        mode: ChatRunSessionMode,
        session_row_id: &str,
        reason: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<ChatRun, ChatError> {
        let at = chat_ts(now);
        let mut conn = writer(self)?;
        let tx = immediate(&mut conn)?;
        let (thread_id, state): (String, String) = tx
            .query_row(
                "SELECT thread_id,state FROM chat_runs WHERE run_id=?1",
                params![run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| ChatError::not_found("run", run_id))?;
        if !matches!(state.as_str(), "running" | "stopping") {
            return Err(ChatError::Conflict(format!(
                "run {run_id} is already finished"
            )));
        }
        let row_thread: Option<String> = tx
            .query_row(
                "SELECT thread_id FROM node_sessions WHERE id=?1 AND kind='cos_chat'",
                params![session_row_id],
                |row| row.get(0),
            )
            .optional()?;
        match row_thread {
            None => return Err(ChatError::not_found("session", session_row_id)),
            Some(t) if t != thread_id => {
                return Err(ChatError::Store(StoreError::Invalid(format!(
                    "session {session_row_id} belongs to another thread"
                ))));
            }
            Some(_) => {}
        }
        tx.execute(
            "UPDATE chat_runs SET session_row_id=?2,resolved_config_json=json_set(\
             CASE WHEN json_valid(resolved_config_json) THEN resolved_config_json ELSE '{}' END,\
             '$.session_mode',?3,'$.session_detail',?4,'$.session_reason',?5) WHERE run_id=?1",
            params![
                run_id,
                session_row_id,
                enum_str(&mode.wire())?,
                mode.as_str(),
                reason
            ],
        )?;
        let run = super::run_store::run_get_conn(&tx, &thread_id, run_id)?
            .ok_or_else(|| ChatError::not_found("run", run_id))?;
        super::store::append_event(
            &tx,
            &thread_id,
            Some(run_id),
            None,
            ChatEventType::Run,
            &ChatEventData::Run(ChatRunData { run: run.clone() }),
            &at,
        )?;
        tx.commit()?;
        Ok(run)
    }

    /// The session choice recorded on a run (all `None` before [`Self::chat_run_record_session`]).
    pub fn chat_run_session_record(&self, run_id: &str) -> Result<ChatRunSessionRecord, ChatError> {
        read_tx(self, |conn| {
            conn.query_row(
                "SELECT session_row_id,json_extract(resolved_config_json,'$.session_detail'),\
                 json_extract(resolved_config_json,'$.session_reason') FROM chat_runs \
                 WHERE run_id=?1",
                params![run_id],
                |row| {
                    Ok(ChatRunSessionRecord {
                        session_row_id: row.get(0)?,
                        detail: row.get(1)?,
                        reason: row.get(2)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| ChatError::not_found("run", run_id))
        })
    }
}
