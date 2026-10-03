use std::sync::Mutex;
use std::time::Duration;

use task_core::{ArtifactRef, DelegateTask, RateLimitObservation};

use super::*;
use crate::protocol::{PROTOCOL_VERSION, RunContext};

#[derive(Default)]
struct RecordingSink {
    progress: Mutex<Vec<String>>,
    /// ADR-0048 D2（Phase 60a）: 構造化した進行（`msg` と一緒に）。
    structured: Mutex<Vec<(String, task_core::ProgressFields)>>,
    delegated: Mutex<Vec<Vec<DelegateTask>>>,
    rate_limits: Mutex<Vec<RateLimitObservation>>,
    /// ADR-0054 D1（Phase 67）: `session_established` の呼び出し。
    sessions: Mutex<Vec<String>>,
    /// ADR-0054 D1（Phase 67）: `session_resume_failed` の呼び出し（理由）。
    resume_failures: Mutex<Vec<String>>,
}

impl EventSink for RecordingSink {
    fn progress(&self, msg: &str) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(msg.to_string());
    }
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        self.progress(msg);
        self.structured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((msg.to_string(), fields.clone()));
    }
    fn artifact(&self, _artifact: &ArtifactRef) {}
    fn delegate(&self, tasks: &[DelegateTask]) {
        self.delegated
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tasks.to_vec());
    }
    fn rate_limit(&self, obs: RateLimitObservation) {
        self.rate_limits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(obs);
    }
    fn session_established(&self, session_id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_id.to_string());
    }
    fn session_resume_failed(&self, reason: &str) {
        self.resume_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(reason.to_string());
    }
}

fn progress_of(sink: &RecordingSink) -> Vec<String> {
    sink.progress
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// ADR-0048 D2（Phase 60a）: `session/update` の標本（`tests/fixtures/acp-session-update.jsonl`）を
/// `handle_notification` に通す。`tool_call` → `tool_use`、終わった `tool_call_update` → `tool_result`、
/// `agent_message_chunk` → `text`、`agent_thought_chunk` → `thinking`。`plan` は何も出さない。
#[test]
fn session_updates_map_to_structured_progress() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/acp-session-update.jsonl"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let sink = RecordingSink::default();
    let mut chunks = ChunkBuffer::default();
    for line in text.lines() {
        let value: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}"));
        handle_notification(&value, &sink, &mut chunks);
    }
    chunks.flush(&sink);
    let items = sink
        .structured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let kinds: Vec<Option<ProgressKind>> = items.iter().map(|(_, f)| f.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Some(ProgressKind::Thinking),
            Some(ProgressKind::Text),
            Some(ProgressKind::ToolUse),
            Some(ProgressKind::ToolResult),
            Some(ProgressKind::ToolResult),
        ],
        "{items:#?}"
    );
    assert_eq!(
        items[0].1.summary.as_deref(),
        Some("どのファイルから見るか考える")
    );
    // 細切れの本文は 1 件にまとまる。
    assert_eq!(items[1].1.summary.as_deref(), Some("テストを回します。"));
    assert_eq!(items[2].1.tool.as_deref(), Some("Bash"));
    assert_eq!(
        items[2].1.summary.as_deref(),
        Some("cargo test --workspace")
    );
    assert!(items[2].0.starts_with("tool: Bash"), "{}", items[2].0);
    assert_eq!(
        items[3].1.summary.as_deref(),
        Some("test result: ok. 812 passed")
    );
    assert!(!items[3].1.error);
    // `failed` は失敗の印（本文は `rawOutput`）。
    assert!(items[4].1.error, "{:?}", items[4]);
    assert_eq!(items[4].1.summary.as_deref(), Some("no such file"));
}

fn stub_acp(dir: &Path, script: &str) -> AcpConfig {
    let path = dir.join("acp_stub.sh");
    crate::test_support::write_executable(&path, &format!("#!/bin/sh\n{script}\n"));
    AcpConfig {
        command: path.to_string_lossy().into_owned(),
        args: Vec::new(),
        startup_timeout: Duration::from_secs(5),
        ..AcpConfig::default()
    }
}

