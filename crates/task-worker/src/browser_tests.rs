use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use task_core::browser_wait::{BrowserWait, BrowserWaitStore, NewBrowserWait};
use task_core::{
    ArtifactRef, BrowserAction, BrowserCapability, BrowserDomainMode, BrowserTaskPolicy,
    EffectiveProfile, SqliteStore, Status, TaskId, TaskStore,
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
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap();
    configure_isolated_runtime(IsolatedBrowserConfig {
        resolver: Some("127.0.0.1".parse().unwrap()),
        record_dir: std::env::temp_dir()
            .join(format!("celeris-browser-unit-{}", std::process::id())),
        bwrap: "/usr/bin/bwrap".into(),
        sandboxd: bin.join("celeris-browser-sandboxd"),
        egress: bin.join("celeris-browser-egress"),
    });
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
fn production_backend_route_checks_existing_loop_without_claiming_sensitive_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let mut req = request(temp.path());
    let grant = req
        .context
        .profile
        .as_ref()
        .unwrap()
        .browser
        .as_ref()
        .unwrap();
    let public = crate::browser_policy::prepare(
        grant,
        req.context.browser_policy.as_ref(),
        SUPPORTED_VERSION,
    )
    .unwrap();
    let routed = route_existing_backend("acp", &public).unwrap();
    assert_eq!(routed.primary, "acp");
    assert!(routed.fallbacks.contains(&"claude-code".to_string()));

    req.context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .allowed_actions = Some(vec![BrowserAction::Navigate, BrowserAction::CredentialUse]);
    req.context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .credential_policy_ids = vec!["pol-example".into()];
    req.context.browser_policy.as_mut().unwrap().allowed_actions =
        vec![BrowserAction::Navigate, BrowserAction::CredentialUse];
    req.context
        .browser_policy
        .as_mut()
        .unwrap()
        .credential_policy_ids = vec!["pol-example".into()];
    let sensitive = crate::browser_policy::prepare(
        req.context
            .profile
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap(),
        req.context.browser_policy.as_ref(),
        SUPPORTED_VERSION,
    )
    .unwrap();
    for backend in ["acp", "claude-code"] {
        assert!(route_existing_backend(backend, &sensitive).is_err());
    }
    assert!(route_existing_backend("unsupported", &public).is_err());
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
        credential_used: false,
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
        "request-credential",
        "snapshot",
        "raw CLI",
    ] {
        assert!(prompt.contains(expected), "missing {expected}");
    }
}

#[test]
fn credential_segment_keeps_paths_when_policy_changes_to_harness() {
    let temp = tempfile::tempdir().unwrap();
    let policy = temp.path().join("policy.json");
    write_private(&policy, crate::browser_credential::segment_policy()).unwrap();
    let before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&policy).unwrap()).unwrap();
    assert!(
        before["allow"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("auth_login"))
    );
    replace_private(&policy, br#"{"default":"deny","allow":["close","launch"]}"#).unwrap();
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&policy).unwrap()).unwrap();
    assert!(
        !after["allow"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("auth_login"))
    );
    assert_eq!(
        std::fs::metadata(&policy).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let close = segment_close_argv(Path::new("/bin/browser"), temp.path(), "session");
    assert_eq!(close[2], temp.path().join("upstream.json").into_os_string());
    assert_eq!(close[6], policy.into_os_string());
}

#[test]
fn credential_harness_policy_removes_plugin_and_auth_actions() {
    let policy = serde_json::json!({"default":"deny", "allow":[
        "launch", "close", "navigate", "snapshot", "gettext", "screenshot",
        "download", "auth_login", task_core::browser::CREDENTIAL_PLUGIN_ACTION
    ]});
    let after = credential_harness_policy(&serde_json::to_vec(&policy).unwrap()).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&after).unwrap();
    assert_eq!(value["default"], "deny");
    assert_eq!(
        value["allow"],
        serde_json::json!(["launch", "close", "navigate"])
    );
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
    let live =
        crate::browser_live::LiveEmitter::new(crate::browser_live::CollectingSink::default());
    let mut offset = 0;
    forward_events(&events, &mut offset, &req, &output, &sink, &live);
    assert_eq!(sink.artifacts.lock().unwrap().len(), 1);
    assert_eq!(sink.artifacts.lock().unwrap()[0].name, "extract-a.json");
    assert!(
        !serde_json::to_string(&*sink.progress.lock().unwrap())
            .unwrap()
            .contains(secret)
    );
    let count = sink.progress.lock().unwrap().len();
    forward_events(&events, &mut offset, &req, &output, &sink, &live);
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

