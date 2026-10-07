//! ADR 2026-10-05 cos-chat-home D2/D4: the codex adapter on a CoS chat run (`context.cos_chat`),
//! driven by a fake `codex` CLI (no network, no real CLI, no sleeps).

use std::sync::Mutex;
use std::time::Duration;

use task_core::{ArtifactRef, DelegateTask, ProgressFields, ProgressKind};

use super::*;
use crate::cos_chat::{HarnessCapabilities, MissingCapability, capability_reason};
use crate::protocol::{
    CosChatAttachment, CosChatContext, CosChatDelivery, CosChatHistory, CosChatHistoryMessage,
    CosChatInput, PROTOCOL_VERSION, RunContext, SessionHandle,
};

#[derive(Default)]
struct Sink {
    progress: Mutex<Vec<(String, ProgressFields)>>,
    sessions: Mutex<Vec<String>>,
    resume_failed: Mutex<Vec<String>>,
}

impl Sink {
    fn fields(&self) -> Vec<ProgressFields> {
        let items = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        items.iter().map(|(_, f)| f.clone()).collect()
    }
    fn statuses(&self) -> Vec<String> {
        self.fields()
            .into_iter()
            .filter(|f| f.kind == Some(ProgressKind::Status))
            .filter_map(|f| f.summary)
            .collect()
    }
    fn sessions(&self) -> Vec<String> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl EventSink for Sink {
    fn progress(&self, _msg: &str) {}
    fn progress_with(&self, msg: &str, fields: &ProgressFields) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((msg.to_string(), fields.clone()));
    }
    fn artifact(&self, _artifact: &ArtifactRef) {}
    fn delegate(&self, _tasks: &[DelegateTask]) {}
    fn session_established(&self, session_id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_id.to_string());
    }
    fn session_resume_failed(&self, reason: &str) {
        self.resume_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(reason.to_string());
    }
}

/// A fake `codex`: records argv (NUL separated) and the stdin prompt, then prints `events`.
/// No `result.json` is written: a CoS chat run replies with its final `agent_message`.
fn fake_codex(dir: &Path, events: &[&str]) -> CodexConfig {
    let mut script = String::from(
        "for a in \"$@\"; do printf '%s\\0' \"$a\" >> args.log; done\ncat > stdin.txt\n",
    );
    for event in events {
        script.push_str(&format!("printf '%s\\n' '{event}'\n"));
    }
    let path = dir.join("codex_fake.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}"));
    CodexConfig {
        command: path.to_string_lossy().into_owned(),
        env: vec![(
            "CODEX_HOME".into(),
            dir.join("codex-home").to_string_lossy().into_owned(),
        )],
        ..CodexConfig::default()
    }
}

const THREAD_STARTED: &str = r#"{"type":"thread.started","thread_id":"thread-cos-1"}"#;
const REPLY: &str =
    r#"{"type":"item.completed","item":{"id":"i9","type":"agent_message","text":"直しました"}}"#;
const TURN_COMPLETED: &str =
    r#"{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":4}}"#;

fn chat() -> CosChatContext {
    CosChatContext {
        thread_id: "thread-a".into(),
        run_id: "chat-run-a".into(),
        inputs: vec![CosChatInput {
            id: "m3".into(),
            seq: 3,
            text: "画面を直して".into(),
            ..CosChatInput::default()
        }],
        summary_through_seq: 0,
        unsummarized: CosChatHistory {
            from_seq: 1,
            through_seq: 2,
            messages: vec![
                CosChatHistoryMessage {
                    id: "m1".into(),
                    seq: 1,
                    role: "user".into(),
                    text: "前回の依頼: ログを見て".into(),
                },
                CosChatHistoryMessage {
                    id: "m2".into(),
                    seq: 2,
                    role: "assistant".into(),
                    text: "ログを確認しました".into(),
                },
            ],
        },
        credential_env: crate::protocol::COS_RUN_CREDENTIAL_ENV.into(),
        api_base_url: "http://127.0.0.1:1/api/v1".into(),
        ..CosChatContext::default()
    }
}

fn cos_req(workspace: &Path, chat: CosChatContext, session: Option<SessionHandle>) -> RunRequest {
    RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: chat.transient_task(
            workspace,
            task_core::Budget {
                max_turns: 10,
                max_wall_secs: 60,
                max_retries: 0,
            },
            time::OffsetDateTime::UNIX_EPOCH,
        ),
        artifacts_dir: workspace.join(".taskd/chat-runs/chat-run-a"),
        workspace: workspace.to_path_buf(),
        work_dir: None,
        context: RunContext {
            cos_chat: Some(chat),
            session,
            ..RunContext::default()
        },
    }
}