fn sample_req(workspace: std::path::PathBuf) -> RunRequest {
    RunRequest {
        cargo_target_dir: None,
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        artifacts_dir: workspace.join("artifacts"),
        workspace,
        work_dir: None,
        context: RunContext::default(),
    }
}

fn default_limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    }
}

/// 基本のハンドシェイク（`initialize` → `session/new` → `session/prompt`）に応じる雛形。呼び出し側が
/// prompt を受け取ったあとの反応（3 番目の `read` 以降）を追加する。
const HANDSHAKE: &str = r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r _new
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-1","configOptions":[]}}'
"#;

#[tokio::test]
async fn browser_rpc_errors_are_redacted_at_initialize_and_prompt() {
    for initialize_failure in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let script = if initialize_failure {
            r#"read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"401 Unauthorized rpc-secret-sentinel"}}'
"#.to_string()
        } else {
            format!(
                r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"error":{{"code":-32000,"message":"401 Unauthorized rpc-secret-sentinel"}}}}'
"#
            )
        };
        let adapter = AcpAdapter::new(stub_acp(dir.path(), &script));
        let mut req = sample_req(dir.path().to_path_buf());
        req.context.browser = Some(crate::browser::BrowserContext {
            credential_used: false,
            run: task_core::BrowserRun {
                task_id: req.task.id,
                run_id: "browser-error-test".into(),
                session_id: "isolated-test".into(),
                state: task_core::BrowserRunState::Running,
                live_view_url: None,
                policy: None,
            },
            cli: dir.path().join("celeris-browser.py"),
        });
        let sink = RecordingSink::default();
        let result = adapter
            .run(req, "browser-error-test", default_limits(), &sink)
            .await;
        let error = result.unwrap_err();
        assert!(!error.to_string().contains("rpc-secret-sentinel"));
        assert!(matches!(error, AdapterError::AuthFailed(_)));
        let run = dir.path().join("runs/browser-error-test");
        for entry in std::fs::read_dir(run).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let bytes = std::fs::read(path).unwrap();
                assert!(!String::from_utf8_lossy(&bytes).contains("rpc-secret-sentinel"));
            }
        }
    }
}

#[tokio::test]
async fn browser_run_discards_raw_logs_but_still_parses_completion() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
echo 'raw-browser-secret-sentinel'
echo 'raw-browser-secret-sentinel' >&2
printf '%s' '{{"summary":"safe browser result","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.browser = Some(crate::browser::BrowserContext {
        credential_used: false,
        run: task_core::BrowserRun {
            task_id: req.task.id,
            run_id: "browser-log-test".into(),
            session_id: "isolated-test".into(),
            state: task_core::BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: dir.path().join("celeris-browser.py"),
    });
    let sink = RecordingSink::default();
    let result = adapter
        .run(req, "browser-log-test", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(result.terminal, Terminal::Done { .. }));
    let run = dir.path().join("runs/browser-log-test");
    assert!(!run.join("stdout.jsonl").exists());
    assert!(!run.join("stderr.log").exists());
    for entry in std::fs::read_dir(run).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("raw-browser-secret-sentinel"));
        }
    }
}

#[tokio::test]
async fn happy_path_progress_and_done_from_result_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"sess-1","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"working on it"}}}}}}}}'
printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"sess-1","update":{{"sessionUpdate":"tool_call","title":"Bash","status":"in_progress"}}}}}}'
printf '%s' '{{"summary":"added usage example","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-1", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done {
            summary,
            evidence,
            usage,
        } => {
            assert_eq!(summary, "added usage example");
            assert!(evidence.is_empty());
            assert_eq!(usage, None);
        }
        other => panic!("expected done, got {other:?}"),
    }
    let progress = sink.progress.lock().unwrap();
    assert!(progress.iter().any(|m| m == "working on it"));
    assert!(progress.iter().any(|m| m.starts_with("tool: Bash")));
    assert!(dir.path().join("runs/run-1/stdout.jsonl").is_file());

    let result_json = std::fs::read_to_string(dir.path().join("runs/run-1/result.json")).unwrap();
    match serde_json::from_str::<crate::protocol::WorkerMessage>(result_json.trim()).unwrap() {
        crate::protocol::WorkerMessage::Done { summary, .. } => {
            assert_eq!(summary, "added usage example")
        }
        other => panic!("expected done in result.json, got {other:?}"),
    }
}

