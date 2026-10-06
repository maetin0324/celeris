use super::*;
use crate::store::SqliteStore;
use rusqlite::params;
use serde_json::json;
use ulid::Ulid;

fn setup() -> (SqliteStore, AuditContext) {
    let store = SqliteStore::open_in_memory().expect("store");
    let ctx = AuditContext {
        actor: ChatActor::Cos,
        thread_id: "thread".into(),
        run_id: "run".into(),
        operation_id: Ulid::new().to_string(),
        reason: "user requested change".into(),
        policy_version: "v1".into(),
    };
    {
        let conn = store.lock().expect("lock");
        conn.execute(
            "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) \
            VALUES('thread','human','Test','open','now','now')",
            [],
        )
        .expect("thread");
        conn.execute("CREATE TABLE marker(value TEXT)", [])
            .expect("marker");
    }
    (store, ctx)
}

fn counts(s: &SqliteStore) -> (i64, i64, i64, i64) {
    let conn = s.lock().expect("lock");
    let count = |table: &str| {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .expect("count")
    };
    (
        count("marker"),
        count("cos_operations"),
        count("events"),
        count("chat_events"),
    )
}

#[test]
fn cos_chat_ops_store_idempotent_same_hash_and_conflict_on_different_hash() {
    let (s, ctx) = setup();
    let first = s
        .cos_operation_apply(
            &ctx,
            "key",
            "hash",
            "project",
            "target",
            Some("1"),
            "edit",
            &json!({}),
            |tx, _| {
                tx.execute("INSERT INTO marker(value) VALUES('once')", [])?;
                Ok(json!({"done":true}))
            },
        )
        .expect("first");
    assert_eq!(first.state, "applied");
    let replay = s
        .cos_operation_apply(
            &ctx,
            "key",
            "hash",
            "project",
            "target",
            Some("1"),
            "edit",
            &json!({}),
            |_, _| panic!("replay must not apply"),
        )
        .expect("replay");
    assert_eq!(first, replay);
    let mut forged = ctx.clone();
    forged.actor = ChatActor::Human;
    assert_eq!(
        s.cos_operation_apply(
            &forged,
            "key",
            "hash",
            "project",
            "target",
            Some("1"),
            "edit",
            &json!({}),
            |_, _| panic!("forged replay must not apply"),
        )
        .expect_err("actor cannot replay")
        .http_status(),
        422
    );
    assert_eq!(
        s.cos_operation_apply(
            &ctx,
            "key",
            "changed",
            "project",
            "target",
            Some("1"),
            "edit",
            &json!({}),
            |_, _| { panic!("conflict must not apply") }
        )
        .expect_err("conflict")
        .http_status(),
        409
    );
    assert_eq!(counts(&s), (1, 1, 1, 1));
}

#[test]
fn cos_chat_ops_store_failed_apply_rolls_back_and_records_rejection() {
    let (s, ctx) = setup();
    let error = s
        .cos_operation_apply(
            &ctx,
            "key",
            "hash",
            "project",
            "target",
            Some("1"),
            "edit",
            &json!({}),
            |tx, _| {
                tx.execute("INSERT INTO marker(value) VALUES('rolled back')", [])?;
                Err(ChatError::Conflict("revision changed".into()))
            },
        )
        .expect_err("failed apply");
    assert_eq!(error.http_status(), 409);
    assert_eq!(counts(&s), (0, 1, 1, 0));
    let row = s
        .cos_operation_get(&ctx.operation_id)
        .expect("get")
        .expect("row");
    assert_eq!(row.state, "rejected");
    assert!(
        row.result
            .expect("reason")
            .to_string()
            .contains("revision changed")
    );
}

#[test]
fn cos_chat_ops_store_event_has_complete_audit_envelope() {
    let (s, ctx) = setup();
    s.cos_operation_apply(
        &ctx,
        "key",
        "hash",
        "project",
        "target",
        Some("1"),
        "edit",
        &json!({}),
        |_, _| Ok(json!({"ok":true})),
    )
    .expect("apply");
    let conn = s.lock().expect("lock");
    let event: String = conn
        .query_row("SELECT json FROM events", [], |r| r.get(0))
        .expect("event");
    let value: serde_json::Value = serde_json::from_str(&event).expect("json");
    for (name, expected) in [
        ("actor", "cos"),
        ("thread_id", "thread"),
        ("run_id", "run"),
        ("operation_id", ctx.operation_id.as_str()),
        ("reason", ctx.reason.as_str()),
        ("policy_version", "v1"),
    ] {
        assert_eq!(value[name], expected);
    }
    let (kind, payload): (String, String) = conn
        .query_row(
            "SELECT type,payload_json FROM chat_events",
            params![],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("card");
    assert_eq!(kind, "card");
    assert!(payload.contains(&ctx.operation_id));
}

#[test]
fn cos_chat_ops_store_invalid_reason_is_rejected_with_audit_reason() {
    let (s, mut ctx) = setup();
    ctx.reason.clear();
    let error = s
        .cos_operation_apply(
            &ctx,
            "key",
            "hash",
            "project",
            "target",
            Some("1"),
            "edit",
            &json!({}),
            |_, _| panic!("invalid request must not apply"),
        )
        .expect_err("reason missing");
    assert_eq!(error.http_status(), 422);
    assert_eq!(counts(&s), (0, 1, 1, 0));
    let conn = s.lock().expect("lock");
    let event: String = conn
        .query_row("SELECT json FROM events", [], |r| r.get(0))
        .expect("event");
    let value: serde_json::Value = serde_json::from_str(&event).expect("json");
    assert_eq!(value["state"], "rejected");
    assert!(
        value["reason"]
            .as_str()
            .expect("reason")
            .contains("reason is required")
    );
}
