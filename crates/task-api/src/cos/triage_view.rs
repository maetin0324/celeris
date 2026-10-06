//! Read side of the CoS triage store and the in-transaction outcome mark used by the resolve API
//! (ADR 2026-10-06 cos-inbox-triage). The resolve API writes the item outcome inside the same
//! `cos_operation_apply` transaction as the audit row, its envelope event and its chat card.
//! Reads open their own connection on the daemon DB (same as chat attachments), so task-core's
//! store API stays as the store leaf left it.
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use task_core::chat::ChatError;
use time::OffsetDateTime;

const ITEM_COLUMNS: &str = "id,source_kind,source_key,source_revision,source_event_id,thread_id,message_id,state,run_id,reason,policy_version,operation_id,created_at,updated_at";

/// One `cos_inbox_items` row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
pub struct CosInboxItem {
    pub id: String,
    pub source_kind: String,
    pub source_key: String,
    pub source_revision: String,
    pub source_event_id: Option<i64>,
    pub thread_id: String,
    pub message_id: String,
    pub state: String,
    pub run_id: Option<String>,
    pub reason: Option<String>,
    pub policy_version: String,
    pub operation_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl CosInboxItem {
    /// `answered` / `observed` / `escalated` / `fallback` / `resolved`: no further CoS outcome.
    pub fn is_terminal(&self) -> bool {
        !matches!(self.state.as_str(), "pending" | "running")
    }
}

fn item_row(r: &Row<'_>) -> rusqlite::Result<CosInboxItem> {
    Ok(CosInboxItem {
        id: r.get(0)?,
        source_kind: r.get(1)?,
        source_key: r.get(2)?,
        source_revision: r.get(3)?,
        source_event_id: r.get(4)?,
        thread_id: r.get(5)?,
        message_id: r.get(6)?,
        state: r.get(7)?,
        run_id: r.get(8)?,
        reason: r.get(9)?,
        policy_version: r.get(10)?,
        operation_id: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

fn open(db: &Path) -> Result<Connection, ChatError> {
    let conn = Connection::open(db)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// Same text form as task-core's chat timestamps (`updated_at` is compared as text).
fn chat_ts(now: OffsetDateTime) -> String {
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

pub(crate) fn item_get(db: &Path, id: &str) -> Result<Option<CosInboxItem>, ChatError> {
    let conn = open(db)?;
    Ok(conn
        .query_row(
            &format!("SELECT {ITEM_COLUMNS} FROM cos_inbox_items WHERE id=?1"),
            [id],
            item_row,
        )
        .optional()?)
}

/// Newest first. `state=None` lists every state.
pub(crate) fn items(
    db: &Path,
    state: Option<&str>,
    limit: usize,
) -> Result<Vec<CosInboxItem>, ChatError> {
    let conn = open(db)?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(&format!(
        "SELECT {ITEM_COLUMNS} FROM cos_inbox_items WHERE (?1 IS NULL OR state=?1) \
         ORDER BY created_at DESC,id DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![state, limit], item_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// True when a later revision of the same source was ingested after `item`.
pub(crate) fn superseded(db: &Path, item: &CosInboxItem) -> Result<bool, ChatError> {
    let conn = open(db)?;
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cos_inbox_items WHERE source_kind=?1 AND source_key=?2 \
         AND id<>?3 AND (created_at>?4 OR (created_at=?4 AND id>?3)))",
        params![item.source_kind, item.source_key, item.id, item.created_at],
        |r| r.get(0),
    )?)
}

/// Mark one non-terminal item inside the caller's transaction. A concurrent outcome (another
/// CoS resolve, a fallback, reconcile) makes this a conflict so the whole transaction rolls back.
pub(crate) fn mark_tx(
    tx: &Transaction<'_>,
    item_id: &str,
    outcome: &str,
    operation_id: &str,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), ChatError> {
    if !matches!(outcome, "answered" | "observed" | "escalated") {
        return Err(ChatError::Invalid("invalid triage outcome".into()));
    }
    let changed = tx.execute(
        "UPDATE cos_inbox_items SET state=?2,operation_id=?3,reason=?4,updated_at=?5 \
         WHERE id=?1 AND state IN ('pending','running')",
        params![item_id, outcome, operation_id, reason, chat_ts(now)],
    )?;
    if changed != 1 {
        return Err(ChatError::Conflict(format!(
            "triage item {item_id} was already resolved"
        )));
    }
    Ok(())
}
