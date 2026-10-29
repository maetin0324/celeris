#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../../tests/support/http_sync.rs"]
mod fake_http;

use super::*;
use std::io::Write;
use std::net::TcpListener;
use std::thread;

pub(crate) fn fake_server() -> (ApiConfig, thread::JoinHandle<(String, Value)>) {
    fake_server_at("/api/v1/cos/operations")
}

fn fake_server_at(path: &'static str) -> (ApiConfig, thread::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, raw) = fake_http::accept(&listener, "POST", path);
        let (headers, body) = raw.split_once("\r\n\r\n").unwrap();
        let body = serde_json::from_str(body).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
            )
            .unwrap();
        (headers.to_owned(), body)
    });
    (
        ApiConfig {
            base_url: format!("http://{addr}/api/v1"),
            token: Some("admin-secret".into()),
        },
        handle,
    )
}

#[test]
fn cos_chat_ops_ctl_credential_wraps_domain_mutation_and_uses_run_bearer() {
    let (api, server) = fake_server();
    let options = Options {
        reason: Some("requested by the operator".into()),
        idempotency_key: Some("retry-1".into()),
        expected_revision: Some("7".into()),
        policy_version: Some("6".into()),
        ..Default::default()
    };
    let result = send(
        &api,
        "post",
        "/api/v1/tasks/01ABC/comments",
        json!({"body":"hello"}),
        &options,
        Some("celeris-cos-run.secret"),
    )
    .unwrap();
    assert_eq!(result["ok"], true);
    let (headers, body) = server.join().unwrap();
    assert!(headers.starts_with("POST /api/v1/cos/operations HTTP/1.1"));
    assert!(headers.contains("Authorization: Bearer celeris-cos-run.secret"));
    assert!(!headers.contains("admin-secret"));
    assert_eq!(body["idempotency_key"], "retry-1");
    assert_eq!(body["expected_revision"], "7");
    assert_eq!(body["reason"], "requested by the operator");
    assert_eq!(body["policy_version"], "6");
    assert_eq!(
        body["request"],
        json!({"method":"POST","path":"/api/v1/tasks/01ABC/comments","body":{"body":"hello"}})
    );
}

#[test]
fn cos_chat_ops_ctl_without_credential_preserves_domain_request() {
    let (api, server) = fake_server_at("/api/v1/tasks");
    send(
        &api,
        "POST",
        "/api/v1/tasks",
        json!({"title":"example"}),
        &Options::default(),
        None,
    )
    .unwrap();
    let (headers, body) = server.join().unwrap();
    assert!(headers.starts_with("POST /api/v1/tasks HTTP/1.1"));
    assert!(headers.contains("Authorization: Bearer admin-secret"));
    assert_eq!(body, json!({"title":"example"}));
}

#[test]
fn cos_chat_ops_ctl_missing_reason_fails_before_connection() {
    let api = ApiConfig {
        base_url: "http://127.0.0.1:1/api/v1".into(),
        token: None,
    };
    let options = Options {
        reason: Some("  ".into()),
        ..Default::default()
    };
    let error = send(
        &api,
        "POST",
        "/api/v1/tasks",
        json!({}),
        &options,
        Some("celeris-cos-run.secret"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("reason"));
}

#[test]
fn cos_chat_ops_ctl_rejects_header_injection_before_connection() {
    let api = ApiConfig {
        base_url: "http://127.0.0.1:1/api/v1".into(),
        token: None,
    };
    let options = Options {
        reason: Some("ok".into()),
        ..Default::default()
    };
    let error = send(
        &api,
        "POST",
        "/api/v1/tasks",
        json!({}),
        &options,
        Some("secret\r\nX-Injected: yes"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("credential"));
}
