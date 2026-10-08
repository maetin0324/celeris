use super::*;
use crate::adapter::NullSink;
use crate::protocol::{PROTOCOL_VERSION, RunContext};
use std::path::Path;
use std::time::Duration;

fn config(dir: &Path, script: &str) -> PiConfig {
    let command = dir.join("pi-stub");
    crate::test_support::write_executable(
        &command,
        &format!("#!/bin/sh\ncat > prompt.copy\n{script}\n"),
    );
    let extension = dir.join("hashline.ts");
    std::fs::write(&extension, "// stub extension path").unwrap();
    PiConfig {
        command: command.to_string_lossy().into(),
        model: Some("opencode-go/deepseek-v4-pro".into()),
        extensions: vec![extension],
        tools: [
            "hashline_read",
            "hashline_edit",
            "bash",
            "grep",
            "find",
            "ls",
        ]
        .map(String::from)
        .into(),
        ..Default::default()
    }
}

fn request(dir: &Path) -> RunRequest {
    RunRequest {
        protocol: PROTOCOL_VERSION,
        task: crate::protocol::tests::sample_task(),
        workspace: dir.to_path_buf(),
        artifacts_dir: dir.join("artifacts"),
        work_dir: None,
        context: RunContext::default(),
        cargo_target_dir: None,
    }
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(60),
        idle_timeout: Duration::from_secs(60),
        kill_grace: Duration::from_millis(200),
    }
}

const SUCCESS: &str = r#"
mkdir -p artifacts
printf '%s' '{"summary":"ok","evidence":[]}' > artifacts/result.json
printf '%s\n' '{"type":"agent_end","messages":[]}'
"#;

#[tokio::test]
async fn pi_adapter_args_tools_isolation_cwd_and_large_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("repo");
    std::fs::create_dir(&work).unwrap();
    let script = format!(
        r#"
for arg in "$@"; do printf '%s\n' "$arg" >> args.copy; done
printf '%s' "$PI_CODING_AGENT_DIR" > agent-dir.copy
pwd > cwd.copy
mkdir -p "$TEST_ARTIFACTS"
printf '%s' '{{"summary":"ok"}}' > "$TEST_ARTIFACTS/result.json"
printf '%s\n' '{{"type":"agent_end"}}'
{SUCCESS}
"#
    );
    let mut cfg = config(dir.path(), &script);
    cfg.env.push((
        "TEST_ARTIFACTS".into(),
        dir.path().join("artifacts").to_string_lossy().into(),
    ));
    cfg.env.push((
        "PI_CODING_AGENT_DIR".into(),
        "/must-not-use-host-agent-dir".into(),
    ));
    let mut req = request(dir.path());
    req.work_dir = Some(work.clone());
    req.task.objective = "long prompt ".repeat(20000);
    let result = PiAdapter::new(cfg.clone())
        .run(req, "args", limits(), &NullSink)
        .await
        .unwrap();
    assert!(matches!(result.terminal, Terminal::Done { .. }));
    let args = std::fs::read_to_string(work.join("args.copy")).unwrap();
    for option in [
        "--mode\njson\n-p\n",
        "--no-extensions\n",
        "--no-skills\n",
        "--no-prompt-templates\n",
        "--session-dir\n",
        "--provider\nopencode-go\n",
        "--model\nopencode-go/deepseek-v4-pro\n",
    ] {
        assert!(args.contains(option), "{option}: {args}");
    }
    assert!(args.contains("--tools\nhashline_read,hashline_edit,bash,grep,find,ls\n"));
    assert!(args.contains(&format!("-e\n{}", cfg.extensions[0].display())));
    assert!(
        !args.contains("subagent") && !args.contains("planner") && !args.contains("--no-session")
    );
    assert!(!args.contains("--no-context-files"));
    assert_eq!(
        std::fs::read_to_string(work.join("prompt.copy")).unwrap(),
        std::fs::read_to_string(dir.path().join("runs/args/prompt.txt")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(work.join("agent-dir.copy")).unwrap(),
        dir.path().join("runs/args/pi-agent").to_string_lossy()
    );
    assert_eq!(
        std::fs::read_to_string(work.join("cwd.copy"))
            .unwrap()
            .trim(),
        work.to_string_lossy()
    );
}