/// ADR-0072 D9（Phase E1）: `result.json` の `{"yield": {...}}` が `Terminal::Yielded` になる。
#[tokio::test]
async fn result_yield_becomes_terminal_yielded() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s' '{{"yield":{{"completed":["A"],"next_action":"do B"}}}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-yield", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Yielded { checkpoint, .. } => {
            assert_eq!(checkpoint["next_action"], "do B");
        }
        other => panic!("expected yielded, got {other:?}"),
    }
}

/// 付属ファイル（入れ子を含む）を持つ skill を KB 側に作る。本文には印の文字列を入れる。
fn skills_fixture_kb(kb: &Path, name: &str) -> crate::protocol::SkillMount {
    let skill_dir = kb.join(name);
    std::fs::create_dir_all(skill_dir.join("references/deep")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n\nBODY-MARKER 文章の書き方\n"),
    )
    .unwrap();
    std::fs::write(skill_dir.join("references/a.md"), "ref a\n").unwrap();
    std::fs::write(skill_dir.join("references/deep/b.md"), "ref b\n").unwrap();
    crate::protocol::SkillMount {
        name: name.into(),
        path: skill_dir.display().to_string(),
        description: "d".into(),
    }
}

fn skills_ok_acp(dir: &Path) -> AcpConfig {
    stub_acp(
        dir,
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    )
}

/// ADR-0127 D1/D3（旧 ADR-0056 D3 の試験を更新）: `context.skills` に乗った skill は、前置き（プロンプト
/// 文面）の末尾に `## Skills（celeris）` 節として名前・説明・パスの一覧で載る（本文と `### <name>` の
/// 見出しは埋め込まない）。
#[tokio::test]
async fn mounted_skills_are_listed_in_the_preamble() {
    let dir = tempfile::tempdir().unwrap();
    let kb = tempfile::tempdir().unwrap();
    let mount = skills_fixture_kb(kb.path(), "writing");
    let adapter = AcpAdapter::new(skills_ok_acp(dir.path()));
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.skills = vec![mount];
    let sink = RecordingSink::default();
    adapter
        .run(req, "run-skills", default_limits(), &sink)
        .await
        .unwrap();
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-skills/prompt.txt")).unwrap();
    assert!(prompt.contains("## Skills（celeris）"), "{prompt}");
    assert!(
        prompt.contains("- `writing` — d（`.agents/skills/writing/SKILL.md`）"),
        "{prompt}"
    );
    assert!(!prompt.contains("### writing"), "{prompt}");
    assert!(!prompt.contains("BODY-MARKER"), "{prompt}");
}

