//! ADR 2026-10-05 cos-chat-home D4: a CoS run pins a chat attachment to the task or KB inbox
//! candidate it created, through the audited `/cos/operations` (the direct references route is 422
//! for a CoS credential).
mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{Value, json};
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

const OPS: &str = "/api/v1/cos/operations";

fn env_with_attachments() -> TestEnv {
    let mut env = admin_env();
    env.state = env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        ChatAttachmentLimits::default(),
    );
    env
}

/// Upload one attachment (admin) to a new thread, then open a CoS run on that thread.
async fn attachment_and_bearer(env: &TestEnv, app: &axum::Router) -> (String, String) {
    let store = &env.store;
    let now = OffsetDateTime::now_utc();
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "pin".into(),
                project_id: None,
                client_thread_id: "pin-thread".into(),
            },
            now,
        )
        .expect("thread")
        .thread;
    let mut body = Vec::new();
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\nContent-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(b"not really a png");
    body.extend_from_slice(b"\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\nup-1\r\n--boundary--\r\n");
    let request = Request::post(format!("/api/v1/chat/threads/{}/attachments", thread.id))
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body))
        .expect("multipart");
    let uploaded = send(app, request).await;
    assert!(uploaded.status.is_success(), "{}", uploaded.text());
    let id = uploaded.json()["attachment"]["id"]
        .as_str()
        .expect("attachment id")
        .to_owned();
    store
        .chat_message_post(
            &thread.id,
            &ChatPostMessageRequest {
                client_message_id: "m-1".into(),
                text: "この画面を直して".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    store
        .chat_run_claim_next(&thread.id, "run-pin", &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(store, &thread.id, "run-pin", Duration::hours(1))
        .expect("issue");
    (id, format!("Bearer {bearer}"))
}

fn op_body(key: &str, path: &str, body: Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人がチャットで添付つきで依頼した",
        "policy_version": "1",
        "request": {"method": "POST", "path": path, "body": body},
    })
}

async fn post_op(app: &axum::Router, bearer: &str, body: &Value) -> Resp {
    send(app, post_json_with(OPS, body, &[("authorization", bearer)])).await
}

fn refs(env: &TestEnv, attachment: &str) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    let mut stmt = conn
        .prepare("SELECT owner_kind, owner_id FROM chat_attachment_refs WHERE attachment_id=?1 AND owner_kind<>'message' ORDER BY owner_kind")
        .expect("prepare");
    stmt.query_map([attachment], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

#[tokio::test]
async fn cos_chat_attach_handoff_cos_pins_attachment_to_created_task_and_kb_candidate() {
    let env = env_with_attachments();
    let app = env.router();
    let (attachment, bearer) = attachment_and_bearer(&env, &app).await;
    let refs_path = format!("/api/v1/chat/attachments/{attachment}/references");

    // The direct references route stays closed to a CoS credential.
    let direct = send(
        &app,
        post_json_with(
            &refs_path,
            &json!({"owner_kind":"task","owner_id":"x","idempotency_key":"k"}),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&direct, 422, "cos_audit_context_required");

    // A pin to an owner that does not exist yet is refused (create the owner first).
    let missing = post_op(
        &app,
        &bearer,
        &op_body(
            "pin-missing",
            &refs_path,
            json!({"owner_kind":"task","owner_id":"01J00000000000000000000000","idempotency_key":"pin-missing"}),
        ),
    )
    .await;
    assert_problem(&missing, 404, "task_not_found");

    // Create the task, then pin the screenshot to it.
    let created = post_op(
        &app,
        &bearer,
        &op_body(
            "create-1",
            "/api/v1/tasks",
            json!({"title":"画面修正","objective":"添付の画面を直す","acceptance":[{"type":"artifact_exists","name":"result.md"}]}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 200, "{}", created.text());
    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    let task: String = conn
        .query_row("SELECT id FROM tasks", [], |r| r.get(0))
        .expect("task id");
    let pin = op_body(
        "pin-task",
        &refs_path,
        json!({"owner_kind":"task","owner_id":task,"idempotency_key":"pin-task"}),
    );
    let pinned = post_op(&app, &bearer, &pin).await;
    assert_eq!(pinned.status.as_u16(), 200, "{}", pinned.text());
    let op = &pinned.json()["operation"];
    assert_eq!(op["action"], "attachment.reference");
    assert_eq!(op["state"], "applied");
    assert_eq!(op["result"]["owner_kind"], "task");
    assert_eq!(op["result"]["owner_id"], task.as_str());
    // Replaying the same operation returns the same record and adds no second row.
    let again = post_op(&app, &bearer, &pin).await;
    assert_eq!(again.json()["operation"]["id"], op["id"]);

    // Record a KB candidate, then pin the attachment to it.
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let candidate = task_ops::knowledge::record(
        &env.knowledge_root,
        &task_ops::knowledge::RecordRequest {
            title: "fern03 の使い方".into(),
            scope: "environment".into(),
            tags: vec![],
            sources: vec!["human:instruction".into()],
            confidence: None,
            body: "添付の PDF の要点。".into(),
            path: Some("environment/servers/fern03.md".into()),
            op: None,
        },
    )
    .expect("record");
    let kb = post_op(
        &app,
        &bearer,
        &op_body(
            "pin-kb",
            &refs_path,
            json!({"owner_kind":"knowledge_inbox","owner_id":candidate.id,"idempotency_key":"pin-kb"}),
        ),
    )
    .await;
    assert_eq!(kb.status.as_u16(), 200, "{}", kb.text());
    assert_eq!(
        refs(&env, &attachment),
        vec![
            ("knowledge_inbox".to_string(), candidate.id.clone()),
            ("task".to_string(), task.clone()),
        ]
    );
}
