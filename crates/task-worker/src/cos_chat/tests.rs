use std::path::{Path, PathBuf};
use std::time::Duration;

use task_core::{ArtifactRef, Budget, DelegateTask};

use super::{
    Continuation, HarnessCapabilities, ImageDelivery, MissingCapability, capability_reason,
    image_delivery,
};
use crate::adapter::{EventSink, RunLimits, Terminal, WorkerAdapter};
use crate::fake::FakeAdapter;
use crate::protocol::tests::sample_task;
use crate::protocol::{
    COS_RUN_CREDENTIAL_ENV, ConversationAddressee, CosChatAttachment, CosChatContext,
    CosChatDelivery, CosChatHistory, CosChatHistoryMessage, CosChatInput, PROTOCOL_VERSION,
    RunContext, RunRequest, is_cos_chat_run,
};

const SECRET: &str = "cosrun_s3cr3t-value-never-logged";

struct NullSink;

impl EventSink for NullSink {
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &ArtifactRef) {}
    fn delegate(&self, _tasks: &[DelegateTask]) {}
}

fn attachment(id: &str, name: &str, media_type: &str) -> CosChatAttachment {
    CosChatAttachment {
        id: id.into(),
        name: name.into(),
        media_type: media_type.into(),
        size_bytes: 123,
        sha256: "ab".repeat(32),
        path: PathBuf::from(format!("/ws/attachments/{id}/{name}")),
        delivery: CosChatDelivery::for_media_type(media_type),
    }
}

fn chat() -> CosChatContext {
    CosChatContext {
        thread_id: "thr1".into(),
        run_id: "crun1".into(),
        inputs: vec![
            CosChatInput {
                id: "m10".into(),
                seq: 10,
                text: "止めてこれを先に".into(),
                interrupt: true,
                attachment_ids: vec!["a1".into()],
            },
            CosChatInput {
                id: "m11".into(),
                seq: 11,
                text: "次はこれ".into(),
                interrupt: false,
                attachment_ids: vec!["a2".into()],
            },
        ],
        summary: Some("決定: X を採用".into()),
        summary_through_seq: 2,
        unsummarized: CosChatHistory {
            from_seq: 3,
            through_seq: 9,
            messages: vec![
                CosChatHistoryMessage {
                    id: "m6".into(),
                    seq: 6,
                    role: "user".into(),
                    text: "六".into(),
                },
                CosChatHistoryMessage {
                    id: "m8".into(),
                    seq: 8,
                    role: "assistant".into(),
                    text: "八".into(),
                },
            ],
        },
        attachments: vec![
            attachment("a1", "screen.png", "image/png"),
            attachment("a2", "spec.pdf", "application/pdf"),
        ],
        harness_capabilities: None,
        skills: vec!["cos-operator".into(), "cos-inbox-triage".into()],
        credential_env: COS_RUN_CREDENTIAL_ENV.into(),
        api_base_url: "http://127.0.0.1:7070/api/v1".into(),
    }
}

