use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// `status_line` と本文を 1 回だけ返す偽 HTTP server。受け取った要求の先頭部分を返す。
async fn serve_once(
    status_line: &'static str,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!(
        "http://{}/zen/go/v1/usage",
        listener.local_addr().expect("addr")
    );
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.expect("read");
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let response = format!(
            "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.expect("write");
        let _ = socket.shutdown().await;
        request
    });
    (url, handle)
}

fn account_with_key(key: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("opencode")).expect("mkdir");
    std::fs::write(
        dir.path().join("opencode/auth.json"),
        format!(r#"{{"opencode-go":{{"type":"api","key":"{key}"}}}}"#),
    )
    .expect("write");
    dir
}

const USAGE: &str = r#"{"usage":{"rolling":{"status":"ok","percent":12,"resetsAt":"2026-10-06T12:00:00.000Z"},"weekly":{"status":"ok","percent":40,"resetsAt":"2026-10-08T00:00:00Z"},"monthly":{"status":"rate-limited","percent":100,"resetsAt":"2026-11-01T00:00:00Z"}}}"#;

#[tokio::test]
async fn ok_response_becomes_a_three_window_observation_and_sends_bearer() {
    let dir = account_with_key("sk-test-secret");
    let (url, server) = serve_once("200 OK", USAGE).await;
    let check = check_account_opencode_go(dir.path(), &url, Duration::from_secs(5)).await;
    assert_eq!(check.result, AccountCheckResult::Ok);
    let obs = check.observation.clone().expect("observation");
    assert!((obs.five_hour.expect("5h").utilization - 0.12).abs() < 1e-9);
    assert!((obs.seven_day.expect("7d").utilization - 0.40).abs() < 1e-9);
    assert_eq!(obs.one_month.expect("month").utilization, 1.0);
    let request = server.await.expect("server");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer sk-test-secret"),
        "{request}"
    );
    assert!(!format!("{check:?}").contains("sk-test-secret"));
}

#[tokio::test]
async fn unauthorized_is_auth_failed_without_observation() {
    let dir = account_with_key("sk-bad");
    let (url, _server) = serve_once("401 Unauthorized", r#"{"error":"AuthError"}"#).await;
    let check = check_account_opencode_go(dir.path(), &url, Duration::from_secs(5)).await;
    assert_eq!(check.result, AccountCheckResult::AuthFailed);
    assert!(check.observation.is_none());
}

#[tokio::test]
async fn missing_auth_json_is_auth_failed_and_never_calls_the_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    let check = check_account_opencode_go(
        dir.path(),
        "http://127.0.0.1:1/zen/go/v1/usage",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(check.result, AccountCheckResult::AuthFailed);
}

#[tokio::test]
async fn unreachable_endpoint_is_a_failed_check_not_an_auth_failure() {
    let dir = account_with_key("sk-x");
    // 一度 bind して閉じたポートへ向ける（接続拒否）。
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        l.local_addr().expect("addr").port()
    };
    let check = check_account_opencode_go(
        dir.path(),
        &format!("http://127.0.0.1:{port}/usage"),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(check.result, AccountCheckResult::SpawnFailed);
    assert!(check.observation.is_none());
}

#[test]
fn credential_debug_is_redacted_and_non_api_entries_are_rejected() {
    assert_eq!(format!("{:?}", GoKey("secret".into())), "GoKey(<redacted>)");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("opencode")).expect("mkdir");
    std::fs::write(
        dir.path().join("opencode/auth.json"),
        r#"{"opencode-go":{"type":"oauth","key":"k"}}"#,
    )
    .expect("write");
    assert!(read_go_key(dir.path()).is_none());
}