#[tokio::test]
async fn pi_adapter_usage_counts_only_final_message_events() {
    let dir = tempfile::tempdir().unwrap();
    let message = r#"{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop","usage":{"input":100,"output":20,"cacheRead":40,"cacheWrite":5,"cost":{"total":0.25}}}"#;
    let script = format!(
        "printf '%s\\n' '{{\"type\":\"message_update\",\"message\":{message}}}' '{{\"type\":\"message_end\",\"message\":{message}}}' '{{\"type\":\"turn_end\",\"message\":{message}}}' '{{\"type\":\"message_end\",\"message\":{message}}}' '{{\"type\":\"agent_end\",\"messages\":[{message}]}}'\nmkdir -p artifacts\nprintf '%s' '{{\"summary\":\"ok\"}}' > artifacts/result.json"
    );
    let outcome = PiAdapter::new(config(dir.path(), &script))
        .run(request(dir.path()), "usage", limits(), &NullSink)
        .await
        .unwrap();
    let Terminal::Done {
        usage: Some(usage), ..
    } = outcome.terminal
    else {
        panic!("{:?}", outcome.terminal)
    };
    assert_eq!(usage.input_tokens, Some(200));
    assert_eq!(usage.output_tokens, Some(40));
    assert_eq!(usage.cache_read_tokens, Some(80));
    assert_eq!(usage.cache_creation_tokens, Some(10));
    assert_eq!(usage.cost_usd, Some(0.5));
}

#[tokio::test]
async fn pi_adapter_missing_extension_path_fails_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), "touch spawned");
    cfg.extensions = vec![dir.path().join("absent")];
    let error = PiAdapter::new(cfg)
        .run(request(dir.path()), "missing", limits(), &NullSink)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("extension does not exist"));
    assert!(!dir.path().join("prompt.copy").exists());
}

#[test]
fn pi_adapter_requires_hashline_tools_and_rejects_subagent_planner_or_cli_overrides() {
    let dir = tempfile::tempdir().unwrap();
    for tool in ["subagent", "planner", "bash,subagent"] {
        let mut cfg = config(dir.path(), SUCCESS);
        cfg.tools.push(tool.into());
        assert!(cfg.validate().is_err());
    }
    let mut cfg = config(dir.path(), SUCCESS);
    cfg.tools.clear();
    assert!(cfg.validate().is_err());
    cfg = config(dir.path(), SUCCESS);
    cfg.extensions.clear();
    assert!(cfg.validate().is_err());
    cfg = config(dir.path(), SUCCESS);
    cfg.args = vec!["--tools".into(), "subagent".into()];
    assert!(cfg.validate().is_err());
}

