//! ADR 2026-11-21-chat-discussion-mode D1: the discussion mode a person picks for one message
//! (`discuss` / `propose` / `execute`), recorded only in `chat_messages.metadata_json`, and the
//! mode of a CoS run derived from its input messages every time it is needed (never stored).

use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::store::{ChatError, read_tx};
use crate::store::SqliteStore;

/// D1.1: what a person allows the CoS run that answers this message to do. Distinct from
/// [`super::ChatSendMode`] (the queue handling of a send).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatDiscussionMode {
    /// Talk and read only: every `/cos/operations` action is refused.
    Discuss,
    /// Talk and leave unapproved drafts as reply files: every `/cos/operations` action is refused.
    Propose,
    /// Today's behaviour: the existing checks of `/cos/operations` decide.
    Execute,
}

/// The `metadata_json` key of [`ChatDiscussionMode`].
pub(crate) const METADATA_KEY: &str = "discussion_mode";

impl ChatDiscussionMode {
    /// D1.2: discuss > propose > execute.
    fn strictness(self) -> u8 {
        match self {
            Self::Discuss => 2,
            Self::Propose => 1,
            Self::Execute => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discuss => "discuss",
            Self::Propose => "propose",
            Self::Execute => "execute",
        }
    }

    /// D1.1: the value written to `metadata_json` — only discuss and propose; an absent field and
    /// `execute` write nothing, so such a post is byte-identical to one made before the field existed.
    pub(crate) fn recorded(mode: Option<Self>) -> Option<Self> {
        mode.filter(|m| *m != Self::Execute)
    }

    /// The recorded mode of a message's `metadata_json` (`None` when absent). A value outside the
    /// three words is read as `discuss`: an unreadable record must not widen what CoS may do.
    pub(crate) fn of_metadata(meta: &Value) -> Option<Self> {
        meta.get(METADATA_KEY).filter(|v| !v.is_null()).map(|v| {
            serde_json::from_value(v.clone())
                .ok()
                .filter(|m| *m != Self::Execute)
                .unwrap_or(Self::Discuss)
        })
    }
}

/// One input message that decided a run's mode (D7: the audit names who chose it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChatDiscussionDecider {
    pub message_id: String,
    /// `human:<name>` / `operator:<name>` / `mcp:<client_id>`; absent for a system input.
    pub author: Option<String>,
}

/// D1.2: the mode of a CoS run and the input messages that carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunDiscussionMode {
    pub mode: ChatDiscussionMode,
    /// The inputs whose own mode is `mode`, oldest first.
    pub decided_by: Vec<ChatDiscussionDecider>,
}

/// D1.2: the mode of run `run_id` of `thread_id` — the strictest mode of its input message `i` and of
/// every non-cancelled `role=user` message `m` of the thread with `prev < m.seq <= i.seq`, where
/// `prev` is the largest input seq of the thread's runs that started before this one and completed.
/// A system input counts as `execute`. Derived from `chat_messages` / `chat_runs` on every call.
pub fn discussion_mode_of_run(
    conn: &Connection,
    thread_id: &str,
    run_id: &str,
) -> Result<RunDiscussionMode, ChatError> {
    let run = conn
        .query_row(
            "SELECT r.started_at, r.rowid, m.id, m.seq, m.role, m.metadata_json \
             FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id \
             WHERE r.run_id=?1 AND r.thread_id=?2",
            params![run_id, thread_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| ChatError::not_found("run", run_id))?;
    let (started_at, rowid, input_id, input_seq, input_role, input_meta) = run;
    let prev: i64 = conn.query_row(
        "SELECT COALESCE(MAX(m.seq),0) FROM chat_runs p JOIN chat_messages m \
         ON m.id=p.input_message_id WHERE p.thread_id=?1 AND p.run_id<>?2 AND p.state='completed' \
         AND (p.started_at<?3 OR (p.started_at=?3 AND p.rowid<?4))",
        params![thread_id, run_id, started_at, rowid],
        |row| row.get(0),
    )?;
    let input_mode = if input_role == "user" {
        let meta: Value = serde_json::from_str(&input_meta).unwrap_or(Value::Null);
        (author_of(&meta), mode_of(&meta))
    } else {
        (None, ChatDiscussionMode::Execute)
    };
    let mut inputs = vec![(input_seq, input_id.clone(), input_mode.0, input_mode.1)];
    let mut stmt = conn.prepare_cached(
        "SELECT seq, id, metadata_json FROM chat_messages WHERE thread_id=?1 AND role='user' \
         AND state<>'cancelled' AND seq>?2 AND seq<?3 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![thread_id, prev, input_seq], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (seq, id, raw) = row?;
        let meta: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        inputs.push((seq, id, author_of(&meta), mode_of(&meta)));
    }
    inputs.sort_by_key(|(seq, ..)| *seq);
    let mode = inputs
        .iter()
        .map(|(.., mode)| *mode)
        .max_by_key(|mode| mode.strictness())
        .unwrap_or(ChatDiscussionMode::Execute);
    let decided_by = inputs
        .into_iter()
        .filter(|(.., m)| *m == mode)
        .map(|(_, message_id, author, _)| ChatDiscussionDecider { message_id, author })
        .collect();
    Ok(RunDiscussionMode { mode, decided_by })
}

fn mode_of(meta: &Value) -> ChatDiscussionMode {
    ChatDiscussionMode::of_metadata(meta).unwrap_or(ChatDiscussionMode::Execute)
}

fn author_of(meta: &Value) -> Option<String> {
    meta.get("author").and_then(Value::as_str).map(str::to_owned)
}

impl SqliteStore {
    /// [`discussion_mode_of_run`] on one read snapshot.
    pub fn chat_run_discussion_mode(
        &self,
        thread_id: &str,
        run_id: &str,
    ) -> Result<RunDiscussionMode, ChatError> {
        read_tx(self, |conn| discussion_mode_of_run(conn, thread_id, run_id))
    }
}
