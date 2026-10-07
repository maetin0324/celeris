mod common;

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::json;
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::{Status, TaskKind};

const BASE: &str = "/api/v1/chat";

fn multipart(path: &str, key: &str, name: &str, file: &[u8]) -> Request<Body> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\n{key}\r\n--boundary--\r\n").as_bytes());
    Request::post(path)
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body))
        .expect("multipart request")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut value = !0u32;
    for byte in bytes {
        value ^= u32::from(*byte);
        for _ in 0..8 {
            value = (value >> 1) ^ if value & 1 != 0 { 0xedb8_8320 } else { 0 };
        }
    }
    !value
}

async fn setup(max_file_bytes: u64) -> (TestEnv, String) {
    let mut env = admin_env();
    let limits = ChatAttachmentLimits {
        max_file_bytes,
        ..Default::default()
    };
    env.state = env
        .state
        .clone()
        .with_chat_attachments(env.dir.path().to_path_buf(), limits);
    let app = env.router();
    let response = send(&app, post_admin(&format!("{BASE}/threads"), &json!({"title":"Attachments","project_id":null,"client_thread_id":"chat-attach-thread"}))).await;
    assert_eq!(response.status.as_u16(), 201, "{}", response.text());
    (
        env,
        response.json()["thread"]["id"]
            .as_str()
            .expect("thread id")
            .to_owned(),
    )
}

async fn upload(app: &axum::Router, thread: &str, key: &str, name: &str, bytes: &[u8]) -> Resp {
    send(
        app,
        multipart(
            &format!("{BASE}/threads/{thread}/attachments"),
            key,
            name,
            bytes,
        ),
    )
    .await
}

