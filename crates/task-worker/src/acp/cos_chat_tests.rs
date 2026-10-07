use super::*;
use crate::protocol::{CosChatAttachment, CosChatContext, CosChatDelivery, SessionHandle};

fn chat_req(dir: &Path) -> RunRequest {
    let mut req = sample_req(dir.to_path_buf());
    req.context.cos_chat = Some(CosChatContext {
        thread_id: "thread-1".into(),
        run_id: "run-1".into(),
        ..CosChatContext::default()
    });
    req
}

fn resumed(req: &mut RunRequest) {
    req.context.session = Some(SessionHandle {
        adapter: "acp".into(),
        session_id: "old-session".into(),
        resume: true,
    });
}

#[test]
fn cos_chat_harness_acp_unknown_update_is_status_only_for_chat() {
    let update = serde_json::json!({
        "method": "session/update",
        "params": {"update": {"sessionUpdate": "plan"}}
    });
    let chat_sink = RecordingSink::default();
    let mut chat_chunks = ChunkBuffer {
        cos_chat: true,
        ..ChunkBuffer::default()
    };
    handle_notification(&update, &chat_sink, &mut chat_chunks);
    assert_eq!(
        chat_sink.structured.lock().unwrap()[0].1.kind,
        Some(ProgressKind::Status)
    );

    let ordinary_sink = RecordingSink::default();
    handle_notification(&update, &ordinary_sink, &mut ChunkBuffer::default());
    assert!(ordinary_sink.structured.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cos_chat_harness_acp_new_maps_updates_and_allows_permission() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
mkdir -p artifacts
read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
read -r new
printf '%s\n' "$new" > new.json
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"new-session"}}'
read -r prompt
printf '%s\n' '{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{"options":[{"optionId":"deny","kind":"reject_once"},{"optionId":"allow","kind":"allow_once"}]}}'
read -r permission
printf '%s\n' "$permission" > permission.json
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello\n"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","title":"Bash","status":"in_progress","rawInput":{"command":"pwd"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","title":"Bash","status":"completed","rawOutput":"ok"}}}'
printf '%s\n' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let mut config = stub_acp(dir.path(), script);
    config.permission = AcpPermission::Deny;
    let adapter = AcpAdapter::new(config);
    let sink = RecordingSink::default();
    let result = adapter
        .run(chat_req(dir.path()), "cos-acp-new", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(result.terminal, Terminal::Done { .. }));
    let new: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("new.json")).unwrap()).unwrap();
    assert_eq!(new["method"], "session/new");
    assert_eq!(new["params"]["cwd"], dir.path().to_string_lossy().as_ref());
    let permission: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("permission.json")).unwrap())
            .unwrap();
    assert_eq!(permission["result"]["outcome"]["optionId"], "allow");
    let kinds: Vec<_> = sink
        .structured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, f)| f.kind)
        .collect();
    assert!(kinds.contains(&ProgressKind::Text));
    assert!(kinds.contains(&ProgressKind::ToolUse));
    assert!(kinds.contains(&ProgressKind::ToolResult), "{kinds:?}");
}

#[tokio::test]
async fn cos_chat_harness_acp_loads_supported_session() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
mkdir -p artifacts
read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
read -r load
printf '%s\n' "$load" > session.json
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{}}'
read -r prompt
printf '%s\n' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let mut req = chat_req(dir.path());
    resumed(&mut req);
    let sink = RecordingSink::default();
    let outcome = AcpAdapter::new(stub_acp(dir.path(), script))
        .run(req, "cos-acp-load", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let msg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("session.json")).unwrap()).unwrap();
    assert_eq!(msg["method"], "session/load");
    assert_eq!(msg["params"]["sessionId"], "old-session");
    assert_eq!(sink.sessions.lock().unwrap().as_slice(), &["old-session"]);
}

#[tokio::test]
async fn cos_chat_harness_acp_missing_load_starts_fresh_once() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
mkdir -p artifacts
read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r new
printf '%s\n' "$new" > session.json
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"fresh"}}'
read -r prompt
printf '%s\n' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let mut req = chat_req(dir.path());
    resumed(&mut req);
    let sink = RecordingSink::default();
    AcpAdapter::new(stub_acp(dir.path(), script))
        .run(req, "cos-acp-fresh", default_limits(), &sink)
        .await
        .unwrap();
    let msg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("session.json")).unwrap()).unwrap();
    assert_eq!(msg["method"], "session/new");
    assert_eq!(sink.sessions.lock().unwrap().as_slice(), &["fresh"]);
    assert_eq!(
        progress_of(&sink)
            .iter()
            .filter(|s| s.contains("starting fresh"))
            .count(),
        1
    );
}