struct WaitSink {
    store: SqliteStore,
    task_id: TaskId,
    browsers: Mutex<Vec<BrowserRun>>,
}
impl EventSink for WaitSink {
    fn progress(&self, _: &str) {}
    fn artifact(&self, _: &ArtifactRef) {}
    fn browser_updated(&self, browser: &BrowserRun) {
        self.browsers.lock().unwrap().push(browser.clone());
    }
    fn browser_wait_open(&self, request: &NewBrowserWait) -> Result<(), String> {
        self.store
            .browser_wait_open(self.task_id, request, time::OffsetDateTime::now_utc())
            .map(|_| ())
            .map_err(|e| e.code().into())
    }
    fn browser_waits(&self) -> Result<Vec<BrowserWait>, String> {
        self.store
            .browser_waits_for_task(self.task_id)
            .map_err(|e| e.to_string())
    }
}

#[tokio::test]
async fn sensitive_backend_is_refused_before_substrate_harness_and_wait_creation() {
    for id in ["acp", "claude-code"] {
        let temp = tempfile::tempdir().unwrap();
        let mut req = request(temp.path());
        req.task.status = Status::Ready;
        let grant = req
            .context
            .profile
            .as_mut()
            .unwrap()
            .browser
            .as_mut()
            .unwrap();
        grant.allowed_actions = Some(vec![BrowserAction::Navigate, BrowserAction::CredentialUse]);
        grant.credential_policy_ids = vec!["pol-example".into()];
        let policy = req.context.browser_policy.as_mut().unwrap();
        policy.allowed_actions = vec![BrowserAction::Navigate, BrowserAction::CredentialUse];
        policy.credential_policy_ids = vec!["pol-example".into()];
        let task_id = req.task.id;
        let store = SqliteStore::open(&temp.path().join("celeris.db")).unwrap();
        store.insert(&req.task).unwrap();
        let sink = WaitSink {
            store,
            task_id,
            browsers: Mutex::default(),
        };
        let error = run_with_executable(
            Arc::new(CliHarness {
                id,
                question: false,
            }),
            req,
            "denied-run",
            limits(),
            &sink,
            &temp.path().join("must-not-launch"),
            None,
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("browser backend lacks required conformance")
        );
        assert!(
            sink.store
                .browser_waits_for_task(task_id)
                .unwrap()
                .is_empty()
        );
        assert!(sink.browsers.lock().unwrap().is_empty());
        assert!(!temp.path().join("runs").exists());
    }
}

#[test]
fn credential_request_origin_outside_effective_domain_is_denied() {
    let temp = tempfile::tempdir().unwrap();
    let mut req = request(temp.path());
    let grant = req
        .context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap();
    grant.allowed_actions = Some(vec![BrowserAction::Navigate, BrowserAction::CredentialUse]);
    grant.credential_policy_ids = vec!["pol-example".into()];
    let policy = req.context.browser_policy.as_mut().unwrap();
    policy.allowed_actions = vec![BrowserAction::Navigate, BrowserAction::CredentialUse];
    policy.credential_policy_ids = vec!["pol-example".into()];
    let prepared = crate::browser_policy::prepare(grant, Some(policy), SUPPORTED_VERSION).unwrap();
    let path = temp.path().join("credential-request.json");
    for (origin, accepted) in [
        ("https://example.com", true),
        ("https://other.example.com", false),
    ] {
        std::fs::write(&path, serde_json::json!({"policy_id":"pol-example", "origin":origin, "purpose":"Read dashboard"}).to_string()).unwrap();
        assert_eq!(read_credential_request(&path, &prepared).is_ok(), accepted);
    }
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
            None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(code), "{error}");
    }
    assert!(sink.browsers.lock().unwrap().is_empty());
    assert!(!temp.path().join("runs/refused").exists());
}

