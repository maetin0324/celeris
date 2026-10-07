//! ADR 2026-10-05 cos-chat-home D4: a file pinned to a KB inbox candidate keeps its provenance
//! (hash, chat thread/message, the request text) on the candidate list and detail, and a pin to an
//! owner that does not exist is 404 on both the REST route and the CoS operation.
mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

const PDF: &[u8] = b"%PDF-1.4 fern03 manual";
const REQUEST: &str = "この PDF を KB に入れておいて";

fn env_with_attachments() -> TestEnv {
    let mut env = admin_env();
    env.state = env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        ChatAttachmentLimits::default(),
    );
    env
}

fn admin() -> String {
    format!("Bearer {TOKEN}")
}

/// A thread with one uploaded PDF; returns (thread id, attachment id).
async fn upload_pdf(env: &TestEnv, app: &axum::Router) -> (String, String) {
    let thread = env
        .store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "kb".into(),
                project_id: None,
                client_thread_id: "kb-thread".into(),
            },
            OffsetDateTime::now_utc(),
        )
        .expect("thread")
        .thread;
    let mut body = Vec::new();
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"manual.pdf\"\r\nContent-Type: application/pdf\r\n\r\n");
    body.extend_from_slice(PDF);
    body.extend_from_slice(b"\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\nup-1\r\n--boundary--\r\n");
    let request = Request::post(format!("/api/v1/chat/threads/{}/attachments", thread.id))
        .header("host", HOST)
        .header("authorization", admin())
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body))
        .expect("multipart");
    let uploaded = send(app, request).await;
    assert!(uploaded.status.is_success(), "{}", uploaded.text());
    let id = uploaded.json()["attachment"]["id"]
        .as_str()
        .expect("attachment id")
        .to_owned();
    (thread.id, id)
}

fn record_candidate(env: &TestEnv) -> String {
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    task_ops::knowledge::record(
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
    .expect("record")
    .id
}

async fn pin(app: &axum::Router, attachment: &str, kind: &str, owner: &str) -> Resp {
    send(
        app,
        post_json_with(
            &format!("/api/v1/chat/attachments/{attachment}/references"),
            &json!({"owner_kind": kind, "owner_id": owner, "idempotency_key": format!("pin-{owner}")}),
            &[("authorization", admin().as_str())],
        ),
    )
    .await
}

async fn get(app: &axum::Router, path: &str) -> Resp {
    send(app, get_with(path, &[("authorization", admin().as_str())])).await
}

fn assert_provenance(p: &Value, attachment: &str, thread: &str, message: &str) {
    assert_eq!(p["attachment_id"], attachment);
    assert_eq!(p["name"], "manual.pdf");
    assert_eq!(p["size_bytes"], PDF.len() as u64);
    assert_eq!(p["sha256"], format!("{:x}", Sha256::digest(PDF)));
    assert_eq!(p["thread_id"], thread);
    assert_eq!(p["message_id"], message);
    assert_eq!(p["request_text"], REQUEST);
    assert!(
        p["pinned_at"].as_str().is_some_and(|s| !s.is_empty()),
        "{p}"
    );
}

#[tokio::test]
async fn cos_chat_attach_handoff_kb_candidate_keeps_provenance() {
    let env = env_with_attachments();
    let app = env.router();
    let (thread, attachment) = upload_pdf(&env, &app).await;
    let message = env
        .store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "m-1".into(),
                text: REQUEST.into(),
                attachment_ids: vec![attachment.clone()],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("message")
        .response
        .message
        .id;
    let candidate = record_candidate(&env);
    let pinned = pin(&app, &attachment, "knowledge_inbox", &candidate).await;
    assert_eq!(pinned.status.as_u16(), 200, "{}", pinned.text());

    // The chat's files (blobs, staging, any run scratch) are gone; provenance is in SQLite.
    std::fs::remove_dir_all(env.dir.path().join("chat")).expect("remove chat files");

    let list = get(&app, "/api/v1/knowledge/inbox").await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let items = list.json()["items"].as_array().expect("items").clone();
    let item = items
        .iter()
        .find(|i| i["id"] == candidate.as_str())
        .expect("candidate listed");
    let provenance = item["provenance"].as_array().expect("provenance");
    assert_eq!(provenance.len(), 1, "{item}");
    assert_provenance(&provenance[0], &attachment, &thread, &message);

    let detail = get(&app, &format!("/api/v1/knowledge/inbox/{candidate}")).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    let detail = detail.json();
    assert_eq!(detail["id"], candidate.as_str());
    assert_eq!(detail["title"], "fern03 の使い方");
    assert_provenance(&detail["provenance"][0], &attachment, &thread, &message);

    // Unknown / out-of-bounds candidate ids are refused like accept / reject.
    let missing = get(&app, "/api/v1/knowledge/inbox/no-such-candidate").await;
    assert_problem(&missing, 404, "candidate_not_found");
    let escape = get(&app, "/api/v1/knowledge/inbox/..").await;
    assert_problem(&escape, 403, "path_forbidden");
}

#[tokio::test]
async fn cos_chat_attach_handoff_pin_missing_owner_404() {
    let env = env_with_attachments();
    let app = env.router();
    let (thread, attachment) = upload_pdf(&env, &app).await;
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");

    // REST route.
    let task = pin(&app, &attachment, "task", "01J00000000000000000000000").await;
    assert_problem(&task, 404, "task_not_found");
    let candidate = pin(&app, &attachment, "knowledge_inbox", "no-such-candidate").await;
    assert_problem(&candidate, 404, "candidate_not_found");

    // The audited CoS operation goes through the same owner check.
    env.store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "m-1".into(),
                text: REQUEST.into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("message");
    env.store
        .chat_run_claim_next(&thread, "run-kb", &json!({}), OffsetDateTime::now_utc())
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, &thread, "run-kb", Duration::hours(1))
        .expect("issue");
    let op = send(
        &app,
        post_json_with(
            "/api/v1/cos/operations",
            &json!({
                "idempotency_key": "pin-missing-kb",
                "expected_revision": null,
                "reason": "人が PDF を KB に入れるよう依頼した",
                "policy_version": "1",
                "request": {
                    "method": "POST",
                    "path": format!("/api/v1/chat/attachments/{attachment}/references"),
                    "body": {"owner_kind": "knowledge_inbox", "owner_id": "no-such-candidate", "idempotency_key": "pin-missing-kb"},
                },
            }),
            &[("authorization", format!("Bearer {bearer}").as_str())],
        ),
    )
    .await;
    assert_problem(&op, 404, "candidate_not_found");

    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    let pins: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_attachment_refs WHERE attachment_id=?1 AND owner_kind<>'message'",
            [&attachment],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(pins, 0);
}
