//! Helpers for `cos_ops_<domain>` integration tests. Uses a fresh live CoS credential,
//! checks direct 422 + rejected audit, then envelope applied + row + event + chat card.
use super::*;
use serde_json::{Value, json};
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

pub const OPS: &str = "/api/v1/cos/operations";

/// A live CoS run of a new thread: `(thread_id, run_id, "Bearer …")`.
pub fn cos_bearer(env: &TestEnv, key: &str) -> (String, String, String) {
    let thread = super::project_cos_thread(&env.store, &task_core::ProjectId::new().to_string());
    let bearer = cos_run_bearer(env, &thread.id, key);
    (thread.id, format!("run-{key}"), bearer)
}

/// A live CoS run `run-{key}` on an existing thread (a queued message, the claimed run, its
/// bearer with policy version 1): `"Bearer …"`.
pub fn cos_run_bearer(env: &TestEnv, thread_id: &str, key: &str) -> String {
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_message_post(
            thread_id,
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
    env.store
        .chat_run_claim_next(thread_id, &run, &json!({}), now)
        .expect("claim")
        .expect("run");
    issue_bearer(env, thread_id, &run, "1")
}

/// The bearer of an already claimed run, with `policy_version` recorded for it: `"Bearer …"`.
pub fn issue_bearer(env: &TestEnv, thread_id: &str, run: &str, policy_version: &str) -> String {
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread_id, run, Duration::hours(1))
        .expect("issue");
    env.store
        .chat_run_record_policy(run, policy_version, "test")
        .expect("policy");
    format!("Bearer {bearer}")
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

/// ADR 2026-11-12-cos-triage-scope D3: CoS's judgment inside the task, grounded on an ADR.
pub fn task_scope_basis() -> Value {
    json!({"category": "task_scope",
           "refs": [{"kind": "adr", "id": "agent-docs/adr/2026-11-12-cos-triage-scope.md"}]})
}

/// `body` (a resolve or `/cos/operations` body) with `basis` set.
pub fn with_basis(mut body: Value, basis: Value) -> Value {
    body["basis"] = basis;
    body
}

pub fn db(env: &TestEnv) -> rusqlite::Connection {
    rusqlite::Connection::open(&env.db_path).expect("open db")
}

/// `POST /cos/operations` with the CoS run's bearer.
pub async fn post_op(app: &axum::Router, bearer: &str, body: &Value) -> Resp {
    send(app, post_json_with(OPS, body, &[("authorization", bearer)])).await
}

/// Who answered the task's decisions (`DecisionAnswered.by`), oldest first.
pub fn decision_answered_by(env: &TestEnv, task: &task_core::Task) -> Vec<String> {
    env.store
        .events_for(task.id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::DecisionAnswered { by, .. } => Some(by),
            _ => None,
        })
        .collect()
}

/// An open planner `Choice` decision `id` (key `h1`, options vault/manual, recommended vault) on `task`.
pub fn raise_decision(env: &TestEnv, task: &task_core::Task, id: &str) {
    use task_core::decision::{
        CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
        DecisionRaisedBy, DecisionRequest, DecisionStatus,
    };
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    env.store
        .append_event(
            task.id,
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: id.into(),
                    key: "h1".into(),
                    kind: DecisionKind::Choice,
                    question: "which?".into(),
                    options: vec![opt("vault"), opt("manual")],
                    recommended: "vault".into(),
                    cost_of_reversal: CostOfReversal::Low,
                    cost_note: None,
                    needed_before: vec!["c".into()],
                    path: vec![DecisionPathEntry {
                        task_id: task.id,
                        title: "root".into(),
                        stage: Some("s1".into()),
                        unit: None,
                    }],
                    raised_by: DecisionRaisedBy {
                        task_id: task.id,
                        run_id: None,
                        origin: DecisionOrigin::Planner,
                    },
                    status: DecisionStatus::Open,
                    answer: None,
                    withdrawn_reason: None,
                }),
            },
        )
        .expect("raise");
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

/// [`run_domain`] for a request that answers a human wait (ADR 2026-10-28-cos-routine-delegated-
/// answers D2): `basis` is the routine basis, or `None` to cite the run's own human input message
/// as `instructed_by` (the person decided).
pub async fn run_domain_answering(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
    basis: Option<Value>,
) -> Value {
    run_domain_inner(env, key, method, path, body, action, None, Some(basis)).await
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
    run_domain_inner(
        env,
        key,
        method,
        path,
        body,
        action,
        expected_revision,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_domain_inner(
    env: &TestEnv,
    key: &str,
    method: &str,
    path: &str,
    body: Value,
    action: &str,
    expected_revision: Option<&str>,
    answering: Option<Option<Value>>,
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
    match &answering {
        None => {}
        Some(Some(basis)) => envelope["basis"] = basis.clone(),
        Some(None) => {
            let input: String = db(env)
                .query_row(
                    "SELECT id FROM chat_messages WHERE thread_id=?1 AND role='user'",
                    [&thread],
                    |row| row.get(0),
                )
                .expect("run input message");
            envelope["instructed_by"] = json!(input);
        }
    }
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
    // An answer to a wait prefixes the reason with its basis or the human instruction.
    let reason = audits[0]["reason"].as_str().unwrap_or_default();
    match answering {
        None => assert_eq!(reason, "人が依頼した", "{key}"),
        Some(Some(_)) => assert!(reason.starts_with("CoS の判断（"), "{key}: {reason}"),
        Some(None) => assert!(reason.starts_with("人の指示（"), "{key}: {reason}"),
    }
    assert!(reason.ends_with("人が依頼した"), "{key}: {reason}");
    for (field, want) in [
        ("actor", "cos"),
        ("thread_id", thread.as_str()),
        ("run_id", run.as_str()),
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
