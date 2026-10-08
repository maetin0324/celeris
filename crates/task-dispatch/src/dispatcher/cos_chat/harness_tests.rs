//! The production chat dispatcher drives each concrete adapter through two queued runs.
//! Stubs speak the CLI/ACP wire protocols; SQLite and an injected clock are the only state.
use super::*;
use crate::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_core::chat::attachments::ChatAttachmentStore;
use task_core::chat::{
    ChatCreateThreadRequest, ChatEventData, ChatEventQuery, ChatEventType, ChatPostMessageRequest,
    ChatRunState, ChatSendMode,
};
use task_worker::acp::{AcpAdapter, AcpConfig, AcpPermission};
use task_worker::claude_code::{ClaudeCodeAdapter, ClaudeCodeConfig};
use task_worker::codex::{CodexAdapter, CodexConfig};

fn stub(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\nset -eu\n{body}\n"));
    path.to_string_lossy().into_owned()
}

fn fixture(
    harness: &str,
    adapter: Arc<dyn WorkerAdapter>,
) -> (
    crate::test_support::WritableTempDir,
    Arc<SqliteStore>,
    Dispatcher,
    String,
) {
    let dir = crate::test_support::WritableTempDir::new();
    let db_path = dir.path().join("chat.sqlite");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let mut d = dispatcher_with_adapter_id(store.clone(), adapter, 2, true, harness);
    d.config.min_free_disk_mb = 0;
    d.config.execution.max_cos_runs = 2;
    d.config.knowledge.root = dir.path().join("knowledge");
    for name in ["cos-operator", "cos-inbox-triage"] {
        let path = d.config.knowledge.root.join("skills").join(name);
        std::fs::create_dir_all(&path).expect("skill dir");
        std::fs::write(
            path.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: stub\n---\n# {name}\n"),
        )
        .expect("skill");
    }
    let now = OffsetDateTime::from_unix_timestamp(1_791_158_400).expect("clock");
    d.test_now = Some(Arc::new(StdMutex::new(now)));
    d.set_cos_chat_launch(
        store.clone(),
        CosChatLaunchConfig {
            enabled: true,
            fallbacks: Vec::new(),
            worker_reserve_five_hour: 0.90,
            harness: harness.into(),
            llm_source: Some("test".into()),
            provider: Some("p1".into()),
            account_id: None,
            model: None,
            tier: Tier::Frontier,
            max_turns: 3,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: dir.path().to_path_buf(),
            db_path,
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:1/api/v1".into(),
            triage: Default::default(),
        },
    );
    let thread = store
        .chat_thread_create(
            "admin",
            &ChatCreateThreadRequest {
                title: "harness".into(),
                project_id: None,
                client_thread_id: "harness".into(),
            },
            now,
        )
        .expect("thread")
        .thread
        .id;
    (dir, store, d, thread)
}

