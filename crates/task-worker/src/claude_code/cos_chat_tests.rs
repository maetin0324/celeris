use super::*;
use crate::cos_chat::HarnessCapabilities;
use crate::protocol::{CosChatAttachment, CosChatContext, CosChatDelivery, SessionHandle};

const SESSION_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

fn chat_request(workspace: &Path, native_image: bool) -> RunRequest {
    let mut req = sample_req(workspace.to_path_buf());
    let mut chat = CosChatContext {
        thread_id: "thread-1".into(),
        run_id: "chat-run-1".into(),
        ..CosChatContext::default()
    };
    if native_image {
        chat.harness_capabilities = HarnessCapabilities::for_adapter("claude-code");
        chat.attachments.push(CosChatAttachment {
            id: "image-1".into(),
            name: "pixel.png".into(),
            media_type: "image/png".into(),
            size_bytes: 4,
            path: workspace.join("pixel.png"),
            delivery: CosChatDelivery::Image,
            ..CosChatAttachment::default()
        });
    }
    req.context.cos_chat = Some(chat);
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    req
}

fn session(req: &mut RunRequest, resume: bool) {
    req.context.session = Some(SessionHandle {
        adapter: ClaudeCodeAdapter::ID.into(),
        session_id: SESSION_ID.into(),
        resume,
    });
}

#[tokio::test]
async fn cos_chat_harness_claude_initial_session_and_full_tools() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), args_log_script()));
    let mut req = chat_request(dir.path(), false);
    session(&mut req, false);
    adapter
        .run(req, "initial", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(args.windows(2).any(|a| a == ["--session-id", SESSION_ID]));
    assert!(!args.contains(&"--resume".into()));
    assert!(!args.contains(&"--allowedTools".into()));
    assert!(!args.contains(&"--no-session-persistence".into()));
}

#[tokio::test]
async fn cos_chat_harness_claude_resumes_same_session() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), args_log_script()));
    let mut req = chat_request(dir.path(), false);
    session(&mut req, true);
    adapter
        .run(req, "resume", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(args.windows(2).any(|a| a == ["--resume", SESSION_ID]));
    assert!(!args.contains(&"--session-id".into()));
    assert!(!args.contains(&"--allowedTools".into()));
}

#[tokio::test]
async fn cos_chat_harness_claude_native_image_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pixel.png"), b"data").unwrap();
    let script = format!("cat > stdin.log\n{}", args_log_script());
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &script));
    let req = chat_request(dir.path(), true);
    adapter
        .run(req, "image", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(
        args.windows(2)
            .any(|a| a == ["--input-format", "stream-json"])
    );
    let input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("stdin.log")).unwrap()).unwrap();
    assert_eq!(input["type"], "user");
    assert_eq!(input["message"]["content"][1]["type"], "image");
    assert_eq!(
        input["message"]["content"][1]["source"]["media_type"],
        "image/png"
    );
    assert_eq!(input["message"]["content"][1]["source"]["data"], "ZGF0YQ==");
}

#[tokio::test]
async fn cos_chat_harness_claude_unsupported_image_has_reason_and_no_image_block() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("cat > stdin.log\n{}", args_log_script());
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &script));
    let mut req = chat_request(dir.path(), true);
    req.context.cos_chat.as_mut().unwrap().harness_capabilities = None;
    adapter
        .run(
            req,
            "unsupported",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"--input-format".into()));
    let input = std::fs::read_to_string(dir.path().join("stdin.log")).unwrap();
    assert!(input.contains("actual=unsupported"));
    assert!(input.contains("image was not inspected"));
}

#[tokio::test]
async fn cos_chat_harness_claude_path_image_requires_read_tool() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("cat > stdin.log\n{}", args_log_script());
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &script));
    let mut req = chat_request(dir.path(), true);
    let caps = req
        .context
        .cos_chat
        .as_mut()
        .unwrap()
        .harness_capabilities
        .as_mut()
        .unwrap();
    caps.native_image_input = false;
    adapter
        .run(req, "path", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    let args = captured_args(dir.path());
    assert!(!args.contains(&"--input-format".into()));
    let input = std::fs::read_to_string(dir.path().join("stdin.log")).unwrap();
    assert!(input.contains("actual=path+tool"));
    assert!(input.contains("use the confirmed image-reading tool"));
}

