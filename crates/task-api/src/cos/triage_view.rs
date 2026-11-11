//! Read side of the CoS triage store (ADR 2026-10-06 cos-inbox-triage). The outcome mark is
//! written by task-core (`task_core::chat::triage::cos_triage_resolve_tx`) inside the same
//! `cos_operation_apply` transaction as the audit row (ADR 2026-11-10-taskstore-consumer-ports
//! D5). Reads open their own connection on the daemon DB (same as chat attachments).
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use task_core::chat::ChatError;

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
