mod common;

#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../tests/support/http_sync.rs"]
mod fake_http;

use std::io::Write;
use std::net::TcpListener;
use std::process::Command;
use std::thread;

use serde_json::{Value, json};

fn capture_one() -> (String, thread::JoinHandle<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, raw) = fake_http::accept(&listener, "POST", "/api/v1/cos/operations");
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let body = serde_json::from_str(body).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
            )
            .unwrap();
        (head.to_owned(), body)
    });
    (format!("http://{addr}/api/v1"), handle)
}

#[test]
fn cos_chat_ops_ctl_add_uses_audited_http_without_opening_database() {
    let (url, server) = capture_one();
    let out = Command::new(common::celerisctl_path())
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
        .env("CELERIS_COS_POLICY_VERSION", "6")
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
    let out = Command::new(common::celerisctl_path())
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
        .env("CELERIS_COS_POLICY_VERSION", "6")
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
    let out = Command::new(common::celerisctl_path())
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

/// ADR 2026-10-28-cos-session-policy-key D2: no default version; the daemon sets the env.
#[test]
fn cos_chat_ops_ctl_without_policy_version_env_fails() {
    let out = Command::new(common::celerisctl_path())
        .args([
            "--api-url",
            "http://127.0.0.1:1/api/v1",
            "api-request",
            "POST",
            "/api/v1/decisions/decision-1/answer",
            "--body",
            r#"{"option":"yes"}"#,
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env("CELERIS_COS_REASON", "operator decision")
        .env_remove("CELERIS_COS_POLICY_VERSION")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("CELERIS_COS_POLICY_VERSION"), "{error}");
}

#[test]
fn cos_chat_ops_ctl_unsupported_db_mutation_is_rejected_before_db_open() {
    let out = Command::new(common::celerisctl_path())
        .args([
            "--db",
            "/a/nonexistent/celeris.sqlite3",
            "knowledge",
            "rerun",
            "01ABC",
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env("CELERIS_COS_POLICY_VERSION", "6")
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
    let out = Command::new(common::celerisctl_path())
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

#[test]
fn cos_chat_ops_ctl_prefers_api_url_env_over_config() {
    // CELERIS_CONFIG points at nothing: reading it would fail, so success proves the env won.
    let (url, server) = capture_one();
    let out = Command::new(common::celerisctl_path())
        .args([
            "--reason",
            "from env",
            "api-request",
            "POST",
            "/api/v1/tasks/01ABC/comments",
            "--body",
            r#"{"body":"hi"}"#,
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env("CELERIS_COS_POLICY_VERSION", "6")
        .env("CELERIS_API_URL", &url)
        .env("CELERIS_CONFIG", "/a/nonexistent/celeris/config.toml")
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
    assert_eq!(body["request"]["path"], "/api/v1/tasks/01ABC/comments");

    // --api-url still wins over the environment.
    let (url, server) = capture_one();
    let out = Command::new(common::celerisctl_path())
        .args([
            "--api-url",
            &url,
            "--reason",
            "flag wins",
            "api-request",
            "POST",
            "/api/v1/tasks/01ABC/comments",
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env("CELERIS_COS_POLICY_VERSION", "6")
        .env("CELERIS_API_URL", "http://127.0.0.1:1/not-api")
        .env("CELERIS_CONFIG", "/a/nonexistent/celeris/config.toml")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (head, _) = server.join().unwrap();
    assert!(head.starts_with("POST /api/v1/cos/operations HTTP/1.1"));

    // The env value gets the same /api/v1 check as the flag, before any connection.
    let out = Command::new(common::celerisctl_path())
        .args([
            "--reason",
            "bad env",
            "api-request",
            "POST",
            "/api/v1/tasks/01ABC/comments",
        ])
        .env("CELERIS_COS_RUN_CREDENTIAL", "celeris-cos-run.test")
        .env("CELERIS_COS_POLICY_VERSION", "6")
        .env("CELERIS_API_URL", "http://127.0.0.1:1/not-api")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("CELERIS_API_URL must end in /api/v1"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every config-backed daemon command must use the shared resolver. Existing transport tests
/// construct ApiConfig directly and cannot detect a command retaining its own config loader.
#[test]
fn daemon_commands_share_config_and_url_environment_resolution() {
    for (args, path, body) in [
        (vec!["cron", "list"], "/api/v1/cron-jobs", "{\"items\":[]}"),
        (
            vec!["models", "list", "--json"],
            "/api/v1/llm/models",
            "{\"items\":[]}",
        ),
        (
            vec!["script", "domains", "--json"],
            "/api/v1/script/domains",
            "{\"items\":[]}",
        ),
    ] {
        for use_url in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let config = dir.path().join("config.toml");
            std::fs::write(dir.path().join("token"), "shared-test-token\n").unwrap();
            std::fs::write(&config, format!(
                "[api]\nlisten = '{addr}'\n[[api.tokens]]\nprincipal = 'operator:test'\ntoken_file = 'token'\n"
            )).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, raw) = fake_http::accept(&listener, "GET", path);
                assert_eq!(
                    raw.contains("Authorization: Bearer shared-test-token\r\n"),
                    !use_url
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            });
            let mut cmd = Command::new(common::celerisctl_path());
            cmd.env_remove("CELERIS_COS_RUN_CREDENTIAL")
                .env_remove("CELERIS_API_URL")
                .env("CELERIS_CONFIG", &config)
                .args(&args);
            if use_url {
                // URL overrides must work without reading config or its administrator token.
                cmd.env("CELERIS_CONFIG", dir.path().join("absent.toml"))
                    .env("CELERIS_API_URL", format!("http://{addr}/api/v1/"));
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            server.join().unwrap();
        }
    }
}
