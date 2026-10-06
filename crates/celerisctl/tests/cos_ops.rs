use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

use serde_json::{Value, json};

fn capture_one() -> (String, thread::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&bytes[..split]);
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= split + 4 + length {
                let body = serde_json::from_slice(&bytes[split + 4..split + 4 + length]).unwrap();
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}").unwrap();
                return (head.into_owned(), body);
            }
        }
    });
    (format!("http://{addr}/api/v1"), handle)
}

#[test]
fn cos_chat_ops_ctl_add_uses_audited_http_without_opening_database() {
    let (url, server) = capture_one();
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args([
            "--db",
            "/a/nonexistent/celeris.sqlite3",
            "--api-url",
            &url,
            "--reason",
            "human request",
            "add",
            "--title",
            "next task",
            "--objective",
            "do the work",
            "--accept",
            "check it",
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env_remove("CELERIS_FOLLOWUPS_FILE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (head, body) = server.join().unwrap();
    assert!(head.starts_with("POST /api/v1/cos/operations HTTP/1.1"));
    assert!(head.contains("Authorization: Bearer celeris-cos-run.test"));
    assert_eq!(body["request"]["path"], "/api/v1/tasks");
    assert_eq!(body["request"]["body"]["title"], "next task");
    assert_eq!(body["reason"], "human request");
    assert_eq!(body["expected_revision"], Value::Null);
    assert!(
        body["idempotency_key"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
}

#[test]
fn cos_chat_ops_ctl_api_request_can_send_registered_comment_shape() {
    let (url, server) = capture_one();
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args([
            "--api-url",
            &url,
            "--reason",
            "follow up",
            "--idempotency-key",
            "comment-1",
            "api-request",
            "POST",
            "/api/v1/tasks/01ABC/comments",
            "--body",
            r#"{"body":"hello"}"#,
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (head, body) = server.join().unwrap();
    assert!(head.starts_with("POST /api/v1/cos/operations HTTP/1.1"));
    assert_eq!(body["idempotency_key"], "comment-1");
    assert_eq!(
        body["request"],
        json!({"method":"POST","path":"/api/v1/tasks/01ABC/comments","body":{"body":"hello"}})
    );
}

#[test]
fn cos_chat_ops_ctl_reason_and_policy_can_come_from_environment() {
    let (url, server) = capture_one();
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args([
            "--api-url",
            &url,
            "api-request",
            "POST",
            "/api/v1/decisions/decision-1/answer",
            "--body",
            r#"{"option":"yes"}"#,
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env("CELERIS_COS_REASON", "operator decision")
        .env("CELERIS_COS_POLICY_VERSION", "2")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (_, body) = server.join().unwrap();
    assert_eq!(body["reason"], "operator decision");
    assert_eq!(body["policy_version"], "2");
    assert_eq!(
        body["request"]["path"],
        "/api/v1/decisions/decision-1/answer"
    );
}

#[test]
fn cos_chat_ops_ctl_unsupported_db_mutation_is_rejected_before_db_open() {
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args(["--db", "/a/nonexistent/celeris.sqlite3", "approve", "01ABC"])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("no audited CoS operation mapping"),
        "{error}"
    );
    assert!(!error.contains("failed to open db"), "{error}");
}

#[test]
fn cos_chat_ops_ctl_api_request_without_run_credential_does_not_send() {
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args([
            "--api-url",
            "http://127.0.0.1:1/api/v1",
            "api-request",
            "POST",
            "/api/v1/tasks",
        ])
        .env_remove("CELERIS_COS_RUN_CREDENTIAL")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("requires CELERIS_COS_RUN_CREDENTIAL"),
        "{error}"
    );
}
