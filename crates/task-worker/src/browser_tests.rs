use super::*;
use std::sync::Mutex;
use task_core::{
    ArtifactRef, BrowserAction, BrowserCapability, BrowserDomainMode, BrowserTaskPolicy,
    EffectiveProfile, TaskId,
};

#[derive(Default)]
struct RecordingSink {
    browsers: Mutex<Vec<BrowserRun>>,
    progress: Mutex<Vec<(String, ProgressFields)>>,
    artifacts: Mutex<Vec<ArtifactRef>>,
    comments: Mutex<Vec<String>>,
}
impl EventSink for RecordingSink {
    fn browser_updated(&self, browser: &BrowserRun) {
        self.browsers.lock().unwrap().push(browser.clone());
    }
    fn progress(&self, msg: &str) {
        self.progress_with(msg, &ProgressFields::default());
    }
    fn progress_with(&self, msg: &str, fields: &ProgressFields) {
        self.progress
            .lock()
            .unwrap()
            .push((msg.into(), fields.clone()));
    }
    fn comment(&self, body: &str) {
        self.comments.lock().unwrap().push(body.into());
    }
    fn artifact(&self, artifact: &ArtifactRef) {
        self.artifacts.lock().unwrap().push(artifact.clone());
    }
}

fn request(workspace: &Path) -> RunRequest {
    let mut task = crate::protocol::tests::sample_task();
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    RunRequest {
        protocol: crate::protocol::PROTOCOL_VERSION,
        task,
        workspace: workspace.into(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: crate::protocol::RunContext {
            profile: Some(EffectiveProfile {
                browser: Some(BrowserCapability {
                    allowed_domains: vec!["example.com".into(), "*.example.org".into()],
                    live_view_url: Some("https://browser.example.com/live".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            browser_policy: Some(task_policy(&[
                BrowserAction::Navigate,
                BrowserAction::Snapshot,
                BrowserAction::Extract,
                BrowserAction::Screenshot,
            ])),
            ..Default::default()
        },
        cargo_target_dir: None,
    }
}

fn task_policy(actions: &[BrowserAction]) -> BrowserTaskPolicy {
    BrowserTaskPolicy {
        policy_id: "public-read".into(),
        revision: 1,
        domain_mode: BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec!["example.com".into(), "docs.example.org".into()],
        allowed_actions: actions.to_vec(),
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    }
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(10),
        idle_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(100),
    }
}

#[test]
fn task_and_execution_isolate_sessions_and_prompt_describes_capability() {
    let first = TaskId::new();
    let second = TaskId::new();
    let session = session_id(first, "run1");
    assert_eq!(session, session_id(first, "run1"));
    assert_ne!(session, session_id(first, "run2"));
    assert_ne!(session, session_id(second, "run1"));
    assert!(
        session
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    );
    let prompt = prompt(&BrowserContext {
        cli: PathBuf::from("/workspace/runs/run1/browser/celeris-browser.py"),
        run: BrowserRun {
            task_id: first,
            run_id: "run1".into(),
            session_id: session.clone(),
            state: BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
    });
    for expected in [
        &session,
        "UNTRUSTED DATA",
        "public unauthenticated",
        "snapshot",
        "raw CLI",
    ] {
        assert!(prompt.contains(expected), "missing {expected}");
    }
}

#[test]
fn event_forwarding_discards_untrusted_fields_and_constrains_artifact_paths() {
    let temp = tempfile::tempdir().unwrap();
    let req = request(temp.path());
    let output = req.artifacts_dir.join("browser");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("extract-a.json"), "{\"untrusted\":true}").unwrap();
    let secret = "PASSWORD_TOTP_DO_NOT_FORWARD";
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("private"), secret).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("private"),
        output.join("screenshot-link.png"),
    )
    .unwrap();
    let events = temp.path().join("events.jsonl");
    let lines = [
        serde_json::json!({"operation":secret,"status":"success"}),
        serde_json::json!({"operation":"extract","status":"success","detail":secret}),
        serde_json::json!({"operation":"click","status":secret}),
        serde_json::json!({"operation":"extract","status":"success","artifact":"../../private"}),
        serde_json::json!({"operation":"screenshot","status":"success","artifact":"screenshot-link.png"}),
        serde_json::json!({"operation":"extract","status":"success","artifact":"extract-a.json"}),
    ];
    let mut content = lines
        .iter()
        .map(|line| line.to_string() + "\n")
        .collect::<String>();
    content.push_str("{\"operation\":\"click\""); // torn writes must wait for a complete line
    std::fs::write(&events, content).unwrap();
    let sink = RecordingSink::default();
    let mut offset = 0;
    forward_events(&events, &mut offset, &req, &output, &sink);
    assert_eq!(sink.artifacts.lock().unwrap().len(), 1);
    assert_eq!(sink.artifacts.lock().unwrap()[0].name, "extract-a.json");
    assert!(
        !serde_json::to_string(&*sink.progress.lock().unwrap())
            .unwrap()
            .contains(secret)
    );
    let count = sink.progress.lock().unwrap().len();
    forward_events(&events, &mut offset, &req, &output, &sink);
    assert_eq!(
        sink.progress.lock().unwrap().len(),
        count,
        "events must not be replayed"
    );
}

struct CliHarness {
    id: &'static str,
    question: bool,
}
#[async_trait::async_trait]
impl WorkerAdapter for CliHarness {
    fn id(&self) -> &str {
        self.id
    }
    async fn run(
        &self,
        req: RunRequest,
        _: &str,
        _: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let secret = "UNTRUSTED_HARNESS_SECRET";
        sink.progress_with(
            secret,
            &ProgressFields {
                detail: Some(secret.into()),
                ..Default::default()
            },
        );
        sink.comment(secret);
        sink.artifact(&ArtifactRef {
            name: secret.into(),
            path: "unregistered.json".into(),
            sha256: "fake".into(),
            kind: "json".into(),
            declared: true,
        });
        let browser = req
            .context
            .browser
            .as_ref()
            .expect("capability reached harness");
        assert_eq!(browser.run.task_id, req.task.id);
        for args in [
            vec!["open", "https://example.com"],
            vec!["screenshot"],
            vec!["extract", "@e1"],
        ] {
            let result = tokio::process::Command::new("python3")
                .arg(&browser.cli)
                .args(args)
                .output()
                .await?;
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let terminal = if self.question {
            Terminal::Question {
                text: "Human approval required".into(),
            }
        } else {
            Terminal::Done {
                summary: "public browsing done".into(),
                evidence: vec![],
                usage: None,
            }
        };
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn substrate(workspace: &Path) -> PathBuf {
    let path = workspace.join("fake-agent-browser");
    crate::test_support::write_executable(
        &path,
        r#"#!/usr/bin/env python3
import json, pathlib, sys
if sys.argv[1:] == ['--version']:
    print('agent-browser 0.38.1')
    sys.exit(0)
args = sys.argv[1:]
command = args[args.index('--json') + 1:]
root = pathlib.Path.cwd()
with (root / 'commands.jsonl').open('a') as out:
    out.write(json.dumps(command) + '\n')
if command[0] == 'screenshot':
    pathlib.Path(command[1]).write_bytes(b'public screenshot')
print(json.dumps({'success': True, 'data': {'text': 'public page'}}))
"#,
    );
    path
}

#[tokio::test]
async fn opencode_and_claude_share_supervised_browser_lifecycle_artifacts_and_cleanup() {
    for (id, question) in [("acp", false), ("claude-code", true)] {
        let temp = tempfile::tempdir().unwrap();
        let executable = substrate(temp.path());
        let req = request(temp.path());
        let task_id = req.task.id;
        let sink = RecordingSink::default();
        let outcome = run_with_executable(
            Arc::new(CliHarness { id, question }),
            req,
            "execution1",
            limits(),
            &sink,
            &executable,
        )
        .await
        .unwrap();
        assert_eq!(
            matches!(outcome.terminal, Terminal::Question { .. }),
            question
        );
        let browsers = sink.browsers.lock().unwrap();
        assert_eq!(browsers.len(), 2);
        assert_eq!(browsers[0].state, BrowserRunState::Running);
        assert_eq!(browsers[0].task_id, task_id);
        assert_eq!(browsers[0].run_id, "execution1");
        assert_eq!(browsers[0].session_id, browsers[1].session_id);
        assert_eq!(
            browsers[1].state,
            if question {
                BrowserRunState::WaitingForHuman
            } else {
                BrowserRunState::Completed
            }
        );
        assert_eq!(sink.artifacts.lock().unwrap().len(), 2);
        assert!(sink.comments.lock().unwrap().is_empty());
        assert!(
            !serde_json::to_string(&*sink.progress.lock().unwrap())
                .unwrap()
                .contains("UNTRUSTED_HARNESS_SECRET")
        );

        for artifact in sink.artifacts.lock().unwrap().iter() {
            assert!(temp.path().join(&artifact.path).is_file());
            assert_eq!(artifact.sha256.len(), 64);
        }
        let calls =
            std::fs::read_to_string(temp.path().join("runs/execution1/browser/commands.jsonl"))
                .unwrap();
        assert_eq!(calls.lines().last(), Some("[\"close\"]"));
        assert!(
            sink.progress
                .lock()
                .unwrap()
                .iter()
                .any(|(_, fields)| fields.tool.as_deref() == Some("browser.extract"))
        );
    }
}

#[tokio::test]
async fn unsupported_adapter_and_missing_administrator_grant_fail_before_harness_execution() {
    let temp = tempfile::tempdir().unwrap();
    let sink = RecordingSink::default();
    let absent = temp.path().join("must-not-launch");
    let req = request(temp.path());
    let error = run_with_executable(
        Arc::new(CliHarness {
            id: "codex",
            question: false,
        }),
        req,
        "bad-adapter",
        limits(),
        &sink,
        &absent,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("requires acp"));
    let mut req = request(temp.path());
    req.context.profile = None;
    let error = run_with_executable(
        Arc::new(CliHarness {
            id: "acp",
            question: false,
        }),
        req,
        "no-grant",
        limits(),
        &sink,
        &absent,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("administrator profile grant"));
    assert!(sink.browsers.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cleanup_failure_marks_browser_failed_instead_of_reporting_completion() {
    let temp = tempfile::tempdir().unwrap();
    let executable = substrate(temp.path());
    let script = std::fs::read_to_string(&executable).unwrap().replace(
        "command = args[args.index('--json') + 1:]",
        "command = args[args.index('--json') + 1:]\nif command[0] == 'close':\n    print(json.dumps({'success': False, 'error': 'not closed'}))\n    sys.exit(1)",
    );
    crate::test_support::write_executable(&executable, &script);
    let sink = RecordingSink::default();
    let outcome = run_with_executable(
        Arc::new(CliHarness {
            id: "acp",
            question: false,
        }),
        request(temp.path()),
        "cleanup-failure",
        limits(),
        &sink,
        &executable,
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome.terminal,
        Terminal::Error {
            retryable: false,
            ..
        }
    ));
    assert_eq!(
        sink.browsers.lock().unwrap().last().unwrap().state,
        BrowserRunState::Failed
    );
}

#[test]
fn python_transport_security_regressions_are_part_of_workspace_gate() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let result = std::process::Command::new("python3")
        .args([
            "-m",
            "unittest",
            "discover",
            "-s",
            "scripts/tests",
            "-p",
            "test_browser_cli.py",
            "-v",
        ])
        .current_dir(repository)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[tokio::test]
async fn launch_uses_generated_policy_and_binds_its_hash_to_the_run() {
    let temp = tempfile::tempdir().unwrap();
    let executable = substrate(temp.path());
    let sink = RecordingSink::default();
    run_with_executable(
        Arc::new(CliHarness {
            id: "acp",
            question: false,
        }),
        request(temp.path()),
        "generated",
        limits(),
        &sink,
        &executable,
    )
    .await
    .unwrap();
    let runtime = temp.path().join("runs/generated/browser");
    let policy = std::fs::read(runtime.join("policy.json")).unwrap();
    assert_eq!(
        policy,
        br#"{"default":"deny","allow":["close","gettext","launch","navigate","screenshot","snapshot"]}"#
    );
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(runtime.join("config.json")).unwrap()).unwrap();
    // grant {example.com, *.example.org} ∩ task {example.com, docs.example.org}
    assert_eq!(
        config["allowed_domains"],
        serde_json::json!(["docs.example.org", "example.com"])
    );
    assert_eq!(
        config["policy_sha256"],
        format!("{:x}", Sha256::digest(&policy))
    );
    let browsers = sink.browsers.lock().unwrap();
    let binding = browsers[0].policy.clone().unwrap();
    assert_eq!(binding.policy_id, "public-read");
    assert_eq!(binding.revision, 1);
    assert!(binding.hash.starts_with("sha256:"));
    assert!(browsers.iter().all(|b| b.policy.as_ref() == Some(&binding)));
}

/// Tries what page content might ask for: actions and hosts outside the task policy.
struct HostileHarness;
#[async_trait::async_trait]
impl WorkerAdapter for HostileHarness {
    fn id(&self) -> &str {
        "acp"
    }
    async fn run(
        &self,
        req: RunRequest,
        _: &str,
        _: RunLimits,
        _: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let cli = &req.context.browser.as_ref().expect("browser").cli;
        let mut codes = Vec::new();
        for args in [
            vec!["click", "@e1"],
            vec!["download", "@e1"],
            vec!["open", "https://evil.example/"],
            vec!["open", "https://example.org/"],
            vec!["eval", "document.cookie"],
            vec!["cookies", "get"],
            vec!["state", "save", "/tmp/x"],
            vec!["open", "https://example.com/ok"],
        ] {
            let output = tokio::process::Command::new("python3")
                .arg(cli)
                .args(args)
                .output()
                .await?;
            codes.push(output.status.code().unwrap_or(-1));
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: format!("{codes:?}"),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

#[tokio::test]
async fn actions_and_domains_outside_task_policy_are_stopped_before_the_substrate() {
    let temp = tempfile::tempdir().unwrap();
    let executable = substrate(temp.path());
    let sink = RecordingSink::default();
    let outcome = run_with_executable(
        Arc::new(HostileHarness),
        request(temp.path()),
        "hostile",
        limits(),
        &sink,
        &executable,
    )
    .await
    .unwrap();
    let Terminal::Done { summary, .. } = outcome.terminal else {
        panic!("unexpected terminal");
    };
    assert_eq!(summary, "[2, 2, 2, 2, 2, 2, 2, 0]");
    let calls =
        std::fs::read_to_string(temp.path().join("runs/hostile/browser/commands.jsonl")).unwrap();
    // Only the permitted navigation and the supervisor's cleanup reached agent-browser.
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["[\"open\", \"https://example.com/ok\"]", "[\"close\"]"]
    );
    let blocked = sink
        .progress
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, f)| f.tool.as_deref() == Some("browser.policy_block"))
        .count();
    assert_eq!(blocked, 7);
}

#[tokio::test]
async fn missing_empty_or_widening_task_policy_fails_before_any_process_starts() {
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("must-not-launch");
    let sink = RecordingSink::default();
    let mut credential = task_policy(&[BrowserAction::Snapshot]);
    credential.credential_policy_ids = vec!["not-granted".into()];
    let mut outside = task_policy(&[BrowserAction::Snapshot]);
    outside.network_domains = vec!["evil.example".into()];
    for (policy, code) in [
        (None, "browser_policy_required"),
        (Some(task_policy(&[])), "empty_browser_actions"),
        (Some(outside), "empty_browser_domains"),
        (Some(credential), "credential_policy_not_granted"),
    ] {
        let mut req = request(temp.path());
        req.context.browser_policy = policy;
        let error = run_with_executable(
            Arc::new(CliHarness {
                id: "acp",
                question: false,
            }),
            req,
            "refused",
            limits(),
            &sink,
            &absent,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(code), "{error}");
    }
    assert!(sink.browsers.lock().unwrap().is_empty());
    assert!(!temp.path().join("runs/refused").exists());
}