#[tokio::test]
async fn pi_adapter_failure_classification_and_json_error_on_zero_exit() {
    for (message, kind) in [
        ("GoUsageLimitError: HTTP 429 usage limit", "throttled"),
        ("HTTP 429 Too Many Requests", "throttled"),
        ("HTTP 401 Unauthorized", "auth"),
        ("quota exhausted", "exhausted"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = format!(
            "{SUCCESS}\nprintf '%s\\n' '{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\"stopReason\":\"error\",\"errorMessage\":\"{message}\"}}}}' '{{\"type\":\"agent_end\"}}'"
        );
        let error = PiAdapter::new(config(dir.path(), &script))
            .run(request(dir.path()), "error", limits(), &NullSink)
            .await
            .unwrap_err();
        assert!(
            matches!(
                (&error, kind),
                (AdapterError::Throttled { .. }, "throttled")
                    | (AdapterError::AuthFailed(_), "auth")
                    | (AdapterError::Exhausted(_), "exhausted")
            ),
            "{error}"
        );
    }
}

#[tokio::test]
async fn pi_adapter_stderr_failure_classification() {
    let dir = tempfile::tempdir().unwrap();
    let error = PiAdapter::new(config(
        dir.path(),
        "echo 'HTTP 401 Unauthorized' >&2; exit 1",
    ))
    .run(request(dir.path()), "stderr", limits(), &NullSink)
    .await
    .unwrap_err();
    assert!(matches!(error, AdapterError::AuthFailed(_)));
}

#[tokio::test]
async fn pi_adapter_rejects_missing_terminal_nonzero_exit_and_stale_result() {
    for script in [
        "exit 0",
        "printf '%s\\n' '{\"type\":\"agent_end\"}'; exit 7",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path());
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        std::fs::write(req.artifact_path("result.json"), r#"{"summary":"stale"}"#).unwrap();
        let outcome = PiAdapter::new(config(dir.path(), script))
            .run(req, "bad", limits(), &NullSink)
            .await
            .unwrap();
        assert!(matches!(
            outcome.terminal,
            Terminal::Error {
                retryable: true,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn pi_adapter_custom_models_and_context_header_use_env_references() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        dir.path(),
        &format!(
            "cp \"$PI_CODING_AGENT_DIR/models.json\" models.copy\nprintf '%s' \"$CELERIS_PI_ROUTING_CONTEXT\" > context.copy\n{SUCCESS}"
        ),
    );
    cfg.model = Some("local/celeris/cheap".into());
    cfg.base_url = Some("http://127.0.0.1:18100/v1".into());
    cfg.env
        .push(("CELERIS_PI_API_KEY".into(), "test-secret".into()));
    let mut req = request(dir.path());
    req.context.routing_context_ref = Some("opaque-ref".into());
    PiAdapter::new(cfg)
        .run(req, "models", limits(), &NullSink)
        .await
        .unwrap();
    let body = std::fs::read_to_string(dir.path().join("models.copy")).unwrap();
    let models: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        models["providers"]["local"]["models"][0]["id"],
        "celeris/cheap"
    );
    assert_eq!(models["providers"]["local"]["apiKey"], "CELERIS_PI_API_KEY");
    assert_eq!(
        models["providers"]["local"]["headers"]["x-celeris-routing-context"],
        "CELERIS_PI_ROUTING_CONTEXT"
    );
    assert!(!body.contains("test-secret") && !body.contains("opaque-ref"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("context.copy")).unwrap(),
        "opaque-ref"
    );
}

#[tokio::test]
async fn pi_adapter_result_question_yield_and_invalid_json() {
    for (body, expected) in [
        (r#"{"question":"which?","summary":"ignored"}"#, "question"),
        (r#"{"yield":{"next_action":"continue"}}"#, "yield"),
        ("not json", "error"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let script = format!(
            "mkdir -p artifacts\nprintf '%s' '{body}' > artifacts/result.json\nprintf '%s\\n' '{{\"type\":\"agent_end\"}}'"
        );
        let outcome = PiAdapter::new(config(dir.path(), &script))
            .run(request(dir.path()), "result", limits(), &NullSink)
            .await
            .unwrap();
        assert!(matches!(
            (outcome.terminal, expected),
            (Terminal::Question { .. }, "question")
                | (Terminal::Yielded { .. }, "yield")
                | (Terminal::Error { .. }, "error")
        ));
    }
}

#[tokio::test]
async fn pi_adapter_selected_go_account_key_overrides_env_without_copying_auth() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join("account");
    std::fs::create_dir_all(account.join("opencode")).unwrap();
    std::fs::write(
        account.join("opencode/auth.json"),
        r#"{"opencode-go":{"type":"api","key":"selected-go-key"}}"#,
    )
    .unwrap();
    let mut cfg = config(
        dir.path(),
        &format!("printf '%s' \"$OPENCODE_API_KEY\" > key.copy\n{SUCCESS}"),
    );
    cfg.env
        .push(("OPENCODE_API_KEY".into(), "wrong-key".into()));
    cfg.env
        .push(("XDG_DATA_HOME".into(), account.to_string_lossy().into()));
    PiAdapter::new(cfg)
        .run(request(dir.path()), "pool", limits(), &NullSink)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("key.copy")).unwrap(),
        "selected-go-key"
    );
    assert!(!dir.path().join("runs/pool/pi-agent/auth.json").exists());
}

#[tokio::test]
async fn pi_adapter_selected_go_account_invalid_key_fails_before_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), SUCCESS);
    cfg.env.push((
        "XDG_DATA_HOME".into(),
        dir.path().join("missing-account").to_string_lossy().into(),
    ));
    let error = PiAdapter::new(cfg)
        .run(request(dir.path()), "auth", limits(), &NullSink)
        .await
        .unwrap_err();
    assert!(matches!(error, AdapterError::AuthFailed(_)));
    assert!(!dir.path().join("prompt.copy").exists());
}

