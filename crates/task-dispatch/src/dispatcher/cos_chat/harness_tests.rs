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
    let store = Arc::new(SqliteStore::open(&dir.path().join("chat.sqlite")).expect("store"));
    fixture_on(store, dir, harness, adapter)
}

/// [`fixture`] on a store opened at `<dir>/chat.sqlite` by the caller.
fn fixture_on(
    store: Arc<SqliteStore>,
    dir: crate::test_support::WritableTempDir,
    harness: &str,
    adapter: Arc<dyn WorkerAdapter>,
) -> (
    crate::test_support::WritableTempDir,
    Arc<SqliteStore>,
    Dispatcher,
    String,
) {
    let db_path = dir.path().join("chat.sqlite");
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

// ---- ADR 2026-10-05 D2 付記: resume 時の差分配送（delivery cursor） ----
// Each route start also adds a system notice after the claimed input/reply pair.
// Notices are durable history and must be included in subsequent delivery ranges.

/// What the delta probe does on one attempt (1-based order of `DeltaProbe::steps`).
#[derive(Clone, Copy, Debug)]
enum DeltaStep {
    /// Runs the FakeAdapter script (a fresh attempt reports its own session id).
    Ok,
    /// Saves `summary` through the run's input, then runs the script.
    Checkpoint(&'static str),
    /// A resumed attempt reports `session_resume_failed`.
    Refuse,
    /// Ends with an error before the run completes.
    Fail,
    /// Applies an operation before hitting a supply limit and switching routes.
    SupplyFail,
    /// Ends with the context budget exhausted.
    ContextExhausted,
    /// The daemon "restarted": the run is taken over as an orphan while the worker is alive.
    Orphan,
}

#[derive(Clone)]
struct DeltaProbe {
    inner: Arc<dyn WorkerAdapter>,
    store: Arc<SqliteStore>,
    steps: Vec<DeltaStep>,
    seen: Arc<StdMutex<Vec<RunRequest>>>,
}

impl DeltaProbe {
    fn new(store: &Arc<SqliteStore>, steps: Vec<DeltaStep>) -> Self {
        Self {
            inner: Arc::new(task_worker::fake::FakeAdapter::new(
                task_worker::fake::FakeAdapter::default_command(),
            )),
            store: Arc::clone(store),
            steps,
            seen: Arc::default(),
        }
    }

    fn chat(&self, attempt: usize) -> task_worker::protocol::CosChatContext {
        self.seen.lock().expect("seen")[attempt]
            .context
            .cos_chat
            .clone()
            .expect("cos chat context")
    }
}

#[async_trait]
impl WorkerAdapter for DeltaProbe {
    fn id(&self) -> &str {
        "fake"
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let n = {
            let mut seen = self.seen.lock().expect("seen");
            seen.push(req.clone());
            seen.len()
        };
        let chat = req.context.cos_chat.clone().expect("cos chat context");
        let resume = req.context.session.as_ref().is_some_and(|s| s.resume);
        let error = |message: &str| {
            Ok(RunOutcome {
                terminal: Terminal::Error {
                    message: message.into(),
                    retryable: false,
                },
                exit_code: Some(1),
            })
        };
        match self.steps.get(n - 1).copied().unwrap_or(DeltaStep::Ok) {
            DeltaStep::Ok => {}
            DeltaStep::Checkpoint(summary) => {
                let through = chat.inputs.iter().map(|i| i.seq).max().unwrap_or(0) as u64;
                self.store
                    .chat_thread_checkpoint_at(
                        &chat.thread_id,
                        &chat.run_id,
                        summary,
                        through,
                        chat.summary_through_seq as u64,
                        OffsetDateTime::from_unix_timestamp(1_791_158_400).expect("clock"),
                    )
                    .expect("checkpoint");
            }
            DeltaStep::Refuse if resume => {
                sink.session_resume_failed("No conversation found with session ID");
                return error("resume refused");
            }
            DeltaStep::Refuse => {}
            DeltaStep::Fail => return error("worker crashed before the end"),
            DeltaStep::SupplyFail => {
                self.store
                    .cos_operation_apply(
                        &task_core::chat::AuditContext {
                            actor: task_core::chat::ChatActor::Cos,
                            thread_id: chat.thread_id.clone(),
                            run_id: chat.run_id.clone(),
                            operation_id: ulid::Ulid::new().to_string(),
                            reason: "fixture operation".into(),
                            policy_version: "v1".into(),
                        },
                        "delta-fallback-key",
                        "fixture-hash",
                        "project",
                        "fixture-project",
                        None,
                        "edit",
                        &serde_json::json!({}),
                        |_, _| Ok(serde_json::json!({"done": true})),
                    )
                    .expect("applied receipt");
                return error("provider exhausted: session limit");
            }
            DeltaStep::ContextExhausted => {
                return Ok(RunOutcome {
                    terminal: Terminal::BudgetExhausted {
                        kind: BudgetKind::Context,
                        message: "prompt is too long".into(),
                        usage: None,
                    },
                    exit_code: Some(1),
                });
            }
            DeltaStep::Orphan => {
                let input = &chat.inputs[0];
                self.store
                    .chat_run_takeover(
                        &chat.run_id,
                        &ChatPostMessageRequest {
                            client_message_id: format!("cos-recover:{}", chat.run_id),
                            text: input.text.clone(),
                            attachment_ids: vec![],
                            reply_to_id: Some(input.id.clone()),
                            mode: ChatSendMode::Interrupt,
                            resume_queue: false,
                        },
                        crate::dispatcher::cos_chat::control::ORPHAN_TAKEOVER_REASON,
                        OffsetDateTime::from_unix_timestamp(1_791_158_400).expect("clock"),
                    )
                    .expect("takeover")
                    .expect("taken");
                return error("daemon restarted");
            }
        }
        if !resume {
            sink.session_established(&format!("probe-session-{n}"));
        }
        self.inner.run(req, run_id, limits, sink).await
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.inner = self.inner.with_env(extra)?;
        Some(Arc::new(next))
    }
}

/// Ticks once and joins the worker; returns the run whatever its terminal state.
async fn run_any(
    d: &mut Dispatcher,
    store: &SqliteStore,
    thread: &str,
) -> task_core::chat::ChatRun {
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
    store.chat_run_get(thread, &id).expect("finished run")
}

fn delta_fixture(
    steps: Vec<DeltaStep>,
) -> (
    crate::test_support::WritableTempDir,
    Arc<SqliteStore>,
    Dispatcher,
    String,
    DeltaProbe,
) {
    let dir = crate::test_support::WritableTempDir::new();
    let store = Arc::new(SqliteStore::open(&dir.path().join("chat.sqlite")).expect("store"));
    let probe = DeltaProbe::new(&store, steps);
    let (data, store, d, thread) = fixture_on(store, dir, "fake", Arc::new(probe.clone()));
    (data, store, d, thread, probe)
}

/// (delivery cursor of the live session, the thread's summary watermark).
fn live_cursor(store: &SqliteStore, thread: &str) -> (i64, i64) {
    let live = store
        .chat_session_active(thread)
        .expect("session")
        .expect("live session");
    let (_, summary_through) = store.chat_thread_summary(thread).expect("summary");
    (live.delivered_through_seq, summary_through as i64)
}

fn session_detail(store: &SqliteStore, run: &str) -> String {
    store
        .chat_run_session_record(run)
        .expect("record")
        .detail
        .unwrap_or_default()
}

fn seqs(chat: &task_worker::protocol::CosChatContext) -> Vec<i64> {
    chat.unsummarized.messages.iter().map(|m| m.seq).collect()
}

#[tokio::test]
async fn cos_chat_resume_delta_resumed_run_gets_only_messages_after_the_cursor() {
    let (_dir, store, mut d, t, probe) = delta_fixture(vec![DeltaStep::Checkpoint("S1")]);
    post(&store, &d, &t, "one", vec![]); // seq 1, reply seq 2
    let first = run(&mut d, &store, &t).await;
    let c1 = probe.chat(0);
    assert_eq!(
        c1.delivered_through_seq, None,
        "a new session gets everything"
    );
    assert_eq!(session_detail(&store, &first.id), "new");
    assert_eq!(
        live_cursor(&store, &t),
        (2, 1),
        "cursor covers the reply; summary is 1"
    );

    store
        .chat_system_message_add(&t, "card between turns", &[], d.now_utc())
        .expect("system message"); // seq 4
    post(&store, &d, &t, "two", vec![]); // seq 5, reply seq 6
    let second = run(&mut d, &store, &t).await;
    assert_eq!(session_detail(&store, &second.id), "resumed");
    let c2 = probe.chat(1);
    assert_eq!(c2.delivered_through_seq, Some(2));
    assert_eq!(c2.summary, None, "the summary is already in the session");
    assert_eq!(c2.summary_through_seq, 1, "the watermark is still passed");
    assert_eq!(
        (c2.unsummarized.from_seq, c2.unsummarized.through_seq),
        (3, 4)
    );
    assert_eq!(seqs(&c2), vec![3, 4]);
    assert_eq!(c2.inputs.len(), 1);
    assert_eq!(c2.inputs[0].seq, 5);
    assert_eq!(live_cursor(&store, &t), (6, 1));

    post(&store, &d, &t, "three", vec![]); // seq 8
    run(&mut d, &store, &t).await;
    let c3 = probe.chat(2);
    assert_eq!(c3.delivered_through_seq, Some(6));
    assert_eq!(
        seqs(&c3),
        vec![7],
        "the route notice after the reply is new"
    );
    assert_eq!(c3.inputs[0].text, "message three");
    assert_eq!(live_cursor(&store, &t), (9, 1));
}

/// A fresh session for every retire reason gets the summary and the unsummarized history.
#[tokio::test]
async fn cos_chat_resume_delta_fresh_reasons_deliver_the_full_text() {
    for reason in [
        "token_rollover",
        "cache_missing",
        "key_changed",
        "context_exhausted",
    ] {
        let steps = if reason == "context_exhausted" {
            vec![DeltaStep::Checkpoint("S1"), DeltaStep::ContextExhausted]
        } else {
            vec![DeltaStep::Checkpoint("S1")]
        };
        let (_dir, store, mut d, t, probe) = delta_fixture(steps);
        post(&store, &d, &t, "one", vec![]); // seq 1, reply 2
        run(&mut d, &store, &t).await;
        let row = store.chat_session_active(&t).expect("s").expect("live");
        assert_eq!(row.delivered_through_seq, 2, "{reason}");
        match reason {
            "token_rollover" => {
                d.config.session_rollover_tokens = 100;
                store
                    .chat_session_touch(
                        &row.id,
                        task_core::chat::ChatSessionUsage {
                            add_tokens: 1_000,
                            context_tokens: Some(1_000),
                            ..Default::default()
                        },
                        d.now_utc(),
                    )
                    .expect("touch");
            }
            "cache_missing" => {
                store.chat_session_set_id(&row.id, "").expect("cache gone");
            }
            "key_changed" => {
                d.cos_chat_launch
                    .as_mut()
                    .expect("launch")
                    .config
                    .llm_source = Some("other".into());
            }
            _ => {
                post(&store, &d, &t, "exhaust", vec![]); // seq 3, reply 4 (not completed)
                let exhausted = run_any(&mut d, &store, &t).await;
                assert_ne!(exhausted.state, ChatRunState::Completed);
                assert_eq!(probe.chat(1).delivered_through_seq, Some(2));
            }
        }
        post(&store, &d, &t, "next", vec![]);
        let next = run(&mut d, &store, &t).await;
        assert_eq!(session_detail(&store, &next.id), "fresh", "{reason}");
        let attempts = probe.seen.lock().expect("seen").len();
        let c = probe.chat(attempts - 1);
        assert_eq!(c.delivered_through_seq, None, "{reason}");
        assert_eq!(c.summary.as_deref(), Some("S1"), "{reason}");
        assert_eq!(c.unsummarized.from_seq, 2, "{reason}");
        assert_eq!(c.unsummarized.through_seq, c.inputs[0].seq - 1, "{reason}");
        assert_eq!(
            seqs(&c).first(),
            Some(&2),
            "{reason}: history after the summary"
        );
        // The fresh row starts its own cursor at this run's input/reply.
        let (cursor, _) = live_cursor(&store, &t);
        assert_eq!(cursor, c.inputs[0].seq + 1, "{reason}");
    }
}

#[tokio::test]
async fn cos_chat_resume_delta_refused_resume_retries_fresh_with_the_full_text() {
    let (_dir, store, mut d, t, probe) =
        delta_fixture(vec![DeltaStep::Checkpoint("S1"), DeltaStep::Refuse]);
    post(&store, &d, &t, "one", vec![]); // seq 1, reply 2
    run(&mut d, &store, &t).await;
    let old = store.chat_session_active(&t).expect("s").expect("live");
    post(&store, &d, &t, "two", vec![]); // seq 4, reply 5
    let second = run(&mut d, &store, &t).await;
    assert_eq!(session_detail(&store, &second.id), "fresh_after_refusal");
    let refused = probe.chat(1);
    let retry = probe.chat(2);
    assert_eq!(refused.delivered_through_seq, Some(2));
    assert_eq!(refused.summary, None);
    assert_eq!(retry.delivered_through_seq, None);
    assert_eq!(retry.summary.as_deref(), Some("S1"));
    assert_eq!(seqs(&retry), vec![2, 3]);
    assert_eq!(
        retry.inputs, refused.inputs,
        "same input, not claimed again"
    );
    // The retired row keeps its cursor; the fresh row covers this run.
    let old = store.chat_session_get(&old.id).expect("get").expect("row");
    assert_eq!(old.delivered_through_seq, 2);
    assert_eq!(live_cursor(&store, &t), (5, 1));
}

#[tokio::test]
async fn cos_chat_resume_delta_interrupt_and_queued_inputs_are_always_delivered() {
    let (_dir, store, mut d, t, probe) = delta_fixture(vec![]);
    post(&store, &d, &t, "one", vec![]); // seq 1, reply 2
    run(&mut d, &store, &t).await;
    post(&store, &d, &t, "two", vec![]); // seq 4, queued
    store
        .chat_message_post(
            &t,
            &ChatPostMessageRequest {
                client_message_id: "three".into(),
                text: "interrupt three".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            d.now_utc(),
        )
        .expect("interrupt"); // seq 5
    run(&mut d, &store, &t).await; // the interrupt first, reply seq 6
    let c2 = probe.chat(1);
    assert_eq!(c2.inputs[0].seq, 5);
    assert_eq!(c2.inputs[0].text, "interrupt three");
    assert_eq!(c2.delivered_through_seq, Some(2));
    assert_eq!(
        seqs(&c2),
        vec![3, 4],
        "the waiting message is visible as history"
    );
    assert_eq!(live_cursor(&store, &t).0, 6);

    // The waiting message is now below the cursor, and still delivered as the input.
    run(&mut d, &store, &t).await;
    let c3 = probe.chat(2);
    assert_eq!(c3.inputs[0].seq, 4);
    assert_eq!(c3.inputs[0].text, "message two");
    assert_eq!(c3.delivered_through_seq, Some(6));
    assert!(c3.unsummarized.messages.is_empty());
    let users = store
        .chat_message_list(
            &t,
            &task_core::chat::ChatMessageQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .expect("messages")
        .items
        .into_iter()
        .filter(|m| m.role == task_core::chat::ChatMessageRole::User)
        .collect::<Vec<_>>();
    assert_eq!(users.len(), 3);
    assert!(
        users
            .iter()
            .all(|m| m.state == task_core::chat::ChatMessageState::Completed),
        "{users:?}"
    );
}

#[tokio::test]
async fn cos_chat_resume_delta_failed_run_keeps_the_cursor_and_redelivers() {
    let (_dir, store, mut d, t, probe) = delta_fixture(vec![DeltaStep::Ok, DeltaStep::Fail]);
    post(&store, &d, &t, "one", vec![]); // seq 1, reply 2
    run(&mut d, &store, &t).await;
    post(&store, &d, &t, "two", vec![]); // seq 4, reply 5
    let failed = run_any(&mut d, &store, &t).await;
    assert_eq!(failed.state, ChatRunState::Failed);
    assert_eq!(
        live_cursor(&store, &t).0,
        2,
        "a failed run does not move the cursor"
    );

    post(&store, &d, &t, "three", vec![]); // seq 7
    let third = run(&mut d, &store, &t).await;
    assert_eq!(session_detail(&store, &third.id), "resumed");
    let c3 = probe.chat(2);
    assert_eq!(c3.delivered_through_seq, Some(2));
    assert_eq!(
        seqs(&c3),
        vec![3, 4, 5, 6],
        "the failed turn is delivered again"
    );
    assert_eq!(live_cursor(&store, &t).0, 8);
}

#[tokio::test]
async fn cos_chat_resume_delta_restart_recovery_delivers_the_full_text() {
    let (_dir, store, mut d, t, probe) =
        delta_fixture(vec![DeltaStep::Checkpoint("S1"), DeltaStep::Orphan]);
    post(&store, &d, &t, "one", vec![]); // seq 1, reply 2
    run(&mut d, &store, &t).await;
    post(&store, &d, &t, "two", vec![]); // seq 4, reply 5
    let orphan = run_any(&mut d, &store, &t).await;
    assert_eq!(orphan.state, ChatRunState::Interrupted);
    assert_eq!(live_cursor(&store, &t).0, 2);

    // The continuation (seq 7) resumes the same session but gets the full text.
    let next = run(&mut d, &store, &t).await;
    assert_eq!(session_detail(&store, &next.id), "resumed");
    let c3 = probe.chat(2);
    assert_eq!(c3.inputs[0].text, "message two");
    assert_eq!(c3.delivered_through_seq, None);
    assert_eq!(c3.summary.as_deref(), Some("S1"));
    assert_eq!(seqs(&c3), vec![2, 3, 4, 5, 6]);
}

#[tokio::test]
async fn cos_chat_resume_delta_checkpoint_watermark_and_cursor_are_independent() {
    let (_dir, store, mut d, t, probe) = delta_fixture(vec![
        DeltaStep::Checkpoint("S1"),
        DeltaStep::Checkpoint("S2"),
        DeltaStep::Fail,
    ]);
    post(&store, &d, &t, "one", vec![]); // seq 1, reply 2
    run(&mut d, &store, &t).await;
    assert_eq!(live_cursor(&store, &t), (2, 1));
    post(&store, &d, &t, "two", vec![]); // seq 4, reply 5
    run(&mut d, &store, &t).await;
    // The summary moved to 4 and the cursor to 5: neither follows the other.
    assert_eq!(live_cursor(&store, &t), (5, 4));
    assert_eq!(store.chat_thread_summary(&t).expect("summary").1, 4);

    // A failed run does not move the cursor even though nothing about the summary changed.
    post(&store, &d, &t, "three", vec![]); // seq 7, reply 8
    run_any(&mut d, &store, &t).await;
    assert_eq!(live_cursor(&store, &t), (5, 4));
    post(&store, &d, &t, "four", vec![]); // seq 10
    run(&mut d, &store, &t).await;
    let c = probe.chat(3);
    // The summary covers up to 4, but the session only holds up to 5: the delta starts at 6.
    assert_eq!(c.summary_through_seq, 4);
    assert_eq!(c.delivered_through_seq, Some(5));
    assert_eq!(seqs(&c), vec![6, 7, 8, 9]);
}

#[tokio::test]
async fn cos_chat_resume_delta_fallback_route_gets_full_history() {
    let (_dir, store, mut d, t, probe) = delta_fixture(vec![
        DeltaStep::Checkpoint("saved summary"),
        DeltaStep::Ok,
        DeltaStep::SupplyFail,
        DeltaStep::Ok,
    ]);
    // Use the same key for both routes to ensure switching always starts another session.
    d.cos_chat_launch.as_mut().unwrap().config.fallbacks =
        vec![crate::dispatcher::cos_chat::launch::CosChatRoute {
            harness: "fake".into(),
            llm_source: Some("test".into()),
            provider: Some("p1".into()),
            account_id: None,
            model: None,
            tier: Tier::Frontier,
            unavailable_reason: None,
        }];
    post(&store, &d, &t, "one", vec![]);
    run(&mut d, &store, &t).await;
    post(&store, &d, &t, "two", vec![]);
    run(&mut d, &store, &t).await;
    let old = store.chat_session_active(&t).unwrap().unwrap();
    assert!(old.delivered_through_seq > 1);
    post(&store, &d, &t, "three", vec![]);
    let failed_attempt = run_any(&mut d, &store, &t).await;
    assert_eq!(failed_attempt.state, ChatRunState::Running);
    assert_eq!(session_detail(&store, &failed_attempt.id), "resumed");
    assert_eq!(probe.chat(2).summary, None);
    assert_eq!(
        probe.chat(2).delivered_through_seq,
        Some(old.delivered_through_seq)
    );
    assert!(!seqs(&probe.chat(2)).contains(&2));
    assert_eq!(
        store
            .chat_session_get(&old.id)
            .unwrap()
            .unwrap()
            .delivered_through_seq,
        old.delivered_through_seq,
        "a failed attempt must not move the cursor"
    );

    // The route retry continues the same durable run/input after the first worker exited.
    let finished = run(&mut d, &store, &t).await;
    assert_eq!(finished.id, failed_attempt.id);
    assert_eq!(finished.input_message_id, failed_attempt.input_message_id);
    let full = probe.chat(3);
    assert_eq!(full.delivered_through_seq, None);
    let summary = full.summary.as_deref().expect("full summary with receipt");
    assert!(summary.starts_with("saved summary"));
    assert!(summary.contains("Applied operations from the previous route (do not reapply)"));
    assert!(summary.contains("edit project:fixture-project"));
    assert!(summary.contains("idempotency_key=delta-fallback-key"));
    assert_eq!(full.summary_through_seq, 1);
    let expected = crate::dispatcher::cos_chat::rollover::history_since_summary(
        &store,
        &t,
        full.inputs[0].seq as u64,
    )
    .unwrap()
    .2;
    assert_eq!(
        full.unsummarized, expected,
        "all unsummarized history is sent"
    );
    assert!(
        seqs(&full).contains(&2),
        "history already held by the old session is resent"
    );
    assert!(
        full.unsummarized
            .messages
            .iter()
            .any(|m| m.text == "message two")
    );
    assert_eq!(full.inputs[0].text, "message three");
    let seen = probe.seen.lock().unwrap();
    assert!(!seen[3].context.session.as_ref().unwrap().resume);
    drop(seen);
    let active = store.chat_session_active(&t).unwrap().unwrap();
    assert_ne!(active.id, old.id);
    assert!(active.delivered_through_seq >= full.inputs[0].seq);
    assert_eq!(
        store
            .chat_run_session_record(&finished.id)
            .unwrap()
            .session_row_id,
        Some(active.id)
    );
    assert_eq!(
        store
            .chat_session_get(&old.id)
            .unwrap()
            .unwrap()
            .delivered_through_seq,
        old.delivered_through_seq,
        "only the completing session advances"
    );
    assert_eq!(
        store
            .cos_operation_applied_for_run(&finished.id)
            .unwrap()
            .len(),
        1
    );
}
