//! `api_get` の要求の検査（決定的。通信を伴う試験は `tests/mcp_integration.rs`）。

use super::*;
use crate::tools::ToolErrorCode;

fn args(path: &str) -> ApiGetArgs {
    ApiGetArgs {
        path: path.to_string(),
        query: None,
        method: None,
    }
}

fn is_rejected(a: &ApiGetArgs) -> bool {
    matches!(check_request(a), Err(e) if e.code == ToolErrorCode::Rejected)
}

#[test]
fn ordinary_read_paths_pass() {
    for p in [
        "/api/v1/tasks",
        "/api/v1/tasks/01M5MWG7FB4V940XSF0C4F93CM/events",
        "/api/v1/routing/evidence",
        "/api/v1/tasks/01M5/routing",
        "/api/v1/llm/accounts",
        "/api/v1/mcp/calls",
        "/api/v1/schema",
    ] {
        assert!(check_request(&args(p)).is_ok(), "{p}");
    }
}

#[test]
fn methods_other_than_get_are_rejected() {
    for m in ["POST", "PUT", "PATCH", "DELETE", "HEAD"] {
        let mut a = args("/api/v1/tasks");
        a.method = Some(m.to_string());
        assert!(is_rejected(&a), "{m}");
    }
    let mut a = args("/api/v1/tasks");
    a.method = Some("get".into());
    assert!(check_request(&a).is_ok());
}

#[test]
fn paths_outside_api_v1_and_traversal_are_rejected() {
    for p in [
        "/mcp",
        "/api/v2/tasks",
        "api/v1/tasks",
        "/api/v1",
        "/api/v1/",
        "/api/v1/../../etc/passwd",
        "/api/v1/tasks/./x",
        "/api/v1//tasks",
        "/api/v1/tasks?limit=1",
        "/api/v1/tasks#x",
        "/api/v1/%2e%2e/secrets",
        "/api/v1/tasks\\x",
        "/api/v1/tasks x",
        "http://example.com/api/v1/tasks",
    ] {
        assert!(is_rejected(&args(p)), "{p}");
    }
}

#[test]
fn deny_list_blocks_secret_paths() {
    for p in [
        "/api/v1/secrets",
        "/api/v1/secrets/abc",
        "/api/v1/browser/credentials",
        "/api/v1/browser/credentials/c1",
        "/api/v1/browser/identities",
        "/api/v1/browser/identities/i1",
        "/api/v1/tasks/t1/browser/live/r1/s1/frames",
        "/api/v1/tasks/t1/browser/live/r1/s1/read",
        "/api/v1/tasks/t1/browser/waits/w1/credential",
        "/api/v1/clusters/pegasus/connect",
        "/api/v1/clusters/pegasus/connect/code",
        "/api/v1/llm/sources/s/accounts/a/login",
        "/api/v1/chat/attachments/a1/content",
        "/api/v1/chat/attachments/a1/preview",
        "/api/v1/stream",
        "/api/v1/console/stream",
        "/api/v1/chat/threads/t1/stream",
        "/api/v1/future/token",
        "/api/v1/x/Cookies",
        "/api/v1/x/password",
    ] {
        assert!(is_rejected(&args(p)), "{p}");
        assert!(denied_reason(p).is_some(), "{p}");
    }
    // 似ているが別の path は通る。
    assert!(denied_reason("/api/v1/browser/waits").is_none());
    assert!(denied_reason("/api/v1/tasks/t1/browser/waits").is_none());
    assert!(denied_reason("/api/v1/chat/threads/t1/runs").is_none());
}

#[test]
fn query_values_must_be_scalars() {
    let mut a = args("/api/v1/tasks");
    a.query = Some(
        serde_json::from_value(serde_json::json!({"limit": 5, "status": "ready", "all": true}))
            .expect("map"),
    );
    let pairs = check_request(&a).expect("ok");
    assert_eq!(pairs.len(), 3);
    a.query = Some(serde_json::from_value(serde_json::json!({"x": [1]})).expect("map"));
    assert!(matches!(check_request(&a), Err(e) if e.code == ToolErrorCode::InvalidParams));
}

#[test]
fn from_listen_rewrites_unspecified_to_loopback() {
    let t = ApiTarget::from_listen("0.0.0.0:7700".parse().expect("addr"), None).expect("t");
    assert_eq!(t.base_url, "http://127.0.0.1:7700");
    let t =
        ApiTarget::from_listen("[::]:7700".parse().expect("addr"), Some("x".into())).expect("t");
    assert_eq!(t.base_url, "http://[::1]:7700");
    assert!(!format!("{t:?}").contains("\"x\""));
}

#[test]
fn audit_request_keeps_path_and_query() {
    assert_eq!(
        audit_request(&serde_json::json!({"path": "/api/v1/tasks"})).as_deref(),
        Some("GET /api/v1/tasks")
    );
    assert_eq!(
        audit_request(&serde_json::json!({"path": "/api/v1/tasks", "query": {"limit": 5}}))
            .as_deref(),
        Some("GET /api/v1/tasks {\"limit\":5}")
    );
}