fn request(workspace: &Path, cos_chat: Option<CosChatContext>) -> RunRequest {
    let task = match &cos_chat {
        Some(c) => c.transient_task(
            workspace,
            Budget {
                max_turns: 70,
                max_wall_secs: 900,
                max_retries: 0,
            },
            time::OffsetDateTime::now_utc(),
        ),
        None => sample_task(),
    };
    RunRequest {
        protocol: PROTOCOL_VERSION,
        task,
        workspace: workspace.to_path_buf(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: RunContext {
            cos_chat,
            ..RunContext::default()
        },
        cargo_target_dir: None,
    }
}

fn prompt(req: &RunRequest) -> String {
    crate::claude_code::build_prompt(&req.task, &req.context, "wrun1", "artifacts")
}

#[test]
fn cos_chat_harness_caps_native_image_route() {
    for adapter in ["claude-code", "codex"] {
        let caps = HarnessCapabilities::for_adapter(adapter).unwrap();
        assert_eq!(
            image_delivery(CosChatDelivery::Image, Some(&caps)),
            ImageDelivery::Native
        );
        assert!(caps.shell && caps.filesystem && caps.mcp);
        let mut c = chat();
        c.harness_capabilities = Some(caps);
        let p = prompt(&request(Path::new("/tmp/ws"), Some(c)));
        assert!(p.contains("delivery=image path=`/ws/attachments/a1/screen.png` actual=native"));
    }
}

#[test]
fn cos_chat_harness_caps_continuation_and_tool_table() {
    let claude = HarnessCapabilities::for_adapter("claude-code").unwrap();
    let codex = HarnessCapabilities::for_adapter("codex").unwrap();
    let acp = HarnessCapabilities::for_adapter("acp").unwrap();
    assert_eq!(claude.continuation, Continuation::ClaudeSessionResume);
    assert_eq!(codex.continuation, Continuation::CodexExecResume);
    assert_eq!(acp.continuation, Continuation::AcpSessionLoad);
    assert!(claude.shell && claude.filesystem && claude.mcp);
    assert!(codex.shell && codex.filesystem && codex.mcp);
    assert!(!acp.shell && !acp.filesystem && !acp.mcp);
    assert!(!acp.native_image_input && !acp.image_read_tool);
}

#[test]
fn cos_chat_harness_caps_path_and_tool_route() {
    let mut caps = HarnessCapabilities::for_adapter("acp").unwrap();
    caps.image_read_tool = true; // negotiated by an ACP adapter
    assert_eq!(
        image_delivery(CosChatDelivery::Image, Some(&caps)),
        ImageDelivery::PathAndTool
    );
    let mut c = chat();
    c.harness_capabilities = Some(caps);
    let p = prompt(&request(Path::new("/tmp/ws"), Some(c)));
    assert!(p.contains("delivery=image path=`/ws/attachments/a1/screen.png` actual=path+tool"));
    assert!(p.contains("use the confirmed image-reading tool"));
}

#[test]
fn cos_chat_harness_caps_unsupported_route_and_reason() {
    let caps = HarnessCapabilities::for_adapter("acp").unwrap();
    assert_eq!(
        image_delivery(CosChatDelivery::Image, Some(&caps)),
        ImageDelivery::Unsupported
    );
    assert_eq!(
        image_delivery(CosChatDelivery::Image, None),
        ImageDelivery::Unsupported
    );
    assert_eq!(
        image_delivery(CosChatDelivery::File, None),
        ImageDelivery::FilePath
    );
    assert!(HarnessCapabilities::for_adapter("unknown").is_none());
    let p = prompt(&request(Path::new("/tmp/ws"), Some(chat())));
    assert!(p.contains("delivery=image path=`/ws/attachments/a1/screen.png` actual=unsupported"));
    assert!(p.contains(capability_reason(MissingCapability::Image)));
    assert!(capability_reason(MissingCapability::Shell).contains("no command was run"));
    assert!(capability_reason(MissingCapability::Mcp).contains("no MCP operation was performed"));
}

#[test]
fn cos_chat_run_proto_absent_context_keeps_request_json_and_prompt() {
    let ws = Path::new("/tmp/ws");
    let mut req = request(ws, None);
    let before = serde_json::to_string_pretty(&req).unwrap_or_default();
    let before_prompt = prompt(&req);
    assert!(!before.contains("cos_chat"));
    assert!(!is_cos_chat_run(&req));
    // Setting and clearing the field is byte-identical: `None` is never serialized.
    req.context.cos_chat = Some(chat());
    assert!(is_cos_chat_run(&req));
    req.context.cos_chat = None;
    assert_eq!(
        serde_json::to_string_pretty(&req).unwrap_or_default(),
        before
    );
    assert_eq!(prompt(&req), before_prompt);
}

#[test]
fn cos_chat_run_proto_is_decided_by_context_not_by_task_values() {
    let ws = Path::new("/tmp/ws");
    let chat_req = request(ws, Some(chat()));
    // A stored task copying the transient task's title/objective is still not a CoS chat run.
    let mut imitation = request(ws, None);
    imitation.task.title = chat_req.task.title.clone();
    imitation.task.objective = chat_req.task.objective.clone();
    assert!(is_cos_chat_run(&chat_req));
    assert!(!is_cos_chat_run(&imitation));
    assert!(!prompt(&imitation).contains("# CoS chat"));
    // The request round-trips through JSON (worker side reads the same value).
    let json = serde_json::to_string(&chat_req).unwrap_or_default();
    let back: RunRequest = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back.context.cos_chat, chat_req.context.cos_chat);
}