fn post(store: &SqliteStore, d: &Dispatcher, thread: &str, key: &str, attachments: Vec<String>) {
    store
        .chat_message_post(
            thread,
            &ChatPostMessageRequest {
                client_message_id: key.into(),
                text: format!("message {key}"),
                attachment_ids: attachments,
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .expect("post");
}

async fn run(d: &mut Dispatcher, store: &SqliteStore, thread: &str) -> task_core::chat::ChatRun {
    d.tick_cos_chat_launch();
    let id = store
        .chat_thread_get(thread)
        .expect("thread")
        .expect("exists")
        .active_run_id
        .expect("claimed run");
    let handle = d
        .cos_chat_launch
        .as_mut()
        .expect("launch")
        .running
        .remove(thread)
        .expect("worker");
    tokio::time::timeout(Duration::from_secs(15), handle)
        .await
        .expect("worker deadline")
        .expect("worker join");
    let run = store.chat_run_get(thread, &id).expect("finished run");
    assert_eq!(run.state, ChatRunState::Completed, "{run:?}");
    run
}

fn events(store: &SqliteStore, thread: &str, run: &str) -> Vec<task_core::chat::ChatEvent> {
    store
        .chat_events_page(
            thread,
            &ChatEventQuery {
                after: None,
                run_id: Some(run.into()),
                limit: Some(500),
            },
        )
        .expect("events")
        .items
}

fn assert_mapped(events: &[task_core::chat::ChatEvent]) {
    let kinds: Vec<_> = events.iter().map(|e| e.event_type).collect();
    assert!(kinds.contains(&ChatEventType::TextDelta), "{kinds:?}");
    assert!(kinds.contains(&ChatEventType::Tool), "{kinds:?}");
    assert!(kinds.contains(&ChatEventType::Status), "{kinds:?}");
}

fn upload_image(dir: &Path, store: &SqliteStore, thread: &str) -> String {
    let attachments = ChatAttachmentStore::open(dir, &dir.join("chat.sqlite"), Default::default())
        .expect("attachments");
    let row = attachments
        .upload(
            thread,
            "image-1",
            "pixel.png",
            None,
            std::io::Cursor::new(b"\x89PNG\r\n\x1a\nimage"),
            OffsetDateTime::from_unix_timestamp(1_791_158_400).unwrap(),
        )
        .expect("image");
    // Same SQLite file backs both stores.
    let _ = store;
    row.id
}

#[tokio::test]
async fn cos_chat_harness_e2e_claude_initial_resume_events_and_native_image() {
    let script = r#"
if [ -e first.args ]; then dest=second; else dest=first; fi
printf '%s\n' "$@" > "$dest.args"
cat > "$dest.input"
find .taskd/chat-runs -mindepth 1 -maxdepth 1 -type d -exec sh -c 'printf "%s" "{\"summary\":\"hello\",\"evidence\":[]}" > "$1/result.json"' sh {} \;
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"thinking","summary":"checking"},{"type":"text","text":"hello"},{"type":"tool_use","name":"Bash","input":{"command":"pwd"}}]}}' '{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"hello"}'
"#;
    let dir = crate::test_support::WritableTempDir::new();
    let command = stub(dir.path(), "claude.sh", script);
    let adapter = Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
        command,
        ..Default::default()
    }));
    let (data, store, mut d, thread) = fixture("claude-code", adapter);
    // The command writes in the per-thread workspace; the stub itself lives outside it.
    let workspace = data
        .path()
        .join("cos/threads")
        .join(&thread)
        .join("workspace");
    post(&store, &d, &thread, "one", vec![]);
    let first = run(&mut d, &store, &thread).await;
    assert_mapped(&events(&store, &thread, &first.id));
    let args = std::fs::read_to_string(workspace.join("first.args")).unwrap();
    assert!(args.contains("--session-id\n"));
    assert!(!args.contains("--allowedTools"));
    let session_id = args
        .lines()
        .skip_while(|arg| *arg != "--session-id")
        .nth(1)
        .expect("initial session id");
    let image = upload_image(data.path(), &store, &thread);
    post(&store, &d, &thread, "two", vec![image]);
    let second = run(&mut d, &store, &thread).await;
    let args = std::fs::read_to_string(workspace.join("second.args")).unwrap();
    assert!(
        args.contains(&format!("--resume\n{session_id}\n")),
        "{args}"
    );
    let input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("second.input")).unwrap()).unwrap();
    assert_eq!(input["message"]["content"][1]["type"], "image");
    assert_eq!(
        input["message"]["content"][1]["source"]["media_type"],
        "image/png"
    );
    assert_mapped(&events(&store, &thread, &second.id));
}