#[tokio::test]
async fn pi_adapter_skills_delivered_with_resources_and_catalog_in_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join("mounted-skill");
    std::fs::create_dir(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "# Sample skill\nRead resource.txt").unwrap();
    std::fs::write(skill.join("resource.txt"), "skill resource").unwrap();
    let mut req = request(dir.path());
    req.context.skills.push(crate::protocol::SkillMount {
        name: "sample-skill".into(),
        path: skill.to_string_lossy().into(),
        description: "Sample instructions".into(),
    });
    PiAdapter::new(config(dir.path(), SUCCESS))
        .run(req, "skills", limits(), &NullSink)
        .await
        .unwrap();
    let copied = dir.path().join(".agents/skills/sample-skill");
    assert_eq!(
        std::fs::read_to_string(copied.join("resource.txt")).unwrap(),
        "skill resource"
    );
    let prompt = std::fs::read_to_string(dir.path().join("prompt.copy")).unwrap();
    assert!(prompt.contains("sample-skill") && prompt.contains("Sample instructions"));
    assert!(!prompt.contains("Read resource.txt"));
}

#[tokio::test]
async fn pi_adapter_wall_budget_and_idle_timeout_kill_process() {
    for wall in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path(), "sleep 30");
        let mut limit = limits();
        if wall {
            limit.wall_clock = Duration::from_millis(50);
        } else {
            limit.idle_timeout = Duration::from_millis(50);
        }
        let outcome = PiAdapter::new(cfg)
            .run(request(dir.path()), "timeout", limit, &NullSink)
            .await
            .unwrap();
        assert!(matches!(
            (outcome.terminal, wall),
            (
                Terminal::BudgetExhausted {
                    kind: task_core::BudgetKind::WallClock,
                    ..
                },
                true
            ) | (
                Terminal::Error {
                    retryable: true,
                    ..
                },
                false
            )
        ));
    }
}

#[tokio::test]
async fn pi_adapter_extension_load_error_cannot_be_reported_as_done() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("echo 'Error: Failed to load extension hashline' >&2\n{SUCCESS}");
    let result = PiAdapter::new(config(dir.path(), &script))
        .run(request(dir.path()), "extension", limits(), &NullSink)
        .await
        .unwrap();
    assert!(matches!(
        result.terminal,
        Terminal::Error {
            retryable: false,
            ..
        }
    ));
}

