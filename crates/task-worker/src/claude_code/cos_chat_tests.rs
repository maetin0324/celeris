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

/// ADR 2026-10-08-cos-chat-prompt-cache D6: claude-code gets the fixed Core in one
/// `--append-system-prompt` argument after HEADLESS_RUN_NOTE, the run specific part on stdin, and
/// `prompt.txt` records both. Two runs of different threads send the same system argument.
#[tokio::test]
async fn cos_chat_core_claude_append_system_prompt_stdin_and_prompt_txt() {
    let mut systems = Vec::new();
    for (thread, run, seq) in [
        ("thread-a", "chat-run-a", 3),
        ("thread-b", "chat-run-b", 41),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = format!("cat > stdin.log\n{}", args_log_script());
        let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &script));
        let mut req = chat_request(dir.path(), false);
        let chat = req.context.cos_chat.as_mut().unwrap();
        chat.thread_id = thread.into();
        chat.run_id = run.into();
        chat.inputs = vec![crate::protocol::CosChatInput {
            id: format!("msg-{seq}"),
            seq,
            text: format!("相談 {seq}"),
            ..Default::default()
        }];
        let core = crate::cos_chat::core(chat);
        adapter
            .run(req, run, default_limits(), &RecordingSink::default())
            .await
            .unwrap();
        let args = captured_args(dir.path());
        let i = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("--append-system-prompt present");
        let system = &args[i + 1];
        assert_eq!(
            system,
            &format!("{}\n{core}", crate::preamble::HEADLESS_RUN_NOTE)
        );
        assert!(!system.contains(thread) && !system.contains(run));
        assert!(!system.contains(&format!("seq {seq}")));
        let stdin = std::fs::read_to_string(dir.path().join("stdin.log")).unwrap();
        assert!(!stdin.contains("# CoS chat Core"), "{stdin}");
        assert!(stdin.contains(&format!("# CoS chat: thread {thread}\n")));
        assert!(stdin.contains(&format!("- `<chat run id>` = `{run}`")));
        assert!(stdin.contains(&format!("### seq {seq} (message msg-{seq})")));
        let recorded =
            std::fs::read_to_string(dir.path().join("runs").join(run).join("prompt.txt")).unwrap();
        assert_eq!(
            recorded,
            crate::cos_chat::prompt_record(
                "--append-system-prompt, after HEADLESS_RUN_NOTE",
                &core,
                &stdin
            )
        );
        assert!(recorded.contains(&core) && recorded.contains(&stdin));
        systems.push(system.clone());
    }
    assert_eq!(systems[0].as_bytes(), systems[1].as_bytes());
}

/// ADR 2026-11-04-project-controller §A4: the raw record of a budget scenario
/// (`CELERIS_GCC_E2E_OUT`, read by `scripts/dev/test-general-cos-consult-e2e.sh`).
fn gcc_record(name: &str, row: serde_json::Value) {
    let Ok(dir) = std::env::var("CELERIS_GCC_E2E_OUT") else {
        return;
    };
    let mut row = row;
    row["schema"] = serde_json::json!("celeris.general-cos-consult/1");
    row["test"] = serde_json::json!(name);
    std::fs::write(
        Path::new(&dir).join(format!("{name}.jsonl")),
        format!("{row}\n"),
    )
    .unwrap();
}

/// A fake claude that runs `n` Bash tool calls (each its own assistant line), then replies.
fn bash_calls_script(commands: &[&str]) -> String {
    let mut script = String::from("cat >/dev/null\n");
    for c in commands {
        let line = serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Bash", "input": {"command": c}}]}});
        script.push_str(&format!("printf '%s\\n' '{line}'\n"));
        script.push_str("printf '%s\\n' '{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"ok\"}]}}'\n");
    }
    script.push_str("printf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"直近 3 件の比較です\"}'\n");
    script
}