#[test]
fn cos_chat_run_proto_manifest_distinguishes_image_and_file_delivery() {
    for mt in ["image/png", "image/jpeg", "IMAGE/WEBP", "image/gif"] {
        assert_eq!(
            CosChatDelivery::for_media_type(mt),
            CosChatDelivery::Image,
            "{mt}"
        );
    }
    for mt in [
        "image/svg+xml",
        "application/pdf",
        "text/html",
        "application/octet-stream",
        "",
    ] {
        assert_eq!(
            CosChatDelivery::for_media_type(mt),
            CosChatDelivery::File,
            "{mt}"
        );
    }
    let req = request(Path::new("/tmp/ws"), Some(chat()));
    let json = serde_json::to_value(&req).unwrap_or_default();
    let manifest = &json["context"]["cos_chat"]["attachments"];
    assert_eq!(manifest[0]["delivery"], "image");
    assert_eq!(manifest[1]["delivery"], "file");
    for key in [
        "id",
        "name",
        "media_type",
        "size_bytes",
        "sha256",
        "path",
        "delivery",
    ] {
        assert!(manifest[0].get(key).is_some(), "manifest lacks {key}");
    }
    let p = prompt(&req);
    assert!(
        p.contains("delivery=image path=`/ws/attachments/a1/screen.png`"),
        "{p}"
    );
    assert!(
        p.contains("delivery=file path=`/ws/attachments/a2/spec.pdf`"),
        "{p}"
    );
    assert!(p.contains("読めたと答えず"));
}

/// ADR cos-chat-home D4: with attachments, the CoS prompt explains how to pin them to a task or a
/// KB inbox candidate through the references API (wrapped in `/cos/operations`).
#[test]
fn cos_chat_attach_handoff_cos_preamble_explains_pin() {
    let p = prompt(&request(Path::new("/tmp/ws"), Some(chat())));
    assert!(p.contains("/chat/attachments/<添付 id>/references"), "{p}");
    assert!(p.contains("/cos/operations"), "{p}");
    assert!(p.contains("owner_kind は `task`"), "{p}");
    assert!(p.contains("`knowledge_inbox`"), "{p}");
    assert!(p.contains("idempotency_key"), "{p}");
    assert!(p.contains("path を書かない"), "{p}");
    assert!(p.contains("初めて人に「引き渡し済み」と言う"), "{p}");
    assert!(p.contains("celerisctl knowledge record --json"), "{p}");
    // Without attachments there is nothing to pin.
    let mut bare = chat();
    bare.attachments.clear();
    let p = prompt(&request(Path::new("/tmp/ws"), Some(bare)));
    assert!(!p.contains("/references"), "{p}");
}

#[test]
fn cos_chat_run_proto_unsummarized_range_is_explicit() {
    let c = chat();
    assert_eq!(
        c.unsummarized.missing_ranges(),
        vec![(3, 5), (7, 7), (9, 9)]
    );
    let full = CosChatHistory {
        from_seq: 1,
        through_seq: 2,
        messages: vec![
            CosChatHistoryMessage {
                seq: 1,
                ..Default::default()
            },
            CosChatHistoryMessage {
                seq: 2,
                ..Default::default()
            },
        ],
    };
    assert!(full.missing_ranges().is_empty());
    let empty = CosChatHistory {
        from_seq: 5,
        through_seq: 4,
        messages: Vec::new(),
    };
    assert!(empty.missing_ranges().is_empty());

    let p = prompt(&request(Path::new("/tmp/ws"), Some(c)));
    assert!(p.contains("## これまでの要約 (summary through seq 2)\n決定: X を採用"));
    assert!(p.contains("## 要約未作成の範囲 (seq 3..=9)"), "{p}");
    assert!(p.contains("- seq 6 [user] m6: 六"));
    assert!(
        p.contains("このプロンプトに載せていない範囲: seq 3..=5, seq 7, seq 9。"),
        "{p}"
    );
}