// Re-exec this unit test in a private network namespace. Its public-looking fixture address
// has no route to the outside world; only the host-side egress process can reach it.
#[test]
fn missing_egress_resolver_refuses_before_browser_start() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "browser::tests::missing_egress_resolver_inner",
            "--nocapture",
        ])
        .env("CELERIS_MISSING_EGRESS_INNER", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn missing_egress_resolver_inner() {
    if std::env::var("CELERIS_MISSING_EGRESS_INNER").is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap();
    configure_isolated_runtime(IsolatedBrowserConfig {
        resolver: None,
        record_dir: temp.path().join("records"),
        bwrap: "/usr/bin/bwrap".into(),
        sandboxd: bin.join("celeris-browser-sandboxd"),
        egress: bin.join("celeris-browser-egress"),
    });
    let req = request(temp.path());
    let error = run_with_executable(
        Arc::new(CliHarness {
            id: "acp",
            question: false,
        }),
        req,
        "no-egress",
        limits(),
        &RecordingSink::default(),
        &temp.path().join("must-not-start"),
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("isolated_runtime_unavailable"));
    assert!(!temp.path().join("runs").exists());
}

#[test]
fn production_action_path_reaches_fixture_through_real_browser_and_egress() {
    if std::env::var("CELERIS_BROWSER_ACTION_INNER").is_ok() {
        return;
    }
    let out = std::process::Command::new("/usr/bin/unshare")
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "browser::tests::production_action_path_inner",
            "--nocapture",
        ])
        .env("CELERIS_BROWSER_ACTION_INNER", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success()
            && stdout.contains("1 passed")
            && stderr.contains("ACTION-EGRESS positive"),
        "real action runtime failed: {stdout}\n{stderr}"
    );
}

fn fixture_dns(listener: std::net::TcpListener) {
    use std::io::{Read, Write};
    for conn in listener.incoming() {
        let Ok(mut c) = conn else { continue };
        let mut length = [0u8; 2];
        if c.read_exact(&mut length).is_err() {
            continue;
        }
        let mut query = vec![0; u16::from_be_bytes(length) as usize];
        if c.read_exact(&mut query).is_err() {
            continue;
        }
        let mut pos = 12;
        while pos < query.len() && query[pos] != 0 {
            pos += usize::from(query[pos]) + 1;
        }
        if pos + 4 >= query.len() {
            continue;
        }
        let typ = u16::from_be_bytes([query[pos + 1], query[pos + 2]]);
        let mut answer = query[..pos + 5].to_vec();
        answer[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        if typ == 1 {
            answer[6..8].copy_from_slice(&1u16.to_be_bytes());
            answer.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 216, 34]);
        }
        let _ = c.write_all(&(answer.len() as u16).to_be_bytes());
        let _ = c.write_all(&answer);
    }
}

