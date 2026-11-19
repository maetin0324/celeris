//! The CoS inbox API's read side of the triage table (ADR 2026-11-19-chat-attachment-store-connection
//! D2 and its addendum): one `cos_inbox_items` row with its route, read on the store's read
//! connection (`with_read_conn`, ADR-0064 D4). The writes stay in `chat::triage` inside
//! `cos_operation_apply`. Like `cos_fallback` and `cos_revision`, this module only holds SQL over
//! `cos_inbox_items` and `cos_notification_routes`; it sits outside `chat` because that module is
//! at its size limit (`size.responsibility-growth`).
//!
//! Never call these inside a `write_tx` / `cos_operation_apply` closure: without a read pool
//! (in-memory, `read_pool_size = 0`) the read waits on the write connection's mutex that closure
//! holds (ADR §3).

use rusqlite::{OptionalExtension, params};
use schemars::JsonSchema;
use serde::Serialize;

use crate::chat::ChatError;
use crate::store::SqliteStore;

const ITEM_COLUMNS: &str = "i.id,i.source_kind,i.source_key,i.source_revision,i.source_event_id,i.thread_id,i.message_id,i.state,i.run_id,i.reason,i.policy_version,i.operation_id,i.created_at,i.updated_at,r.route";
/// ADR 2026-11-12-cos-triage-scope D2: the route row says who an escalated item is for.
const ITEM_FROM: &str = "cos_inbox_items i LEFT JOIN cos_notification_routes r ON r.item_id=i.id";

/// One `cos_inbox_items` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
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
    /// ADR 2026-11-12-cos-triage-scope D2: `human` (escalated or fallback to the person, Discord)
    /// or `operator` (escalated to the external operator, not sent); absent while not routed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addressee: Option<String>,
}

impl CosInboxItem {
    /// `answered` / `observed` / `escalated` / `fallback` / `resolved`: no further CoS outcome.
    pub fn is_terminal(&self) -> bool {
        !matches!(self.state.as_str(), "pending" | "running")
    }
}

fn item_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CosInboxItem> {
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
        addressee: r.get::<_, Option<String>>(14)?.map(|route| {
            if route == crate::cos_fallback::OPERATOR_ROUTE {
                "operator".to_owned()
            } else {
                "human".to_owned()
            }
        }),
    })
}

/// One triage item with its route, or `None`.
pub fn cos_inbox_item(store: &SqliteStore, id: &str) -> Result<Option<CosInboxItem>, ChatError> {
    Ok(store.with_read_conn(|conn| {
        Ok(conn
            .query_row(
                &format!("SELECT {ITEM_COLUMNS} FROM {ITEM_FROM} WHERE i.id=?1"),
                [id],
                item_row,
            )
            .optional()?)
    })?)
}

/// Triage items newest first. `state=None` lists every state.
pub fn cos_inbox_items(
    store: &SqliteStore,
    state: Option<&str>,
    limit: usize,
) -> Result<Vec<CosInboxItem>, ChatError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    Ok(store.with_read_conn(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM {ITEM_FROM} WHERE (?1 IS NULL OR i.state=?1) \
             ORDER BY i.created_at DESC,i.id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![state, limit], item_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    })?)
}

/// ADR 2026-11-11-cos-fallback-timing D2: the item is with the person by the fallback, but the
/// notifier has not sent it, so CoS may still answer, observe or escalate it.
pub fn cos_inbox_fallback_unsent(store: &SqliteStore, id: &str) -> Result<bool, ChatError> {
    Ok(store.with_read_conn(|conn| Ok(crate::cos_fallback::unsent(conn, id)?))?)
}

/// True when a later revision of the same source was ingested after the item (the store's one
/// check, ADR 2026-11-12-discord-person-or-operator D1).
pub fn cos_inbox_superseded(store: &SqliteStore, item_id: &str) -> Result<bool, ChatError> {
    Ok(store.with_read_conn(|conn| Ok(crate::cos_revision::superseded(conn, item_id)?))?)
}