/// live-check 不具合 1: a CoS chat Pi run writes no `result.json`; the last assistant message is
/// the reply. A regular run without it still fails.
#[tokio::test]
async fn cos_chat_harness_pi_done_without_result_json() {
    let script = r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"了解しました"}],"stopReason":"stop"}}' '{"type":"agent_end","messages":[]}'"#;
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path());
    req.context.cos_chat = Some(crate::protocol::CosChatContext {
        thread_id: "thread-1".into(),
        run_id: "run-1".into(),
        ..Default::default()
    });
    let outcome = PiAdapter::new(config(dir.path(), script))
        .run(req, "cos", limits(), &NullSink)
        .await
        .unwrap();
    match outcome.terminal {
        Terminal::Done { summary, .. } => assert_eq!(summary, "了解しました"),
        other => panic!("unexpected terminal {other:?}"),
    }

    let dir = tempfile::tempdir().unwrap();
    let outcome = PiAdapter::new(config(dir.path(), script))
        .run(request(dir.path()), "regular", limits(), &NullSink)
        .await
        .unwrap();
    assert!(
        matches!(outcome.terminal, Terminal::Error { .. }),
        "{:?}",
        outcome.terminal
    );
}

/// ADR 2026-10-08-cos-chat-prompt-cache D6.2: a CoS chat Pi run passes the fixed Core as one
/// `--append-system-prompt` argument (byte-identical across threads and runs), keeps the skill list
/// and the run specific part on stdin, and records both in `prompt.txt`. A regular run gets no flag.
#[tokio::test]
async fn cos_chat_core_pi_append_system_prompt_stdin_and_prompt_txt() {
    let script = r#"for arg in "$@"; do printf '%s\0' "$arg" >> args.nul; done
printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"ok"}],"stopReason":"stop"}}' '{"type":"agent_end","messages":[]}'"#;
    let mut systems = Vec::new();
    for (thread, run, seq) in [
        ("thread-a", "chat-run-a", 3),
        ("thread-b", "chat-run-b", 41),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(dir.path());
        let chat = crate::protocol::CosChatContext {
            thread_id: thread.into(),
            run_id: run.into(),
            inputs: vec![crate::protocol::CosChatInput {
                id: format!("msg-{seq}"),
                seq,
                text: format!("相談 {seq}"),
                ..Default::default()
            }],
            ..Default::default()
        };
        let core = crate::cos_chat::core(&chat);
        req.context.cos_chat = Some(chat);
        PiAdapter::new(config(dir.path(), script))
            .run(req, run, limits(), &NullSink)
            .await
            .unwrap();
        let raw = std::fs::read_to_string(dir.path().join("args.nul")).unwrap();
        let args: Vec<&str> = raw.split('\0').collect();
        let i = args
            .iter()
            .position(|a| *a == "--append-system-prompt")
            .expect("--append-system-prompt present");
        let system = args[i + 1];
        assert_eq!(system, core);
        assert!(!system.contains(thread) && !system.contains(run));
        assert!(!system.contains(&format!("seq {seq}")));
        let stdin = std::fs::read_to_string(dir.path().join("prompt.copy")).unwrap();
        assert!(!stdin.contains("# CoS chat Core"), "{stdin}");
        assert!(stdin.starts_with(&format!("# CoS chat: thread {thread}\n")));
        assert!(stdin.contains(&format!("- `<chat run id>` = `{run}`")));
        assert!(stdin.contains(&format!("### seq {seq} (message msg-{seq})")));
        let recorded =
            std::fs::read_to_string(dir.path().join("runs").join(run).join("prompt.txt")).unwrap();
        assert_eq!(
            recorded,
            crate::cos_chat::prompt_record("--append-system-prompt", &core, &stdin)
        );
        systems.push(system.to_string());
    }
    assert_eq!(systems[0].as_bytes(), systems[1].as_bytes());

    let dir = tempfile::tempdir().unwrap();
    let _ = PiAdapter::new(config(dir.path(), script))
        .run(request(dir.path()), "regular", limits(), &NullSink)
        .await
        .unwrap();
    let raw = std::fs::read_to_string(dir.path().join("args.nul")).unwrap();
    assert!(!raw.split('\0').any(|a| a == "--append-system-prompt"));
}
