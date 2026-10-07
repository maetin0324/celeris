//! ADR 2026-10-07 cos-live-fixes D2: `POST /api/v1/knowledge/inbox` creates a KB candidate
//! (`_inbox/<id>.md`) with the chat attachments it names pinned as provenance, and the CoS runs it
//! as the audited operation `knowledge.record`. The scope is `project:<slug>` (`projects/<slug>` is
//! normalized), an unknown scope is 422, and a failed pin leaves no candidate file.
mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{Value, json};
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::{ChatCreateThreadRequest, ChatPostMessageRequest, ChatSendMode};
use time::{Duration, OffsetDateTime};

const OPS: &str = "/api/v1/cos/operations";
const PDF: &[u8] = b"%PDF-1.4 fern03 manual";

fn env_with_kb() -> TestEnv {
    let mut env = admin_env();
    env.state = env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        ChatAttachmentLimits::default(),
    );
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    env
}

fn admin() -> String {
    format!("Bearer {TOKEN}")
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

async fn upload(app: &axum::Router, thread: &str, upload_id: &str) -> String {
    let mut body = Vec::new();
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"manual.pdf\"\r\nContent-Type: application/pdf\r\n\r\n");
    body.extend_from_slice(PDF);
    body.extend_from_slice(
        format!("\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\n{upload_id}\r\n--boundary--\r\n")
            .as_bytes(),
    );
    let request = Request::post(format!("/api/v1/chat/threads/{thread}/attachments"))
        .header("host", HOST)
        .header("authorization", admin())
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
fn cos_bearer(env: &TestEnv, thread: &str, run: &str) -> String {
    let now = OffsetDateTime::now_utc();
    env.store
        .chat_message_post(
            thread,
            &ChatPostMessageRequest {
                client_message_id: format!("m-{run}"),
                text: "この PDF を KB に入れておいて".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("message");
    env.store
        .chat_run_claim_next(thread, run, &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = task_api::cos::issue_run_bearer(&env.store, thread, run, Duration::hours(1))
        .expect("issue");
    format!("Bearer {bearer}")
}

/// A project whose KB slug is `agent-platform`.
async fn project(app: &axum::Router) -> String {
    let created = send(
        app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "Celeris", "request": "作る"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let id = created.json()["id"].as_str().expect("id").to_owned();
    let patched = send(
        app,
        Request::patch(format!("/api/v1/projects/{id}"))
            .header("host", HOST)
            .header("authorization", admin())
            .header("content-type", "application/json")
            .body(Body::from(json!({"slug": "agent-platform"}).to_string()))
            .expect("patch"),
    )
    .await;
    assert!(patched.status.is_success(), "{}", patched.text());
    id
}

fn candidate_body(scope: &str, attachment_ids: &[&str]) -> Value {
    json!({
        "title": "fern03 の使い方",
        "scope": scope,
        "body": "添付の PDF の要点。",
        "sources": ["message:01J00000000000000000000000"],
        "tags": ["fern03"],
        "confidence": "high",
        "attachment_ids": attachment_ids,
    })
}

fn envelope(key: &str, body: &Value) -> Value {
    json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "人が PDF を KB に入れるよう依頼した",
        "policy_version": "1",
        "request": {"method": "POST", "path": "/api/v1/knowledge/inbox", "body": body},
    })
}

async fn get(app: &axum::Router, path: &str) -> Resp {
    send(app, get_with(path, &[("authorization", admin().as_str())])).await
}

fn inbox_files(env: &TestEnv) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(env.knowledge_root.join("_inbox"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name != ".gitkeep")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn count(env: &TestEnv, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(&env.db_path).expect("db");
    conn.query_row(sql, [], |r| r.get(0)).expect("count")
}

#[tokio::test]
async fn cos_live_fix_d2_api_creates_candidate_with_provenance() {
    let env = env_with_kb();
    let app = env.router();
    project(&app).await;
    let thread = thread(&env, "d2-human");
    let attachment = upload(&app, &thread, "up-1").await;

    // `projects/<slug>` is accepted and recorded as `project:<slug>`.
    let created = send(
        &app,
        post_admin(
            "/api/v1/knowledge/inbox",
            &candidate_body("projects/agent-platform", &[&attachment]),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let created = created.json();
    let id = created["id"].as_str().expect("candidate id").to_owned();
    assert_eq!(created["scope"], "project:agent-platform");
    assert_eq!(created["path"], format!("_inbox/{id}.md"));
    assert_eq!(created["attachment_ids"], json!([attachment]));
    assert!(created["sha"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);
    let raw = std::fs::read_to_string(env.knowledge_root.join(format!("_inbox/{id}.md")))
        .expect("candidate file");
    assert!(raw.contains("scope: \"project:agent-platform\""), "{raw}");

    let list = get(&app, "/api/v1/knowledge/inbox").await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let items = list.json()["items"].as_array().expect("items").clone();
    let item = items
        .iter()
        .find(|i| i["id"] == id.as_str())
        .expect("listed");
    assert_eq!(item["confidence"], "high");
    assert_eq!(item["provenance"][0]["attachment_id"], attachment.as_str());
    assert_eq!(item["provenance"][0]["thread_id"], thread.as_str());

    let detail = get(&app, &format!("/api/v1/knowledge/inbox/{id}")).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    let detail = detail.json();
    assert_eq!(detail["title"], "fern03 の使い方");
    assert_eq!(
        detail["provenance"][0]["attachment_id"],
        attachment.as_str()
    );
    assert_eq!(detail["provenance"][0]["name"], "manual.pdf");
}

#[tokio::test]
async fn cos_live_fix_d2_invalid_scope_is_422_and_writes_nothing() {
    let env = env_with_kb();
    let app = env.router();
    for scope in ["nonsense", "project:no-such-project", ""] {
        let resp = send(
            &app,
            post_admin("/api/v1/knowledge/inbox", &candidate_body(scope, &[])),
        )
        .await;
        assert_problem(&resp, 422, "validation");
    }
    // An unknown attachment is 422 and the candidate file is not left behind.
    let resp = send(
        &app,
        post_admin(
            "/api/v1/knowledge/inbox",
            &candidate_body("environment", &["01J00000000000000000000000"]),
        ),
    )
    .await;
    assert_problem(&resp, 422, "invalid_attachment");
    assert!(inbox_files(&env).is_empty(), "{:?}", inbox_files(&env));
}

#[tokio::test]
async fn cos_live_fix_d2_cos_operation_records_audit_row() {
    let env = env_with_kb();
    let app = env.router();
    project(&app).await;
    let thread = thread(&env, "d2-cos");
    let attachment = upload(&app, &thread, "up-1").await;
    let bearer = cos_bearer(&env, &thread, "run-d2");

    // Directly the CoS credential cannot write (it needs the audit context).
    let direct = send(
        &app,
        post_json_with(
            "/api/v1/knowledge/inbox",
            &candidate_body("project:agent-platform", &[&attachment]),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&direct, 422, "cos_audit_context_required");

    let body = candidate_body("project:agent-platform", &[&attachment]);
    let op = send(
        &app,
        post_json_with(
            OPS,
            &envelope("kb-1", &body),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(op.status.as_u16(), 200, "{}", op.text());
    let op = op.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "knowledge.record");
    assert_eq!(op["target_kind"], "knowledge");
    let id = op["result"]["id"]
        .as_str()
        .expect("candidate id")
        .to_owned();
    assert_eq!(op["target_id"], id.as_str());
    assert_eq!(op["result"]["attachment_ids"], json!([attachment]));
    assert_eq!(op["result"]["scope"], "project:agent-platform");
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE action='knowledge.record' AND state='applied'"
        ),
        1
    );
    let detail = get(&app, &format!("/api/v1/knowledge/inbox/{id}")).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    assert_eq!(
        detail.json()["provenance"][0]["attachment_id"],
        attachment.as_str()
    );

    // The same key and request is the same operation (no second candidate).
    let again = send(
        &app,
        post_json_with(
            OPS,
            &envelope("kb-1", &body),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["operation"]["result"]["id"], id.as_str());
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);

    // An attachment of another thread is refused, recorded as rejected, and leaves no file.
    let other = thread_with_upload(&env, &app).await;
    let foreign = send(
        &app,
        post_json_with(
            OPS,
            &envelope("kb-2", &candidate_body("project:agent-platform", &[&other])),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&foreign, 422, "invalid_attachment");
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);
    // A bad scope through the operation is 422 and audited as rejected.
    let bad = send(
        &app,
        post_json_with(
            OPS,
            &envelope("kb-3", &candidate_body("projects", &[])),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&bad, 422, "validation");
    assert_eq!(
        count(
            &env,
            "SELECT COUNT(*) FROM cos_operations WHERE idempotency_key IN ('kb-2','kb-3') AND state='rejected'"
        ),
        2
    );
}

async fn thread_with_upload(env: &TestEnv, app: &axum::Router) -> String {
    let other = thread(env, "d2-other");
    upload(app, &other, "up-other").await
}