struct FixtureHarness;
#[async_trait::async_trait]
impl WorkerAdapter for FixtureHarness {
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
        let cli = &req.context.browser.as_ref().unwrap().cli;
        let open = tokio::process::Command::new("python3")
            .arg(cli)
            .args(["open", "https://fixture.example.com/index.html"])
            .output()
            .await?;
        assert!(
            open.status.success(),
            "open: {}",
            String::from_utf8_lossy(&open.stdout)
        );
        let blocked = tokio::process::Command::new("python3")
            .arg(cli)
            .args(["--proxy-server=http://127.0.0.1:1"])
            .output()
            .await?;
        assert_eq!(blocked.status.code(), Some(2), "forbidden flag");
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "fixture reached".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

#[tokio::test]
async fn production_action_path_inner() {
    if std::env::var("CELERIS_BROWSER_ACTION_INNER").is_err() {
        return;
    }
    use std::net::TcpStream;
    use std::time::Instant;
    let temp = tempfile::tempdir().unwrap();
    let sh = |command: &str| {
        assert!(
            std::process::Command::new("/bin/sh")
                .args(["-c", command])
                .status()
                .unwrap()
                .success(),
            "{command}"
        );
    };
    sh("ip link set lo up && ip addr add 93.184.216.34/32 dev lo");
    let dns = std::net::TcpListener::bind("127.0.0.1:53").unwrap();
    std::thread::spawn(move || fixture_dns(dns));
    std::fs::write(
        temp.path().join("index.html"),
        "<html>celeris-action-fixture-9581</html>",
    )
    .unwrap();
    let cert = std::process::Command::new("/usr/bin/openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "key.pem",
            "-out",
            "cert.pem",
            "-days",
            "1",
            "-subj",
            "/CN=fixture.example.com",
            "-addext",
            "subjectAltName=DNS:fixture.example.com",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(cert.status.success(), "{:?}", cert.status);
    let mut fixture = std::process::Command::new("/usr/bin/openssl")
        .args([
            "s_server",
            "-quiet",
            "-WWW",
            "-cert",
            "cert.pem",
            "-key",
            "key.pem",
            "-accept",
            "93.184.216.34:443",
        ])
        .current_dir(temp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("93.184.216.34", 443)).is_err() {
        assert!(Instant::now() < deadline, "fixture did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let browser = {
        let mut candidates: Vec<_> = std::fs::read_dir(
            Path::new(&std::env::var("HOME").unwrap()).join(".cache/ms-playwright"),
        )
        .unwrap()
        .flatten()
        .map(|e| {
            e.path()
                .join("chrome-headless-shell-linux64/chrome-headless-shell")
        })
        .filter(|p| p.is_file())
        .collect();
        candidates.sort();
        candidates
            .pop()
            .expect("real chrome-headless-shell required")
    };
    let script = temp.path().join("fixture-agent-browser");
    let source = format!(
        r#"#!/usr/bin/python3
import json, pathlib, subprocess, sys
if sys.argv[1:] == ['--version']:
    print('agent-browser 0.38.1')
    sys.exit(0)
args = sys.argv[1:]
action = args[args.index('--json') + 1:]
if action[0] == 'open':
    browser = subprocess.run([{browser:?}, '--headless', '--no-sandbox', '--no-zygote',
        '--disable-gpu', '--disable-dev-shm-usage', '--disable-background-networking',
        '--disable-component-update', '--no-first-run', '--ignore-certificate-errors',
        '--proxy-server=http://127.0.0.1:3128', '--proxy-bypass-list=<-loopback>',
        '--user-data-dir=/session/profile', '--dump-dom', action[1]],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=35)
    ok = b'celeris-action-fixture-9581' in browser.stdout
    if ok:
        pathlib.Path('/session/fixture-reached').write_text('real chrome through egress')
    print(json.dumps({{'success': ok, 'data': {{'text': 'fixture' if ok else 'unreachable'}}}}))
    sys.exit(0 if ok else 1)
print(json.dumps({{'success': True, 'data': {{}}}}))
"#,
        browser = browser.to_string_lossy()
    );
    crate::test_support::write_executable(&script, &source);
    let exe = std::env::current_exe().unwrap();
    let bin = exe.parent().unwrap().parent().unwrap();
    configure_isolated_runtime(IsolatedBrowserConfig {
        resolver: Some("127.0.0.1".parse().unwrap()),
        record_dir: temp.path().join("records"),
        bwrap: "/usr/bin/bwrap".into(),
        sandboxd: bin.join("celeris-browser-sandboxd"),
        egress: bin.join("celeris-browser-egress"),
    });
    let mut req = request(temp.path());
    req.context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .allowed_domains = vec!["fixture.example.com".into()];
    req.context.browser_policy.as_mut().unwrap().network_domains =
        vec!["fixture.example.com".into()];
    let outcome = run_with_executable(
        Arc::new(FixtureHarness),
        req,
        "real-egress",
        RunLimits {
            wall_clock: Duration::from_secs(90),
            idle_timeout: Duration::from_secs(80),
            kill_grace: Duration::from_millis(100),
        },
        &RecordingSink::default(),
        &script,
        None,
    )
    .await
    .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("runs/real-egress/browser/fixture-reached"))
            .unwrap(),
        "real chrome through egress"
    );
    eprintln!(
        "ACTION-EGRESS positive: shim -> action.sock -> sandboxd -> chrome -> egress -> fixture; forbidden flag rejected"
    );
    let _ = fixture.kill();
    let _ = fixture.wait();
}

mod live_wiring {
    use super::*;
    use crate::browser_live::{CollectingSink, LiveEmitter};