/// ADR-0127 D1/D2/D6: acp では skill のディレクトリが付属ファイルまで `.agents/skills/<name>/` に
/// 丸写しされ、unmount（次の run で `skills` 空）で写しが消え前置きの節も無くなる。人が置いた
/// `.agents/skills/<別名>/`・`.claude/skills/<別名>/`・`.agents/` の他のファイルには触れない。
#[tokio::test]
async fn acp_skills_deliver_sibling_files_and_unmount_keeps_human_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agents/skills/human-skill")).unwrap();
    std::fs::write(
        dir.path().join(".agents/skills/human-skill/SKILL.md"),
        "human\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join(".claude/skills/human-claude")).unwrap();
    std::fs::write(
        dir.path().join(".claude/skills/human-claude/SKILL.md"),
        "human claude\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".agents/notes.txt"), "keep\n").unwrap();
    let kb = tempfile::tempdir().unwrap();
    let mount = skills_fixture_kb(kb.path(), "ui-ux-quality-gate");
    let adapter = AcpAdapter::new(skills_ok_acp(dir.path()));

    let mut req = sample_req(dir.path().to_path_buf());
    req.context.skills = vec![mount];
    let sink = RecordingSink::default();
    adapter
        .run(req, "run-skills-1", default_limits(), &sink)
        .await
        .unwrap();
    let copy = dir.path().join(".agents/skills/ui-ux-quality-gate");
    assert!(
        std::fs::read_to_string(copy.join("SKILL.md"))
            .unwrap()
            .contains("BODY-MARKER")
    );
    assert_eq!(
        std::fs::read_to_string(copy.join("references/a.md")).unwrap(),
        "ref a\n"
    );
    assert_eq!(
        std::fs::read_to_string(copy.join("references/deep/b.md")).unwrap(),
        "ref b\n"
    );
    assert!(copy.join(".gitignore").is_file());
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-skills-1/prompt.txt")).unwrap();
    assert!(
        prompt.contains("`.agents/skills/ui-ux-quality-gate/SKILL.md`"),
        "{prompt}"
    );
    assert!(!prompt.contains("BODY-MARKER"), "{prompt}");

    let req = sample_req(dir.path().to_path_buf());
    adapter
        .run(req, "run-skills-2", default_limits(), &sink)
        .await
        .unwrap();
    assert!(!copy.exists());
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-skills-2/prompt.txt")).unwrap();
    assert!(!prompt.contains("## Skills（celeris）"), "{prompt}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".agents/skills/human-skill/SKILL.md")).unwrap(),
        "human\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".claude/skills/human-claude/SKILL.md")).unwrap(),
        "human claude\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".agents/notes.txt")).unwrap(),
        "keep\n"
    );
}

/// ADR-0036 D1/D2: 共有 workspace のタスクは `.taskd/artifacts/<task_id>/result.json` を読む。
#[tokio::test]
async fn a_shared_workspace_task_uses_its_own_artifacts_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
mkdir -p .taskd/artifacts/T1
printf '%s' '{{"summary":"mine","evidence":[]}}' > .taskd/artifacts/T1/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    std::fs::create_dir_all(dir.path().join("artifacts")).unwrap();
    std::fs::write(
        dir.path().join("artifacts/result.json"),
        r#"{"summary":"sibling"}"#,
    )
    .unwrap();
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.artifacts_dir = dir.path().join(".taskd/artifacts/T1");
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-shared", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "mine"),
        other => panic!("expected done, got {other:?}"),
    }
    let prompt = std::fs::read_to_string(dir.path().join("runs/run-shared/prompt.txt")).unwrap();
    assert!(
        prompt.contains(".taskd/artifacts/T1/result.json"),
        "{prompt}"
    );
}