fn resume(id: &str) -> Option<SessionHandle> {
    Some(SessionHandle {
        adapter: CodexAdapter::ID.into(),
        session_id: id.into(),
        resume: true,
    })
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(60),
        idle_timeout: Duration::from_secs(60),
        kill_grace: Duration::from_millis(200),
    }
}

fn args(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("args.log"))
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

async fn run(config: CodexConfig, req: RunRequest, sink: &Sink) -> RunOutcome {
    CodexAdapter::new(config)
        .run(req, "worker-run-1", limits(), sink)
        .await
        .unwrap_or_else(|e| panic!("codex run failed: {e}"))
}

fn fresh_reason() -> String {
    capability_reason(MissingCapability::Continuation).to_string()
}

/// The first run is a plain `codex exec`; the id of `thread.started` is the session, the final
/// `agent_message` is the reply, and the sandbox is workspace-write (not the read-only CoS one).
#[tokio::test]
async fn cos_chat_harness_codex_first_run_confirms_thread_started_id() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
    let sink = Sink::default();
    let outcome = run(config, cos_req(dir.path(), chat(), None), &sink).await;
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "直しました"),
        other => panic!("unexpected terminal {other:?}"),
    }
    assert_eq!(sink.sessions(), vec!["thread-cos-1".to_string()]);
    let args = args(dir.path());
    assert_eq!(args[0], "exec", "{args:?}");
    assert!(!args.contains(&"resume".to_string()), "{args:?}");
    assert!(args.contains(&"sandbox_mode=\"workspace-write\"".to_string()));
    assert!(!args.contains(&"sandbox_mode=\"read-only\"".to_string()));
    for forbidden in [
        "--dangerously-bypass-approvals-and-sandbox",
        "--ignore-rules",
    ] {
        assert!(!args.contains(&forbidden.to_string()), "{args:?}");
    }
    // The artifacts dir is still granted on the fresh form.
    assert!(args.contains(&"--add-dir".to_string()), "{args:?}");
    assert!(sink.statuses().iter().all(|s| !s.contains(&fresh_reason())));
}

/// A resumed CoS run uses `codex exec resume <id>` (the id confirmed by the first run), with no
/// explicit-fresh status, and keeps the `-c` sandbox override.
#[tokio::test]
async fn cos_chat_harness_codex_resume_uses_exec_resume() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
    let sink = Sink::default();
    let outcome = run(
        config,
        cos_req(dir.path(), chat(), resume("thread-cos-1")),
        &sink,
    )
    .await;
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let args = args(dir.path());
    assert_eq!(&args[..3], ["exec", "resume", "thread-cos-1"], "{args:?}");
    assert!(args.contains(&"sandbox_mode=\"workspace-write\"".to_string()));
    assert!(!args.contains(&"--add-dir".to_string()), "{args:?}");
    assert!(sink.statuses().iter().all(|s| !s.contains(&fresh_reason())));
    assert!(sink.resume_failed.lock().unwrap().is_empty());
}

/// A codex that cannot guarantee `exec resume` (the experimental resume of old versions) starts
/// an explicit fresh session: no resume argument, a status with the reason, and the DB history
/// (summary + unsummarized messages) in the prompt.
#[tokio::test]
async fn cos_chat_harness_codex_unguaranteed_resume_starts_explicit_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
    config.resume_mode = CodexResumeMode::ExperimentalResume;
    let sink = Sink::default();
    let outcome = run(
        config,
        cos_req(dir.path(), chat(), resume("thread-old")),
        &sink,
    )
    .await;
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let args = args(dir.path());
    assert!(!args.contains(&"resume".to_string()), "{args:?}");
    assert!(!args.iter().any(|a| a.contains("experimental_resume")));
    assert!(!args.iter().any(|a| a.contains("thread-old")), "{args:?}");
    let statuses = sink.statuses();
    assert!(
        statuses
            .iter()
            .any(|s| s.contains(&fresh_reason()) && s.contains("experimental")),
        "{statuses:?}"
    );
    // The new thread id replaces the old one.
    assert_eq!(sink.sessions(), vec!["thread-cos-1".to_string()]);
    let prompt = std::fs::read_to_string(dir.path().join("stdin.txt")).unwrap();
    assert!(prompt.contains("前回の依頼: ログを見て"), "{prompt}");
    assert!(prompt.contains("ログを確認しました"), "{prompt}");
    assert!(prompt.contains("画面を直して"), "{prompt}");
}

