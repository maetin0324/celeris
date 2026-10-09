//! Helpers for `cos_ops_<domain>` integration tests. Uses a fresh live CoS credential,
//! checks direct 422 + rejected audit, then envelope applied + row + event + chat card.
use super::*;
use serde_json::{Value, json};
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

pub const OPS: &str = "/api/v1/cos/operations";

/// A live CoS run of a new thread: `(thread_id, run_id, "Bearer …")`.
pub fn cos_bearer(env: &TestEnv, key: &str) -> (String, String, String) {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: format!("thread {key}"),
                project_id: None,
                client_thread_id: key.into(),
            },
            now,
        )
        .expect("thread")
        .thread;
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "input".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    let run = format!("run-{key}");
    store
        .chat_run_claim_next(&thread.id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(store, &thread.id, &run, Duration::hours(1))
        .expect("issue");
    (thread.id, run, format!("Bearer {bearer}"))
}

pub fn op_body(key: &str, method: &str, path: &str, body: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人が依頼した",
        "policy_version": "1",
        "request": {"method": method, "path": path, "body": body},
    })
}

pub fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

/// One domain: the operation succeeds with `action`, leaves its audit envelope event and its chat
/// card, and the same request sent directly with the CoS credential is 422 without a domain write.
pub async fn run_domain(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
) -> Value {
    run_domain_with_revision(env, key, method, path, body, action, None).await
}

/// The same checks with a domain-specific envelope revision, for optimistic conflict tests.
pub async fn run_domain_with_revision(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
    expected_revision: Option<&str>,
) -> Value {
    let app = env.router();
    let (thread, run, bearer) = cos_bearer(env, key);
    let headers = [("authorization", bearer.as_str())];

    // Direct call first: the domain state stays untouched, so the operation below still applies.
    let direct = match method {
        "POST" => send(&app, post_json_with(path, &body, &headers)).await,
        "PATCH" => send(&app, patch_json_with(path, &body, &headers)).await,
        "PUT" => send(&app, put_json_with(path, &body, &headers)).await,
        "DELETE" => send(&app, delete_with(path, &headers)).await,
        other => panic!("unsupported mutation method {other}"),
    };
    let problem = assert_problem(&direct, 422, "cos_audit_context_required");
    assert_eq!(problem["instead"], OPS, "{key}");
    let direct_id: String = db(env)
        .query_row(
            "SELECT id FROM cos_operations WHERE thread_id=?1 AND state='rejected'",
            [&thread],
            |row| row.get(0),
        )
        .expect("direct rejection row");
    let events = audit_events(env, &direct_id);
    assert_eq!(events.len(), 1, "direct audit: {events:?}");
    assert_eq!(events[0]["state"], "rejected");
    assert_eq!(events[0]["actor"], "cos");

    let mut envelope = op_body(key, method, path, body);
    envelope["expected_revision"] = json!(expected_revision);
    let resp = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(resp.status.as_u16(), 200, "{key}: {}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{key}: {op}");
    assert_eq!(op["action"], action, "{key}");
    assert_eq!(op["actor"], "cos");
    assert_eq!(op["thread_id"], thread.as_str());
    assert_eq!(op["run_id"], run.as_str());
    let op_id = op["id"].as_str().expect("id").to_string();

    // The audit envelope event of this operation.
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    let audits: Vec<Value> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("json"))
        .filter(|e| e["type"] == "cos_operation" && e["operation_id"] == op_id.as_str())
        .collect();
    assert_eq!(audits.len(), 1, "{key}: {audits:?}");
    for (field, want) in [
        ("actor", "cos"),
        ("thread_id", thread.as_str()),
        ("run_id", run.as_str()),
        ("reason", "人が依頼した"),
        ("policy_version", "1"),
        ("state", "applied"),
    ] {
        assert_eq!(audits[0][field], want, "{key}: {field}");
    }
    // The chat card event the operation points at.
    let card: String = conn
        .query_row(
            "SELECT payload_json FROM chat_events WHERE thread_id=?1 AND type='card' AND id=?2",
            rusqlite::params![
                thread,
                op["event_id"]
                    .as_str()
                    .expect("event id")
                    .parse::<i64>()
                    .expect("int")
            ],
            |row| row.get(0),
        )
        .expect("card event");
    assert!(card.contains(&op_id), "{key}: {card}");
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_operations WHERE id=?1 AND state='applied'",
            [&op_id],
            |row| row.get(0),
        )
        .expect("row");
    assert_eq!(rows, 1, "{key}");
    op
}

/// Audit envelope events for one operation, for domain-specific assertions.
pub fn audit_events(env: &TestEnv, operation_id: &str) -> Vec<Value> {
    let conn = db(env);
    let mut stmt = conn
        .prepare("SELECT json FROM events ORDER BY seq")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|raw| serde_json::from_str::<Value>(&raw.expect("row")).expect("event JSON"))
        .filter(|event| event["type"] == "cos_operation" && event["operation_id"] == operation_id)
        .collect()
}

/// One external-effect (C) operation: the direct call is 422, the envelope settles `applied` with
/// a `pending` then `applied` audit event, and the same request resent returns the recorded
/// operation (the caller checks its effect ran once).
pub async fn run_external(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
) -> Value {
    let app = env.router();
    let (_, run, bearer) = cos_bearer(env, key);
    let headers = [("authorization", bearer.as_str())];
    let direct = match method {
        "POST" => send(&app, post_json_with(path, &body, &headers)).await,
        "PATCH" => send(&app, patch_json_with(path, &body, &headers)).await,
        "PUT" => send(&app, put_json_with(path, &body, &headers)).await,
        "DELETE" => send(&app, delete_with(path, &headers)).await,
        other => panic!("unsupported {other}"),
    };
    assert_problem(&direct, 422, "cos_audit_context_required");
    let envelope = op_body(key, method, path, body);
    let resp = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(resp.status.as_u16(), 200, "{key}: {}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{key}: {op}");
    assert_eq!(op["action"], action, "{key}");
    assert_eq!(op["run_id"], run.as_str());
    let states: Vec<Value> = audit_events(env, op["id"].as_str().expect("id"))
        .into_iter()
        .map(|e| e["state"].clone())
        .collect();
    assert_eq!(states, vec![json!("pending"), json!("applied")], "{key}");
    let again = send(&app, post_json_with(OPS, &envelope, &headers)).await;
    assert_eq!(again.status.as_u16(), 200, "{key} resend: {}", again.text());
    assert_eq!(again.json()["operation"]["id"], op["id"], "{key} resend");
    op
}