#[tokio::test]
async fn cos_chat_harness_e2e_codex_initial_resume_events_and_native_image() {
    let script = r#"
if [ -e first.args ]; then dest=second; else dest=first; fi
printf '%s\n' "$@" > "$dest.args"
cat > "$dest.input"
find .taskd/chat-runs -mindepth 1 -maxdepth 1 -type d -exec sh -c 'printf "%s" "{\"summary\":\"hello\",\"evidence\":[]}" > "$1/result.json"' sh {} \;
printf '%s\n' '{"type":"thread.started","thread_id":"thread-cos-1"}' '{"type":"item.completed","item":{"id":"s","type":"todo_list","text":"checking"}}' '{"type":"item.completed","item":{"id":"m","type":"agent_message","text":"hello"}}' '{"type":"item.started","item":{"id":"t","type":"command_execution","command":"pwd"}}' '{"type":"item.completed","item":{"id":"t","type":"command_execution","command":"pwd","aggregated_output":"ok","exit_code":0}}' '{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":4}}'
"#;
    let dir = crate::test_support::WritableTempDir::new();
    let command = stub(dir.path(), "codex.sh", script);
    let adapter = Arc::new(CodexAdapter::new(CodexConfig {
        command,
        env: vec![(
            "CODEX_HOME".into(),
            dir.path().join("codex-home").to_string_lossy().into_owned(),
        )],
        ..Default::default()
    }));
    let (data, store, mut d, thread) = fixture("codex", adapter);
    let workspace = data
        .path()
        .join("cos/threads")
        .join(&thread)
        .join("workspace");
    post(&store, &d, &thread, "one", vec![]);
    let first = run(&mut d, &store, &thread).await;
    assert_mapped(&events(&store, &thread, &first.id));
    let args = std::fs::read_to_string(workspace.join("first.args")).unwrap();
    assert!(args.contains("exec\n"));
    assert!(!args.contains("sandbox_mode=\"read-only\""));
    let image = upload_image(data.path(), &store, &thread);
    post(&store, &d, &thread, "two", vec![image]);
    let second = run(&mut d, &store, &thread).await;
    let args = std::fs::read_to_string(workspace.join("second.args")).unwrap();
    assert!(args.contains("resume\nthread-cos-1\n"), "{args}");
    assert!(args.contains("--image\n"), "{args}");
    let image_path = args
        .lines()
        .skip_while(|arg| *arg != "--image")
        .nth(1)
        .expect("image path");
    assert!(std::path::Path::new(image_path).is_file());
    assert_mapped(&events(&store, &thread, &second.id));
}

#[tokio::test]
async fn cos_chat_harness_e2e_acp_new_load_events_and_image_capabilities() {
    let script = r#"
read -r init
if [ -e enable-native ]; then
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"promptCapabilities":{"image":true}}}}'
else
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
fi
read -r session
printf '%s\n' "$session" > session.json
if [ -e first.session ]; then printf '%s\n' "$session" > second.session; else printf '%s\n' "$session" > first.session; fi
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"acp-session-1"}}'
read -r prompt
printf '%s\n' "$prompt" > prompt.json
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"plan"}}}' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","title":"Bash","status":"in_progress"}}}' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","title":"Bash","status":"completed","rawOutput":"ok"}}}'
find .taskd/chat-runs -mindepth 1 -maxdepth 1 -type d -exec sh -c 'printf "%s" "{\"summary\":\"hello\",\"evidence\":[]}" > "$1/result.json"' sh {} \;
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#;
    let dir = crate::test_support::WritableTempDir::new();
    let command = stub(dir.path(), "acp.sh", script);
    let adapter = Arc::new(AcpAdapter::new(AcpConfig {
        command,
        args: vec![],
        permission: AcpPermission::Deny,
        startup_timeout: Duration::from_secs(5),
        ..Default::default()
    }));
    let (data, store, mut d, thread) = fixture("acp", adapter);
    let workspace = data
        .path()
        .join("cos/threads")
        .join(&thread)
        .join("workspace");
    post(&store, &d, &thread, "one", vec![]);
    let first = run(&mut d, &store, &thread).await;
    assert_mapped(&events(&store, &thread, &first.id));
    let first_req: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("first.session")).unwrap()).unwrap();
    assert_eq!(first_req["method"], "session/new");
    let image = upload_image(data.path(), &store, &thread);
    post(&store, &d, &thread, "two", vec![image]);
    let second = run(&mut d, &store, &thread).await;
    let second_req: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("second.session")).unwrap()).unwrap();
    assert_eq!(second_req["method"], "session/load");
    assert_eq!(second_req["params"]["sessionId"], "acp-session-1");
    let prompt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("prompt.json")).unwrap()).unwrap();
    let prompt_text = prompt.to_string();
    assert!(
        prompt_text.contains("unsupported") && prompt_text.contains("not inspected"),
        "{prompt_text}"
    );
    assert_mapped(&events(&store, &thread, &second.id));
    assert!(
        !events(&store, &thread, &second.id)
            .iter()
            .any(|e| matches!(&e.data,
        ChatEventData::Status(s) if s.summary.contains("inspected image")))
    );

    std::fs::write(workspace.join("enable-native"), "1").unwrap();
    let image = upload_image(data.path(), &store, &thread);
    post(&store, &d, &thread, "three", vec![image]);
    let third = run(&mut d, &store, &thread).await;
    let prompt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(workspace.join("prompt.json")).unwrap()).unwrap();
    assert_eq!(prompt["params"]["prompt"][1]["type"], "image");
    assert_eq!(prompt["params"]["prompt"][1]["mimeType"], "image/png");
    assert!(
        prompt["params"]["prompt"][1]["data"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_mapped(&events(&store, &thread, &third.id));
}

/// live-check 不具合 1: the claude CLI streams the reply and writes no `result.json` (the CoS chat
/// preamble does not ask for it); the chat run completes and the output message is the reply.
#[tokio::test]
async fn cos_chat_harness_e2e_claude_without_result_json_completes() {
    let script = r#"
cat > /dev/null
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"確認します"},{"type":"tool_use","name":"Bash","input":{"command":"pwd"}}]}}' '{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"了解しました"}'
"#;
    let dir = crate::test_support::WritableTempDir::new();
    let command = stub(dir.path(), "claude.sh", script);
    let adapter = Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
        command,
        ..Default::default()
    }));
    let (_data, store, mut d, thread) = fixture("claude-code", adapter);
    post(&store, &d, &thread, "one", vec![]);
    // `run` asserts `ChatRunState::Completed`.
    let finished = run(&mut d, &store, &thread).await;
    assert_eq!(finished.reason, None, "{finished:?}");
    let output_id = finished.output_message_id.clone().expect("output message");
    let output = store
        .chat_message_list(
            &thread,
            &task_core::chat::ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .find(|m| m.id == output_id)
        .expect("output message row");
    assert_eq!(output.text, "了解しました");
}