#[tokio::test]
async fn cos_chat_harness_acp_rejected_load_starts_fresh_once() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
mkdir -p artifacts
read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
read -r load
printf '%s\n' "$load" > load.json
printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"expired"}}'
read -r new
printf '%s\n' "$new" > new.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"sessionId":"fresh"}}'
read -r prompt
printf '%s\n' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
"#;
    let mut req = chat_req(dir.path());
    resumed(&mut req);
    let sink = RecordingSink::default();
    AcpAdapter::new(stub_acp(dir.path(), script))
        .run(req, "cos-acp-reject", default_limits(), &sink)
        .await
        .unwrap();
    let msg: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("new.json")).unwrap()).unwrap();
    assert_eq!(msg["method"], "session/new");
    assert_eq!(sink.sessions.lock().unwrap().as_slice(), &["fresh"]);
    assert_eq!(
        progress_of(&sink)
            .iter()
            .filter(|s| s.contains("starting fresh"))
            .count(),
        1
    );
}

#[tokio::test]
async fn cos_chat_harness_acp_image_resource_and_unsupported() {
    for (caps, expected) in [
        (r#"{"promptCapabilities":{"image":true}}"#, "image"),
        (
            r#"{"promptCapabilities":{"resource":true}}"#,
            "resource_link",
        ),
        (
            r#"{"promptCapabilities":{"embeddedContext":true}}"#,
            "resource_link",
        ),
        (r#"{}"#, "unsupported"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen.png");
        std::fs::write(&path, b"image-bytes").unwrap();
        let mut req = chat_req(dir.path());
        req.context
            .cos_chat
            .as_mut()
            .unwrap()
            .attachments
            .push(CosChatAttachment {
                id: "image-1".into(),
                name: "screen.png".into(),
                media_type: "image/png".into(),
                size_bytes: 11,
                sha256: String::new(),
                path: path.clone(),
                delivery: CosChatDelivery::Image,
            });
        let script = format!(
            r#"
mkdir -p artifacts
read -r init
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"agentCapabilities":{caps}}}}}'
read -r new
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"fresh"}}}}'
read -r prompt
printf '%s\n' "$prompt" > prompt.json
printf '%s\n' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        );
        let sink = RecordingSink::default();
        AcpAdapter::new(stub_acp(dir.path(), &script))
            .run(req, "cos-acp-image", default_limits(), &sink)
            .await
            .unwrap();
        let msg: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("prompt.json")).unwrap())
                .unwrap();
        let blocks = msg["params"]["prompt"].as_array().unwrap();
        if expected == "unsupported" {
            assert_eq!(blocks.len(), 1);
            assert!(
                progress_of(&sink)
                    .iter()
                    .any(|s| s.contains("not inspected"))
            );
        } else {
            assert_eq!(blocks[1]["type"], expected);
            if expected == "image" {
                assert_eq!(blocks[1]["mimeType"], "image/png");
                assert_eq!(blocks[1]["data"], "aW1hZ2UtYnl0ZXM=");
            }
            if expected == "resource_link" {
                assert!(blocks[1]["uri"].as_str().unwrap().contains("screen.png"));
            }
        }
    }
}

/// live-check 不具合 1: an ACP CoS chat run writes no `result.json`; an `end_turn` stop is done with
/// the streamed agent message chunks (other stop reasons stay failed).
#[tokio::test]
async fn cos_chat_harness_acp_done_without_result_json() {
    let script = |stop: &str| {
        format!(
            r#"
read -r init
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"agentCapabilities":{{}}}}}}'
read -r new
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"new-session"}}}}'
read -r prompt
printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"了解"}}}}}}}}'
printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"しました"}}}}}}}}'
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"{stop}"}}}}'
"#
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let adapter = AcpAdapter::new(stub_acp(dir.path(), &script("end_turn")));
    let result = adapter
        .run(
            chat_req(dir.path()),
            "cos-acp-no-result",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    match result.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "了解しました"),
        other => panic!("unexpected terminal {other:?}"),
    }

    let dir = tempfile::tempdir().unwrap();
    let adapter = AcpAdapter::new(stub_acp(dir.path(), &script("refusal")));
    let result = adapter
        .run(
            chat_req(dir.path()),
            "cos-acp-refusal",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(result.terminal, Terminal::Error { .. }),
        "{:?}",
        result.terminal
    );
}