#[test]
fn cos_chat_run_proto_prompt_puts_interrupt_first_and_forbids_actions() {
    let mut c = chat();
    let mut req = request(Path::new("/tmp/ws"), Some(c.clone()));
    // A legacy CoS conversation marker must not bring back the `actions` instructions.
    req.context.conversation_addressee = Some(ConversationAddressee::Secretary);
    let p = prompt(&req);
    assert!(p.starts_with("# CoS chat: thread thr1\n"));
    let interrupt = p
        .find("### seq 10 (message m10) 【割り込み】")
        .unwrap_or(usize::MAX);
    let queued = p.find("### seq 11 (message m11)").unwrap_or(0);
    assert!(interrupt < queued, "{p}");
    assert!(!p.contains("declaring actions"), "{p}");
    assert!(p.contains("`actions`（旧 CoS の宣言）は**使わない**"));
    assert!(p.contains("http://127.0.0.1:7070/api/v1/cos/threads/thr1/checkpoint"));
    assert!(p.contains("\"run_id\":\"crun1\",\"summary\":\"…\",\"through_seq\":11,\"expected_summary_through_seq\":2"), "{p}");
    assert!(p.contains("/api/v1/chat/threads/thr1/messages?before_seq=<seq>&limit=50"));
    assert!(p.contains("`cos-operator`、`cos-inbox-triage`"));
    // No summary yet: said so explicitly.
    c.summary = None;
    c.summary_through_seq = 0;
    let p = prompt(&request(Path::new("/tmp/ws"), Some(c)));
    assert!(p.contains("要約はまだ無い（summary_through_seq = 0）"));
}

#[tokio::test]
async fn cos_chat_run_proto_credential_value_stays_out_of_request_json_and_prompt() {
    let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let ws = tmp.path().to_path_buf();
    let req = request(&ws, Some(chat()));
    // The fake harness proves the credential reaches the process only through its env var.
    let mut adapter = FakeAdapter::new(vec![
        "sh".into(),
        "-c".into(),
        format!(
            "cat >/dev/null; if [ \"${COS_RUN_CREDENTIAL_ENV}\" = '{SECRET}' ]; then \
             echo '{{\"type\":\"done\",\"summary\":\"env ok\",\"evidence\":[]}}'; else \
             echo '{{\"type\":\"error\",\"msg\":\"no credential env\",\"retryable\":false}}'; fi"
        ),
    ]);
    adapter.set_env(vec![(COS_RUN_CREDENTIAL_ENV.into(), SECRET.into())]);
    let limits = RunLimits {
        wall_clock: Duration::from_secs(600),
        idle_timeout: Duration::from_secs(600),
        kill_grace: Duration::from_secs(5),
    };
    let outcome = adapter
        .run(req.clone(), "wrun1", limits, &NullSink)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "env ok"),
        other => panic!("unexpected terminal: {other:?}"),
    }
    let run_dir = ws.join("runs").join("wrun1");
    for name in ["request.json", "stdout.jsonl", "stderr.log"] {
        let text = std::fs::read_to_string(run_dir.join(name)).unwrap_or_default();
        assert!(!text.contains(SECRET), "{name} leaks the credential");
    }
    let request_json =
        std::fs::read_to_string(run_dir.join("request.json")).unwrap_or_else(|e| panic!("{e}"));
    assert!(request_json.contains(&format!("\"credential_env\": \"{COS_RUN_CREDENTIAL_ENV}\"")));
    let p = prompt(&req);
    assert!(!p.contains(SECRET));
    assert!(p.contains(&format!("`${COS_RUN_CREDENTIAL_ENV}`")));
}