    #[test]
    fn browser_live_forwarded_events_are_scrubbed_status_only() {
        let temp = tempfile::tempdir().unwrap();
        let req = request(temp.path());
        let output = req.artifacts_dir.join("browser");
        std::fs::create_dir_all(&output).unwrap();
        let events = temp.path().join("events.jsonl");
        let secret = "Cookie: session=abc; token=SECRET_TOKEN";
        let lines = [
            serde_json::json!({"operation":"navigate","status":"success"}),
            serde_json::json!({"operation":"click","status":"success","detail":secret}),
            serde_json::json!({"operation":secret,"status":"success"}),
        ];
        std::fs::write(
            &events,
            lines
                .iter()
                .map(|l| l.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let emitter = LiveEmitter::new(CollectingSink::default());
        let mut offset = 0;
        forward_events(
            &events,
            &mut offset,
            &req,
            &output,
            &RecordingSink::default(),
            &emitter,
        );
        let sent = emitter.sink().events();
        assert_eq!(sent.len(), 1, "untyped lines are dropped before live");
        let json = serde_json::to_string(&sent).unwrap();
        for bad in [
            "frame",
            "title",
            "Cookie",
            "cookie",
            "SECRET_TOKEN",
            "token",
        ] {
            assert!(!json.contains(bad), "{bad} leaked: {json}");
        }
        assert!(json.contains("\"kind\":\"status\""));
    }

    /// ADR-0080 H3: the production `forward_events` forwards nothing from inside the auth
    /// section — no progress (LLM-visible / persisted), no artifact, no live event — and
    /// consumes the lines instead of buffering them.
    #[test]
    fn browser_auth_section_forward_events_drops_progress_artifact_and_live() {
        let temp = tempfile::tempdir().unwrap();
        let req = request(temp.path());
        let output = req.artifacts_dir.join("browser");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("extract-a.json"), "{}").unwrap();
        let events = temp.path().join("events.jsonl");
        std::fs::write(
            &events,
            "{\"operation\":\"navigate\",\"status\":\"success\"}\n\
             {\"operation\":\"extract\",\"status\":\"success\",\"artifact\":\"extract-a.json\"}\n",
        )
        .unwrap();
        let emitter = LiveEmitter::new(CollectingSink::default());
        let sink = RecordingSink::default();
        let mut offset = 0;
        {
            let _auth = emitter.auth_section();
            forward_events(&events, &mut offset, &req, &output, &sink, &emitter);
        }
        assert!(
            emitter.sink().events().is_empty(),
            "no live events during auth"
        );
        assert!(
            sink.progress.lock().unwrap().is_empty(),
            "no progress during auth"
        );
        assert!(
            sink.artifacts.lock().unwrap().is_empty(),
            "no artifact during auth"
        );
        // not buffered: leaving the section does not replay them
        forward_events(&events, &mut offset, &req, &output, &sink, &emitter);
        assert!(emitter.sink().events().is_empty());
        assert!(sink.progress.lock().unwrap().is_empty());
        // lines written after the section are forwarded as usual
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&events)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            b"{\"operation\":\"click\",\"status\":\"success\"}\n",
        )
        .unwrap();
        forward_events(&events, &mut offset, &req, &output, &sink, &emitter);
        assert_eq!(emitter.sink().events().len(), 1);
        assert_eq!(sink.progress.lock().unwrap().len(), 1);
    }
}
