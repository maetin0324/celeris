use super::*;
use std::net::SocketAddr;
use std::sync::Arc;

async fn spawn(
    scopes: Vec<task_core::McpScope>,
) -> (
    SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let store = Arc::new(task_core::SqliteStore::open_in_memory().expect("open"));
    {
        use task_core::McpClientStore;
        store
            .mcp_client_create(&task_core::McpClient {
                id: "c1".into(),
                name: "c1".into(),
                token_hash: Some(crate::auth::hash_token("secret")),
                scopes,
                created_at: time::OffsetDateTime::now_utc(),
                last_used_at: None,
                revoked_at: None,
            })
            .expect("create client");
    }
    let state = crate::state::McpState::from_store(
        store,
        60,
        Vec::new(),
        Vec::new(),
        "secretary".to_string(),
        None,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(crate::serve(
        listener,
        state,
        crate::config::ListenerAuth::Token,
        async {
            let _ = stop_rx.await;
        },
    ));
    (addr, stop_tx, handle)
}

#[test]
fn list_tools_returns_names_and_descriptions_filtered_by_scope() {
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let (addr, stop_tx, handle) = rt.block_on(spawn(vec![task_core::McpScope::TasksRead]));
    let base_url = format!("http://{addr}");

    let tools = list_tools(&base_url, Some("secret")).expect("list_tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"tasks_list"), "{names:?}");
    assert!(names.contains(&"tasks_get"), "{names:?}");
    assert!(!names.contains(&"knowledge_list"), "{names:?}");
    assert!(!tools[0].description.is_empty());

    let _ = stop_tx.send(());
    rt.block_on(handle).expect("join").expect("serve");
}

#[test]
fn call_tool_returns_structured_content_as_pretty_json() {
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let (addr, stop_tx, handle) = rt.block_on(spawn(vec![task_core::McpScope::TasksRead]));
    let base_url = format!("http://{addr}");

    let out = call_tool(
        &base_url,
        Some("secret"),
        "tasks_list",
        serde_json::json!({}),
    )
    .expect("call_tool");
    let parsed: Value = serde_json::from_str(&out).expect("pretty json");
    assert_eq!(parsed["items"], serde_json::json!([]));

    let _ = stop_tx.send(());
    rt.block_on(handle).expect("join").expect("serve");
}

#[test]
fn call_tool_without_the_scope_is_a_rpc_error() {
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let (addr, stop_tx, handle) = rt.block_on(spawn(vec![]));
    let base_url = format!("http://{addr}");

    let err = call_tool(
        &base_url,
        Some("secret"),
        "tasks_list",
        serde_json::json!({}),
    )
    .expect_err("should fail");
    assert!(matches!(err, CallError::Rpc { code, .. } if code == crate::rpc::METHOD_NOT_FOUND));

    let _ = stop_tx.send(());
    rt.block_on(handle).expect("join").expect("serve");
}

#[test]
fn missing_or_wrong_token_is_a_http_error() {
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let (addr, stop_tx, handle) = rt.block_on(spawn(vec![task_core::McpScope::TasksRead]));
    let base_url = format!("http://{addr}");

    let err = call_tool(
        &base_url,
        Some("wrong"),
        "tasks_list",
        serde_json::json!({}),
    )
    .expect_err("should fail");
    assert!(matches!(err, CallError::Http { status: 401, .. }), "{err}");

    let _ = stop_tx.send(());
    rt.block_on(handle).expect("join").expect("serve");
}

#[test]
fn unreachable_server_is_a_transport_error() {
    // `bind` した口を先に閉じる（何も listen していないアドレス）ことで接続不可を作る。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let base_url = format!("http://{addr}");

    let err = call_tool(
        &base_url,
        Some("secret"),
        "tasks_list",
        serde_json::json!({}),
    )
    .expect_err("should fail");
    assert!(matches!(err, CallError::Transport(_)), "{err}");
}