#[tokio::test]
async fn chat_attach_api_unconfigured_is_unavailable() {
    let env = admin_env();
    let app = env.router();
    let response = send(
        &app,
        post_admin(
            &format!("{BASE}/threads"),
            &json!({"title":"No storage","project_id":null,"client_thread_id":"unconfigured"}),
        ),
    )
    .await;
    let id = response.json()["thread"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert_problem(
        &send(
            &app,
            multipart(
                &format!("{BASE}/threads/{id}/attachments"),
                "key",
                "x.txt",
                b"x",
            ),
        )
        .await,
        503,
        "attachments_unavailable",
    );
}

#[tokio::test]
async fn chat_attach_api_upload_limits_replay_and_content() {
    let (env, thread) = setup(12).await;
    let app = env.router();
    let path = format!("{BASE}/threads/{thread}/attachments");
    let data = b"hello world!";
    let first = send(&app, multipart(&path, "key-1", "a.txt", data)).await;
    assert_eq!(first.status.as_u16(), 201, "{}", first.text());
    let item = &first.json()["attachment"];
    let id = item["id"].as_str().expect("attachment id").to_owned();
    assert_eq!(item["size_bytes"], 12);
    assert_eq!(
        item["sha256"],
        "7509e5bda0c762d2bac7f90d758b5b2263fa01ccbc542ab5e3df163be08e6ca9"
    );
    let again = send(&app, multipart(&path, "key-1", "a.txt", data)).await;
    assert_eq!(again.status.as_u16(), 200, "{}", again.text());
    assert_eq!(again.json()["attachment"]["id"], id);
    assert_problem(
        &send(&app, multipart(&path, "key-1", "b.txt", data)).await,
        409,
        "chat_conflict",
    );
    assert_problem(
        &send(&app, multipart(&path, "key-2", "a.txt", b"hello world!!")).await,
        413,
        "payload_too_large",
    );
    let content = send(&app, get_admin(&format!("{BASE}/attachments/{id}/content"))).await;
    assert_eq!(content.status.as_u16(), 200);
    assert_eq!(content.body, data);
    assert_eq!(content.header("x-content-type-options"), Some("nosniff"));
    assert!(
        content
            .header("content-disposition")
            .is_some_and(|s| s.starts_with("attachment;"))
    );
}

#[tokio::test]
async fn chat_attach_api_preview_delete_and_references() {
    let (env, thread) = setup(1024).await;
    let app = env.router();
    let path = format!("{BASE}/threads/{thread}/attachments");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("png");
    let png = png.into_inner();
    let uploaded = send(&app, multipart(&path, "png", "small.png", &png)).await;
    assert_eq!(uploaded.status.as_u16(), 201, "{}", uploaded.text());
    let id = uploaded.json()["attachment"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert!(uploaded.json()["attachment"]["preview_url"].is_string());
    let preview = send(&app, get_admin(&format!("{BASE}/attachments/{id}/preview"))).await;
    assert_eq!(preview.status.as_u16(), 200);
    assert_eq!(preview.header("content-type"), Some("image/png"));
    assert!(preview.body.starts_with(b"\x89PNG"));
    // A valid IHDR can claim more than 40 MP while the compressed body stays tiny.
    // The dimension guard must reject it before decoding any pixel buffer.
    let mut huge = png.clone();
    huge[16..20].copy_from_slice(&8_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&5_001u32.to_be_bytes());
    let checksum = crc32(&huge[12..29]);
    huge[29..33].copy_from_slice(&checksum.to_be_bytes());
    assert_eq!(
        image::ImageReader::new(std::io::Cursor::new(&huge))
            .with_guessed_format()
            .expect("format")
            .into_dimensions()
            .expect("dimensions"),
        (8_000, 5_001)
    );
    let large = send(&app, multipart(&path, "large", "large.png", &huge)).await;
    assert_eq!(large.status.as_u16(), 201, "{}", large.text());
    assert!(large.json()["attachment"]["preview_url"].is_null());
    let large_value = large.json();
    let large_id = large_value["attachment"]["id"].as_str().expect("large id");
    assert_problem(
        &send(
            &app,
            get_admin(&format!("{BASE}/attachments/{large_id}/preview")),
        )
        .await,
        404,
        "chat_not_found",
    );
    for (key, name, bytes) in [
        ("svg", "x.svg", b"<svg/>".as_slice()),
        ("pdf", "x.pdf", b"%PDF-1.5".as_slice()),
    ] {
        let item = send(&app, multipart(&path, key, name, bytes)).await.json();
        assert!(item["attachment"]["preview_url"].is_null());
        let other = item["attachment"]["id"].as_str().expect("other id");
        assert_problem(
            &send(
                &app,
                get_admin(&format!("{BASE}/attachments/{other}/preview")),
            )
            .await,
            404,
            "chat_not_found",
        );
    }
    // The pin's owner must exist (ADR 2026-10-05 cos-chat-home D4).
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    let task_id = task.id.to_string();
    let reference = send(
        &app,
        post_admin(
            &format!("{BASE}/attachments/{id}/references"),
            &json!({"owner_kind":"task","owner_id":task_id,"idempotency_key":"ref-1"}),
        ),
    )
    .await;
    assert_eq!(reference.status.as_u16(), 200, "{}", reference.text());
    let repeated = send(
        &app,
        post_admin(
            &format!("{BASE}/attachments/{id}/references"),
            &json!({"owner_kind":"task","owner_id":task_id,"idempotency_key":"ref-1"}),
        ),
    )
    .await;
    assert_eq!(repeated.status.as_u16(), 200);
    assert_problem(
        &send(
            &app,
            post_admin(
                &format!("{BASE}/attachments/{id}/references"),
                &json!({"owner_kind":"task","owner_id":"task-2","idempotency_key":"ref-1"}),
            ),
        )
        .await,
        409,
        "chat_conflict",
    );
    let deletion = Request::delete(format!("{BASE}/attachments/{id}"))
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .expect("delete");
    assert_problem(&send(&app, deletion).await, 409, "chat_conflict");
    let loose = send(&app, multipart(&path, "loose", "loose.txt", b"x"))
        .await
        .json();
    let loose_id = loose["attachment"]["id"].as_str().expect("loose id");
    let deletion = || {
        Request::delete(format!("{BASE}/attachments/{loose_id}"))
            .header("host", HOST)
            .header("authorization", format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .expect("delete")
    };
    assert_eq!(send(&app, deletion()).await.status.as_u16(), 204);
    assert_eq!(send(&app, deletion()).await.status.as_u16(), 204);
}

#[tokio::test]
async fn chat_attach_api_requires_client_upload_id() {
    let (env, thread) = setup(64).await;
    let body = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\n\r\nx\r\n--boundary--\r\n";
    let request = Request::post(format!("{BASE}/threads/{thread}/attachments"))
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body.as_slice()))
        .expect("request");
    assert_problem(&send(&env.router(), request).await, 400, "bad_request");
}

#[tokio::test]
async fn chat_attach_api_rejects_empty_client_upload_id() {
    let (env, thread) = setup(64).await;
    assert_problem(
        &upload(&env.router(), &thread, "", "x.txt", b"x").await,
        400,
        "bad_request",
    );
}

#[tokio::test]
async fn chat_attach_api_rejects_duplicate_file_fields() {
    let (env, thread) = setup(64).await;
    let body = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\na\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.txt\"\r\n\r\nb\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\nkey\r\n--boundary--\r\n";
    let request = Request::post(format!("{BASE}/threads/{thread}/attachments"))
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body.as_slice()))
        .expect("request");
    assert_problem(&send(&env.router(), request).await, 400, "bad_request");
}

