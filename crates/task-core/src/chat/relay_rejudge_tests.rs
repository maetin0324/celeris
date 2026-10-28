//! ADR 2026-10-27-relayed-instructions 付記 A2: the store side of re-judging escalated items.
use super::relay_rejudge::RelayRejudge;
use super::triage::CosTriageSource;
use super::{ChatRelayRequest, ChatRunState};
use crate::store::SqliteStore;
use time::OffsetDateTime;

fn at(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + n).expect("time")
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

fn general(s: &SqliteStore) -> String {
    s.cos_thread_open(
        &crate::chat::ChatOpenThreadRequest { project_id: None },
        at(0),
    )
    .expect("general")
    .thread
    .id
}

fn relay(s: &SqliteStore, thread: &str, key: &str, now: OffsetDateTime, expires: Option<i64>) {
    s.chat_relay_post(
        thread,
        &ChatRelayRequest {
            client_message_id: key.into(),
            text: "それは続けてよい".into(),
            said_at: rfc3339(now),
            task_ids: Vec::new(),
            expires_at: expires.map(|n| rfc3339(at(n))),
        },
        "operator:claude-code",
        now,
    )
    .expect("relay");
}

/// One item claimed by a triage run and escalated; returns `(item id, triage run id)`.
fn escalated_item(s: &SqliteStore, thread: &str) -> (String, String) {
    s.cos_triage_ingest_batch(
        "events",
        "1",
        &[CosTriageSource {
            source_kind: "authorization".into(),
            source_key: "authorization-a1".into(),
            source_revision: "r1".into(),
            source_event_id: None,
            operation_id: None,
            summary: "node-hours".into(),
            detail: String::new(),
            policy_version: "6".into(),
            task_id: None,
        }],
        at(1),
    )
    .expect("ingest");
    let claim = s
        .cos_triage_claim(thread, "run-triage", at(2))
        .expect("claim")
        .expect("claimed");
    let item = claim.item_ids[0].clone();
    assert!(
        s.cos_triage_resolve(&item, "escalated", None, Some("人に回す"), at(3))
            .expect("resolve")
    );
    (item, claim.run_id)
}

/// R1/R2: a relay posted while the triage run runs makes the escalated item a candidate; the
/// claim waits for the run, then hands the item with the relay id once, and not again.
#[test]
fn relay_after_the_triage_run_started_rejudges_the_escalated_item_once_after_the_run() {
    let s = SqliteStore::open_in_memory().expect("store");
    let thread = general(&s);
    let (item, triage_run) = escalated_item(&s, &thread);
    assert!(
        s.cos_triage_relay_candidates(at(4))
            .expect("candidates")
            .is_empty(),
        "no relay yet"
    );
    relay(&s, &thread, "r-1", at(4), None);
    let candidates = s.cos_triage_relay_candidates(at(5)).expect("candidates");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].item_id, item);
    assert_eq!(candidates[0].source_key, "authorization-a1");
    let relay_id = candidates[0].relays[0].id.clone();
    let rejudge = [RelayRejudge {
        item_id: item.clone(),
        source_key: "authorization-a1".into(),
        relay_message_ids: vec![relay_id.clone()],
    }];

    // R2: the triage run is still running.
    assert!(
        s.cos_triage_relay_claim(&thread, "run-rejudge", &rejudge, at(5))
            .expect("claim")
            .is_none()
    );
    s.chat_run_finish(&triage_run, ChatRunState::Completed, None, None, at(6))
        .expect("finish");
    let claim = s
        .cos_triage_relay_claim(&thread, "run-rejudge", &rejudge, at(7))
        .expect("claim")
        .expect("claimed after the run");
    assert_eq!(claim.item_ids, vec![item.clone()]);
    let input = s
        .chat_message_get(&thread, &claim.message_id)
        .expect("input");
    let mut lines = input.text.lines();
    assert_eq!(
        lines.next(),
        Some(format!("CoS 受信箱の一次対応: item_ids=[\"{item}\"]").as_str())
    );
    assert_eq!(
        lines.next(),
        Some(
            format!(
                "取り次ぎによる再判断: [{{\"item_id\":\"{item}\",\"source_key\":\"authorization-a1\",\
                 \"relay_message_ids\":[\"{relay_id}\"]}}]"
            )
            .as_str()
        )
    );
    let report = s
        .cos_triage_item_report(&item)
        .expect("report")
        .expect("item");
    assert_eq!(report.state, "running");
    assert_eq!(report.run_id.as_deref(), Some("run-rejudge"));

    // Escalated again by the re-judging run: the same relay does not hand it over twice.
    assert!(
        s.cos_triage_resolve(&item, "escalated", None, Some("範囲外"), at(8))
            .expect("resolve")
    );
    s.chat_run_finish("run-rejudge", ChatRunState::Completed, None, None, at(9))
        .expect("finish");
    assert!(
        s.cos_triage_relay_candidates(at(10))
            .expect("candidates")
            .is_empty()
    );
}

/// R1: expired relays and items that are no longer escalated are not handed over.
#[test]
fn expired_relays_and_settled_items_are_not_rejudged() {
    let s = SqliteStore::open_in_memory().expect("store");
    let thread = general(&s);
    let (item, triage_run) = escalated_item(&s, &thread);
    s.chat_run_finish(&triage_run, ChatRunState::Completed, None, None, at(4))
        .expect("finish");
    relay(&s, &thread, "r-old", at(5), Some(20));
    assert_eq!(s.cos_triage_relay_candidates(at(10)).expect("c").len(), 1);
    assert!(
        s.cos_triage_relay_candidates(at(21))
            .expect("candidates")
            .is_empty(),
        "expired"
    );
    let rejudge = [RelayRejudge {
        item_id: item.clone(),
        source_key: "authorization-a1".into(),
        relay_message_ids: vec!["m".into()],
    }];
    s.lock()
        .expect("lock")
        .execute(
            "UPDATE cos_inbox_items SET state='resolved' WHERE id=?1",
            [&item],
        )
        .expect("settle");
    assert!(
        s.cos_triage_relay_claim(&thread, "run-x", &rejudge, at(11))
            .expect("claim")
            .is_none()
    );
}