/// A short read inside the budget completes the run with the reply (no task is involved here).
#[tokio::test]
async fn general_cos_consult_short_read_within_the_budget_completes() {
    let dir = tempfile::tempdir().unwrap();
    let commands = [
        "celerisctl list --limit 3",
        "celerisctl show 01A",
        "wc -l notes.md",
    ];
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &bash_calls_script(&commands)));
    let mut req = chat_request(dir.path(), false);
    req.context
        .cos_chat
        .as_mut()
        .unwrap()
        .inline_read_max_commands = Some(3);
    let outcome = adapter
        .run(req, "read", default_limits(), &RecordingSink::default())
        .await
        .unwrap();
    match &outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "直近 3 件の比較です"),
        other => panic!("unexpected terminal {other:?}"),
    }
    gcc_record(
        "general_cos_consult_short_read_within_the_budget_completes",
        serde_json::json!({"reads": commands.len(), "max": 3, "terminal": "done", "llm_launches": 0}),
    );
}

/// The command after the limit stops the run (`cos_inline_budget_exceeded`); the reply that
/// would have followed is never taken.
#[tokio::test]
async fn general_cos_consult_read_over_the_budget_stops_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let commands = ["cat a", "cat b", "cat c", "cat d"];
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &bash_calls_script(&commands)));
    let mut req = chat_request(dir.path(), false);
    req.context
        .cos_chat
        .as_mut()
        .unwrap()
        .inline_read_max_commands = Some(2);
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "over", default_limits(), &sink)
        .await
        .unwrap();
    let message = match &outcome.terminal {
        Terminal::Error {
            message, retryable, ..
        } => {
            assert!(!retryable);
            message.clone()
        }
        other => panic!("unexpected terminal {other:?}"),
    };
    assert!(
        message.starts_with(crate::tool_policy::COS_INLINE_BUDGET_EXCEEDED),
        "{message}"
    );
    assert!(message.contains("3 shell commands"), "{message}");
    let progress = sink.progress.lock().unwrap().join("\n");
    assert!(
        progress.contains(crate::tool_policy::COS_INLINE_BUDGET_EXCEEDED),
        "{progress}"
    );
    gcc_record(
        "general_cos_consult_read_over_the_budget_stops_the_run",
        serde_json::json!({"reads": 3, "max": 2, "terminal": "error", "stop": message, "llm_launches": 0}),
    );
}

/// An LLM CLI launched from a CoS chat run is reported as before (`WorkerPolicyViolation`) and
/// now also stops the run; a regular worker run still only warns (ADR 2026-10-07 D5).
#[tokio::test]
async fn general_cos_consult_llm_cli_launch_stops_the_cos_run() {
    let dir = tempfile::tempdir().unwrap();
    let script = bash_calls_script(&["claude -p compare-the-last-three"]);
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &script));
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(
            chat_request(dir.path(), false),
            "llm",
            default_limits(),
            &sink,
        )
        .await
        .unwrap();
    let message = match &outcome.terminal {
        Terminal::Error { message, .. } => message.clone(),
        other => panic!("unexpected terminal {other:?}"),
    };
    assert!(
        message.starts_with(crate::tool_policy::COS_LLM_LAUNCH_REFUSED),
        "{message}"
    );
    let violations = sink.violations.lock().unwrap().clone();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].kind, task_core::ToolPolicyKind::LlmCli);

    // The same line in a regular (non-chat) run is a warning only: the run completes.
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "mkdir -p artifacts\nprintf '%s' '{{\"summary\":\"ok\",\"evidence\":[]}}' > artifacts/result.json\n{script}"
    );
    let adapter = ClaudeCodeAdapter::new(stub_claude(dir.path(), &script));
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(
            sample_req(dir.path().to_path_buf()),
            "worker",
            default_limits(),
            &sink,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Done { .. }),
        "{:?}",
        outcome.terminal
    );
    assert_eq!(sink.violations.lock().unwrap().len(), 1);
    gcc_record(
        "general_cos_consult_llm_cli_launch_stops_the_cos_run",
        serde_json::json!({"cos_terminal": "error", "stop": message, "violations": 1,
                           "worker_terminal": "done", "llm_launches": 0}),
    );
}