#[tokio::test]
async fn question_in_result_file_blocks_task() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s' '{{"question":"which crate version?"}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-2", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Question { text } => assert_eq!(text, "which crate version?"),
        other => panic!("expected question, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_result_file_after_stop_reason_is_retryable_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"refusal"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-3", default_limits(), &sink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("refusal"), "{message}");
            assert!(message.contains("artifacts/result.json"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

#[tokio::test]
async fn protocol_version_mismatch_is_a_spawn_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"agentCapabilities":{}}}'
sleep 5
"#,
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-4", default_limits(), &sink)
        .await
        .expect_err("expected a spawn failure");
    match err {
        AdapterError::Spawn(e) => assert!(e.to_string().contains("protocol version"), "{e}"),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[tokio::test]
async fn startup_timeout_without_any_response_is_a_spawn_failure() {
    let dir = tempfile::tempdir().unwrap();
    let config = AcpConfig {
        startup_timeout: Duration::from_millis(200),
        ..stub_acp(dir.path(), "read -r _init\nsleep 5\n")
    };
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let start = Instant::now();
    let err = adapter
        .run(req, "run-5", default_limits(), &sink)
        .await
        .expect_err("expected a spawn failure");
    assert!(start.elapsed() < Duration::from_secs(5));
    match err {
        AdapterError::Spawn(e) => assert!(e.to_string().contains("startup timeout"), "{e}"),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

/// ADR-0026 D4: `permission = allow` なら "allow" 系の選択肢を選ぶ。
#[tokio::test]
async fn permission_allow_selects_an_allow_option() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{{"sessionId":"sess-1","options":[{{"optionId":"reject-once","name":"Reject","kind":"reject_once"}},{{"optionId":"allow-once","name":"Allow once","kind":"allow_once"}},{{"optionId":"allow-always","name":"Allow always","kind":"allow_always"}}]}}}}'
read -r permresp
echo "$permresp" > permission_response.json
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-6", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let response = std::fs::read_to_string(dir.path().join("permission_response.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["result"]["outcome"]["optionId"], "allow-always");
}

/// ADR-0026 D4: `permission = deny` なら "reject" 系の選択肢を選ぶ。
#[tokio::test]
async fn permission_deny_selects_a_reject_option() {
    let dir = tempfile::tempdir().unwrap();
    let config = AcpConfig {
        permission: AcpPermission::Deny,
        ..stub_acp(
            dir.path(),
            &format!(
                r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{{"sessionId":"sess-1","options":[{{"optionId":"allow-once","name":"Allow once","kind":"allow_once"}},{{"optionId":"reject-always","name":"Reject always","kind":"reject_always"}}]}}}}'
read -r permresp
echo "$permresp" > permission_response.json
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
            ),
        )
    };
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-7", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let response = std::fs::read_to_string(dir.path().join("permission_response.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(value["result"]["outcome"]["optionId"], "reject-always");
}

/// ADR-0054 D2（Phase 68）: CoS の対話 run（`conversation_addressee = Secretary`）は、設定が
/// `permission = allow` でも道具の許可要求を常に拒否する（fail-closed。ACP には道具単位の
/// 読み取り許可が無いため）。
#[tokio::test]
async fn the_cos_conversation_run_denies_permission_requests_even_when_configured_to_allow() {
    let dir = tempfile::tempdir().unwrap();
    let config = AcpConfig {
        permission: AcpPermission::Allow,
        ..stub_acp(
            dir.path(),
            &format!(
                r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":100,"method":"session/request_permission","params":{{"sessionId":"sess-1","options":[{{"optionId":"reject-once","name":"Reject","kind":"reject_once"}},{{"optionId":"allow-once","name":"Allow once","kind":"allow_once"}},{{"optionId":"allow-always","name":"Allow always","kind":"allow_always"}}]}}}}'
read -r permresp
echo "$permresp" > permission_response.json
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
            ),
        )
    };
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.conversation_addressee = Some(crate::protocol::ConversationAddressee::Secretary);
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-cos", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let response = std::fs::read_to_string(dir.path().join("permission_response.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        value["result"]["outcome"]["optionId"], "reject-once",
        "no reject_always option offered here, so it falls back to reject_once"
    );
}

/// 壁時計の超過で `session/cancel` を送ってから、応答が無ければプロセスグループごと SIGKILL する
/// （ADR-0026 D4）。子プロセスが本当に居なくなることを `/proc/<pid>` の消滅で確認する。
#[tokio::test]
async fn wall_clock_exceeded_cancels_then_kills_the_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid.txt");
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
echo $$ > {pid}
while true; do sleep 0.1; done
"#,
            pid = pid_file.display()
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_millis(500),
        idle_timeout: Duration::from_secs(30),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-8", limits, &sink).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::BudgetExhausted { kind, message, .. } => {
            // ADR-0072 D7/§6 (i)（Phase E1）: acp には turn の上限が無いので wall-clock だけが入口。
            assert_eq!(kind, task_core::BudgetKind::WallClock);
            assert!(message.contains("wall clock exceeded"), "{message}");
        }
        other => panic!("expected budget_exhausted, got {other:?}"),
    }
    // stub は 3 番目の read の直後に自分の pid を書く。読めなかった (=タイムアウト前に到達できなかった)
    // 場合はテストの前提が崩れているのでそこで失敗させる。
    let pid_text = std::fs::read_to_string(&pid_file)
        .expect("stub should have recorded its pid before looping");
    let pid: i32 = pid_text
        .trim()
        .parse()
        .expect("pid.txt should contain a pid");
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "process {pid} should have been killed"
    );
}