/// Operator `extra_args` that `exec resume` cannot take also make a CoS run start explicit fresh
/// (the dangerous bypass is never used, ADR-0095 D-b), and the fresh form keeps the flags.
#[tokio::test]
async fn cos_chat_harness_codex_untranslatable_args_start_explicit_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
    config.extra_args = vec!["--search".into()];
    config.resume_bypass = CodexResumeBypass::Dangerous;
    let sink = Sink::default();
    let outcome = run(
        config,
        cos_req(dir.path(), chat(), resume("thread-cos-1")),
        &sink,
    )
    .await;
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let args = args(dir.path());
    assert!(!args.contains(&"resume".to_string()), "{args:?}");
    assert!(args.contains(&"--search".to_string()), "{args:?}");
    assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    let statuses = sink.statuses();
    assert!(
        statuses
            .iter()
            .any(|s| s.contains(&fresh_reason()) && s.contains("extra_args")),
        "{statuses:?}"
    );
}

/// `agent_message` → text (whole body, newlines kept), `command_execution` / `mcp_tool_call` →
/// tool_use then tool_result, an unknown item → status.
#[tokio::test]
async fn cos_chat_harness_codex_maps_message_command_mcp_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(
        dir.path(),
        &[
            THREAD_STARTED,
            r#"{"type":"item.started","item":{"id":"c1","type":"command_execution","command":"ls -la","status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"ls -la","aggregated_output":"total 0","exit_code":0,"status":"completed"}}"#,
            r#"{"type":"item.started","item":{"id":"p1","type":"mcp_tool_call","server":"celeris","tool":"task_list","status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"p1","type":"mcp_tool_call","server":"celeris","tool":"task_list","output":"3 tasks","status":"completed"}}"#,
            r#"{"type":"item.completed","item":{"id":"x1","type":"hologram","text":"?"}}"#,
            r#"{"type":"item.completed","item":{"id":"i9","type":"agent_message","text":"一行目\n二行目"}}"#,
            TURN_COMPLETED,
        ],
    );
    let sink = Sink::default();
    let outcome = run(config, cos_req(dir.path(), chat(), None), &sink).await;
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "一行目\n二行目"),
        other => panic!("unexpected terminal {other:?}"),
    }
    let items: Vec<ProgressFields> = sink.fields();
    let kinds: Vec<Option<ProgressKind>> = items.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::Status),
            Some(ProgressKind::Text),
        ],
        "{items:#?}"
    );
    assert_eq!(items[0].tool.as_deref(), Some("command_execution"));
    assert_eq!(items[0].summary.as_deref(), Some("ls -la"));
    assert_eq!(items[1].summary.as_deref(), Some("total 0"));
    assert!(!items[1].error);
    assert_eq!(items[2].tool.as_deref(), Some("mcp_tool_call"));
    assert_eq!(items[2].summary.as_deref(), Some("task_list"));
    assert_eq!(items[3].summary.as_deref(), Some("3 tasks"));
    assert_eq!(items[4].summary.as_deref(), Some("item.completed hologram"));
    assert_eq!(items[5].detail.as_deref(), Some("一行目\n二行目"));
}

fn attachment(dir: &Path, id: &str, name: &str, media_type: &str) -> CosChatAttachment {
    let path = dir.join("attachments").join(id).join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"\x89PNG fake").unwrap();
    CosChatAttachment {
        id: id.into(),
        name: name.into(),
        media_type: media_type.into(),
        size_bytes: 9,
        sha256: "0".repeat(64),
        path,
        delivery: CosChatDelivery::for_media_type(media_type),
    }
}

/// With confirmed native image input, a `delivery=image` attachment reaches codex as `--image`
/// (before `--json`, so the trailing `-` prompt is not taken as an image), on fresh and resume;
/// a `delivery=file` attachment is never passed as an image.
#[tokio::test]
async fn cos_chat_harness_codex_native_image_uses_image_argument() {
    for session in [None, resume("thread-cos-1")] {
        let dir = tempfile::tempdir().unwrap();
        let config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
        let mut chat = chat();
        let image = attachment(dir.path(), "a1", "screen.png", "image/png");
        let file = attachment(dir.path(), "a2", "notes.pdf", "application/pdf");
        chat.attachments = vec![image.clone(), file.clone()];
        chat.harness_capabilities = HarnessCapabilities::for_adapter("codex");
        let sink = Sink::default();
        run(config, cos_req(dir.path(), chat, session.clone()), &sink).await;
        let args = args(dir.path());
        let at = args
            .iter()
            .position(|a| a == "--image")
            .unwrap_or_else(|| panic!("no --image in {args:?}"));
        assert_eq!(args[at + 1], image.path.to_string_lossy());
        assert_eq!(args.iter().filter(|a| *a == "--image").count(), 1);
        let json = args.iter().position(|a| a == "--json").unwrap();
        assert!(at < json, "{args:?}");
        assert!(!args.contains(&file.path.to_string_lossy().into_owned()));
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert_eq!(
            args.contains(&"resume".to_string()),
            session.is_some(),
            "{args:?}"
        );
        let prompt = std::fs::read_to_string(dir.path().join("stdin.txt")).unwrap();
        assert!(prompt.contains("actual=native"), "{prompt}");
    }
}

