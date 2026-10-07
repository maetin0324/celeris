//! ADR 2026-10-07 cos-live-fixes D1: `POST /api/v1/tasks` (and the CoS operation `task.create`)
//! takes `attachment_ids` and pins them in the transaction that creates the task, so the
//! dispatcher never sees the task without its pins. Any invalid id is 422 and creates nothing.
mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{Value, json};
use task_core::chat::attachments::{ChatAttachmentLimits, ChatAttachmentStore};
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

fn thread(env: &TestEnv, client_id: &str) -> String {
    env.store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: client_id.into(),
                project_id: None,
                client_thread_id: client_id.into(),
            },
            OffsetDateTime::now_utc(),
        )
        .expect("thread")
        .thread
        .id
}

/// Upload one attachment (admin) to `thread` and return its id.
async fn upload(app: &axum::Router, thread: &str, upload_id: &str) -> String {
    let mut body = Vec::new();
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\nContent-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(b"not really a png");
    body.extend_from_slice(
        format!("\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\n{upload_id}\r\n--boundary--\r\n")
            .as_bytes(),
    );
    let request = Request::post(format!("/api/v1/chat/threads/{thread}/attachments"))
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body))
        .expect("multipart");
    let uploaded = send(app, request).await;
    assert!(uploaded.status.is_success(), "{}", uploaded.text());
    uploaded.json()["attachment"]["id"]
        .as_str()
        .expect("attachment id")
        .to_owned()
}

/// Open a CoS run on `thread` and return its bearer.
fn cos_bearer(env: &TestEnv, thread: &str) -> String {
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_message_post(
            thread,
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
    env.store
        .chat_run_claim_next(thread, "run-d1", &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread, "run-d1", Duration::hours(1))
        .expect("issue");
    format!("Bearer {bearer}")
}

fn task_body(attachment_ids: &[&str]) -> Value {
    json!({
        "title": "画面修正",
        "objective": "添付の画面を直す",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        "attachment_ids": attachment_ids,
    })
}

fn count(env: &TestEnv, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}

fn task_pins(env: &TestEnv, task: &str) -> Vec<String> {
    let store = ChatAttachmentStore::open(
        env.dir.path(),
        &env.db_path,
        ChatAttachmentLimits::default(),
    )
    .expect("attachment store");
    // The dispatcher stages the first run's input manifest from exactly this list.
    store
        .list_for_owner("task", task)
        .expect("pins")
        .into_iter()
        .map(|row| row.id)
        .collect()
}

#[tokio::test]
async fn cos_live_fix_d1_created_task_already_has_its_pins() {
    let env = env_with_attachments();
    let app = env.router();
    let thread = thread(&env, "d1-human");
    let first = upload(&app, &thread, "up-1").await;
    let second = upload(&app, &thread, "up-2").await;

    let created = send(
        &app,
        post_admin("/api/v1/tasks", &task_body(&[&first, &second])),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let task = created.json()["id"].as_str().expect("task id").to_owned();
    assert_eq!(created.json()["status"], "ready");
    let mut pins = task_pins(&env, &task);
    pins.sort();
    let mut expected = vec![first.clone(), second.clone()];
    expected.sort();
    assert_eq!(pins, expected);
    // A pinned attachment no longer expires.
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM chat_attachments WHERE expires_at IS NOT NULL"
        ),
        0
    );

    // Omitting the field keeps the old behaviour (no pins).
    let plain = send(
        &app,
        post_admin(
            "/api/v1/tasks",
            &json!({"title":"t","objective":"o","acceptance":[{"type":"artifact_exists","name":"r.md"}]}),
        ),
    )
    .await;
    assert_eq!(plain.status.as_u16(), 201, "{}", plain.text());
    let plain_id = plain.json()["id"].as_str().expect("id").to_owned();
    assert!(task_pins(&env, &plain_id).is_empty());
}

#[tokio::test]
async fn cos_live_fix_d1_invalid_attachment_creates_no_task() {
    let env = env_with_attachments();
    let app = env.router();
    let thread = thread(&env, "d1-invalid");
    let good = upload(&app, &thread, "up-good").await;
    let expired = upload(&app, &thread, "up-expired").await;
    {
        let conn = rusqlite::Connection::open(&env.db_path).expect("db");
        conn.execute(
            "UPDATE chat_attachments SET expires_at='2000-01-01T00:00:00.000000000Z' WHERE id=?1",
            [&expired],
        )
        .expect("expire");
    }
    let unknown = "01J00000000000000000000000";
    for ids in [
        vec![good.as_str(), unknown],
        vec![good.as_str(), expired.as_str()],
        vec![good.as_str(), good.as_str()],
        vec!["not-an-id"],
    ] {
        let resp = send(&app, post_admin("/api/v1/tasks", &task_body(&ids))).await;
        assert_problem(&resp, 422, "invalid_attachment");
    }
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 0);
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM chat_attachment_refs WHERE owner_kind='task'"
        ),
        0
    );
}

#[tokio::test]
async fn cos_live_fix_d1_cos_task_create_pins_and_audits() {
    let env = env_with_attachments();
    let app = env.router();
    let own = thread(&env, "d1-cos");
    let other = thread(&env, "d1-other");
    let attachment = upload(&app, &own, "up-own").await;
    let foreign = upload(&app, &other, "up-foreign").await;
    let bearer = cos_bearer(&env, &own);
    let op = |key: &str, ids: &[&str]| {
        json!({
            "idempotency_key": key,
            "expected_revision": null,
            "reason": "人がチャットで添付つきで依頼した",
            "policy_version": "1",
            "request": {"method": "POST", "path": "/api/v1/tasks", "body": task_body(ids)},
        })
    };

    // An attachment from another thread is refused, recorded as rejected, and creates nothing.
    let refused = send(
        &app,
        post_json_with(
            OPS,
            &op("create-foreign", &[&attachment, &foreign]),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&refused, 422, "invalid_attachment");
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 0);
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE idempotency_key='create-foreign' AND state='rejected'"
        ),
        1
    );

    let created = send(
        &app,
        post_json_with(
            OPS,
            &op("create-1", &[&attachment]),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 200, "{}", created.text());
    let operation = &created.json()["operation"];
    assert_eq!(operation["action"], "task.create");
    assert_eq!(operation["state"], "applied");
    assert_eq!(operation["result"]["attachment_ids"], json!([attachment]));
    let task = operation["result"]["task_id"]
        .as_str()
        .expect("task id")
        .to_owned();
    assert_eq!(task_pins(&env, &task), vec![attachment.clone()]);
    assert_eq!(count(&env, "SELECT COUNT(*) FROM tasks"), 1);
}
