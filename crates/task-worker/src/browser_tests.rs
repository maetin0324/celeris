use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use task_core::browser_wait::{BrowserWait, BrowserWaitStore, NewBrowserWait};
use task_core::{
    ArtifactRef, BrowserAction, BrowserCapability, BrowserDomainMode, BrowserTaskPolicy,
    EffectiveProfile, SqliteStore, Status, TaskId, TaskStore,
};

/// Mock ledger for unit tests of routing and the supervisor. These tests do not certify a
/// production backend; the real conformance runner is responsible for production records.
pub(super) fn test_record(workspace: &Path) -> PathBuf {
    let path = workspace.join("browser-conformance-test.json");
    let cases = [
        "open_allowed_origin",
        "refuse_denied_origin",
        "resume_after_crash",
        "snapshot_has_refs",
        "click_by_ref",
        "screenshot_artifact",
        "download_to_artifacts",
    ];
    let results: Vec<_> = ["acp", "claude-code", "browser-specialist"]
        .into_iter()
        .map(|backend_id| {
            serde_json::json!({
                "backend_id": backend_id, "version": SUPPORTED_VERSION, "passed": cases,
            })
        })
        .collect();
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "source": "celeris-browser-conformance", "results": results,
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

#[derive(Default)]
struct RecordingSink {
    browsers: Mutex<Vec<BrowserRun>>,
    progress: Mutex<Vec<(String, ProgressFields)>>,
    artifacts: Mutex<Vec<ArtifactRef>>,
    comments: Mutex<Vec<String>>,
}
impl EventSink for RecordingSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<std::sync::Arc<dyn crate::browser_live::ControlGate>> {
        Some(std::sync::Arc::new(crate::browser_live::InMemoryGate::new()))
    }
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
        live_sessions: None,
        runtime: Default::default(),
        resolver: Some("127.0.0.1".parse().unwrap()),
        record_dir: workspace.join("browser-record"),
        bwrap: "/usr/bin/bwrap".into(),
        sandboxd: bin.join("celeris-browser-sandboxd"),
        egress: bin.join("celeris-browser-egress"),
    });
    let mut task = crate::protocol::tests::sample_task();
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://example.com".into()],
    });
    RunRequest {
        protocol: crate::protocol::PROTOCOL_VERSION,
        task,
        workspace: workspace.into(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: crate::protocol::RunContext {
            profile: Some(EffectiveProfile {
                browser: Some(BrowserCapability {
                    approval_actions: vec![],
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

#[test]
fn p4c_fallback_ledger_parsing_and_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let record = test_record(dir.path());
    let ids = conformant_backend_ids(&record).unwrap();
    assert!(ids.contains("acp") && ids.contains("claude-code"));
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    ledger["results"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["backend_id"] == "acp");
    std::fs::write(&record, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let ids = conformant_backend_ids(&record).unwrap();
    assert_eq!(ids, ["acp".to_string()].into());
    let req = request(dir.path());
    let grant = req
        .context
        .profile
        .as_ref()
        .unwrap()
        .browser
        .as_ref()
        .unwrap();
    let policy = crate::browser_policy::prepare(
        grant,
        req.context.browser_policy.as_ref(),
        SUPPORTED_VERSION,
    )
    .unwrap();
    let route = route_existing_backend("acp", &policy, &record).unwrap();
    assert!(route.fallbacks.is_empty());
    assert!(route_existing_backend("claude-code", &policy, &record).is_err());
    ledger["source"] = serde_json::json!("celeris-browser-conformance-scripted");
    std::fs::write(&record, serde_json::to_vec(&ledger).unwrap()).unwrap();
    assert!(conformant_backend_ids(&record).is_err());
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(10),
        idle_timeout: Duration::from_secs(5),
        kill_grace: Duration::from_millis(100),
    }
}

/// Invoked by scripts/browser-conformance.py in a separate process for each backend and
/// phase. This runs the real adapter protocol against a scripted LLM while the Python
/// runner provides the real agent-browser executable and loopback site. It is ignored
/// in the normal workspace gate because those external local executables are optional.
#[tokio::test]
#[ignore]
async fn p4c_conformance_backend_protocol() {
    let backend = std::env::var("CELERIS_BROWSER_BACKEND").expect("runner backend");
    let phase = std::env::var("CELERIS_BROWSER_PHASE").expect("runner phase");
    let runtime =
        PathBuf::from(std::env::var_os("CELERIS_BROWSER_RUNTIME").expect("runner runtime"));
    let cli = PathBuf::from(std::env::var_os("CELERIS_BROWSER_CLI").expect("runner cli"));
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/browser-conformance-harness.py");
    let mut req = request(&runtime);
    req.context.browser = Some(BrowserContext {
        run: BrowserRun {
            task_id: req.task.id,
            run_id: format!("p4c-{phase}"),
            session_id: format!("celeris-p4c-{backend}"),
            state: BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli,
        credential_used: false,
        approval_actions: Vec::new(),
        approved_operation: None,
    });
    let command = script.to_string_lossy().into_owned();
    let adapter: Arc<dyn WorkerAdapter> = match backend.as_str() {
        "acp" => Arc::new(crate::AcpAdapter::new(crate::AcpConfig {
            command,
            args: vec![],
            startup_timeout: Duration::from_secs(10),
            ..Default::default()
        })),
        "claude-code" => Arc::new(crate::ClaudeCodeAdapter::new(crate::ClaudeCodeConfig {
            command,
            ..Default::default()
        })),
        "browser-specialist" => Arc::new(crate::BrowserSpecialistAdapter::new(Arc::new(
            crate::AcpAdapter::new(crate::AcpConfig {
                command,
                args: vec![],
                startup_timeout: Duration::from_secs(10),
                ..Default::default()
            }),
        ))),
        _ => panic!("unknown backend: {backend}"),
    };
    let result = adapter
        .run(
            req,
            &format!("p4c-{phase}"),
            RunLimits {
                wall_clock: Duration::from_secs(90),
                idle_timeout: Duration::from_secs(90),
                kill_grace: Duration::from_millis(100),
            },
            &RecordingSink::default(),
        )
        .await;
    if phase == "resume" {
        assert!(matches!(result.unwrap().terminal, Terminal::Done { .. }));
    } else {
        assert_eq!(phase, "open");
        assert!(!matches!(
            result,
            Ok(RunOutcome {
                terminal: Terminal::Done { .. },
                ..
            })
        ));
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
    let record = test_record(temp.path());
    let routed = route_existing_backend("acp", &public, &record).unwrap();
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
        assert!(route_existing_backend(backend, &sensitive, &record).is_err());
    }
    assert!(route_existing_backend("unsupported", &public, &record).is_err());

    let scripted = temp.path().join("scripted.json");
    let bytes = std::fs::read(&record).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["source"] = serde_json::json!("celeris-browser-conformance-scripted");
    std::fs::write(&scripted, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(route_existing_backend("acp", &public, &scripted).is_err());

    value["source"] = serde_json::json!("celeris-browser-conformance");
    value["results"][0]["version"] = serde_json::json!("0.38.0");
    std::fs::write(&scripted, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(route_existing_backend("acp", &public, &scripted).is_err());
}

/// ADR-0112: the P4-B ledger with per-test evidence, as `scripts/browser-conformance.py
/// --p4b-evidence` writes it, for `backends` only.
fn p4b_record(dir: &Path, backends: &[&str], evidence: bool) -> PathBuf {
    use task_core::browser_backend::{
        ConformanceEvidence, EvidenceOutcome, FixtureCase, required_evidence,
    };
    let mut passed: Vec<serde_json::Value> = [
        "open_allowed_origin",
        "refuse_denied_origin",
        "resume_after_crash",
        "snapshot_has_refs",
        "click_by_ref",
        "screenshot_artifact",
        "download_to_artifacts",
        "isolation_suite",
        "egress_negative_suite",
        "injection_attack_suite",
        "auth_section_observation_stop",
    ]
    .into_iter()
    .map(serde_json::Value::from)
    .collect();
    passed.sort_by_key(|v| v.to_string());
    let proof: Vec<ConformanceEvidence> = if evidence {
        [
            FixtureCase::InjectionAttackSuite,
            FixtureCase::AuthSectionObservationStop,
        ]
        .into_iter()
        .flat_map(|case| {
            required_evidence(case)
                .into_iter()
                .map(move |test| ConformanceEvidence {
                    case,
                    test,
                    outcome: EvidenceOutcome::Passed,
                })
        })
        .collect()
    } else {
        Vec::new()
    };
    let results: Vec<_> = backends
        .iter()
        .map(|id| {
            serde_json::json!({
                "backend_id": id, "version": SUPPORTED_VERSION,
                "passed": passed, "evidence": proof,
            })
        })
        .collect();
    let path = dir.join(format!("p4b-{}-{evidence}.json", backends.join("-")));
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "source": "celeris-browser-conformance", "results": results,
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn credential_policy(workspace: &Path) -> crate::browser_policy::PreparedBrowserPolicy {
    let mut req = request(workspace);
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
    let task_policy = req.context.browser_policy.as_mut().unwrap();
    task_policy.allowed_actions = vec![BrowserAction::Navigate, BrowserAction::CredentialUse];
    task_policy.credential_policy_ids = vec!["pol-example".into()];
    crate::browser_policy::prepare(
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
    .unwrap()
}

/// ADR-0112 D1/D2: CredentialUse is released only by a ledger carrying the P4-B measured
/// evidence; the same case names without evidence stay refused.
#[test]
fn credential_use_is_released_only_by_p4b_evidence_in_the_ledger() {
    let temp = tempfile::tempdir().unwrap();
    let policy = credential_policy(temp.path());
    let bare = p4b_record(temp.path(), &["acp"], false);
    let error = route_existing_backend("acp", &policy, &bare).unwrap_err();
    assert!(error.to_string().contains("lacks required conformance"));
    let released = p4b_record(temp.path(), &["acp"], true);
    assert_eq!(
        route_existing_backend("acp", &policy, &released)
            .unwrap()
            .primary,
        "acp"
    );
    // one failed attack mark closes it again
    let bytes = std::fs::read(&released).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["results"][0]["evidence"][8]["outcome"] = serde_json::json!("failed");
    let failed = temp.path().join("p4b-failed.json");
    std::fs::write(&failed, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(route_existing_backend("acp", &policy, &failed).is_err());
}

/// ADR-0112 D3: after the release, a backend without its own P4-B record and a runtime that is
/// not isolated are still refused.
#[test]
fn released_ledger_still_refuses_unconformant_backend_and_unisolated_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let policy = credential_policy(temp.path());
    let released = p4b_record(temp.path(), &["acp"], true);
    assert!(route_existing_backend("acp", &policy, &released).is_ok());
    for backend in ["claude-code", "browser-specialist", "unsupported"] {
        assert!(route_existing_backend(backend, &policy, &released).is_err());
    }
    let unisolated = |bwrap: &str| IsolatedBrowserConfig {
        live_sessions: None,
        runtime: Default::default(),
        resolver: Some("127.0.0.1".parse().unwrap()),
        record_dir: temp.path().join("records"),
        bwrap: bwrap.into(),
        sandboxd: temp.path().join("missing-sandboxd"),
        egress: temp.path().join("missing-egress"),
    };
    let unavailable = |r: Result<&IsolatedBrowserConfig, AdapterError>| {
        r.err()
            .is_some_and(|e| e.to_string().contains("isolated_runtime_unavailable"))
    };
    assert!(unavailable(isolated_runtime_ready(None)));
    assert!(unavailable(isolated_runtime_ready(Some(&unisolated(
        "/nonexistent/bwrap"
    )))));
    let mut no_resolver = unisolated("/usr/bin/bwrap");
    no_resolver.resolver = None;
    assert!(unavailable(isolated_runtime_ready(Some(&no_resolver))));
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
        approval_actions: Vec::new(),
        approved_operation: None,
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

struct FailingHarness;

#[async_trait::async_trait]
impl WorkerAdapter for FailingHarness {
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
        assert!(req.context.browser.is_some());
        Err(AdapterError::Other("scripted backend failed".into()))
    }
}

#[tokio::test]
async fn execution_fallback_uses_fresh_session_and_refuses_without_conformance() {
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let record = test_record(temp.path());
    let executable = substrate(temp.path());
    let sink = RecordingSink::default();
    let outcome = run_with_executable_candidates_record(
        Arc::new(FailingHarness),
        vec![Arc::new(CliHarness {
            id: "claude-code",
            question: false,
        })],
        request(temp.path()),
        "fallback-run",
        limits(),
        &sink,
        &executable,
        None,
        &record,
    )
    .await
    .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    assert!(
        temp.path()
            .join("runs/fallback-run/browser-fallback-1/config.json")
            .exists()
    );
    {
        let browsers = sink.browsers.lock().unwrap();
        assert_eq!(browsers.len(), 4);
        assert_ne!(browsers[0].session_id, browsers[2].session_id);
    }

    let denied = temp.path().join("empty-conformance.json");
    std::fs::write(
        &denied,
        br#"{"schema":1,"source":"celeris-browser-conformance","results":[]}"#,
    )
    .unwrap();
    let empty = tempfile::tempdir().unwrap();
    let error = run_with_executable_candidates_record(
        Arc::new(FailingHarness),
        vec![Arc::new(CliHarness {
            id: "claude-code",
            question: false,
        })],
        request(empty.path()),
        "denied-run",
        limits(),
        &RecordingSink::default(),
        &executable,
        None,
        &denied,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("lacks required conformance"));
    assert!(!empty.path().join("runs/denied-run").exists());
}

/// The Python conformance runner calls this with its just-produced ledger before
/// publishing it. Routing and the fallback execution path must consume that same file.
#[tokio::test]
#[ignore]
async fn p4c_runner_record_routes_and_falls_back() {
    let record = PathBuf::from(
        std::env::var_os("CELERIS_BROWSER_CONFORMANCE_FILE").expect("runner record path"),
    );
    let temp = tempfile::tempdir().unwrap();
    let req = request(temp.path());
    let grant = req
        .context
        .profile
        .as_ref()
        .unwrap()
        .browser
        .as_ref()
        .unwrap();
    let policy = crate::browser_policy::prepare(
        grant,
        req.context.browser_policy.as_ref(),
        SUPPORTED_VERSION,
    )
    .unwrap();
    for backend in ["acp", "claude-code", "browser-specialist"] {
        assert_eq!(
            route_existing_backend(backend, &policy, &record)
                .unwrap()
                .primary,
            backend
        );
    }
    let mut sensitive = request(temp.path());
    let sensitive_grant = sensitive
        .context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap();
    sensitive_grant.allowed_actions =
        Some(vec![BrowserAction::Navigate, BrowserAction::CredentialUse]);
    sensitive_grant.credential_policy_ids = vec!["pol-example".into()];
    let sensitive_policy = sensitive.context.browser_policy.as_mut().unwrap();
    sensitive_policy.allowed_actions = vec![BrowserAction::Navigate, BrowserAction::CredentialUse];
    sensitive_policy.credential_policy_ids = vec!["pol-example".into()];
    let effective = crate::browser_policy::prepare(
        sensitive
            .context
            .profile
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap(),
        sensitive.context.browser_policy.as_ref(),
        SUPPORTED_VERSION,
    )
    .unwrap();
    for backend in ["acp", "claude-code", "browser-specialist"] {
        assert!(route_existing_backend(backend, &effective, &record).is_err());
    }
    let executable = substrate(temp.path());
    let outcome = run_with_executable_candidates_record(
        Arc::new(FailingHarness),
        vec![Arc::new(CliHarness {
            id: "claude-code",
            question: false,
        })],
        req,
        "runner-ledger-fallback",
        limits(),
        &RecordingSink::default(),
        &executable,
        None,
        &record,
    )
    .await
    .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    assert!(
        temp.path()
            .join("runs/runner-ledger-fallback/browser-fallback-1/config.json")
            .exists()
    );
}

/// The conformance runner supplies its measured ledger, real 0.38.1 executable and
/// loopback origin. The primary ACP harness is SIGKILLed after navigation; the worker
/// then starts Claude in a fresh browser session and completes the same fixture.
#[tokio::test]
#[ignore]
async fn p4c_fallback_real_harness_scenario() {
    let root = PathBuf::from(std::env::var_os("CELERIS_BROWSER_FALLBACK_ROOT").unwrap());
    let record = PathBuf::from(std::env::var_os("CELERIS_BROWSER_CONFORMANCE_FILE").unwrap());
    let executable = PathBuf::from(std::env::var_os("CELERIS_BROWSER_EXECUTABLE").unwrap());
    // The browser runs in the isolated runtime, whose egress refuses loopback except a
    // test-only `127.0.0.1:<port>` literal; reach the runner's fixture through that.
    let fixture = url::Url::parse(&std::env::var("CELERIS_BROWSER_ORIGIN").unwrap()).unwrap();
    let port = fixture.port().expect("runner fixture port");
    let origin = format!("http://127.0.0.1:{port}/");
    let _ = crate::browser::TEST_LOOPBACK_ALLOW.set([format!("127.0.0.1:{port}")].into());
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/browser-conformance-harness.py");
    let command = script.to_string_lossy().into_owned();
    let primary: Arc<dyn WorkerAdapter> = Arc::new(crate::AcpAdapter::new(crate::AcpConfig {
        command: command.clone(),
        startup_timeout: Duration::from_secs(10),
        ..Default::default()
    }));
    let alternate: Arc<dyn WorkerAdapter> =
        Arc::new(crate::ClaudeCodeAdapter::new(crate::ClaudeCodeConfig {
            command,
            ..Default::default()
        }));
    let env = |phase: &str, runtime: &Path| {
        vec![
            ("CELERIS_BROWSER_PHASE".into(), phase.into()),
            (
                "CELERIS_BROWSER_BACKEND".into(),
                if phase == "open" {
                    "acp"
                } else {
                    "claude-code"
                }
                .into(),
            ),
            (
                "CELERIS_BROWSER_RUNTIME".into(),
                runtime.to_string_lossy().into_owned(),
            ),
            ("CELERIS_BROWSER_ORIGIN".into(), origin.clone()),
            (
                "CELERIS_BROWSER_KILL_HARNESS".into(),
                if phase == "open" { "1" } else { "0" }.into(),
            ),
        ]
    };
    let primary_dir = root.join("primary");
    let alternate_dir = root.join("alternate");
    std::fs::create_dir_all(&primary_dir).unwrap();
    std::fs::create_dir_all(&alternate_dir).unwrap();
    let primary = primary.with_env(&env("open", &primary_dir)).unwrap();
    let alternate = alternate
        .with_env(&env("fallback", &alternate_dir))
        .unwrap();
    let primary = primary
        .with_env(&[(
            "CELERIS_BROWSER_CLI".into(),
            root.join("runs/p4c-real-fallback/browser/celeris-browser.py")
                .to_string_lossy()
                .into_owned(),
        )])
        .unwrap();
    let alternate = alternate
        .with_env(&[(
            "CELERIS_BROWSER_CLI".into(),
            root.join("runs/p4c-real-fallback/browser-fallback-1/celeris-browser.py")
                .to_string_lossy()
                .into_owned(),
        )])
        .unwrap();
    let mut req = request(&root);
    // Canonical origin (scheme, host, port): a bare "localhost" means https://localhost:443,
    // which neither intersects the task requirement nor reaches the loopback fixture.
    let host = origin.trim_end_matches('/').to_string();
    req.task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec![host.clone()],
    });
    req.context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .allowed_domains = vec![host.clone()];
    let policy = req.context.browser_policy.as_mut().unwrap();
    policy.network_domains = vec![host];
    policy.allowed_actions = vec![
        BrowserAction::Navigate,
        BrowserAction::Snapshot,
        BrowserAction::Click,
        BrowserAction::Screenshot,
        BrowserAction::Download,
    ];
    let sink = RecordingSink::default();
    let outcome = run_with_candidates(
        primary.clone(),
        vec![alternate.clone()],
        req.clone(),
        "p4c-real-fallback",
        RunLimits {
            wall_clock: Duration::from_secs(120),
            idle_timeout: Duration::from_secs(120),
            kill_grace: Duration::from_millis(100),
        },
        &sink,
    )
    .await
    .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    assert!(primary_dir.join("harness-killed.pid").exists());
    {
        let sessions = sink.browsers.lock().unwrap();
        assert!(sessions.len() >= 4);
        assert_eq!(sessions[1].state, BrowserRunState::Failed);
        assert_ne!(sessions[0].session_id, sessions[2].session_id);
    }
    let no_alternate_record = root.join("no-alternate-conformance.json");
    let mut no_alternate_ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
    no_alternate_ledger["results"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["backend_id"] == "acp");
    std::fs::write(
        &no_alternate_record,
        serde_json::to_vec(&no_alternate_ledger).unwrap(),
    )
    .unwrap();
    let no_candidate_primary = primary
        .with_env(&[(
            "CELERIS_BROWSER_CLI".into(),
            root.join("runs/p4c-no-candidate/browser/celeris-browser.py")
                .to_string_lossy()
                .into_owned(),
        )])
        .unwrap();
    let no_candidate = run_with_executable_candidates_record(
        no_candidate_primary,
        vec![alternate.clone()],
        req.clone(),
        "p4c-no-candidate",
        limits(),
        &RecordingSink::default(),
        &executable,
        None,
        &no_alternate_record,
    )
    .await
    .unwrap_err();
    assert!(
        no_candidate
            .to_string()
            .contains("all capable backends failed")
    );
    assert!(
        !root
            .join("runs/p4c-no-candidate/browser-fallback-1")
            .exists()
    );
    let mut sensitive = req;
    sensitive
        .context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .allowed_actions = Some(vec![BrowserAction::Navigate, BrowserAction::CredentialUse]);
    sensitive
        .context
        .profile
        .as_mut()
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .credential_policy_ids = vec!["pol-example".into()];
    sensitive
        .context
        .browser_policy
        .as_mut()
        .unwrap()
        .allowed_actions = vec![BrowserAction::Navigate, BrowserAction::CredentialUse];
    sensitive
        .context
        .browser_policy
        .as_mut()
        .unwrap()
        .credential_policy_ids = vec!["pol-example".into()];
    let credential = run_with_executable_candidates_record(
        primary,
        vec![alternate],
        sensitive,
        "p4c-credential",
        limits(),
        &RecordingSink::default(),
        &executable,
        None,
        &record,
    )
    .await
    .unwrap_err();
    assert!(
        credential
            .to_string()
            .contains("lacks required conformance")
    );
    assert!(!root.join("runs/p4c-credential").exists());
    std::fs::write(
        root.join("fallback-outcome.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "primary_harness_killed": true, "alternate_completed": true,
            "fresh_session": true, "no_candidate_refused": no_candidate.to_string(),
            "credential_refused": credential.to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn specialist_wraps_existing_harness_and_runs_same_browser_task() {
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let record = test_record(temp.path());
    let executable = substrate(temp.path());
    let sink = RecordingSink::default();
    let specialist = crate::BrowserSpecialistAdapter::new(Arc::new(CliHarness {
        id: "acp",
        question: false,
    }));
    let outcome = run_with_executable_record(
        Arc::new(specialist),
        request(temp.path()),
        "specialist-run",
        limits(),
        &sink,
        &executable,
        None,
        &record,
    )
    .await
    .unwrap();
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
    assert_eq!(sink.artifacts.lock().unwrap().len(), 2);
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
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<std::sync::Arc<dyn crate::browser_live::ControlGate>> {
        Some(std::sync::Arc::new(crate::browser_live::InMemoryGate::new()))
    }
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
    fn browser_operation_approval_consume(
        &self,
        wait: &BrowserWait,
    ) -> Result<task_core::browser_wait::ConsumedBrowserOperation, String> {
        task_core::browser_wait::consume_operation_approval(
            &self.store,
            self.task_id,
            wait,
            time::OffsetDateTime::now_utc(),
        )
        .map_err(String::from)
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
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
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
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
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
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let executable = substrate(temp.path());
    let sink = RecordingSink::default();
    let mut req = request(temp.path());
    // ADR 2026-10-05 browser-allowed-origins: the task asks for explicit origins; the legacy
    // host-form grant `*.example.org` must contain `https://docs.example.org` (HTTPS 443 only).
    req.task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec![
            "https://example.com".into(),
            "https://docs.example.org".into(),
        ],
    });
    run_with_executable(
        Arc::new(CliHarness {
            id: "acp",
            question: false,
        }),
        req,
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
    // stored policy {example.com, docs.example.org} ∩ requirements {https://example.com,
    // https://docs.example.org} ∩ legacy grant {example.com, *.example.org}, as HTTPS origins.
    assert_eq!(
        config["allowed_domains"],
        serde_json::json!(["https://docs.example.org", "https://example.com"])
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
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
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
        live_sessions: None,
        runtime: Default::default(),
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
    if crate::test_support::skip_unless_userns_tests() {
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
        '--user-data-dir=/session/fixture-profile', '--dump-dom', action[1]],
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
        live_sessions: None,
        runtime: Default::default(),
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
    // ADR 2026-10-05: effective = policy ∩ Task.requirements.browser ∩ grant; the task must
    // ask for the fixture origin too, or admission rejects with empty_browser_domains.
    req.task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://fixture.example.com".into()],
    });
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
    fn h3_injected_session_forward_events_stays_stopped_after_injection() {
        let temp = tempfile::tempdir().unwrap();
        let req = request(temp.path());
        let output = req.artifacts_dir.join("browser");
        std::fs::create_dir_all(&output).unwrap();
        let events = temp.path().join("events.jsonl");
        let emitter = LiveEmitter::new(CollectingSink::default());
        let sink = RecordingSink::default();
        let mut offset = 0;
        let session_guard = emitter.auth_section();
        std::fs::write(
            &events,
            "{\"operation\":\"navigate\",\"status\":\"success\"}\n",
        )
        .unwrap();
        forward_events(&events, &mut offset, &req, &output, &sink, &emitter);
        std::fs::write(
            &events,
            "{\"operation\":\"click\",\"status\":\"success\"}\n",
        )
        .unwrap();
        offset = 0;
        forward_events(&events, &mut offset, &req, &output, &sink, &emitter);
        assert!(sink.progress.lock().unwrap().is_empty());
        assert!(sink.artifacts.lock().unwrap().is_empty());
        assert!(emitter.sink().events().is_empty());
        drop(session_guard);
    }

    #[test]
    fn h3_injected_session_live_view_stays_stopped_after_injection() {
        let emitter = LiveEmitter::new(CollectingSink::default());
        let session_guard = emitter.auth_section();
        for state in ["injecting", "adapter-running", "session-closing"] {
            assert!(!emitter.emit(&task_core::browser_live::LiveEvent::Status {
                state: state.into(),
            }));
        }
        assert!(emitter.sink().events().is_empty());
        drop(session_guard);
    }

    #[test]
    fn h3_injected_session_guard_releases_at_session_end() {
        let emitter = LiveEmitter::new(CollectingSink::default());
        let session_guard = emitter.auth_section();
        assert!(emitter.in_auth_section());
        // Production drops the guard only after close_with and the final drain.
        drop(session_guard);
        assert!(!emitter.in_auth_section());
    }

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

    /// ADR-0080 H3 / ADR-0101 D4: identity の復元で立った旗の後は、`forward_events` が
    /// progress・artifact・live event を流さず、session の終わりまで戻らない。
    #[test]
    fn browser_restored_session_forward_events_drops_until_session_end() {
        let temp = tempfile::tempdir().unwrap();
        let req = request(temp.path());
        let output = req.artifacts_dir.join("browser");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(output.join("extract-a.json"), "{}").unwrap();
        let events = temp.path().join("events.jsonl");
        std::fs::write(
            &events,
            "{\"operation\":\"extract\",\"status\":\"success\",\"artifact\":\"extract-a.json\"}\n",
        )
        .unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let emitter = LiveEmitter::with_observation_stop(CollectingSink::default(), stop);
        let sink = RecordingSink::default();
        let mut offset = 0;
        forward_events(&events, &mut offset, &req, &output, &sink, &emitter);
        {
            // 認証区間を開いて閉じても解除されない。
            let _auth = emitter.auth_section();
        }
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
        assert!(emitter.sink().events().is_empty());
        assert!(sink.progress.lock().unwrap().is_empty());
        assert!(sink.artifacts.lock().unwrap().is_empty());
        assert!(emitter.in_auth_section());
    }
}

/// ADR 2026-10-08 D2 fixtures: a task whose policy allows click but asks a human approval for it.
fn click_approval_request(workspace: &Path) -> RunRequest {
    let mut req = request(workspace);
    let policy = req.context.browser_policy.as_mut().unwrap();
    policy.allowed_actions = vec![
        BrowserAction::Navigate,
        BrowserAction::Snapshot,
        BrowserAction::Click,
        BrowserAction::Download,
    ];
    policy.approval_actions = vec![BrowserAction::Click];
    req
}

fn prepared(req: &RunRequest) -> crate::browser_policy::PreparedBrowserPolicy {
    crate::browser_policy::prepare_for_task(
        req.context
            .profile
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap(),
        &req.task,
        req.context.browser_policy.as_ref(),
        SUPPORTED_VERSION,
    )
    .unwrap()
}

fn approved_click_wait(policy: &crate::browser_policy::PreparedBrowserPolicy) -> BrowserWait {
    let now = time::OffsetDateTime::now_utc();
    BrowserWait {
        wait_id: "wait-1".into(),
        task_id: TaskId::new(),
        work_unit_id: None,
        run_id: "run-1".into(),
        session_id: "celeris-session-1".into(),
        reason: task_core::browser_wait::BrowserWaitReason::WaitingForApproval,
        origin: "https://example.com".into(),
        purpose: "Press export".into(),
        credential_policy_id: None,
        credential: None,
        operation: Some(task_core::browser_wait::OperationIntent {
            intent_id: "intent-1".into(),
            action: "click".into(),
            args_digest: None,
        }),
        trusted_login: None,
        approval_id: Some("approval-1".into()),
        policy_revision: policy.binding.revision,
        policy_hash: policy.binding.hash.clone(),
        owner_id: None,
        deadline: now,
        resume_key: "operation:x:run-1".into(),
        version: 2,
        state: BrowserWaitState::Approved,
        resolution_code: None,
        created_at: now,
        resolved_at: None,
    }
}

/// ADR 2026-10-08 D2: the shim's approval request is validated against the effective policy
/// before it becomes a wait, and an approved operation only resumes under the same policy.
#[test]
fn approval_request_is_bound_to_the_policy_and_resumes_only_under_the_same_policy() {
    let temp = tempfile::tempdir().unwrap();
    let req = click_approval_request(temp.path());
    let policy = prepared(&req);
    assert_eq!(
        policy.effective.operation_approval_actions(),
        vec![BrowserAction::Click]
    );
    assert_eq!(
        shim_approval_actions(&policy, None),
        vec!["click".to_string()]
    );
    assert!(shim_approval_actions(&policy, Some(BrowserAction::Click)).is_empty());
    let path = temp.path().join("approval-request.json");
    let write = |value: serde_json::Value| {
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, value.to_string()).unwrap();
    };
    let valid = serde_json::json!({
        "action": "click", "target": "@e7", "origin": "https://example.com", "purpose": "Press export"
    });
    write(valid.clone());
    let (action, intent) = read_approval_request(&path, &policy).unwrap();
    assert_eq!(action, BrowserAction::Click);
    let wait = operation_wait(req.task.id, "run-1", "celeris-s1", &policy, action, &intent);
    wait.validate().unwrap();
    assert_eq!(
        wait.reason,
        task_core::browser_wait::BrowserWaitReason::WaitingForApproval
    );
    assert!(wait.credential.is_none() && wait.credential_policy_id.is_none());
    let op = wait.operation.as_ref().unwrap();
    assert_eq!(op.action, "click");
    assert_eq!(
        op.args_digest.as_deref(),
        Some(format!("sha256:{:x}", Sha256::digest("click @e7")).as_str())
    );
    assert_eq!(wait.resume_key, format!("operation:{}:run-1", req.task.id));
    assert_eq!(wait.policy_hash, policy.binding.hash);

    // Denied: an action without approval, credential use, a bad ref, an origin outside the
    // effective domains, an invalid purpose, an unknown field.
    for (bad, code) in [
        (
            serde_json::json!({"action": "download", "target": "@e7", "origin": "https://example.com", "purpose": "p"}),
            "denied",
        ),
        (
            serde_json::json!({"action": "credential_use", "target": "@e7", "origin": "https://example.com", "purpose": "p"}),
            "denied",
        ),
        (
            serde_json::json!({"action": "click", "target": "e7", "origin": "https://example.com", "purpose": "p"}),
            "denied",
        ),
        (
            serde_json::json!({"action": "click", "target": "@e7", "origin": "https://evil.example", "purpose": "p"}),
            "denied",
        ),
        (
            serde_json::json!({"action": "click", "target": "@e7", "origin": "https://example.com/", "purpose": "p"}),
            "denied",
        ),
        (
            serde_json::json!({"action": "click", "target": "@e7", "origin": "https://example.com", "purpose": ""}),
            "denied",
        ),
        (
            serde_json::json!({"action": "click", "target": "@e7", "origin": "https://example.com", "purpose": "p", "selector": "#x"}),
            "invalid",
        ),
    ] {
        write(bad);
        let error = read_approval_request(&path, &policy)
            .unwrap_err()
            .to_string();
        assert!(error.contains(code), "{error}");
    }

    // Resume: the approved click is accepted only under the same policy hash/revision.
    let approved = approved_click_wait(&policy);
    assert_eq!(
        approved_operation(&approved, &policy).unwrap(),
        Some(BrowserAction::Click)
    );
    let mut other_hash = approved.clone();
    other_hash.policy_hash = "sha256:other".into();
    assert!(approved_operation(&other_hash, &policy).is_err());
    let mut download = approved.clone();
    download.operation.as_mut().unwrap().action = "download".into();
    assert!(approved_operation(&download, &policy).is_err());
    let mut with_credential = approved.clone();
    with_credential.credential = Some(task_core::browser_wait::CredentialRef {
        credential_id: "c".into(),
        provider: "manual".into(),
        policy_id: "pol".into(),
    });
    assert!(approved_operation(&with_credential, &policy).is_err());
    let mut credential_use = approved.clone();
    credential_use.operation.as_mut().unwrap().action = "credential_use".into();
    assert_eq!(approved_operation(&credential_use, &policy).unwrap(), None);
    // The resumed harness policy carries click once more; the launcher refuses nothing here.
    let resumed: task_core::AgentBrowserActionPolicy =
        serde_json::from_slice(&resumed_policy_bytes(&policy, BrowserAction::Click).unwrap())
            .unwrap();
    assert!(resumed.allow.iter().any(|a| a == "click"));
    let base: task_core::AgentBrowserActionPolicy =
        serde_json::from_slice(&policy.action_policy).unwrap();
    assert!(!base.allow.iter().any(|a| a == "click"));

    // The prompt explains both the request and the one approved operation.
    let context = BrowserContext {
        run: BrowserRun {
            task_id: req.task.id,
            run_id: "run-1".into(),
            session_id: "celeris-s1".into(),
            state: BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: PathBuf::from("/w/celeris-browser.py"),
        credential_used: false,
        approval_actions: vec!["click".into()],
        approved_operation: None,
    };
    let text = prompt(&context);
    assert!(text.contains("need a human approval before each use: click"));
    assert!(text.contains("request-approval <click|download> <@eN>"));
    let text = prompt(&BrowserContext {
        approval_actions: Vec::new(),
        approved_operation: Some(ApprovedOperation {
            action: "click".into(),
            origin: "https://example.com".into(),
            purpose: "Press\nexport".into(),
        }),
        ..context
    });
    assert!(text.contains("approved ONE `click` on https://example.com"));
    assert!(text.contains("purpose: Press export"));
    assert!(!text.contains("request-approval <click|download>"));
}

struct ApprovalSink {
    store: SqliteStore,
    task_id: TaskId,
    browsers: Mutex<Vec<BrowserRun>>,
    progress: Mutex<Vec<(String, ProgressFields)>>,
    contexts: Mutex<Vec<BrowserContext>>,
}
impl EventSink for ApprovalSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<std::sync::Arc<dyn crate::browser_live::ControlGate>> {
        Some(std::sync::Arc::new(crate::browser_live::InMemoryGate::new()))
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
    fn browser_operation_approval_consume(
        &self,
        wait: &BrowserWait,
    ) -> Result<task_core::browser_wait::ConsumedBrowserOperation, String> {
        task_core::browser_wait::consume_operation_approval(
            &self.store,
            self.task_id,
            wait,
            time::OffsetDateTime::now_utc(),
        )
        .map_err(String::from)
    }
}

/// Run 1: a click is refused with the approval hint, then `request-approval` stops the run.
/// Run 2 (after approval): one click reaches the browser, the second is blocked.
struct ApprovalHarness {
    contexts: std::sync::Arc<Mutex<Vec<BrowserContext>>>,
}
#[async_trait::async_trait]
impl WorkerAdapter for ApprovalHarness {
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
        let browser = req.context.browser.clone().expect("browser");
        self.contexts.lock().unwrap().push(browser.clone());
        let cli = &browser.cli;
        let commands: Vec<Vec<&str>> = if browser.approved_operation.is_none() {
            vec![
                vec!["open", "https://example.com/export"],
                vec!["click", "@e1"],
                vec![
                    "request-approval",
                    "click",
                    "@e1",
                    "https://example.com",
                    "Press export",
                ],
            ]
        } else {
            vec![
                vec!["open", "https://example.com/export"],
                vec!["click", "@e1"],
                vec!["click", "@e1"],
                vec!["scroll", "up", "10"],
            ]
        };
        let mut codes = Vec::new();
        let mut outputs = Vec::new();
        for args in commands {
            let output = tokio::process::Command::new("python3")
                .arg(cli)
                .args(args)
                .output()
                .await?;
            codes.push(output.status.code().unwrap_or(-1));
            outputs.push(String::from_utf8_lossy(&output.stdout).to_string());
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: format!("{codes:?}|{}", outputs.join("|")),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

/// ADR 2026-10-08 D2 (criterion 0): a click put back under approval opens a durable
/// `waiting_for_approval` wait with the operation intent, and the run resumes in the same
/// logical session after the human's one-time approval, running the click exactly once.
#[tokio::test]
async fn click_approval_opens_a_wait_and_resumes_the_same_session_once() {
    if crate::test_support::skip_unless_userns_tests() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let executable = substrate(temp.path());
    let mut req = click_approval_request(temp.path());
    req.task.status = Status::Ready;
    let task_id = req.task.id;
    let store = SqliteStore::open(&temp.path().join("celeris.db")).unwrap();
    store.insert(&req.task).unwrap();
    assert!(
        store
            .acquire_lease(task_id, "run-1", Duration::from_secs(600))
            .unwrap()
    );
    let contexts = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = ApprovalSink {
        store,
        task_id,
        browsers: Mutex::default(),
        progress: Mutex::default(),
        contexts: Mutex::default(),
    };
    let harness = std::sync::Arc::new(ApprovalHarness {
        contexts: contexts.clone(),
    });

    // Run 1: the click is refused with the approval hint; the request opens the wait.
    let outcome = run_with_executable(
        harness.clone(),
        req.clone(),
        "run-1",
        limits(),
        &sink,
        &executable,
        None,
    )
    .await
    .unwrap();
    let Terminal::Question { text } = outcome.terminal else {
        panic!("expected a question, got {:?}", outcome.terminal);
    };
    assert_eq!(text, "Browser click approval requested");
    let first = contexts.lock().unwrap()[0].clone();
    assert_eq!(first.approval_actions, vec!["click".to_string()]);
    assert!(first.approved_operation.is_none());
    let waits = sink.store.browser_waits_for_task(task_id).unwrap();
    assert_eq!(waits.len(), 1);
    let wait = &waits[0];
    assert_eq!(wait.state, BrowserWaitState::Pending);
    assert_eq!(
        wait.reason,
        task_core::browser_wait::BrowserWaitReason::WaitingForApproval
    );
    assert_eq!(wait.origin, "https://example.com");
    assert_eq!(wait.purpose, "Press export");
    assert!(wait.credential.is_none());
    let intent = wait.operation.as_ref().unwrap();
    assert_eq!(intent.action, "click");
    assert_eq!(
        intent.args_digest.as_deref(),
        Some(format!("sha256:{:x}", Sha256::digest("click @e1")).as_str())
    );
    assert_eq!(wait.session_id, first.run.session_id);
    assert_eq!(
        sink.store.get(task_id).unwrap().unwrap().status,
        Status::Blocked
    );
    assert_eq!(
        sink.browsers.lock().unwrap().last().unwrap().state,
        BrowserRunState::WaitingForApproval
    );
    // No click reached the substrate; the refusal and the request are audited.
    let calls =
        std::fs::read_to_string(temp.path().join("runs/run-1/browser/commands.jsonl")).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["[\"open\", \"https://example.com/export\"]", "[\"close\"]"]
    );
    let tools: Vec<String> = sink
        .progress
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, f)| f.tool.clone())
        .collect();
    assert!(tools.contains(&"browser.policy_block".to_string()));
    assert!(tools.contains(&"browser.approval_request".to_string()));

    // The human approves once (task → ready); the dispatcher leases a new run.
    let decided = sink
        .store
        .browser_wait_decide(
            task_id,
            &wait.wait_id,
            &task_core::browser_wait::HumanDecision {
                decision: task_core::browser_wait::BrowserDecision::ApproveOnce,
                expected_version: wait.version,
                actor_id: "owner".into(),
                owner_session_hash: "sess-hash".into(),
                policy_hash: wait.policy_hash.clone(),
                nonce: "n-1".into(),
                idempotency_key: "idem-1".into(),
            },
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
    assert_eq!(decided.task_status, Status::Ready);
    assert!(
        sink.store
            .acquire_lease(task_id, "run-2", Duration::from_secs(600))
            .unwrap()
    );

    // Run 2: the same logical session, the click runs once, the second click is refused by
    // the action server, and an action outside the policy is still refused by the shim.
    let outcome = run_with_executable(
        harness.clone(),
        req,
        "run-2",
        limits(),
        &sink,
        &executable,
        None,
    )
    .await
    .unwrap();
    let Terminal::Done { summary, .. } = outcome.terminal else {
        panic!("unexpected terminal {:?}", outcome.terminal);
    };
    let (codes, outputs) = summary.split_once('|').unwrap();
    assert_eq!(codes, "[0, 0, 1, 2]", "{outputs}");
    let second = contexts.lock().unwrap()[1].clone();
    assert_eq!(second.run.session_id, wait.session_id);
    assert!(second.approval_actions.is_empty());
    assert_eq!(
        second.approved_operation,
        Some(ApprovedOperation {
            action: "click".into(),
            origin: "https://example.com".into(),
            purpose: "Press export".into(),
        })
    );
    let calls =
        std::fs::read_to_string(temp.path().join("runs/run-2/browser/commands.jsonl")).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        [
            "[\"open\", \"https://example.com/export\"]",
            "[\"click\", \"@e1\"]",
            "[\"close\"]"
        ]
    );
    let policy: task_core::AgentBrowserActionPolicy = serde_json::from_slice(
        &std::fs::read(temp.path().join("runs/run-2/browser/policy.json")).unwrap(),
    )
    .unwrap();
    assert!(policy.allow.iter().any(|a| a == "click"));
    // download has no approval in this task policy and stays allowed; scroll is not granted.
    assert!(policy.allow.iter().any(|a| a == "download"));
    assert!(!policy.allow.iter().any(|a| a == "scroll"));
    let resumed = sink.store.browser_wait_get(&wait.wait_id).unwrap().unwrap();
    assert_eq!(resumed.state, BrowserWaitState::Resumed);
    assert_eq!(
        sink.browsers.lock().unwrap().last().unwrap().state,
        BrowserRunState::Completed
    );
    assert!(sink.contexts.lock().unwrap().is_empty());
}
