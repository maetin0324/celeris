use super::triage::CosTriageSource;
use crate::notify::NotificationStore;
use crate::store::SqliteStore;
use rusqlite::params;
use time::OffsetDateTime;

fn at(n: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + n).expect("time")
}
fn item(key: &str, revision: &str) -> CosTriageSource {
    CosTriageSource {
        source_kind: "decision".into(),
        source_key: key.into(),
        source_revision: revision.into(),
        source_event_id: Some(7),
        operation_id: None,
        summary: "approve?".into(),
        policy_version: "v1".into(),
    }
}
fn count(s: &SqliteStore, table: &str) -> i64 {
    s.lock()
        .expect("lock")
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .expect("count")
}

#[test]
fn cos_chat_triage_store_ingest_is_atomic_idempotent_and_skips_cos_operations() {
    let s = SqliteStore::open_in_memory().expect("store");
    assert_eq!(
        s.cos_triage_ingest_batch("decisions", "7", &[item("a", "1")], at(0))
            .expect("ingest"),
        1
    );
    let original: (String, String) = s
        .lock()
        .expect("lock")
        .query_row("SELECT id,updated_at FROM cos_inbox_items", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("row");
    assert_eq!(
        s.cos_triage_ingest_batch("decisions", "8", &[item("a", "1")], at(10))
            .expect("replay"),
        0
    );
    assert_eq!(
        s.lock()
            .expect("lock")
            .query_row("SELECT id,updated_at FROM cos_inbox_items", [], |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?
            )))
            .expect("row"),
        original
    );
    let mut cos = item("b", "1");
    cos.operation_id = Some("op".into());
    assert_eq!(
        s.cos_triage_ingest_batch("decisions", "9", &[cos], at(11))
            .expect("skip"),
        0
    );
    let bad = item("", "2");
    assert!(
        s.cos_triage_ingest_batch("decisions", "10", &[item("c", "1"), bad], at(12))
            .is_err()
    );
    assert_eq!(count(&s, "cos_inbox_items"), 1);
    let cursor: String = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT value FROM feed_cursor WHERE name='cos_triage:decisions'",
            [],
            |r| r.get(0),
        )
        .expect("cursor");
    assert_eq!(cursor, "9");
}

#[test]
fn cos_chat_triage_store_reconcile_claim_and_resolve() {
    let s = SqliteStore::open_in_memory().expect("store");
    let items: Vec<_> = (0..22).map(|i| item(&format!("d{i}"), "1")).collect();
    assert_eq!(
        s.cos_triage_reconcile("decision", &items, at(0))
            .expect("reconcile"),
        22
    );
    let claim = s
        .cos_triage_claim("run-1", at(1))
        .expect("claim")
        .expect("items");
    assert_eq!(claim.item_ids.len(), 20);
    assert!(
        s.cos_triage_claim("run-2", at(1))
            .expect("active run")
            .is_none()
    );
    let (input,text): (String,String) = s.lock().expect("lock").query_row("SELECT r.input_message_id,m.text FROM chat_runs r JOIN chat_messages m ON m.id=r.input_message_id WHERE r.run_id='run-1'", [], |r| Ok((r.get(0)?,r.get(1)?))).expect("run message");
    assert_eq!(input, claim.message_id);
    for id in &claim.item_ids {
        assert!(text.contains(id));
    }
    assert!(
        s.cos_triage_resolve(
            &claim.item_ids[0],
            "answered",
            Some("op"),
            Some("safe"),
            at(2)
        )
        .expect("resolve")
    );
    assert!(
        !s.cos_triage_resolve(&claim.item_ids[0], "answered", None, None, at(3))
            .expect("terminal")
    );
    assert_eq!(
        s.cos_triage_reconcile("decision", &[], at(4))
            .expect("empty"),
        0
    );
    assert_eq!(count(&s, "cos_inbox_items"), 22);
}