#[tokio::test]
async fn idle_timeout_kills_and_reports_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"sess-1","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"start"}}}}}}}}'
while true; do sleep 0.1; done
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let limits = RunLimits {
        wall_clock: Duration::from_secs(30),
        idle_timeout: Duration::from_millis(300),
        kill_grace: Duration::from_millis(200),
    };
    let start = Instant::now();
    let outcome = adapter.run(req, "run-9", limits, &sink).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    match outcome.terminal {
        Terminal::Error { retryable, message } => {
            assert!(retryable);
            assert!(message.contains("idle timeout"), "{message}");
        }
        other => panic!("expected error, got {other:?}"),
    }
}

/// ADR-0016 M8: run の終わりに `artifacts/delegate.json` があれば `sink.delegate` が呼ばれる。
#[tokio::test]
async fn delegate_json_written_by_worker_is_forwarded_to_sink() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s' '{{"summary":"delegated two subtasks","evidence":[]}}' > artifacts/result.json
printf '%s' '{{"tasks":[{{"title":"a","objective":"do a","acceptance":[{{"text":"c","check":{{"type":"human"}}}}]}},{{"title":"b","objective":"do b","acceptance":[{{"text":"c","check":{{"type":"human"}}}}]}}]}}' > artifacts/delegate.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-10", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let delegated = sink.delegated.lock().unwrap();
    assert_eq!(delegated.len(), 1);
    assert_eq!(delegated[0].len(), 2);
}

/// 本文のチャンクはまとめて `progress` にする（実機の opencode は 1 タスクで 259 件・平均 9 文字を出した）。
/// 改行が来たらそこで区切り、ツール呼び出しの前には溜め分を先に出し、最後に残りを出し切る。
#[tokio::test]
async fn agent_message_chunks_are_coalesced_into_few_progress_lines() {
    let sink = RecordingSink::default();
    let mut buffer = ChunkBuffer::default();
    for chunk in ["Cre", "ated ", "artifacts", "/ok.txt"] {
        buffer.push(chunk, ProgressKind::Text, &sink);
    }
    assert!(
        progress_of(&sink).is_empty(),
        "改行も上限も来ていないので、まだ出さない"
    );

    buffer.push(" done\nnext line", ProgressKind::Text, &sink);
    assert_eq!(
        progress_of(&sink),
        vec!["Created artifacts/ok.txt done".to_string()]
    );

    buffer.flush(&sink);
    assert_eq!(
        progress_of(&sink),
        vec![
            "Created artifacts/ok.txt done".to_string(),
            "next line".to_string()
        ]
    );

    // 上限（FLUSH_AT）を超えたら改行が無くても出す。
    let sink2 = RecordingSink::default();
    let mut buffer2 = ChunkBuffer::default();
    for _ in 0..ChunkBuffer::FLUSH_AT {
        buffer2.push("x", ProgressKind::Text, &sink2);
    }
    assert_eq!(progress_of(&sink2).len(), 1);
}

