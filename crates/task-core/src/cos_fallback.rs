//! ADR 2026-11-11-cos-fallback-timing D2: a CoS fallback notification that the notifier has not
//! sent yields to CoS's judgment. SQL over `cos_inbox_items` and `notifications` only, so the chat
//! triage store calls it inside its own transactions and the resolve API reads it.

use rusqlite::{Connection, params};

/// `notifications.error` of a fallback withdrawn because CoS judged the item before the send.
pub const WITHDRAWN: &str = "withdrawn: CoS judged before the send";

/// The fallback handed the item to the person but the notifier has not sent it (or it was already
/// withdrawn for a CoS judgment). A sent fallback stays with the person.
pub fn unsent(conn: &Connection, item_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM cos_inbox_items i JOIN notifications n \
         ON n.kind='cos_fallback' AND n.key=i.id WHERE i.id=?1 AND i.state='fallback' \
         AND n.sent_at IS NULL AND (n.ok IS NULL OR n.error=?2))",
        params![item_id, WITHDRAWN],
        |r| r.get(0),
    )
}

/// Withdraw the unsent fallback notification of `item_id`; `false` when there is none (never
/// claimed, or already sent). A send already in flight (the POST under way) is not stopped. The
/// write comes first: a deferred transaction that reads before it writes fails with
/// `SQLITE_BUSY_SNAPSHOT` when another connection commits in between.
pub fn withdraw_unsent(conn: &Connection, item_id: &str) -> rusqlite::Result<bool> {
    conn.execute(
        "UPDATE notifications SET ok=0,error=?2 WHERE kind='cos_fallback' AND key=?1 \
         AND ok IS NULL AND sent_at IS NULL \
         AND EXISTS(SELECT 1 FROM cos_inbox_items WHERE id=?1 AND state='fallback')",
        params![item_id, WITHDRAWN],
    )?;
    unsent(conn, item_id)
}

/// The state a CoS judgment (`outcome`) takes the item from: `fallback` when the judgment withdrew
/// the item's unsent fallback, else `running` (the item is pending or running).
pub fn judged_from(
    conn: &Connection,
    item_id: &str,
    outcome: &str,
) -> rusqlite::Result<&'static str> {
    let judged =
        matches!(outcome, "answered" | "observed" | "escalated") && withdraw_unsent(conn, item_id)?;
    Ok(if judged { "fallback" } else { "running" })
}

/// D2: CoS routes an item whose fallback is unsent (to the person or to the operator): the
/// fallback is withdrawn and `route` takes the fallback's route row with `notification_id`
/// (`Some(rows)`; 0 when the row was already taken). `None` when there is no unsent fallback.
fn take_unsent_fallback(
    conn: &Connection,
    item_id: &str,
    route: &str,
    notification_id: Option<&str>,
    at: &str,
) -> rusqlite::Result<Option<usize>> {
    if !withdraw_unsent(conn, item_id)? {
        return Ok(None);
    }
    conn.execute(
        "UPDATE cos_notification_routes SET route=?2,notification_id=?3,updated_at=?4 \
         WHERE item_id=?1 AND route='fallback'",
        params![item_id, route, notification_id, at],
    )
    .map(Some)
}

/// CoS escalates an item whose fallback is unsent: the fallback is withdrawn and the escalation
/// takes its route row (`Some(rows)`). `None` leaves the claim to the normal insert.
pub fn take_route(
    conn: &Connection,
    route: &str,
    item_id: &str,
    notification_id: &str,
    at: &str,
) -> rusqlite::Result<Option<usize>> {
    if route != "escalation" {
        return Ok(None);
    }
    take_unsent_fallback(conn, item_id, route, Some(notification_id), at)
}

/// ADR 2026-11-12-cos-triage-scope D2: `cos_notification_routes.route` of an item CoS routed to the
/// external operator. The route has no notification (nothing is sent to Discord).
pub const OPERATOR_ROUTE: &str = "operator";

/// D2: whether a `cos_notification_routes.route` is the operator route. Such an item is not a
/// person's wait: the digest, the web cards and the request ledger keep it apart.
pub fn is_operator_route(route: &str) -> bool {
    route == OPERATOR_ROUTE
}

/// D2: take the operator route of `item_id` (in the caller's transaction). An unsent fallback is
/// withdrawn and its route row becomes the operator route (no notification), as an escalation to
/// the person takes it in [`take_route`]. `false` when the item already has a route (a retried
/// escalate, or a fallback the notifier sent).
pub fn take_operator_route(conn: &Connection, item_id: &str, at: &str) -> rusqlite::Result<bool> {
    if let Some(taken) = take_unsent_fallback(conn, item_id, OPERATOR_ROUTE, None, at)? {
        return Ok(taken > 0);
    }
    let claimed = conn.execute(
        "INSERT INTO cos_notification_routes(item_id,notification_id,route,source_kind,source_key,\
         source_revision,created_at,updated_at) SELECT id,NULL,?2,source_kind,source_key,\
         source_revision,?3,?3 FROM cos_inbox_items WHERE id=?1 ON CONFLICT DO NOTHING",
        params![item_id, OPERATOR_ROUTE, at],
    )?;
    Ok(claimed > 0)
}

#[cfg(test)]
#[path = "cos_fallback_tests.rs"]
mod tests;