#[tokio::test]
async fn chat_attach_api_replay_different_bytes_conflicts() {
    let (env, thread) = setup(64).await;
    let app = env.router();
    assert_eq!(
        upload(&app, &thread, "same", "x.txt", b"first")
            .await
            .status
            .as_u16(),
        201
    );
    assert_problem(
        &upload(&app, &thread, "same", "x.txt", b"other").await,
        409,
        "chat_conflict",
    );
}

#[tokio::test]
async fn chat_attach_api_metadata_exposes_download_url() {
    let (env, thread) = setup(64).await;
    let app = env.router();
    let created = upload(&app, &thread, "metadata", "x.txt", b"hello")
        .await
        .json();
    let id = created["attachment"]["id"].as_str().expect("id");
    let metadata = send(&app, get_admin(&format!("{BASE}/attachments/{id}"))).await;
    assert_eq!(metadata.status.as_u16(), 200);
    assert_eq!(metadata.json()["attachment"]["id"], id);
    assert_eq!(
        metadata.json()["attachment"]["download_url"],
        format!("{BASE}/attachments/{id}/content")
    );
}

#[tokio::test]
async fn chat_attach_api_knowledge_inbox_reference() {
    let (env, thread) = setup(64).await;
    let app = env.router();
    let created = upload(&app, &thread, "knowledge", "x.txt", b"hello")
        .await
        .json();
    let id = created["attachment"]["id"].as_str().expect("id");
    let path = format!("{BASE}/attachments/{id}/references");
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    let candidate = task_ops::knowledge::record(
        &env.knowledge_root,
        &task_ops::knowledge::RecordRequest {
            title: "inbox".into(),
            scope: "environment".into(),
            tags: vec![],
            sources: vec!["human:instruction".into()],
            confidence: None,
            body: "x".into(),
            path: Some("environment/servers/inbox.md".into()),
            op: None,
        },
    )
    .expect("record");
    let body = json!({"owner_kind":"knowledge_inbox","owner_id":candidate.id,"idempotency_key":"ref-knowledge"});
    let added = send(&app, post_admin(&path, &body)).await;
    assert_eq!(added.status.as_u16(), 200, "{}", added.text());
    assert_eq!(added.json()["owner_kind"], "knowledge_inbox");
}

#[tokio::test]
async fn chat_attach_api_deleted_content_is_unavailable() {
    let (env, thread) = setup(64).await;
    let app = env.router();
    let created = upload(&app, &thread, "delete", "x.txt", b"hello")
        .await
        .json();
    let id = created["attachment"]["id"].as_str().expect("id");
    let deletion = Request::delete(format!("{BASE}/attachments/{id}"))
        .header("host", HOST)
        .header("authorization", format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .expect("delete");
    assert_eq!(send(&app, deletion).await.status.as_u16(), 204);
    assert_problem(
        &send(&app, get_admin(&format!("{BASE}/attachments/{id}/content"))).await,
        404,
        "chat_not_found",
    );
}
