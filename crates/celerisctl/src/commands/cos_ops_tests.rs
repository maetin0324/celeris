use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn fake_server() -> (ApiConfig, thread::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 2048];
        loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0, "request ended before the body");
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..split]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|v| v.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= split + 4 + length {
                    let body =
                        serde_json::from_slice(&bytes[split + 4..split + 4 + length]).unwrap();
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").unwrap();
                    return (headers.into_owned(), body);
                }
            }
        }
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
    assert_eq!(
        body["request"],
        json!({"method":"POST","path":"/api/v1/tasks/01ABC/comments","body":{"body":"hello"}})
    );
}

#[test]
fn cos_chat_ops_ctl_without_credential_preserves_domain_request() {
    let (api, server) = fake_server();
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