/// ADR 2026-10-08-cos-chat-prompt-cache T5: `cos-inbox-triage` is mounted only for the inbox scene.
#[test]
fn cos_chat_skills_mount_triage_only_in_the_inbox_thread_or_with_inbox_items() {
    use crate::dispatcher::cos_chat::launch::cos_chat_skills;
    assert_eq!(cos_chat_skills(false, &[]), vec!["cos-operator"]);
    assert_eq!(
        cos_chat_skills(true, &[]),
        vec!["cos-operator", "cos-inbox-triage"]
    );
    let item = task_worker::protocol::CosChatInboxItem {
        item_id: "i1".into(),
        state: "pending".into(),
        source_kind: "question".into(),
        source_key: "q1".into(),
        source_revision: "1".into(),
        summary: "s".into(),
        reason: None,
        decision: None,
        answer_path: None,
        created_at: "2026-10-08T00:00:00Z".into(),
    };
    assert_eq!(
        cos_chat_skills(false, &[item]),
        vec!["cos-operator", "cos-inbox-triage"]
    );
}

/// An ordinary thread's run gets `cos-operator` only: neither the delivered skill directory nor
/// the prompt carries the triage procedure.
#[tokio::test]
async fn cos_chat_skill_ordinary_thread_run_does_not_mount_inbox_triage() {
    let script = r#"
cat > first.input
find .taskd/chat-runs -mindepth 1 -maxdepth 1 -type d -exec sh -c 'printf "%s" "{\"summary\":\"hello\",\"evidence\":[]}" > "$1/result.json"' sh {} \;
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"hello"}'
"#;
    let dir = crate::test_support::WritableTempDir::new();
    let command = stub(dir.path(), "claude.sh", script);
    let adapter = Arc::new(ClaudeCodeAdapter::new(ClaudeCodeConfig {
        command,
        ..Default::default()
    }));
    let (data, store, mut d, thread) = fixture("claude-code", adapter);
    let workspace = data
        .path()
        .join("cos/threads")
        .join(&thread)
        .join("workspace");
    post(&store, &d, &thread, "one", vec![]);
    run(&mut d, &store, &thread).await;
    let skills = workspace.join(".claude/skills");
    assert!(skills.join("cos-operator/SKILL.md").is_file());
    assert!(!skills.join("cos-inbox-triage").exists());
    let input = std::fs::read_to_string(workspace.join("first.input")).unwrap();
    assert!(input.contains("`cos-operator`"), "{input}");
    assert!(!input.contains("cos-inbox-triage"), "{input}");
}