/// codex splits `--image` values on `,`: such a path is passed as a staged copy instead.
#[tokio::test]
async fn cos_chat_harness_codex_image_path_with_comma_is_staged() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
    let mut chat = chat();
    chat.attachments = vec![attachment(dir.path(), "a1", "a,b.png", "image/png")];
    chat.harness_capabilities = HarnessCapabilities::for_adapter("codex");
    run(config, cos_req(dir.path(), chat, None), &Sink::default()).await;
    let args = args(dir.path());
    let at = args.iter().position(|a| a == "--image").unwrap();
    let staged = std::path::PathBuf::from(&args[at + 1]);
    assert!(!args[at + 1].contains(','), "{args:?}");
    assert_eq!(std::fs::read(&staged).unwrap(), b"\x89PNG fake");
}

/// Without confirmed abilities the image is unsupported: no `--image`, a status saying it was not
/// inspected. With only an image-reading tool it is path+tool: no `--image`, a status naming it.
#[tokio::test]
async fn cos_chat_harness_codex_image_unsupported_or_path_is_not_passed_natively() {
    let path_only = HarnessCapabilities {
        native_image_input: false,
        ..HarnessCapabilities::for_adapter("codex").unwrap()
    };
    for (caps, expected) in [
        (None, capability_reason(MissingCapability::Image)),
        (Some(path_only), "use the confirmed image-reading tool"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
        let mut chat = chat();
        chat.attachments = vec![attachment(dir.path(), "a1", "screen.png", "image/png")];
        chat.harness_capabilities = caps;
        let sink = Sink::default();
        run(config, cos_req(dir.path(), chat, None), &sink).await;
        let args = args(dir.path());
        assert!(!args.contains(&"--image".to_string()), "{args:?}");
        let statuses = sink.statuses();
        assert!(
            statuses
                .iter()
                .any(|s| s.contains("attachment a1") && s.contains(expected)),
            "{statuses:?}"
        );
    }
}

/// The non-CoS read-only conversation is unchanged: no `cos_chat` → read-only sandbox, no image
/// handling, and a direct reply is not accepted without `task.conversation`.
#[tokio::test]
async fn cos_chat_harness_codex_non_cos_secretary_stays_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(dir.path(), &[THREAD_STARTED, REPLY, TURN_COMPLETED]);
    let mut req = cos_req(dir.path(), chat(), None);
    req.context.cos_chat = None;
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let outcome = run(config, req, &Sink::default()).await;
    assert!(matches!(outcome.terminal, Terminal::Error { .. }));
    let args = args(dir.path());
    assert!(
        args.contains(&"sandbox_mode=\"read-only\"".to_string()),
        "{args:?}"
    );
    assert!(!args.contains(&"--image".to_string()));
}

/// live-check 不具合 1: a CoS chat codex run writes no `result.json`; the last `agent_message`
/// (after a tool item) is the reply.
#[tokio::test]
async fn cos_chat_harness_codex_done_without_result_json() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(
        dir.path(),
        &[
            THREAD_STARTED,
            r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"調べます"}}"#,
            r#"{"type":"item.started","item":{"id":"i2","type":"command_execution","command":"pwd"}}"#,
            r#"{"type":"item.completed","item":{"id":"i2","type":"command_execution","command":"pwd","aggregated_output":"/w","exit_code":0,"status":"completed"}}"#,
            REPLY,
            TURN_COMPLETED,
        ],
    );
    let sink = Sink::default();
    let req = cos_req(dir.path(), chat(), None);
    let result_json = req.artifacts_dir.join("result.json");
    let outcome = run(config, req, &sink).await;
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "直しました"),
        other => panic!("unexpected terminal {other:?}"),
    }
    // The reply is plain text; it is not recovered into result.json.
    assert!(!result_json.exists());
}

/// A failed turn of a CoS chat codex run stays failed even with a streamed message.
#[tokio::test]
async fn cos_chat_harness_codex_failed_turn_without_result_json_stays_failed() {
    let dir = tempfile::tempdir().unwrap();
    let config = fake_codex(
        dir.path(),
        &[
            THREAD_STARTED,
            REPLY,
            r#"{"type":"turn.failed","error":{"message":"boom"}}"#,
        ],
    );
    let sink = Sink::default();
    let outcome = run(config, cos_req(dir.path(), chat(), None), &sink).await;
    assert!(
        matches!(outcome.terminal, Terminal::Error { .. }),
        "{:?}",
        outcome.terminal
    );
}