/// `model` を指定すると `session/set_config_option` が送られ、値がそのまま渡る。
#[tokio::test]
async fn provider_kind_acp_sends_proxy_cheap_model_to_stub() {
    let dir = tempfile::tempdir().unwrap();
    let config = AcpConfig {
        model: Some("celeris/cheap".to_string()),
        ..stub_acp(
            dir.path(),
            r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
read -r _new
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-1","configOptions":[{"id":"model","type":"select","currentValue":"a","options":["a","b"]}]}}'
read -r l3
echo "$l3" >> received.log
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{}}'
read -r l4
echo "$l4" >> received.log
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
"#,
        )
    };
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-11", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let received = std::fs::read_to_string(dir.path().join("received.log")).unwrap();
    assert!(received.contains("session/set_config_option"), "{received}");
    assert!(received.contains("celeris/cheap"), "{received}");
    assert!(received.contains("\"configId\":\"model\""), "{received}");
}

/// `model` が空ならば `session/set_config_option` は送らない。
#[tokio::test]
async fn model_option_is_not_set_when_model_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}'
read -r _new
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-1","configOptions":[{"id":"model","type":"select","currentValue":"a","options":["a","b"]}]}}'
read -r l3
echo "$l3" >> received.log
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#,
    );
    assert!(config.model.is_none());
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-12", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let received = std::fs::read_to_string(dir.path().join("received.log")).unwrap();
    assert!(
        !received.contains("session/set_config_option"),
        "{received}"
    );
    assert!(received.contains("session/prompt"), "{received}");
}

/// ADR-0026 D5: `session/prompt` の JSON-RPC エラーを分類し `AdapterError::Throttled` として返す。
#[tokio::test]
async fn prompt_error_classified_as_throttled_surfaces_as_adapter_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"error":{{"code":-32000,"message":"429 rate limit exceeded"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-13", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::Throttled { .. }), "{err:?}");
    assert!(dir.path().join("runs/run-13/result.json").is_file());
}

/// 同じく `AuthFailed` の分類（ADR-0026 D5）。
#[tokio::test]
async fn prompt_error_classified_as_auth_failed_surfaces_as_adapter_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        &format!(
            r#"{HANDSHAKE}
read -r _prompt
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"error":{{"code":-32000,"message":"401 Unauthorized: not logged in"}}}}'
"#
        ),
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let err = adapter
        .run(req, "run-14", default_limits(), &sink)
        .await
        .expect_err("expected a provider failure");
    assert!(matches!(err, AdapterError::AuthFailed(_)), "{err:?}");
    assert!(dir.path().join("runs/run-14/result.json").is_file());
}

/// ADR-0024/0026 と同じ規則: `with_env` の追加分は既存の同名キーより後に環境を組み立てるので勝つ。
#[tokio::test]
async fn with_env_overrides_a_same_name_key_already_in_config_env() {
    let dir = tempfile::tempdir().unwrap();
    let out_file = dir.path().join("env-seen.txt");
    let mut config = stub_acp(
        dir.path(),
        &format!(
            r#"printf '%s' "$ACP_TEST_VAR" > {out}
{HANDSHAKE}
read -r _prompt
printf '%s' '{{"summary":"ok","evidence":[]}}' > artifacts/result.json
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"stopReason":"end_turn"}}}}'
"#,
            out = out_file.display()
        ),
    );
    config
        .env
        .push(("ACP_TEST_VAR".to_string(), "old".to_string()));
    let base = AcpAdapter::new(config);
    let with_env = base
        .with_env(&[("ACP_TEST_VAR".to_string(), "new".to_string())])
        .expect("acp supports with_env");

    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = with_env
        .run(req, "run-15", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let seen = std::fs::read_to_string(&out_file).unwrap();
    assert_eq!(seen, "new");
}

/// ADR-0054 D1（Phase 67）: `context.session` が無ければ Phase 66 までと同じ `session/new`。
#[tokio::test]
async fn without_a_session_the_agent_sees_session_new() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r new_req
printf '%s\n' "$new_req" >> methods.log
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-new","configOptions":[]}}'
read -r _prompt
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#,
    );
    let adapter = AcpAdapter::new(config);
    let req = sample_req(dir.path().to_path_buf());
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-16", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let methods = std::fs::read_to_string(dir.path().join("methods.log")).unwrap();
    assert!(methods.contains("\"method\":\"session/new\""), "{methods}");
    assert!(!methods.contains("session/load"), "{methods}");
    assert!(
        sink.sessions.lock().unwrap().is_empty(),
        "no session tracked for this run, so nothing to report"
    );
}

