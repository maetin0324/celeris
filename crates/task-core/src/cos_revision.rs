//! ADR 2026-11-12-discord-person-or-operator D1: a newer revision of the same CoS inbox source
//! closes the older ones, and an older revision is never routed to a person. SQL over
//! `cos_inbox_items`, `cos_notification_routes` and `notifications` only (like `cos_fallback`), so
//! the chat triage store calls it inside its own transactions and the resolve API and the notifier
//! read it.

use rusqlite::{Connection, params};

/// The `reason` of an older revision closed by the judgment of a newer one.
pub const CLOSED_BY_NEWER_REVISION: &str = "新しい revision の判定で閉じた";

/// A newer revision of the same source was ingested after `item_id` (a later `created_at`, ties
/// broken by id). The one check of the resolve API, the outbox claim (escalation and fallback) and
/// the notifier just before the send.
pub fn superseded(conn: &Connection, item_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cos_inbox_items o JOIN cos_inbox_items i ON i.id=?1 \
         WHERE o.source_kind=i.source_kind AND o.source_key=i.source_key AND o.id<>i.id \
         AND (o.created_at>i.created_at OR (o.created_at=i.created_at AND o.id>i.id)))",
        [item_id],
        |r| r.get(0),
    )
}

/// The outbox claim of a superseded item: an item that still waits for CoS (`pending`, `running`)
/// closes as `resolved` instead of taking a route. `true` when `item_id` is superseded (the claim
/// routes nothing).
pub(crate) fn close_if_superseded(
    conn: &Connection,
    item_id: &str,
    at: &str,
) -> rusqlite::Result<bool> {
    let newer = superseded(conn, item_id)?;
    if newer {
        conn.execute(
            "UPDATE cos_inbox_items SET state='resolved',reason=?2,updated_at=?3 \
             WHERE id=?1 AND state IN ('pending','running')",
            params![item_id, CLOSED_BY_NEWER_REVISION, at],
        )?;
    }
    Ok(newer)
}

/// After the judgment of `item_id` (`marked` = the rows its outcome update changed): a judged item
/// closes the older revisions of the same source that still wait for CoS (`pending`, `running`) or
/// sit with a route (`escalated`, `fallback`). They become `resolved` and their unsent
/// notifications are withdrawn (`superseded`); a sent notification stays sent. Returns whether the
/// judgment marked the item.
pub(crate) fn after_judgment(
    conn: &Connection,
    item_id: &str,
    marked: usize,
    at: &str,
) -> rusqlite::Result<bool> {
    const OLDER: &str = "SELECT o.id FROM cos_inbox_items o JOIN cos_inbox_items i ON i.id=?1 \
         WHERE o.source_kind=i.source_kind AND o.source_key=i.source_key AND o.id<>i.id \
         AND (o.created_at<i.created_at OR (o.created_at=i.created_at AND o.id<i.id)) \
         AND o.state IN ('pending','running','escalated','fallback')";
    if marked != 1 {
        return Ok(false);
    }
    conn.execute(
        &format!(
            "UPDATE notifications SET ok=0,error='superseded' WHERE ok IS NULL AND sent_at IS NULL \
             AND id IN (SELECT notification_id FROM cos_notification_routes WHERE item_id IN ({OLDER}))"
        ),
        [item_id],
    )?;
    conn.execute(
        &format!(
            "UPDATE cos_inbox_items SET state='resolved',reason=?2,updated_at=?3 WHERE id IN ({OLDER})"
        ),
        params![item_id, CLOSED_BY_NEWER_REVISION, at],
    )?;
    Ok(true)
}
