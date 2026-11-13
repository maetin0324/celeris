use super::{OPERATOR_ROUTE, WITHDRAWN, take_operator_route};
use crate::chat::triage::{CosTriageSource, cos_triage_resolve_tx};
use crate::store::SqliteStore;
use time::OffsetDateTime;

fn at(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + n).expect("time")
}

fn source(key: &str) -> CosTriageSource {
    CosTriageSource {
        source_kind: "decision".into(),
        source_key: key.into(),
        source_revision: "1".into(),
        source_event_id: Some(7),
        operation_id: None,
        summary: "push the policy file to main?".into(),
        detail: String::new(),
        policy_version: "12".into(),
        task_id: None,
    }
}

/// The resolve API's operator escalation (`escalate_to_operator`) in one transaction: take the
/// operator route, then mark the item `escalated`. Returns what `take_operator_route` returned and
/// whether the mark landed.
fn escalate_to_operator(s: &SqliteStore, item: &str, n: i64) -> (bool, bool) {
    let mut conn = s.lock().expect("lock");
    let tx = conn.transaction().expect("tx");
    let stamp = format!("2026-11-12T00:00:{n:02}Z");
    let routed = take_operator_route(&tx, item, &stamp).expect("operator route");
    let marked = cos_triage_resolve_tx(
        &tx,
        item,
        "escalated",
        Some("op"),
        Some("outside Celeris"),
        at(n),
    )
    .expect("mark");
    tx.commit().expect("commit");
    (routed, marked)
}

/// ADR 2026-11-12-cos-triage-scope D2 (付記 2026-11-12) and ADR 2026-11-11-cos-fallback-timing D2:
/// CoS routes an item whose fallback the notifier has not sent to the external operator. The
/// fallback is withdrawn (`ok=0`, `error=WITHDRAWN`), its route row becomes the operator route with
/// no notification (`notification_id` NULL), no notification row is added, and the item is
/// `escalated`. A fallback already sent stays with the person: the route stays `fallback`.
/// The older tests could not catch the operator escalation leaving the fallback route in place:
/// `cos_triage_judgment_before_the_send_withdraws_the_fallback` covers only the escalation to the
/// person (`take_route`), and `operator_route_is_not_a_human_wait` takes the operator route of an
/// item without a fallback.
#[test]
fn cos_triage_operator_escalation_before_the_send_takes_the_fallback_route() {
    let s = SqliteStore::open_in_memory().expect("store");
    s.cos_triage_ingest_batch("decision", "1", &[source("u"), source("sent")], at(0))
        .expect("ingest");
    let id_of = |key: &str| -> String {
        s.lock()
            .expect("lock")
            .query_row(
                "SELECT id FROM cos_inbox_items WHERE source_key=?1",
                [key],
                |r| r.get(0),
            )
            .expect("id")
    };
    let (unsent, sent) = (id_of("u"), id_of("sent"));
    let mut fallback = Vec::new();
    for id in [&unsent, &sent] {
        fallback.push(
            s.cos_triage_outbox_claim(id, "fallback", "fixed text", at(1))
                .expect("claim")
                .expect("fallback"),
        );
        assert!(
            s.cos_triage_resolve(id, "fallback", None, Some("safety net"), at(1))
                .expect("mark fallback")
        );
    }
    let notifications = |s: &SqliteStore| -> i64 {
        s.lock()
            .expect("lock")
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .expect("count")
    };
    let notification = |id: &str| -> (Option<i64>, Option<String>) {
        s.lock()
            .expect("lock")
            .query_row(
                "SELECT ok,error FROM notifications WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("notification")
    };
    let route = |id: &str| -> (String, Option<String>) {
        s.lock()
            .expect("lock")
            .query_row(
                "SELECT route,notification_id FROM cos_notification_routes WHERE item_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("route")
    };
    let state = |id: &str| -> String {
        s.lock()
            .expect("lock")
            .query_row("SELECT state FROM cos_inbox_items WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .expect("state")
    };
    let before = notifications(&s);

    // Unsent: the operator route takes the fallback's row; the fallback is withdrawn.
    assert_eq!(escalate_to_operator(&s, &unsent, 2), (true, true));
    assert_eq!(state(&unsent), "escalated");
    assert_eq!(route(&unsent), (OPERATOR_ROUTE.to_owned(), None));
    assert_eq!(
        notification(&fallback[0]),
        (Some(0), Some(WITHDRAWN.to_owned()))
    );
    assert!(
        !s.cos_triage_outbox_sendable(&fallback[0], true, at(2))
            .expect("not sendable")
    );
    assert_eq!(notifications(&s), before);
    // A retry finds the route taken.
    {
        let conn = s.lock().expect("lock");
        assert!(!take_operator_route(&conn, &unsent, "2026-11-12T00:00:03Z").expect("retry"));
    }

    // Sent: the person has it; the operator route is not taken and nothing changes.
    s.lock()
        .expect("lock")
        .execute(
            "UPDATE notifications SET ok=1,sent_at='2026-11-12T00:00:01Z' WHERE id=?1",
            [&fallback[1]],
        )
        .expect("sent");
    assert_eq!(escalate_to_operator(&s, &sent, 4), (false, false));
    assert_eq!(state(&sent), "fallback");
    assert_eq!(
        route(&sent),
        ("fallback".to_owned(), Some(fallback[1].clone()))
    );
    assert_eq!(notification(&fallback[1]), (Some(1), None));
    assert_eq!(notifications(&s), before);
}