/// ADR-0054 D1（Phase 67）: 継続セッションの**最初の run**（`resume: false`）は `session/new` の
/// ままだが、agent が割り当てた `sessionId` を `session_established` で報告する。
#[tokio::test]
async fn a_fresh_session_reports_the_agent_assigned_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r new_req
printf '%s\n' "$new_req" >> methods.log
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-fresh","configOptions":[]}}'
read -r _prompt
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#,
    );
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: AcpAdapter::ID.to_string(),
        session_id: "placeholder".to_string(),
        resume: false,
    });
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-17", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let methods = std::fs::read_to_string(dir.path().join("methods.log")).unwrap();
    assert!(methods.contains("\"method\":\"session/new\""), "{methods}");
    assert_eq!(
        sink.sessions.lock().unwrap().as_slice(),
        &["sess-fresh".to_string()]
    );
}

/// ADR-0054 D1（Phase 67）: 継続セッションの**2 回目以降**（`resume: true`）は `session/load`。
#[tokio::test]
async fn a_continuing_session_sends_session_load_and_reports_it_established() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r new_req
printf '%s\n' "$new_req" >> methods.log
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{}}'
read -r _prompt
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#,
    );
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: AcpAdapter::ID.to_string(),
        session_id: "sess-continue".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-18", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let methods = std::fs::read_to_string(dir.path().join("methods.log")).unwrap();
    assert!(methods.contains("\"method\":\"session/load\""), "{methods}");
    assert!(methods.contains("sess-continue"), "{methods}");
    // `session/load` の応答は `sessionId` を含まなくてよい（渡した id をそのまま使う）。
    assert_eq!(
        sink.sessions.lock().unwrap().as_slice(),
        &["sess-continue".to_string()]
    );
    assert!(sink.resume_failures.lock().unwrap().is_empty());
}

/// ADR-0054 D1（Phase 67）: `session/load` が拒否されたら（セッションが無い・失効）、
/// `session_resume_failed` を報告した上で run 自体は失敗する（ディスパッチャが retire して作り直す）。
#[tokio::test]
async fn a_rejected_session_load_reports_resume_failed() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r _new
printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"unknown session"}}'
"#,
    );
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: AcpAdapter::ID.to_string(),
        session_id: "sess-gone".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let result = adapter.run(req, "run-19", default_limits(), &sink).await;
    assert!(result.is_err(), "session/load rejection fails this run");
    let failures = sink.resume_failures.lock().unwrap();
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("unknown session"), "{failures:?}");
    assert!(sink.sessions.lock().unwrap().is_empty());
}

/// `context.session` が別アダプタ向けなら無視する（`session/new` のまま）。
#[tokio::test]
async fn a_session_for_another_adapter_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let config = stub_acp(
        dir.path(),
        r#"
mkdir -p artifacts
read -r _init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{}}}'
read -r new_req
printf '%s\n' "$new_req" >> methods.log
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-1","configOptions":[]}}'
read -r _prompt
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
"#,
    );
    let adapter = AcpAdapter::new(config);
    let mut req = sample_req(dir.path().to_path_buf());
    req.context.session = Some(crate::protocol::SessionHandle {
        adapter: "claude-code".to_string(),
        session_id: "cc-session".to_string(),
        resume: true,
    });
    let sink = RecordingSink::default();
    let outcome = adapter
        .run(req, "run-20", default_limits(), &sink)
        .await
        .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    let methods = std::fs::read_to_string(dir.path().join("methods.log")).unwrap();
    assert!(methods.contains("\"method\":\"session/new\""), "{methods}");
    assert!(sink.sessions.lock().unwrap().is_empty());
}