#[tokio::test]
async fn cos_chat_harness_claude_resume_rejection_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = ClaudeCodeAdapter::new(stub_claude(
        dir.path(),
        "printf '%s\\n' 'Session not found' >&2\nexit 1",
    ));
    let mut req = chat_request(dir.path(), false);
    session(&mut req, true);
    let sink = RecordingSink::default();
    let _ = adapter.run(req, "rejected", default_limits(), &sink).await;
    assert!(!sink.session_resume_failed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cos_chat_harness_claude_event_mapping_hides_thinking_body() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"cat >/dev/null
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"private body","summary":"public summary"},{"type":"text","text":"hello"},{"type":"tool_use","name":"Bash","input":{"command":"pwd"}}]}}' '{"type":"user","message":{"content":[{"type":"tool_result","content":"done"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'
"#;
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), script));
    let sink = RecordingSink::default();
    adapter
        .run(
            chat_request(dir.path(), false),
            "events",
            default_limits(),
            &sink,
        )
        .await
        .unwrap();
    let items = sink.structured.lock().unwrap();
    let kinds: Vec<_> = items.iter().map(|(_, f)| f.kind).collect();
    assert!(kinds.contains(&Some(task_core::ProgressKind::Thinking)));
    assert!(kinds.contains(&Some(task_core::ProgressKind::Text)));
    assert!(kinds.contains(&Some(task_core::ProgressKind::ToolUse)));
    assert!(kinds.contains(&Some(task_core::ProgressKind::ToolResult)));
    assert!(
        items
            .iter()
            .any(|(_, f)| f.summary.as_deref() == Some("public summary"))
    );
    assert!(!format!("{items:?}").contains("private body"));
}

/// live-check 不具合 1: the CoS chat preamble does not ask for `result.json`, so a successful run
/// without it is done with the `result` text (no `artifacts/result.json` is written here).
#[tokio::test]
async fn cos_chat_harness_claude_done_without_result_json() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"見ています"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"了解しました"}'
"#;
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), script));
    let req = chat_request(dir.path(), false);
    let outcome = adapter
        .run(
            req,
            "no-result-json",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .expect("a CoS chat run without result.json is not an adapter error");
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "了解しました"),
        other => panic!("unexpected terminal {other:?}"),
    }
    assert!(!dir.path().join("artifacts/result.json").exists());

    // An empty `result` falls back to the streamed assistant text.
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"一つ目"},{"type":"tool_use","name":"Bash","input":{"command":"pwd"}}]}}' '{"type":"assistant","message":{"content":[{"type":"text","text":"二つ目"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":""}'
"#;
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), script));
    let outcome = adapter
        .run(
            chat_request(dir.path(), false),
            "streamed",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "一つ目\n\n二つ目"),
        other => panic!("unexpected terminal {other:?}"),
    }

    // An existing result.json still wins.
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
mkdir -p artifacts
printf '%s' '{"summary":"from file","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"from body"}'
"#;
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), script));
    let outcome = adapter
        .run(
            chat_request(dir.path(), false),
            "with-file",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "from file"),
        other => panic!("unexpected terminal {other:?}"),
    }
}

/// A failed `result` (`is_error`) of a CoS chat run stays failed even without `result.json`.
#[tokio::test]
async fn cos_chat_harness_claude_error_without_result_json_stays_failed() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"途中"}]}}' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"boom"}'
"#;
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), script));
    let outcome = adapter
        .run(
            chat_request(dir.path(), false),
            "failed",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Error { .. }),
        "{:?}",
        outcome.terminal
    );
}

/// The exception is CoS chat only: a regular run without `result.json` keeps the
/// `RESULT_JSON_MISSING_MARKER` adapter error (InfraRequeue).
#[tokio::test]
async fn cos_chat_harness_claude_non_chat_run_still_requires_result_json() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
"#;
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), script));
    let err = adapter
        .run(
            sample_req(dir.path().to_path_buf()),
            "regular",
            default_limits(),
            &RecordingSink::default(),
        )
        .await
        .expect_err("result.json is required outside CoS chat");
    assert!(
        err.to_string().contains(RESULT_JSON_MISSING_MARKER),
        "{err}"
    );
}