#[test]
fn cos_chat_triage_store_outbox_unique_and_withdraws_before_send() {
    let s = SqliteStore::open_in_memory().expect("store");
    s.cos_triage_ingest_batch("decision", "1", &[item("a", "1")], at(0))
        .expect("ingest");
    let id: String = s
        .lock()
        .expect("lock")
        .query_row("SELECT id FROM cos_inbox_items", [], |r| r.get(0))
        .expect("id");
    let notification = s
        .cos_triage_outbox_claim(&id, "fallback", "body", at(1))
        .expect("fallback")
        .expect("claim");
    assert!(
        s.cos_triage_outbox_claim(&id, "escalation", "other", at(2))
            .expect("race")
            .is_none()
    );
    assert_eq!(count(&s, "notifications"), 1);
    assert_eq!(s.notification_pending().expect("outbox reader").len(), 1);
    assert!(
        s.cos_triage_outbox_sendable(&notification, true, at(3))
            .expect("sendable")
    );
    assert!(
        !s.cos_triage_outbox_sendable(&notification, false, at(4))
            .expect("withdraw")
    );
    let (ok, error): (i64, String) = s
        .lock()
        .expect("lock")
        .query_row(
            "SELECT ok,error FROM notifications WHERE id=?1",
            [notification],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("notification");
    assert_eq!((ok, error), (0, "superseded".into()));
}

#[test]
fn cos_chat_triage_store_cutover_supersedes_only_old_pending_once() {
    let s = SqliteStore::open_in_memory().expect("store");
    let pending = ulid::Ulid::new().to_string();
    let sent = ulid::Ulid::new().to_string();
    {
        let conn = s.lock().expect("lock");
        conn.execute(
            "INSERT INTO notifications(id,kind,key,created_at) VALUES(?1,'inbox_new','a','now')",
            params![pending],
        )
        .expect("pending");
        conn.execute("INSERT INTO notifications(id,kind,key,created_at,sent_at,ok) VALUES(?1,'inbox_new','b','now','now',1)",params![sent]).expect("sent");
    }
    let links = vec![
        (pending.clone(), "decision".into(), "a".into(), "1".into()),
        (sent.clone(), "decision".into(), "b".into(), "1".into()),
    ];
    assert!(
        s.cos_triage_cutover(
            "decision",
            "10",
            &[item("a", "1"), item("b", "1")],
            &links,
            at(0)
        )
        .expect("cutover")
    );
    assert!(
        !s.cos_triage_cutover("decision", "11", &[], &[], at(1))
            .expect("replay")
    );
    let conn = s.lock().expect("lock");
    assert_eq!(
        conn.query_row("SELECT ok FROM notifications WHERE id=?1", [pending], |r| r
            .get::<_, i64>(0))
            .expect("old pending"),
        0
    );
    assert_eq!(
        conn.query_row("SELECT ok FROM notifications WHERE id=?1", [sent], |r| r
            .get::<_, i64>(0))
            .expect("sent"),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM cos_legacy_notification_links",
            [],
            |r| r.get::<_, i64>(0)
        )
        .expect("links"),
        2
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM feed_cursor WHERE name='cos_triage:decision'",
            [],
            |r| r.get::<_, String>(0)
        )
        .expect("cursor"),
        "10"
    );
}

#[test]
fn cos_chat_triage_store_report_open_items_and_digest_bookkeeping() {
    use super::triage::{COS_TRIAGE_DIGEST_KEY_PREFIX, decision_from_outbox};
    use crate::chat::{ChatActor, ChatCard, ChatCardKind, ChatRunState};
    let s = SqliteStore::open_in_memory().expect("store");
    let mut b = item("b", "1");
    b.summary = "second".into();
    s.cos_triage_ingest_batch("decisions", "7", &[item("a", "1"), b], at(0))
        .expect("ingest");
    let claim = s
        .cos_triage_claim("run-1", at(1))
        .expect("claim")
        .expect("items");
    assert_eq!(claim.item_ids.len(), 2);
    let (a, b) = (&claim.item_ids[0], &claim.item_ids[1]);
    let report = s.cos_triage_run_report("run-1").expect("report");
    assert_eq!(report.len(), 2);
    assert_eq!(report[0].summary, "approve?");
    assert_eq!(report[1].title(), "second");
    assert_eq!(report[0].state, "running");
    assert!(report[0].decision.is_none());

    let body = serde_json::json!({"summary":"公開前の確認","options":[{"key":"go","label":"公開する"},{"key":"hold","label":"保留"}],"recommended":"hold","recommendation_reason":"未確認","web_path":"/tasks/t1"});
    s.cos_triage_outbox_claim(a, "escalation", &body.to_string(), at(2))
        .expect("claim")
        .expect("routed");
    assert!(
        s.cos_triage_resolve(a, "escalated", Some("op-1"), Some("人の判断"), at(2))
            .expect("resolve")
    );
    let report = s.cos_triage_run_report("run-1").expect("report");
    let d = report[0].decision.as_ref().expect("packet");
    assert_eq!(d.summary, "公開前の確認");
    assert_eq!(d.options.len(), 2);
    assert_eq!(d.options[1].label, "保留");
    assert_eq!(d.recommended.as_deref(), Some("hold"));
    assert_eq!(d.web_path, "/tasks/t1");
    assert_eq!(report[0].route.as_deref(), Some("escalation"));
    assert_eq!(report[0].reason.as_deref(), Some("人の判断"));
    let fb = decision_from_outbox(
        r#"{"summary":"x","options":["公開する","保留"],"recommended":"保留","recommendation_text":"CoS の推奨なし","web_path":"/inbox"}"#,
    )
    .expect("fallback packet");
    assert_eq!(fb.options[0].key, "公開する");
    assert_eq!(fb.recommendation_reason.as_deref(), Some("CoS の推奨なし"));

    let open = s.cos_triage_open_items(10).expect("open");
    assert_eq!(open.len(), 2);
    assert!(open.iter().any(|i| &i.item_id == a && i.state == "escalated"));
    assert!(open.iter().any(|i| &i.item_id == b && i.state == "running"));
    assert_eq!(
        s.cos_triage_item_report(a).expect("one").expect("row").item_id,
        *a
    );

    assert!(s.cos_triage_runs_without_digest(10).expect("scan").is_empty());
    s.chat_run_finish("run-1", ChatRunState::Completed, None, None, at(3))
        .expect("finish");
    assert_eq!(
        s.cos_triage_runs_without_digest(10).expect("scan"),
        vec![("run-1".to_string(), "completed".to_string())]
    );
    let card = ChatCard {
        kind: ChatCardKind::Decision,
        id: "a".into(),
        title: "approve?".into(),
        state: "escalated".into(),
        href: "/tasks/t1".into(),
        actor: ChatActor::Cos,
        reason: None,
        operation_id: Some("op-1".into()),
    };
    let key = format!("{COS_TRIAGE_DIGEST_KEY_PREFIX}run-1");
    let m = s
        .chat_assistant_message_add_once(
            &claim.thread_id,
            &key,
            "digest",
            std::slice::from_ref(&card),
            Some("run-1"),
            at(4),
        )
        .expect("digest");
    assert_eq!(m.role, crate::chat::ChatMessageRole::Assistant);
    assert_eq!(m.run_id.as_deref(), Some("run-1"));
    assert_eq!(m.cards, vec![card.clone()]);
    let again = s
        .chat_assistant_message_add_once(&claim.thread_id, &key, "digest", &[card], Some("run-1"), at(5))
        .expect("idempotent");
    assert_eq!(again.id, m.id);
    assert!(s.cos_triage_runs_without_digest(10).expect("scan").is_empty());
    assert_eq!(
        s.chat_message_get(&claim.thread_id, &m.id).expect("get").text,
        "digest"
    );
}
