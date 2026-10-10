//! ADR-0116 D5: launcher 経由 runtime の daemon 側試験。偽 launcher は実の
//! [`LauncherServer`] に偽の backend を差したもの（テスト内の Unix socket server）。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::browser_launcher::protocol::AuthenticateArgs;
use crate::browser_launcher::{
    BackendSession, ErrorCode, Launched, LauncherLimits, LauncherServer, Registry, Response,
    ServerConfig, ServerHandle, SessionBackend, StartRequest,
};
use crate::browser_live::InMemoryGate;

/// 観測が返す map の外側（launcher の UID と subuid の先頭）。daemon（試験 process）と別の値。
const LAUNCHER_UID: u32 = 4_000_001;
const SUBUID: u32 = 5_000_000;

#[derive(Default)]
struct Log {
    actions: Vec<(Verb, ActionArgs)>,
    started: usize,
    stopped: usize,
    /// v8: the bytes a screenshot / download produces (`None`: no file; `Some(Err)`: the verb
    /// fails in the launcher, as a canceled other-origin download does).
    artifact_body: Option<Result<Vec<u8>, ()>>,
    /// 付記 2026-10-10j: the launcher's runner failed the verb and returns these tokens.
    failure_text: Option<String>,
}

struct FakeBackend {
    facts: SessionFacts,
    log: Arc<Mutex<Log>>,
    test_loopback: Vec<String>,
}

struct FakeSession {
    facts: SessionFacts,
    log: Arc<Mutex<Log>>,
    child: std::process::Child,
    /// the launcher session dir's `output` (files the fake "browser" wrote).
    output: tempfile::TempDir,
    produced: Vec<(String, Verb)>,
}

impl SessionBackend for FakeBackend {
    fn start(&self, _req: &StartRequest) -> Result<Launched, ErrorCode> {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep")
            .arg("600")
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|_| ErrorCode::LaunchFailed)?;
        let pid = child.id() as i32;
        let starttime =
            crate::browser_runtime::process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
        self.log.lock().expect("lock").started += 1;
        Ok(Launched {
            session: Box::new(FakeSession {
                facts: self.facts.clone(),
                log: Arc::clone(&self.log),
                child,
                output: tempfile::tempdir().map_err(|_| ErrorCode::LaunchFailed)?,
                produced: Vec::new(),
            }),
            pid,
            pgid: pid,
            starttime,
            runtime_pid: pid,
            runtime_starttime: starttime,
            ns_inodes: task_core::browser_isolation::collect_ns_inodes("self")
                .map_err(|_| ErrorCode::LaunchFailed)?,
        })
    }
    fn test_loopback_allow(&self) -> Vec<String> {
        self.test_loopback.clone()
    }
}

impl BackendSession for FakeSession {
    fn action(&mut self, verb: Verb, args: &ActionArgs) -> Result<Observation, ErrorCode> {
        self.log
            .lock()
            .expect("lock")
            .actions
            .push((verb, args.clone()));
        let mut artifact = None;
        if matches!(verb, Verb::Screenshot | Verb::Download)
            && let Some(text) = self.log.lock().expect("lock").failure_text.clone()
        {
            return Ok(Observation {
                text: Some(format!(
                    "{}{text}",
                    crate::browser_launcher::backend::ARTIFACT_FAILURE_PREFIX
                )),
                artifact: None,
            });
        }
        if matches!(verb, Verb::Screenshot | Verb::Download) {
            let body = self.log.lock().expect("lock").artifact_body.clone();
            match body {
                Some(Ok(bytes)) => {
                    let name = format!(
                        "{}-{}.{}",
                        if verb == Verb::Screenshot {
                            "screenshot"
                        } else {
                            "download"
                        },
                        crate::browser_launcher::random_id().expect("id"),
                        if verb == Verb::Screenshot {
                            "png"
                        } else {
                            "bin"
                        }
                    );
                    std::fs::write(self.output.path().join(&name), bytes).expect("write");
                    self.produced.push((name.clone(), verb));
                    artifact = Some(name);
                }
                Some(Err(())) => return Err(ErrorCode::BadRequest),
                None => {}
            }
        }
        Ok(Observation {
            text: Some(format!(
                r#"{{"success":true,"data":{{"verb":"{verb:?}"}}}}"#
            )),
            artifact,
        })
    }
    fn fetch_artifact(
        &mut self,
        name: &str,
        offset: u64,
    ) -> Result<crate::browser_launcher::ArtifactChunk, ErrorCode> {
        // The launcher backend's own reader (name, type, size checks).
        let verb = self
            .produced
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
            .ok_or(ErrorCode::Unauthorized)?;
        crate::browser_launcher::backend::read_artifact_chunk(
            self.output.path(),
            name,
            verb,
            offset,
        )
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        (SessionState::Running, self.facts.clone())
    }
    fn isolation_ok(&mut self) -> bool {
        true
    }
    fn auth_begin(&mut self, _auth_section_id: &str) -> Result<String, ErrorCode> {
        Ok("TARGET-FAKE".into())
    }
    fn authenticate(
        &mut self,
        args: &AuthenticateArgs,
        _broker: UnixStream,
    ) -> Result<crate::browser_launcher::protocol::LoginResult, ErrorCode> {
        if args.session_id.is_empty()
            || args.auth_section_id.is_empty()
            || args.credential_lease_id.is_empty()
            || args.login_url.is_empty()
        {
            return Err(ErrorCode::BadRequest);
        }
        Ok(crate::browser_launcher::protocol::LoginResult::HELD)
    }
    fn stop(mut self: Box<Self>) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.log.lock().expect("lock").stopped += 1;
    }
}

fn good_facts() -> SessionFacts {
    SessionFacts {
        host_uid: LAUNCHER_UID,
        host_gid: LAUNCHER_UID,
        uid_map: format!("0 {LAUNCHER_UID} 1\n1000 {SUBUID} 1\n"),
        gid_map: format!("0 {LAUNCHER_UID} 1\n1000 {SUBUID} 1\n"),
        ns_owner_uid: Some(LAUNCHER_UID),
        cap_eff: "0000000000000000".into(),
        no_new_privs: true,
        listen_count: 0,
    }
}

struct FakeLauncher {
    _dir: tempfile::TempDir,
    sock: PathBuf,
    log: Arc<Mutex<Log>>,
    _handle: ServerHandle,
}

fn fake_launcher(facts: SessionFacts) -> FakeLauncher {
    fake_launcher_with(facts, Vec::new())
}

fn fake_launcher_with(facts: SessionFacts, test_loopback: Vec<String>) -> FakeLauncher {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("launcher.sock");
    let registry = Registry::open(dir.path().join("state"), "inst-test").expect("registry");
    let log = Arc::new(Mutex::new(Log::default()));
    let server = LauncherServer::bind(
        &sock,
        ServerConfig {
            allowed_uids: vec![DaemonIds::current().uid],
            limits: LauncherLimits::default(),
        },
        Arc::new(FakeBackend {
            facts,
            log: Arc::clone(&log),
            test_loopback,
        }),
        registry,
    )
    .expect("bind");
    let handle = server.spawn().expect("spawn");
    FakeLauncher {
        _dir: dir,
        sock,
        log,
        _handle: handle,
    }
}

fn policy() -> SessionPolicy {
    session_policy(
        &["navigate".into(), "snapshot".into(), "close".into()],
        &["example.com".into()],
        Duration::from_secs(600),
    )
}

fn wait_stopped(log: &Mutex<Log>) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let n = log.lock().expect("lock").stopped;
        if n > 0 || std::time::Instant::now() > deadline {
            return n;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn launcher_runtime_starts_acts_observes_and_stops_through_the_client() {
    let launcher = fake_launcher(good_facts());
    let (runtime, attestation) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-1", policy()).expect("start");
    assert_eq!(attestation.runtime_uid(), SUBUID);
    assert_eq!(attestation.session_id(), runtime.session_id);
    let (receipt, observation) = runtime
        .action(
            Verb::Open,
            ActionArgs {
                url: Some("https://example.com/".into()),
                ..ActionArgs::default()
            },
        )
        .expect("action");
    assert!(receipt.isolation_ok);
    assert!(observation.text.expect("text").contains("Open"));
    let (state, facts) = runtime.observe().expect("observe");
    assert_eq!(state, SessionState::Running);
    assert_eq!(facts.ns_owner_uid, Some(LAUNCHER_UID));
    let stopped = runtime.stop().expect("stop").expect("receipt");
    assert_eq!(stopped.outcome, Outcome::Stopped);
    assert_eq!(runtime.stop().expect("second stop"), None);
    assert_eq!(wait_stopped(&launcher.log), 1);
    let log = launcher.log.lock().expect("lock");
    assert_eq!(log.actions.len(), 1);
    assert_eq!(
        log.actions[0].1.url.as_deref(),
        Some("https://example.com/")
    );
}

#[test]
fn launcher_credential_authenticate_fake_backend_returns_status_only() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-auth", policy()).expect("start");
    assert_eq!(
        runtime.protocol_version().expect("hello"),
        crate::browser_launcher::protocol::PROTOCOL_VERSION
    );
    assert_eq!(
        runtime.auth_begin("auth-1").expect("auth_begin"),
        "TARGET-FAKE"
    );
    let (broker, _peer) = UnixStream::pair().expect("pair");
    let status = runtime
        .authenticate(
            AuthenticateArgs {
                session_id: runtime.session_id.clone(),
                lease_id: runtime.lease_id.clone(),
                auth_section_id: "auth-1".into(),
                credential_lease_id: "credlease1".into(),
                origin: "https://example.com".into(),
                login_url: "https://example.com/login".into(),
                password_selector: "#password".into(),
                submit_selector: None,
                username_selector: None,
                post_login: None,
                report_held_reason: false,
                consent: None,
                report_consent_controls: false,
            },
            std::os::fd::OwnedFd::from(broker),
        )
        .expect("fake broker accepted authentication");
    assert_eq!(
        status,
        (
            AuthenticationStatus::Success,
            crate::browser_launcher::protocol::LoginResult::HELD
        )
    );
    runtime.stop().expect("stop");
}

#[test]
fn dropping_the_runtime_stops_the_launcher_session() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-2", policy()).expect("start");
    drop(runtime);
    assert_eq!(wait_stopped(&launcher.log), 1);
}

#[test]
fn unreachable_launcher_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = LauncherRuntime::start(&dir.path().join("missing.sock"), "t", "r", policy())
        .expect_err("no launcher");
    assert_eq!(err, UNAVAILABLE);
    // socket はあるが launcher ではない（応答せずに閉じる）場合も同じ。
    let sock = dir.path().join("mute.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    let mute = std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            drop(stream);
        }
    });
    let err = LauncherRuntime::start(&sock, "t", "r", policy()).expect_err("mute peer");
    assert_eq!(err, UNAVAILABLE);
    mute.join().expect("join");
}

#[test]
fn daemon_uid_in_the_namespace_fails_closed_and_stops_the_session() {
    let daemon = DaemonIds::current();
    // owner が daemon UID（ADR-0115 の漏洩の形）。
    let owned = SessionFacts {
        ns_owner_uid: Some(daemon.uid),
        ..good_facts()
    };
    let launcher = fake_launcher(owned);
    let err = LauncherRuntime::start(&launcher.sock, "t", "r", policy()).expect_err("owner");
    assert_eq!(err, UNAVAILABLE);
    assert_eq!(wait_stopped(&launcher.log), 1);
    // daemon UID が map の外側に現れる。
    let mapped = SessionFacts {
        uid_map: format!("0 {} 1\n1000 {SUBUID} 1\n", daemon.uid),
        ..good_facts()
    };
    let launcher = fake_launcher(mapped);
    let err = LauncherRuntime::start(&launcher.sock, "t", "r", policy()).expect_err("map");
    assert_eq!(err, UNAVAILABLE);
    assert_eq!(wait_stopped(&launcher.log), 1);
}

#[test]
fn runtime_facts_follow_the_observation() {
    let daemon = DaemonIds {
        uid: 1001,
        gid: 1001,
    };
    let facts = runtime_facts("s1", &daemon, &good_facts(), true).expect("facts");
    assert_eq!(facts.host_uid, 1001);
    assert_eq!(facts.runtime_uid, SUBUID);
    assert!(verify_isolation(&facts).is_ok());
    // launcher が隔離を証明しなければ daemon 側の検査も通らない。
    let unattested = runtime_facts("s1", &daemon, &good_facts(), false).expect("facts");
    assert!(verify_isolation(&unattested).is_err());
    // capability が残る・owner 不明は通らない。
    let caps = SessionFacts {
        cap_eff: "000001ffffffffff".into(),
        ..good_facts()
    };
    assert!(verify_isolation(&runtime_facts("s1", &daemon, &caps, true).expect("f")).is_err());
    // netns 内の proxy と shared-CDP relay は TCP を listen するが、Chrome CDP は pipe。
    let private_listeners = SessionFacts {
        listen_count: 2,
        ..good_facts()
    };
    let observed = runtime_facts("s1", &daemon, &private_listeners, true).expect("f");
    assert_eq!(observed.cdp, CdpEndpoint::Pipe);
    assert!(verify_isolation(&observed).is_ok());
    assert!(
        verify_isolation(&runtime_facts("s1", &daemon, &private_listeners, false).expect("f"))
            .is_err()
    );
    let unknown_owner = SessionFacts {
        ns_owner_uid: None,
        ..good_facts()
    };
    assert!(runtime_facts("s1", &daemon, &unknown_owner, true).is_none());
    let gid_leak = SessionFacts {
        gid_map: "0 1001 1\n1000 5000000 1\n".into(),
        ..good_facts()
    };
    assert!(runtime_facts("s1", &daemon, &gid_leak, true).is_none());
    assert!(
        runtime_facts(
            "s1",
            &daemon,
            &SessionFacts {
                uid_map: "garbage".into(),
                ..good_facts()
            },
            true
        )
        .is_none()
    );
}

#[test]
fn session_policy_maps_harness_actions_to_launcher_verbs() {
    let p = session_policy(
        &[
            "navigate".into(),
            "gettext".into(),
            "launch".into(),
            "scroll".into(),
            "navigate".into(),
        ],
        &["*.example.com".into()],
        Duration::from_secs(0),
    );
    assert_eq!(
        p.allowed_actions,
        vec![Verb::Open, Verb::Extract, Verb::Scroll]
    );
    assert_eq!(p.lease_seconds, 1);
    assert!(p.validate().is_ok());
}

/// shim と同じ形の要求を action socket に送る（`browser_cli.py` の接続と同じ）。
fn shim_request(sock: &std::path::Path, verb: &str, args: &[&str]) -> serde_json::Value {
    let mut stream = UnixStream::connect(sock).expect("connect");
    let req = serde_json::json!({"verb": verb, "args": args, "artifact": null});
    stream.write_all(req.to_string().as_bytes()).expect("write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    let mut out = String::new();
    stream.read_to_string(&mut out).expect("read");
    serde_json::from_str(&out).expect("json")
}

#[test]
fn harness_actions_reach_the_launcher_through_the_gated_action_server() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) = LauncherRuntime::start(
        &launcher.sock,
        "task-1",
        "run-3",
        session_policy(
            &["navigate".into(), "snapshot".into(), "scroll".into()],
            &["example.com".into()],
            Duration::from_secs(600),
        ),
    )
    .expect("start");
    let runtime = Arc::new(runtime);
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("browser.action.sock");
    let server = ActionServer::start_with(
        &sock,
        Arc::new(LauncherExecutor::new(
            Arc::clone(&runtime),
            std::path::PathBuf::from("/nonexistent"),
            crate::browser_launcher::PROTOCOL_VERSION,
        )),
        vec!["example.com".into()],
        vec!["navigate".into(), "snapshot".into(), "scroll".into()],
        Vec::new(),
        Arc::new(InMemoryGate::new()),
    )
    .expect("action server");
    let opened = shim_request(&sock, "open", &["https://example.com/a"]);
    assert_eq!(opened["status"], 0);
    let data: serde_json::Value =
        serde_json::from_str(opened["stdout"].as_str().expect("stdout")).expect("stdout json");
    assert_eq!(data["success"], true);
    assert_eq!(shim_request(&sock, "scroll", &["up", "300"])["status"], 0);
    // policy の外（domain・未許可 verb）は launcher に届かない。
    assert_eq!(
        shim_request(&sock, "open", &["https://evil.test/"])["status"],
        2
    );
    assert_eq!(shim_request(&sock, "click", &["@e1"])["status"], 2);
    drop(server);
    {
        let log = launcher.log.lock().expect("lock");
        assert_eq!(log.actions.len(), 2);
        assert_eq!(log.actions[0].0, Verb::Open);
        assert_eq!(
            log.actions[1],
            (
                Verb::Scroll,
                ActionArgs {
                    y: Some(-300),
                    ..ActionArgs::default()
                }
            )
        );
    }
    drop(runtime);
    assert_eq!(wait_stopped(&launcher.log), 1);
}

/// ADR 2026-10-08 D2: a human-approved action is handed to the browser once; the second
/// request is refused by the action server before it reaches the launcher.
#[test]
fn single_use_action_reaches_the_launcher_once() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) = LauncherRuntime::start(
        &launcher.sock,
        "task-1",
        "run-4",
        session_policy(
            &["navigate".into(), "snapshot".into(), "click".into()],
            &["example.com".into()],
            Duration::from_secs(600),
        ),
    )
    .expect("start");
    let runtime = Arc::new(runtime);
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("browser.action.sock");
    let server = ActionServer::start_with(
        &sock,
        Arc::new(LauncherExecutor::new(
            Arc::clone(&runtime),
            std::path::PathBuf::from("/nonexistent"),
            crate::browser_launcher::PROTOCOL_VERSION,
        )),
        vec!["example.com".into()],
        vec!["navigate".into(), "snapshot".into(), "click".into()],
        vec!["click".into()],
        Arc::new(InMemoryGate::new()),
    )
    .expect("action server");
    assert_eq!(
        shim_request(&sock, "open", &["https://example.com/a"])["status"],
        0
    );
    assert_eq!(shim_request(&sock, "click", &["@e1"])["status"], 0);
    assert_eq!(shim_request(&sock, "click", &["@e2"])["status"], 2);
    // Other actions stay available after the approval is spent.
    assert_eq!(shim_request(&sock, "snapshot", &[])["status"], 0);
    drop(server);
    let log = launcher.log.lock().expect("lock");
    assert_eq!(
        log.actions.iter().map(|(v, _)| *v).collect::<Vec<_>>(),
        vec![Verb::Open, Verb::Click, Verb::Snapshot]
    );
}

#[test]
fn launcher_executor_refuses_unknown_and_malformed_artifact_requests() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-4", policy()).expect("start");
    let out = tempfile::tempdir().expect("out");
    let exec = LauncherExecutor::new(
        Arc::new(runtime),
        out.path().to_path_buf(),
        crate::browser_launcher::PROTOCOL_VERSION,
    );
    // The supervisor's version probe has no launcher verb.
    let probe = ActionRequest {
        verb: "__version__".into(),
        args: vec![],
        artifact: None,
    };
    assert!(exec.run(1, &probe).is_err());
    // screenshot / download without the shim's generated name (or a path) never reach the launcher.
    for (verb, args, artifact) in [
        ("screenshot", vec![], None),
        ("download", vec!["@e1".to_string()], None),
        (
            "download",
            vec!["@e1".to_string()],
            Some("../download-x.bin"),
        ),
        (
            "screenshot",
            vec![],
            Some(format!("download-{}.bin", "a".repeat(32)).as_str()),
        ),
    ] {
        let req = ActionRequest {
            verb: verb.into(),
            args,
            artifact: artifact.map(str::to_owned),
        };
        let out = exec.run(1, &req).expect("answered");
        assert_eq!(out["status"], 1, "{verb}");
        assert_eq!(out["reason"], ARTIFACT_FAILED, "{verb}");
    }
    assert!(launcher.log.lock().expect("lock").actions.is_empty());
    assert_eq!(std::fs::read_dir(out.path()).expect("dir").count(), 0);
}

#[test]
fn default_runtime_is_the_daemon_path_and_launcher_skips_local_binaries() {
    assert_eq!(
        super::super::BrowserRuntimeKind::default(),
        super::super::BrowserRuntimeKind::Daemon
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = |runtime| super::super::IsolatedBrowserConfig {
        resolver: None,
        record_dir: dir.path().join("records"),
        bwrap: dir.path().join("missing-bwrap"),
        sandboxd: dir.path().join("missing-sandboxd"),
        egress: dir.path().join("missing-egress"),
        live_sessions: None,
        runtime,
    };
    // daemon 経路は従来どおり local の binary と resolver を要る。
    assert!(super::super::isolated_runtime_ready(Some(&cfg(Default::default()))).is_err());
    // launcher 経路の binary は launcher 側にあり、到達性は接続時に確かめる。
    let launcher = cfg(super::super::BrowserRuntimeKind::Launcher {
        socket: dir.path().join("launcher.sock"),
        refuse_test_loopback: true,
        launcher_uid: None,
    });
    assert!(super::super::isolated_runtime_ready(Some(&launcher)).is_ok());
}

// ---- ADR-0138 D-L: daemon 側で照合した launcher session 証明 ----

/// 独自の process group の `sleep`（runtime の leader の代わり）。
fn leader() -> (std::process::Child, i32, u64) {
    use std::os::unix::process::CommandExt;
    let child = std::process::Command::new("sleep")
        .arg("600")
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let pid = child.id() as i32;
    let st = crate::browser_runtime::process_starttime(pid).expect("starttime");
    (child, pid, st)
}

fn started(pid: i32, starttime: u64, owner: Option<u32>) -> StartedSession {
    StartedSession {
        session_id: "s1".into(),
        instance_id: "inst-1".into(),
        receipt: Receipt {
            session_id: "s1".into(),
            instance_id: "inst-1".into(),
            verb: None,
            outcome: Outcome::Started,
            at_unix_ms: 1,
            isolation_ok: true,
            binding: Some(crate::browser_launcher::SessionBinding {
                pid,
                starttime,
                ns_owner_uid: owner,
                ns_inodes: task_core::browser_isolation::collect_ns_inodes("self")
                    .expect("launcher-reported ns inodes"),
            }),
        },
    }
}

fn reap(mut child: std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn launcher_proof_is_built_only_when_pid_starttime_and_owner_match() {
    let daemon = DaemonIds::current();
    let (child, pid, st) = leader();
    let proof = launcher_session_proof(
        &started(pid, st, Some(LAUNCHER_UID)),
        Some(LAUNCHER_UID),
        &daemon,
    )
    .expect("proof");
    assert_eq!(
        proof,
        LauncherSessionProof {
            session_id: "s1".into(),
            instance_id: "inst-1".into(),
            pid,
            starttime: st,
            ns_owner_uid: Some(LAUNCHER_UID),
            launcher_uid: LAUNCHER_UID,
            isolation_ok: true,
            ns_inodes: task_core::browser_isolation::collect_ns_inodes("self").expect("ns inodes"),
        }
    );
    reap(child);
}

fn test_credential_proof() -> LauncherSessionProof {
    LauncherSessionProof {
        session_id: "credential-session".into(),
        instance_id: "credential-instance".into(),
        pid: 42,
        starttime: 99,
        ns_owner_uid: Some(LAUNCHER_UID),
        launcher_uid: LAUNCHER_UID,
        isolation_ok: true,
        ns_inodes: REQUIRED_NAMESPACES.iter().map(|ns| (*ns, 1)).collect(),
    }
}

#[test]
fn launcher_credential_admits_proven_isolated_session() {
    assert!(credential_admitted(
        true,
        Some(&test_credential_proof()),
        true,
        1000
    ));
}

#[test]
fn launcher_credential_rejects_missing_proof() {
    assert!(!credential_admitted(true, None, true, 1000));
}

#[test]
fn launcher_credential_rejects_forged_isolation_claim() {
    let mut proof = test_credential_proof();
    proof.isolation_ok = false;
    assert!(!credential_admitted(true, Some(&proof), true, 1000));
}

#[test]
fn launcher_credential_rejects_expired_process_proof() {
    let (child, pid, st) = leader();
    let mut proof = test_credential_proof();
    proof.pid = pid;
    proof.starttime = st + 1;
    // The same live-process/starttime verifier used when creating proofs rejects stale bindings.
    assert!(live_starttime(pid).is_some_and(|actual| actual != proof.starttime));
    assert!(!credential_admitted(true, None, true, 1000));
    reap(child);
}

#[test]
fn launcher_credential_rejects_daemon_namespace_owner() {
    let mut proof = test_credential_proof();
    proof.ns_owner_uid = Some(1000);
    assert!(!credential_admitted(true, Some(&proof), true, 1000));
}

#[test]
fn launcher_credential_rejects_failed_isolation_attestation() {
    assert!(!credential_admitted(
        true,
        Some(&test_credential_proof()),
        false,
        1000
    ));
}

#[test]
fn launcher_credential_rejects_missing_namespace_binding() {
    let mut proof = test_credential_proof();
    proof.ns_inodes.clear();
    assert!(!credential_admitted(true, Some(&proof), true, 1000));
}

#[test]
fn launcher_credential_requires_a_credential_request() {
    assert!(!credential_admitted(
        false,
        Some(&test_credential_proof()),
        true,
        1000
    ));
}

#[test]
fn launcher_credential_rejects_uid_mismatch() {
    let mut proof = test_credential_proof();
    proof.ns_owner_uid = Some(1001);
    assert!(credential_admitted(true, Some(&proof), true, 1000));
    assert!(!credential_admitted(true, Some(&proof), true, 1001));
}

#[test]
fn launcher_credential_shim_flags_follow_admission() {
    let policy = prepared("example.com");
    for admitted in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = dir.path().join("browser");
        write_shim_files(&runtime, "s", &policy, &policy.action_policy, &[], admitted)
            .expect("shim");
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(runtime.join("config.json")).expect("config"))
                .expect("json");
        assert_eq!(config["credential_use"], admitted);
        assert_eq!(
            config["credential_policy_ids"]
                .as_array()
                .unwrap()
                .is_empty(),
            !admitted || policy.effective.credential_policy_ids.is_empty()
        );
    }
}

#[test]
fn launcher_proof_is_not_built_on_starttime_mismatch() {
    let daemon = DaemonIds::current();
    let (child, pid, st) = leader();
    // PID 再利用・process 入替えの形: pid は生きているが starttime が束縛と違う。
    let s = started(pid, st + 1, Some(LAUNCHER_UID));
    assert_eq!(
        launcher_session_proof(&s, Some(LAUNCHER_UID), &daemon),
        None
    );
    reap(child);
}

#[test]
fn launcher_proof_is_not_built_when_the_pid_is_gone() {
    let daemon = DaemonIds::current();
    let (child, pid, st) = leader();
    reap(child);
    let s = started(pid, st, Some(LAUNCHER_UID));
    assert_eq!(
        launcher_session_proof(&s, Some(LAUNCHER_UID), &daemon),
        None
    );
    // pid 0・1 は束縛として受けない。
    let init = started(1, st, Some(LAUNCHER_UID));
    assert_eq!(
        launcher_session_proof(&init, Some(LAUNCHER_UID), &daemon),
        None
    );
}

#[test]
fn launcher_proof_is_not_built_when_owner_is_daemon_or_unknown() {
    let daemon = DaemonIds::current();
    let (child, pid, st) = leader();
    let owned = started(pid, st, Some(daemon.uid));
    assert_eq!(
        launcher_session_proof(&owned, Some(LAUNCHER_UID), &daemon),
        None
    );
    let unknown = started(pid, st, None);
    assert_eq!(
        launcher_session_proof(&unknown, Some(LAUNCHER_UID), &daemon),
        None
    );
    reap(child);
}

#[test]
fn launcher_proof_is_not_built_without_peer_uid_binding_or_isolation() {
    let daemon = DaemonIds::current();
    let (child, pid, st) = leader();
    let good = started(pid, st, Some(LAUNCHER_UID));
    assert!(launcher_session_proof(&good, Some(LAUNCHER_UID), &daemon).is_some());
    // 応答の送り手（SCM_CREDENTIALS）が採れない。
    assert_eq!(launcher_session_proof(&good, None, &daemon), None);
    // v1 の launcher（束縛の無い receipt）。
    let mut v1 = good.clone();
    v1.receipt.binding = None;
    assert_eq!(
        launcher_session_proof(&v1, Some(LAUNCHER_UID), &daemon),
        None
    );
    // v2 の launcher（束縛に namespace の inode が無い）。
    let mut v2 = good.clone();
    if let Some(b) = v2.receipt.binding.as_mut() {
        b.ns_inodes.clear();
    }
    assert_eq!(
        launcher_session_proof(&v2, Some(LAUNCHER_UID), &daemon),
        None
    );
    // inode が 1 つ欠けた束縛。
    let mut partial = good.clone();
    if let Some(b) = partial.receipt.binding.as_mut() {
        b.ns_inodes
            .remove(&task_core::browser_isolation::Namespace::Net);
    }
    assert_eq!(
        launcher_session_proof(&partial, Some(LAUNCHER_UID), &daemon),
        None
    );
    // launcher 自身の隔離検査が偽。
    let mut bad = good.clone();
    bad.receipt.isolation_ok = false;
    assert_eq!(
        launcher_session_proof(&bad, Some(LAUNCHER_UID), &daemon),
        None
    );
    // receipt と応答の instance が食い違う。
    let mut other = good.clone();
    other.receipt.instance_id = "inst-2".into();
    assert_eq!(
        launcher_session_proof(&other, Some(LAUNCHER_UID), &daemon),
        None
    );
    reap(child);
}

#[test]
fn v1_receipt_without_binding_decodes_as_no_proof() {
    let body = br#"{"type":"started","session_id":"s1","instance_id":"i1","receipt":{"session_id":"s1","instance_id":"i1","outcome":"started","at_unix_ms":1,"isolation_ok":true}}"#;
    let resp: crate::browser_launcher::Response = serde_json::from_slice(body).expect("v1");
    let crate::browser_launcher::Response::Started { receipt, .. } = resp else {
        panic!("started");
    };
    assert_eq!(receipt.binding, None);
}

#[test]
fn launcher_runtime_carries_the_launcher_binding_into_the_proof() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-p", policy()).expect("start");
    let proof = runtime.session_proof().expect("proof").clone();
    assert_eq!(proof.session_id, runtime.session_id);
    assert_eq!(proof.instance_id, "inst-test");
    assert_eq!(proof.ns_owner_uid, Some(LAUNCHER_UID));
    // 偽 launcher は試験 process 自身なので応答の送り手（SCM_CREDENTIALS）は daemon の UID。
    let daemon = DaemonIds::current();
    assert_eq!(proof.launcher_uid, daemon.uid);
    assert!(crate::browser_runtime::same_process_alive(
        proof.pid,
        proof.starttime
    ));
    // 証明を持つだけでは本番 admission は通らない（launcher UID が daemon UID）。
    let facts = runtime_facts(&runtime.session_id, &daemon, &good_facts(), true).expect("facts");
    let seen = task_core::browser_isolation::LauncherObservation {
        session_id: runtime.session_id.clone(),
        instance_id: proof.instance_id.clone(),
        peer_uid: Some(daemon.uid),
        configured_launcher_uid: daemon.uid,
        runtime_pid: proof.pid,
        runtime_starttime: Some(proof.starttime),
    };
    let err = task_core::browser_isolation::verify_launcher_session(&facts, Some(&proof), &seen)
        .expect_err("privileged launcher uid");
    assert!(err.contains(
        &task_core::browser_isolation::IsolationViolation::LauncherProofInvalid {
            defect: task_core::browser_isolation::LauncherProofDefect::LauncherUidPrivileged,
        }
    ));
    drop(runtime);
    assert_eq!(wait_stopped(&launcher.log), 1);
}

// ---- ADR 2026-10-06-browser-action-socket-path: action socket の path と shim の設定 ----

/// 本番相当の深さ（`/local/celeris/data/workspaces/<task>/runs/<run>/browser-fallback-N`）。
/// 旧来の `<session dir>.action.sock` はこれで 107 byte を超えた。
fn deep_session_dir() -> PathBuf {
    PathBuf::from(
        "/local/celeris/data/workspaces/01M47QXZR0QMCYZM9KAZC81BCD/wu/run-path/repos/agent-platform\
         /01M47NP5QD1DHHHADJ84BG9HKH/runs/01M47NP5VDFZ4K4123QRBC0V8H/browser-fallback-2",
    )
}

#[test]
fn action_socket_path_is_short_and_fixed_length_for_a_deep_workspace() {
    let deep = deep_session_dir();
    assert!(
        deep.with_extension("action.sock").as_os_str().len() > crate::browser_action::SUN_PATH_MAX
    );
    let a = crate::browser_action::action_socket_path(&deep).expect("socket path");
    let b = crate::browser_action::action_socket_path(&deep.with_file_name("browser"))
        .expect("socket path");
    assert!(
        a.as_os_str().len() <= crate::browser_action::SUN_PATH_MAX,
        "{a:?}"
    );
    assert_eq!(a.as_os_str().len(), b.as_os_str().len());
    assert_ne!(a, b, "session dir ごとに別の socket");
    assert_eq!(
        a,
        crate::browser_action::action_socket_path(&deep).expect("again"),
        "同じ session dir なら同じ path"
    );
    // 実際に bind できる（sun_path に収まる）。
    let listener = std::os::unix::net::UnixListener::bind(&a).expect("bind short socket");
    drop(listener);
    // 前の run の socket が残っていても次は同じ path を使える。
    assert_eq!(
        crate::browser_action::action_socket_path(&deep).expect("stale removed"),
        a
    );
    assert!(!a.exists());
}

#[test]
fn action_socket_path_over_the_limit_fails_before_bind_with_path_and_length() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("b".repeat(100));
    let err = crate::browser_action::action_socket_path_in(&base, &deep_session_dir())
        .expect_err("too long");
    let AdapterError::Other(message) = err else {
        panic!("unexpected error kind");
    };
    let expected = base.join(format!(
        "celeris-browser-{}",
        nix::unistd::geteuid().as_raw()
    ));
    assert!(
        message.contains(&expected.display().to_string()),
        "{message}"
    );
    assert!(message.contains("bytes"), "{message}");
    assert!(message.contains("107"), "{message}");
    assert!(!base.exists(), "長すぎる path では何も作らない");
}

#[test]
fn action_socket_dir_must_be_private_to_this_user() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let sockets = dir.path().join(format!(
        "celeris-browser-{}",
        nix::unistd::geteuid().as_raw()
    ));
    std::fs::create_dir(&sockets).expect("mkdir");
    std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o777)).expect("chmod");
    assert!(crate::browser_action::action_socket_path_in(dir.path(), &deep_session_dir()).is_err());
    std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(crate::browser_action::action_socket_path_in(dir.path(), &deep_session_dir()).is_ok());
}

fn prepared(origin: &str) -> crate::browser_policy::PreparedBrowserPolicy {
    prepared_with(
        origin,
        vec![
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Snapshot,
        ],
    )
}

fn prepared_with(
    origin: &str,
    allowed_actions: Vec<task_core::BrowserAction>,
) -> crate::browser_policy::PreparedBrowserPolicy {
    let grant = task_core::BrowserCapability {
        approval_actions: vec![],
        allowed_domains: vec![origin.into()],
        ..Default::default()
    };
    let task = task_core::BrowserTaskPolicy {
        policy_id: "launcher-shim".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec![origin.into()],
        allowed_actions,
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    };
    crate::browser_policy::prepare(&grant, Some(&task), super::super::SUPPORTED_VERSION)
        .expect("policy")
}

fn run_shim(cli: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("python3")
        .arg(cli)
        .args(args)
        .output()
        .expect("python3")
}

/// D6: launcher 経路が書く config.json/policy.json を実の shim（`browser_cli.py`）が受け入れ、
/// 許可 origin の navigate は偽 launcher に Open として届き、不許可 origin は拒否される。
#[test]
fn launcher_shim_files_pass_load_policy_and_gate_navigation_by_origin() {
    for (n, (origin, allowed, denied)) in [
        ("example.com", "https://example.com/a", "https://evil.test/"),
        (
            "http://127.0.0.1:17730",
            "http://127.0.0.1:17730/",
            "http://127.0.0.1:17731/",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let policy = prepared(origin);
        let launcher = fake_launcher(good_facts());
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime_dir = dir
            .path()
            .join("runs")
            .join(format!("run-{n}"))
            .join("browser");
        let (cli, action_socket) = write_shim_files(
            &runtime_dir,
            "celeris-test-session",
            &policy,
            &policy.action_policy,
            &[],
            false,
        )
        .expect("shim files");
        assert!(action_socket.as_os_str().len() <= crate::browser_action::SUN_PATH_MAX);

        let config: serde_json::Value = serde_json::from_slice(
            &std::fs::read(runtime_dir.join("config.json")).expect("config"),
        )
        .expect("config json");
        let policy_bytes = std::fs::read(runtime_dir.join("policy.json")).expect("policy");
        assert_eq!(
            config["policy_sha256"],
            format!(
                "{:x}",
                <sha2::Sha256 as sha2::Digest>::digest(&policy_bytes)
            )
        );
        assert_eq!(config["action_socket"], action_socket.display().to_string());
        let file: task_core::AgentBrowserActionPolicy =
            serde_json::from_slice(&policy_bytes).expect("policy json");
        assert!(file.allow.iter().any(|a| a == "launch"), "{:?}", file.allow);
        // shim の load_policy そのものに通す。
        let check = std::process::Command::new("python3")
            .arg("-c")
            .arg(
                "import importlib.util,json,sys,pathlib\n\
                 spec=importlib.util.spec_from_file_location('cli',sys.argv[1])\n\
                 cli=importlib.util.module_from_spec(spec);spec.loader.exec_module(cli)\n\
                 c=json.loads((pathlib.Path(sys.argv[1]).parent/'config.json').read_text())\n\
                 sys.exit(0 if cli.load_policy(c) is not None else 3)",
            )
            .arg(&cli)
            .output()
            .expect("python3");
        assert!(
            check.status.success(),
            "load_policy rejected the launcher files: {}",
            String::from_utf8_lossy(&check.stderr)
        );

        let allowed_actions: task_core::AgentBrowserActionPolicy =
            serde_json::from_slice(&policy.action_policy).expect("allow");
        let (runtime, _) = LauncherRuntime::start(
            &launcher.sock,
            "task-1",
            &format!("run-shim-{n}"),
            session_policy(
                &allowed_actions.allow,
                policy.allowed_domains(),
                Duration::from_secs(600),
            ),
        )
        .expect("start");
        let runtime = Arc::new(runtime);
        let server = ActionServer::start_with(
            &action_socket,
            Arc::new(LauncherExecutor::new(
                Arc::clone(&runtime),
                std::path::PathBuf::from("/nonexistent"),
                crate::browser_launcher::PROTOCOL_VERSION,
            )),
            policy.allowed_domains().to_vec(),
            allowed_actions.allow.clone(),
            Vec::new(),
            Arc::new(InMemoryGate::new()),
        )
        .expect("action server");

        let opened = run_shim(&cli, &["open", allowed]);
        assert!(
            opened.status.success(),
            "{origin}: {}",
            String::from_utf8_lossy(&opened.stdout)
        );
        let blocked = run_shim(&cli, &["open", denied]);
        assert_eq!(blocked.status.code(), Some(2), "{origin}");
        assert!(
            String::from_utf8_lossy(&blocked.stdout).contains("not permitted"),
            "{}",
            String::from_utf8_lossy(&blocked.stdout)
        );
        // shim を経ずに socket へ直接来た不許可 origin も daemon の検査で止まる。
        assert_eq!(shim_request(&action_socket, "open", &[denied])["status"], 2);
        drop(server);
        assert!(!action_socket.exists());
        {
            let log = launcher.log.lock().expect("lock");
            assert_eq!(log.actions.len(), 1, "{origin}");
            assert_eq!(log.actions[0].0, Verb::Open);
        }
        drop(runtime);
        assert_eq!(wait_stopped(&launcher.log), 1);
    }
}

// ---- 付記 E2（daemon 側）: 本番の daemon は試験専用 loopback 許可の launcher を使わない ----

#[test]
fn egress_test_loopback_production_daemon_refuses_test_launcher() {
    let test_launcher = fake_launcher_with(good_facts(), vec!["127.0.0.1:18080".into()]);
    // 本番（または判定不能）の daemon: hello で申告を見て、session を作らずに拒否する。
    let err = LauncherRuntime::start_guarded(&test_launcher.sock, true, "t", "r", policy())
        .expect_err("production daemon must refuse");
    assert_eq!(err, UNAVAILABLE);
    assert_eq!(test_launcher.log.lock().expect("lock").started, 0);
    // 本番でない daemon は同じ launcher を使える。
    let (runtime, _) =
        LauncherRuntime::start_guarded(&test_launcher.sock, false, "t", "r", policy())
            .expect("non-production daemon");
    drop(runtime);
    assert_eq!(test_launcher.log.lock().expect("lock").started, 1);

    // 試験許可の無い（既定の）launcher は本番の daemon でも使える。
    let plain = fake_launcher(good_facts());
    let (runtime, _) = LauncherRuntime::start_guarded(&plain.sock, true, "t", "r", policy())
        .expect("default launcher");
    drop(runtime);
}

#[test]
fn egress_test_loopback_production_daemon_refuses_launcher_without_hello() {
    use crate::browser_launcher::Response;
    use crate::browser_launcher::protocol::{read_frame, write_message};
    // hello を知らない（旧版の）launcher: 未知の要求に bad_request を返して閉じる。
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("old.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let body = read_frame(&mut stream, 64 * 1024).expect("frame");
        let req: serde_json::Value = serde_json::from_slice(&body).expect("json");
        write_message(
            &mut stream,
            &Response::Error {
                code: ErrorCode::BadRequest,
            },
            64 * 1024,
        )
        .expect("write");
        req["type"].as_str().map(str::to_owned)
    });
    let err = LauncherRuntime::start_guarded(&sock, true, "t", "r", policy())
        .expect_err("unanswered hello must refuse");
    assert_eq!(err, UNAVAILABLE);
    assert_eq!(server.join().expect("join").as_deref(), Some("hello"));
}

// ---- ADR 2026-10-09 付記「接続前 gate」: 二段 gate ----

const CREDENTIAL_DENIED_TEXT: &str =
    "browser credential use is not available through the launcher runtime";

fn credential_run() -> CredentialDemand {
    CredentialDemand {
        requested: true,
        refused_without_admission: true,
    }
}

/// 接続を数えるだけの Unix socket（応答しない）。nonblocking の accept が WouldBlock なら接続 0。
fn counting_listener(path: &std::path::Path) -> std::os::unix::net::UnixListener {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    let listener = std::os::unix::net::UnixListener::bind(path).expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    listener
}

fn connections(listener: &std::os::unix::net::UnixListener) -> usize {
    let mut n = 0;
    while listener.accept().is_ok() {
        n += 1;
    }
    n
}

fn denied_text(err: AdapterError) -> String {
    match err {
        AdapterError::Other(text) => text,
        other => format!("{other:?}"),
    }
}

/// 接続前に決まる条件（launcher UID 未設定・daemon UID と同一・0・credentiald 経路なし・launcher
/// socket 未設定）がどれか欠ければ、launcher socket に 1 度も接続せず従来の文言で拒否する。
#[tokio::test]
async fn launcher_credential_preconnect_unmet_conditions_refuse_without_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("launcher.sock");
    let listener = counting_listener(&sock);
    let credd = dir.path().join("credd-runtime");
    let daemon_uid = DaemonIds::current().uid;
    let empty = PathBuf::new();
    let cases: [(&std::path::Path, Option<u32>, Option<&std::path::Path>); 5] = [
        (&sock, None, Some(&credd)),
        (&sock, Some(daemon_uid), Some(&credd)),
        (&sock, Some(0), Some(&credd)),
        (&sock, Some(LAUNCHER_UID), None),
        (&empty, Some(LAUNCHER_UID), Some(&credd)),
    ];
    for (socket, launcher_uid, credd_runtime) in cases {
        let err = open_launcher_session(
            LauncherTarget {
                socket,
                refuse_test_loopback: false,
                launcher_uid,
            },
            "task",
            "run",
            policy(),
            credential_run(),
            credd_runtime,
        )
        .await
        .expect_err("refused before connecting");
        assert_eq!(denied_text(err), CREDENTIAL_DENIED_TEXT);
    }
    assert_eq!(connections(&listener), 0, "no launcher connection");
}

/// admission の対象でないが admission 無しでは拒否される run（credential 以外の登録済み wait 等）も、
/// 接続後に必ず拒否されるので接続前に拒否する。
#[tokio::test]
async fn launcher_credential_preconnect_unadmittable_wait_refuses_without_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("launcher.sock");
    let listener = counting_listener(&sock);
    let credd = dir.path().join("credd-runtime");
    let err = open_launcher_session(
        LauncherTarget {
            socket: &sock,
            refuse_test_loopback: false,
            launcher_uid: Some(LAUNCHER_UID),
        },
        "task",
        "run",
        policy(),
        CredentialDemand {
            requested: false,
            refused_without_admission: true,
        },
        Some(&credd),
    )
    .await
    .expect_err("refused");
    assert_eq!(denied_text(err), CREDENTIAL_DENIED_TEXT);
    assert_eq!(connections(&listener), 0);
}

/// 接続前条件が揃えば従来どおり launcher に接続して session を作る。その後の証明依存の不成立
/// （ここでは証明の launcher UID が設定値と違う）は session を stop してから従来の文言で拒否し、
/// credentiald の制御 socket には 1 度も接続しない（登録が起きない）。
#[tokio::test]
async fn launcher_credential_preconnect_proof_failure_stops_session_before_credentiald() {
    let launcher = fake_launcher(good_facts());
    let dir = tempfile::tempdir().expect("tempdir");
    let control = celeris_credentiald::injection_ipc::injection_socket(dir.path())
        .with_file_name("control.sock");
    let credd = counting_listener(&control);
    let err = open_launcher_session(
        LauncherTarget {
            socket: &launcher.sock,
            refuse_test_loopback: false,
            launcher_uid: Some(LAUNCHER_UID),
        },
        "task",
        "run",
        policy(),
        credential_run(),
        Some(dir.path()),
    )
    .await
    .expect_err("proof-dependent refusal");
    assert_eq!(denied_text(err), CREDENTIAL_DENIED_TEXT);
    assert_eq!(launcher.log.lock().expect("lock").started, 1, "connected");
    assert_eq!(wait_stopped(&launcher.log), 1, "session stopped");
    assert!(launcher.log.lock().expect("lock").actions.is_empty());
    assert_eq!(connections(&credd), 0, "no credentiald registration");
}

/// 接続前条件が揃った CredentialUse run は launcher に接続する（session が作られる）。
#[tokio::test]
async fn launcher_credential_preconnect_met_conditions_connect_to_the_launcher() {
    let launcher = fake_launcher(good_facts());
    let dir = tempfile::tempdir().expect("tempdir");
    let _ = open_launcher_session(
        LauncherTarget {
            socket: &launcher.sock,
            refuse_test_loopback: false,
            launcher_uid: Some(LAUNCHER_UID),
        },
        "task",
        "run",
        policy(),
        credential_run(),
        Some(dir.path()),
    )
    .await;
    assert_eq!(launcher.log.lock().expect("lock").started, 1);
}

/// CredentialUse を求めない run は接続前 gate を素通しし、launcher UID・credentiald 経路の設定が
/// 無くても従来どおり session を開く（admission は偽）。
#[tokio::test]
async fn launcher_credential_preconnect_non_credential_run_is_unchanged() {
    let launcher = fake_launcher(good_facts());
    let (runtime, admitted) = open_launcher_session(
        LauncherTarget {
            socket: &launcher.sock,
            refuse_test_loopback: false,
            launcher_uid: None,
        },
        "task",
        "run",
        policy(),
        CredentialDemand::default(),
        None,
    )
    .await
    .expect("session");
    assert!(!admitted);
    assert_eq!(launcher.log.lock().expect("lock").started, 1);
    drop(runtime);
    assert_eq!(wait_stopped(&launcher.log), 1);
}

#[test]
fn launcher_credential_preconnect_demand_follows_policy() {
    let policy = prepared("example.com");
    assert_eq!(credential_demand(&policy, &[]), CredentialDemand::default());
}

// ADR 2026-10-09 付記「launcher runtime の credential wait」: launcher 経路
// （`browser_launcher_run.rs::run`）が run 後に呼ぶ共有段 `browser.rs::shim_request_wait` の試験。
// 偽 sink と tempdir の request file だけを使う（userns・実 launcher・実 process・ネットワーク不要）。

use task_core::browser_wait::{BrowserWaitReason, NewBrowserWait};

/// 開かれた wait・browser 更新・progress を記録するだけの偽 sink。`open_ok = false` で
/// 「wait を開けない」場合の fail closed を見る。
struct RecordingWaitSink {
    waits: Mutex<Vec<NewBrowserWait>>,
    browsers: Mutex<Vec<BrowserRun>>,
    events: Mutex<Vec<String>>,
    open_ok: bool,
}

impl RecordingWaitSink {
    fn new(open_ok: bool) -> Self {
        Self {
            waits: Mutex::new(Vec::new()),
            browsers: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            open_ok,
        }
    }
    fn opened(&self) -> Vec<NewBrowserWait> {
        self.waits.lock().expect("lock").clone()
    }
    /// sink に渡った全部（wait・browser 更新・progress）の serialize。秘密が混ざらないことの検査に使う。
    fn recorded(&self) -> String {
        let mut text = serde_json::to_string(&self.opened()).expect("waits");
        text.push_str(
            &serde_json::to_string(&self.browsers.lock().expect("lock").clone()).expect("browsers"),
        );
        text.push_str(&self.events.lock().expect("lock").join("\n"));
        text
    }
}

impl EventSink for RecordingWaitSink {
    fn browser_wait_open(&self, request: &NewBrowserWait) -> Result<(), String> {
        if !self.open_ok {
            return Err("browser wait store unavailable".into());
        }
        self.waits.lock().expect("lock").push(request.clone());
        Ok(())
    }
    fn browser_updated(&self, browser: &BrowserRun) {
        self.browsers.lock().expect("lock").push(browser.clone());
    }
    fn progress(&self, msg: &str) {
        self.events.lock().expect("lock").push(msg.to_owned());
    }
    fn artifact(&self, artifact: &task_core::ArtifactRef) {
        self.events
            .lock()
            .expect("lock")
            .push(format!("{artifact:?}"));
    }
}

/// shim の policy: `origin` だけを許可し、`actions` と approval 対象を呼び手が決める。
/// credential policy は `pol-example` 一つ（`CredentialUse` を入れたときだけ意味を持つ）。
fn shim_policy(
    origin: &str,
    actions: &[task_core::BrowserAction],
    approval: &[task_core::BrowserAction],
) -> crate::browser_policy::PreparedBrowserPolicy {
    let grant = task_core::BrowserCapability {
        allowed_domains: vec![origin.into()],
        allowed_actions: Some(actions.to_vec()),
        approval_actions: approval.to_vec(),
        credential_policy_ids: vec!["pol-example".into()],
        ..Default::default()
    };
    let task = task_core::BrowserTaskPolicy {
        policy_id: "launcher-shim".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec![origin.into()],
        allowed_actions: actions.to_vec(),
        approval_actions: approval.to_vec(),
        credential_policy_ids: vec!["pol-example".into()],
        artifact_policy_id: None,
    };
    crate::browser_policy::prepare(&grant, Some(&task), super::super::SUPPORTED_VERSION)
        .expect("policy")
}

/// CredentialUse を許可した policy（launcher 経路の credential run）。
fn credential_policy() -> crate::browser_policy::PreparedBrowserPolicy {
    shim_policy(
        "example.com",
        &[
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Snapshot,
            task_core::BrowserAction::CredentialUse,
        ],
        &[],
    )
}

fn write_request(runtime: &Path, name: &str, request: &serde_json::Value) {
    std::fs::create_dir_all(runtime).expect("mkdir");
    std::fs::write(
        runtime.join(name),
        serde_json::to_vec(request).expect("json"),
    )
    .expect("write");
}

fn credential_request_json(policy_id: &str, origin: &str, purpose: &str) -> serde_json::Value {
    serde_json::json!({
        "policy_id": policy_id,
        "origin": origin,
        "purpose": purpose,
    })
}

/// harness が Done を返した run（request file だけが wait の理由になる）。
fn done_outcome() -> Result<RunOutcome, AdapterError> {
    Ok(RunOutcome {
        terminal: crate::Terminal::Done {
            summary: "harness finished".into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    })
}

/// (a) `credential-request.json` があると `WaitingForAuth` wait が 1 件開き、outcome は
/// `Terminal::Question`、browser state は `WaitingForAuth`。resume key は `auth:<task>:<run>` で、
/// wait に入るのは origin / purpose / policy_id だけ（credential 値も operation intent も無し）。
#[test]
fn launcher_credential_request_opens_a_waiting_for_auth_wait_and_questions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = dir.path().join("browser");
    write_request(
        &runtime,
        "credential-request.json",
        &credential_request_json(
            "pol-example",
            "https://example.com",
            "Register the operator login",
        ),
    );
    let policy = credential_policy();
    let sink = RecordingWaitSink::new(true);
    let task_id = task_core::TaskId::new();
    let (outcome, state) = super::super::shim_request_wait(
        &runtime,
        task_id,
        "run-1",
        "celeris-s1",
        &policy,
        &sink,
        done_outcome(),
    );
    assert_eq!(state, Some(BrowserRunState::WaitingForAuth));
    match outcome {
        Ok(RunOutcome {
            terminal: crate::Terminal::Question { text },
            exit_code: None,
        }) => assert_eq!(text, super::super::CREDENTIAL_REQUEST_QUESTION),
        other => panic!("expected a question, got {other:?}"),
    }
    let waits = sink.opened();
    assert_eq!(waits.len(), 1, "exactly one wait");
    let wait = &waits[0];
    assert_eq!(wait.reason, BrowserWaitReason::WaitingForAuth);
    assert_eq!(wait.resume_key, format!("auth:{task_id}:run-1"));
    assert_eq!(wait.run_id, "run-1");
    assert_eq!(wait.session_id, "celeris-s1");
    assert_eq!(wait.origin, "https://example.com");
    assert_eq!(wait.purpose, "Register the operator login");
    assert_eq!(wait.credential_policy_id.as_deref(), Some("pol-example"));
    assert!(wait.credential.is_none(), "no secret in the wait");
    assert!(
        wait.operation.is_none(),
        "a credential wait carries no operation"
    );
    assert_eq!(wait.policy_hash, policy.binding.hash);
    assert_eq!(wait.policy_revision, policy.binding.revision);
}

/// (b) policy に合わない request（許可外 origin・知らない policy id・http origin・CredentialUse を
/// 許可しない policy）は wait を開かず `Err`（fail closed）。
#[test]
fn launcher_credential_request_outside_policy_is_refused_without_a_wait() {
    let cases: Vec<(
        &str,
        serde_json::Value,
        crate::browser_policy::PreparedBrowserPolicy,
    )> = vec![
        (
            "origin outside the allowed domains",
            credential_request_json("pol-example", "https://evil.test", "Steal the login"),
            credential_policy(),
        ),
        (
            "unknown credential policy id",
            credential_request_json("pol-other", "https://example.com", "Register"),
            credential_policy(),
        ),
        (
            "non-https origin",
            credential_request_json("pol-example", "http://example.com", "Register"),
            credential_policy(),
        ),
        (
            "policy without CredentialUse",
            credential_request_json("pol-example", "https://example.com", "Register"),
            prepared("example.com"),
        ),
    ];
    for (name, request, policy) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = dir.path().join("browser");
        write_request(&runtime, "credential-request.json", &request);
        let sink = RecordingWaitSink::new(true);
        let (outcome, state) = super::super::shim_request_wait(
            &runtime,
            task_core::TaskId::new(),
            "run-1",
            "celeris-s1",
            &policy,
            &sink,
            done_outcome(),
        );
        assert_eq!(state, None, "{name}: no browser state");
        assert_eq!(
            denied_text(outcome.expect_err("refused")),
            "browser credential request denied",
            "{name}"
        );
        assert!(sink.opened().is_empty(), "{name}: no wait opened");
    }
}

/// 両方の request file があるときは daemon 経路と同じ順で credential が先（`approval-request.json`
/// は読まれない）。credential が無ければ従来どおり approval の wait になる。
#[test]
fn launcher_credential_request_takes_priority_over_approval_request() {
    let policy = shim_policy(
        "example.com",
        &[
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Click,
            task_core::BrowserAction::CredentialUse,
        ],
        &[task_core::BrowserAction::Click],
    );
    let approval = serde_json::json!({
        "action": "click",
        "target": "@e12",
        "origin": "https://example.com",
        "purpose": "Press export",
    });
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = dir.path().join("browser");
    write_request(
        &runtime,
        "credential-request.json",
        &credential_request_json("pol-example", "https://example.com", "Register"),
    );
    write_request(&runtime, "approval-request.json", &approval);
    let sink = RecordingWaitSink::new(true);
    let (outcome, state) = super::super::shim_request_wait(
        &runtime,
        task_core::TaskId::new(),
        "run-1",
        "celeris-s1",
        &policy,
        &sink,
        done_outcome(),
    );
    assert_eq!(state, Some(BrowserRunState::WaitingForAuth));
    assert!(matches!(
        outcome,
        Ok(RunOutcome {
            terminal: crate::Terminal::Question { .. },
            ..
        })
    ));
    let waits = sink.opened();
    assert_eq!(waits.len(), 1, "the credential request wins");
    assert_eq!(waits[0].reason, BrowserWaitReason::WaitingForAuth);

    // credential-request.json が無いときだけ approval-request.json を見る。
    std::fs::remove_file(runtime.join("credential-request.json")).expect("remove");
    let sink = RecordingWaitSink::new(true);
    let (_, state) = super::super::shim_request_wait(
        &runtime,
        task_core::TaskId::new(),
        "run-1",
        "celeris-s1",
        &policy,
        &sink,
        done_outcome(),
    );
    assert_eq!(state, Some(BrowserRunState::WaitingForApproval));
    let waits = sink.opened();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].reason, BrowserWaitReason::WaitingForApproval);
}

/// wait を開けなければ `Err`（fail closed）。Question を返して人に届かない wait を作らない。
#[test]
fn launcher_credential_request_without_a_wait_store_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = dir.path().join("browser");
    write_request(
        &runtime,
        "credential-request.json",
        &credential_request_json("pol-example", "https://example.com", "Register"),
    );
    let sink = RecordingWaitSink::new(false);
    let (outcome, state) = super::super::shim_request_wait(
        &runtime,
        task_core::TaskId::new(),
        "run-1",
        "celeris-s1",
        &credential_policy(),
        &sink,
        done_outcome(),
    );
    assert_eq!(state, None);
    assert_eq!(
        denied_text(outcome.expect_err("fail closed")),
        // A store message that is not a fixed code is not echoed.
        "browser wait could not be opened (other)"
    );
}

/// (c) request に試験用の秘密文字列を混ぜても、wait の中身・sink の event・browser 更新の
/// serialize に出ない。未知の欄を持つ request（秘密を混ぜる唯一の口）は `deny_unknown_fields` で
/// 拒否され、拒否の文言にも request の中身は写らない。
#[test]
fn launcher_credential_request_keeps_the_secret_out_of_wait_events_and_state() {
    const SECRET: &str = "s3cr3t-token-value";
    let policy = credential_policy();
    let task_id = task_core::TaskId::new();

    // 秘密を未知の欄に混ぜた request: 拒否され、wait は開かず、文言にも出ない。
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = dir.path().join("browser");
    let mut smuggled = credential_request_json("pol-example", "https://example.com", "Register");
    smuggled["credential"] = serde_json::json!(SECRET);
    write_request(&runtime, "credential-request.json", &smuggled);
    let sink = RecordingWaitSink::new(true);
    let (outcome, state) = super::super::shim_request_wait(
        &runtime,
        task_id,
        "run-1",
        "celeris-s1",
        &policy,
        &sink,
        done_outcome(),
    );
    assert_eq!(state, None);
    let text = denied_text(outcome.expect_err("refused"));
    assert!(
        !text.contains(SECRET),
        "the refusal does not echo the request"
    );
    assert!(sink.opened().is_empty());
    assert!(!sink.recorded().contains(SECRET));

    // 正しい request: shim が同じ runtime dir に秘密を書き残していても、wait・browser 更新・
    // outcome のどこにも秘密は出ない（wait の `credential` は常に `None`）。
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = dir.path().join("browser");
    write_request(
        &runtime,
        "credential-request.json",
        &credential_request_json("pol-example", "https://example.com", "Register"),
    );
    std::fs::write(runtime.join("credential.json"), SECRET).expect("write");
    let sink = RecordingWaitSink::new(true);
    let (outcome, state) = super::super::shim_request_wait(
        &runtime,
        task_id,
        "run-1",
        "celeris-s1",
        &policy,
        &sink,
        done_outcome(),
    );
    let wait_state = state.expect("a wait is open");
    // run が sink に渡す browser 更新（`browser.state` は wait の state から来る）。
    sink.browser_updated(&BrowserRun {
        task_id,
        run_id: "run-1".into(),
        session_id: "celeris-s1".into(),
        state: wait_state,
        live_view_url: None,
        policy: Some(policy.binding.clone()),
    });
    let recorded = sink.recorded();
    let outcome_text = format!("{:?}", outcome.expect("question"));
    assert!(
        !recorded.contains(SECRET),
        "no secret in waits/events/state"
    );
    assert!(!outcome_text.contains(SECRET), "no secret in the outcome");
    assert_eq!(sink.opened()[0].credential, None);
}

// ---- 2026-10-09 本番不具合: launcher 経路の shim file 二重作成（EEXIST） ----

/// control gate を返し、browser 更新を記録するだけの偽 sink（launcher 経路の `run` 用）。
#[derive(Default)]
struct LauncherRunSink {
    browsers: Mutex<Vec<BrowserRun>>,
}

impl EventSink for LauncherRunSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<Arc<dyn crate::browser_live::ControlGate>> {
        Some(Arc::new(InMemoryGate::new()))
    }
    fn browser_updated(&self, browser: &BrowserRun) {
        self.browsers.lock().expect("lock").push(browser.clone());
    }
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &task_core::ArtifactRef) {}
}

fn launcher_run_request(workspace: &Path) -> RunRequest {
    let mut task = crate::protocol::tests::sample_task();
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    RunRequest {
        protocol: crate::protocol::PROTOCOL_VERSION,
        task,
        workspace: workspace.into(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: Default::default(),
        cargo_target_dir: None,
    }
}

/// 本番 2026-10-09（task 01M4GYJ3XGJNWZQDF35F1MDE0H）: launcher session が起動した直後、`run` が
/// shim file を fail-closed で先に書いたあと admission の結果で**もう一度 `create_new` で**書き、
/// `celeris-browser.py` の作成が `File exists (os error 17)` で失敗して harness に届かなかった。
/// `run` を偽 launcher と fake harness で最後まで通し、harness が走り、session が stop され、
/// config は fail closed のまま（admission 無し）で、置換用の一時 file も残らないことを見る。
#[tokio::test]
async fn launcher_run_creates_shim_files_once_and_reaches_the_harness() {
    let launcher = fake_launcher(good_facts());
    let dir = tempfile::tempdir().expect("tempdir");
    let policy = prepared("example.com");
    let sink = LauncherRunSink::default();
    let outcome = run(
        Arc::new(crate::fake::FakeAdapter::default()),
        launcher_run_request(dir.path()),
        "run-eexist",
        RunLimits {
            wall_clock: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(60),
            kill_grace: Duration::from_secs(1),
        },
        &sink,
        LauncherTarget {
            socket: &launcher.sock,
            refuse_test_loopback: false,
            launcher_uid: None,
        },
        &policy,
        None,
    )
    .await
    .expect("launcher run reaches the harness");
    assert!(
        matches!(outcome.terminal, crate::Terminal::Done { .. }),
        "{:?}",
        outcome.terminal
    );
    assert_eq!(launcher.log.lock().expect("lock").started, 1);
    assert_eq!(wait_stopped(&launcher.log), 1);
    let runtime_dir = dir.path().join("runs").join("run-eexist").join("browser");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(runtime_dir.join("config.json")).expect("config"))
            .expect("json");
    assert_eq!(config["credential_use"], false);
    assert!(!runtime_dir.join("config.next").exists());
    let states: Vec<_> = sink
        .browsers
        .lock()
        .expect("lock")
        .iter()
        .map(|b| b.state)
        .collect();
    assert_eq!(
        states,
        vec![BrowserRunState::Running, BrowserRunState::Completed]
    );
}

/// admission が通った run の config 置換: 先に書いた fail-closed の config を `config.json` だけ
/// 置き換え（shim・policy は書き直さない）、credential の flag が立つ。shim file の新規作成は
/// run ごとに一度だけで、二度目は `create_new` で失敗する（だから置換は別の段にする）。
#[test]
fn launcher_admitted_shim_config_replaces_only_the_config() {
    let policy = shim_policy(
        "example.com",
        &[
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::CredentialUse,
        ],
        &[],
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime_dir = dir.path().join("browser");
    write_shim_files(
        &runtime_dir,
        "s",
        &policy,
        &policy.action_policy,
        &[],
        false,
    )
    .expect("shim files");
    let policy_before = std::fs::read(runtime_dir.join("policy.json")).expect("policy");
    let again = write_shim_files(&runtime_dir, "s", &policy, &policy.action_policy, &[], true);
    assert!(
        matches!(&again, Err(AdapterError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{again:?}"
    );
    admit_shim_config(&runtime_dir, "s", &policy, &policy.action_policy, &[], true)
        .expect("admitted config");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(runtime_dir.join("config.json")).expect("config"))
            .expect("json");
    assert_eq!(config["credential_use"], true);
    assert_eq!(
        config["credential_policy_ids"],
        serde_json::json!(["pol-example"])
    );
    assert_eq!(
        std::fs::read(runtime_dir.join("policy.json")).expect("policy"),
        policy_before
    );
    assert!(!runtime_dir.join("config.next").exists());
}

// ---- 2026-10-09 本番不具合: launcher 経路で登録済み credential が承認待ちにならない ----

use task_core::browser_wait::{BrowserWait, CredentialRef, TrustedLogin};

/// 本番 task 01M4GYJ3XGJNWZQDF35F1MDE0H の形の登録済み wait（`WaitingForAuth` → `Registered`、
/// credential 参照あり、operation 無し）。
fn registered_wait(policy: &crate::browser_policy::PreparedBrowserPolicy) -> BrowserWait {
    let now = time::OffsetDateTime::now_utc();
    BrowserWait {
        wait_id: "wait-reg-1".into(),
        task_id: task_core::TaskId::new(),
        work_unit_id: None,
        run_id: "run-asked".into(),
        session_id: "celeris-asked".into(),
        reason: BrowserWaitReason::WaitingForAuth,
        origin: "https://example.com".into(),
        purpose: "Log in".into(),
        credential_policy_id: Some("pol-example".into()),
        credential: Some(CredentialRef {
            credential_id: "cred-1".into(),
            provider: "local".into(),
            policy_id: "pol-example".into(),
        }),
        operation: None,
        trusted_login: None,
        approval_id: None,
        policy_revision: policy.binding.revision,
        policy_hash: policy.binding.hash.clone(),
        owner_id: Some("owner-1".into()),
        deadline: now,
        resume_key: "auth:t:run-asked".into(),
        version: 2,
        state: BrowserWaitState::Registered,
        resolution_code: None,
        created_at: now,
        resolved_at: None,
    }
}

fn trusted_login() -> TrustedLogin {
    TrustedLogin {
        policy_id: "pol-example".into(),
        revision: 1,
        login_url: "https://example.com/login".into(),
        password_selector: "#password".into(),
        submit_selector: Some("#submit".into()),
        username_selector: None,
        post_login: None,
        consent: None,
    }
}

/// `describe_policy` に一度だけ答える偽 credentiald の control socket（`<runtime>/celeris-credentiald/
/// control.sock`）。秘密は返さない。答えた回数を返す thread を返す。
fn fake_credentiald(runtime: &Path) -> std::thread::JoinHandle<usize> {
    let sock = runtime.join("celeris-credentiald/control.sock");
    std::fs::create_dir_all(sock.parent().expect("parent")).expect("mkdir");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    let reply = serde_json::to_vec(&serde_json::json!({
        "success": true,
        "trusted_login": trusted_login(),
    }))
    .expect("json");
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return 0;
        };
        let mut request = Vec::new();
        stream.read_to_end(&mut request).expect("read");
        let request: serde_json::Value = serde_json::from_slice(&request).expect("request");
        assert_eq!(request["op"], "describe_policy");
        stream.write_all(&reply).expect("write");
        1
    })
}

fn supervisor(runtime: &Path) -> crate::browser_credential::CredentialSupervisor {
    crate::browser_credential::CredentialSupervisor {
        broker: Arc::new(crate::browser_credential::UnixLeaseBroker {
            control_socket: runtime.join("unused.sock"),
        }),
        bridge: PathBuf::from("/nonexistent/celeris-credentiald"),
        runtime_dir: Some(runtime.to_path_buf()),
    }
}

/// wait の一覧を返し、開いた wait を記録して一覧の末尾に `Open` として足す偽 sink。
struct RegisteredSink {
    waits: Mutex<Vec<BrowserWait>>,
    opened: Mutex<Vec<NewBrowserWait>>,
    browsers: Mutex<Vec<BrowserRun>>,
}

impl RegisteredSink {
    fn new(waits: Vec<BrowserWait>) -> Self {
        Self {
            waits: Mutex::new(waits),
            opened: Mutex::new(Vec::new()),
            browsers: Mutex::new(Vec::new()),
        }
    }
    fn opened(&self) -> Vec<NewBrowserWait> {
        self.opened.lock().expect("lock").clone()
    }
}

impl EventSink for RegisteredSink {
    fn browser_waits(&self) -> Result<Vec<BrowserWait>, String> {
        Ok(self.waits.lock().expect("lock").clone())
    }
    fn browser_wait_open(&self, request: &NewBrowserWait) -> Result<(), String> {
        self.opened.lock().expect("lock").push(request.clone());
        let mut waits = self.waits.lock().expect("lock");
        let mut open = waits.last().cloned().expect("a wait");
        open.wait_id = format!("wait-open-{}", waits.len());
        open.run_id = request.run_id.clone();
        open.session_id = request.session_id.clone();
        open.reason = request.reason;
        open.credential = request.credential.clone();
        open.operation = request.operation.clone();
        open.trusted_login = request.trusted_login.clone();
        open.resume_key = request.resume_key.clone();
        open.state = BrowserWaitState::Pending;
        waits.push(open);
        Ok(())
    }
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<Arc<dyn crate::browser_live::ControlGate>> {
        Some(Arc::new(InMemoryGate::new()))
    }
    fn browser_updated(&self, browser: &BrowserRun) {
        self.browsers.lock().expect("lock").push(browser.clone());
    }
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &task_core::ArtifactRef) {}
}

fn limits() -> RunLimits {
    RunLimits {
        wall_clock: Duration::from_secs(60),
        idle_timeout: Duration::from_secs(60),
        kill_grace: Duration::from_secs(1),
    }
}

/// 本番 2026-10-09: launcher runtime では最後の wait が `Registered` でも承認 wait が開かず、harness が
/// 再びログイン画面で credential を要求し続けた。修正後は daemon 経路と同じ共有段で
/// `WaitingForApproval`（operation `credential_use`、resume key `approval:<wait_id>`、固定した trusted
/// login）を開き、launcher に接続せず harness も走らせずに `Terminal::Question` で止まる。次の run は
/// 最後の wait が承認待ちなので、承認 wait を二度開かない（再要求の繰り返しにならない）。
#[tokio::test]
async fn launcher_registered_credential_opens_an_approval_wait_without_a_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("launcher.sock");
    let listener = counting_listener(&sock);
    let credd = dir.path().join("credd");
    let answered = fake_credentiald(&credd);
    let sup = supervisor(&credd);
    let policy = credential_policy();
    let registered = registered_wait(&policy);
    let sink = RegisteredSink::new(vec![registered.clone()]);
    let req = launcher_run_request(dir.path());
    let task_id = req.task.id;
    let outcome = run(
        Arc::new(crate::fake::FakeAdapter::default()),
        req.clone(),
        "run-resumed",
        limits(),
        &sink,
        LauncherTarget {
            socket: &sock,
            refuse_test_loopback: false,
            launcher_uid: Some(LAUNCHER_UID),
        },
        &policy,
        Some(&sup),
    )
    .await
    .expect("approval requested");
    assert_eq!(answered.join().expect("join"), 1, "trusted login described");
    assert!(
        matches!(
            &outcome.terminal,
            crate::Terminal::Question { text } if text == "Browser credential use approval requested"
        ),
        "{:?}",
        outcome.terminal
    );
    let opened = sink.opened();
    assert_eq!(opened.len(), 1);
    let wait = &opened[0];
    assert_eq!(wait.reason, BrowserWaitReason::WaitingForApproval);
    assert_eq!(
        wait.operation.as_ref().map(|o| o.action.as_str()),
        Some("credential_use")
    );
    assert_eq!(wait.resume_key, "approval:wait-reg-1");
    assert_eq!(wait.credential, registered.credential);
    assert_eq!(wait.trusted_login, Some(trusted_login()));
    assert_eq!(wait.origin, "https://example.com");
    assert_eq!(wait.run_id, "run-resumed");
    assert_eq!(
        wait.session_id,
        super::super::session_id(task_id, "run-resumed")
    );
    assert_eq!(wait.policy_hash, policy.binding.hash);
    let states: Vec<_> = sink
        .browsers
        .lock()
        .expect("lock")
        .iter()
        .map(|b| b.state)
        .collect();
    assert_eq!(states, vec![BrowserRunState::WaitingForApproval]);
    assert_eq!(connections(&listener), 0, "no launcher session");
    assert!(
        !dir.path().join("runs").join("run-resumed").exists(),
        "no shim files, no harness"
    );

    // 次の run（承認前に再 dispatch されても）: 最後の wait は承認待ちなので二度目の承認 wait は開かない。
    let again = run(
        Arc::new(crate::fake::FakeAdapter::default()),
        req,
        "run-again",
        limits(),
        &sink,
        LauncherTarget {
            socket: &sock,
            refuse_test_loopback: false,
            launcher_uid: None,
        },
        &policy,
        Some(&sup),
    )
    .await;
    assert!(again.is_err(), "the credential run still needs admission");
    assert_eq!(sink.opened().len(), 1, "no second approval wait");
    assert_eq!(connections(&listener), 0);
}

/// fail closed: policy に合わない登録（policy hash 違い）や credentiald に届かない場合（supervisor
/// 無し）は承認 wait を開かず、launcher にも接続しない。
#[tokio::test]
async fn launcher_registered_credential_outside_policy_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("launcher.sock");
    let listener = counting_listener(&sock);
    let policy = credential_policy();
    let mut stale = registered_wait(&policy);
    stale.policy_hash = "sha256:stale".into();
    let no_sup = registered_wait(&policy);
    for (wait, expected) in [
        (stale, "browser credential approval request denied"),
        (no_sup, "policy_changed (supervisor_missing)"),
    ] {
        let sink = RegisteredSink::new(vec![wait]);
        let err = run(
            Arc::new(crate::fake::FakeAdapter::default()),
            launcher_run_request(dir.path()),
            "run-x",
            limits(),
            &sink,
            LauncherTarget {
                socket: &sock,
                refuse_test_loopback: false,
                launcher_uid: Some(LAUNCHER_UID),
            },
            &policy,
            None,
        )
        .await
        .expect_err("refused");
        assert_eq!(denied_text(err), expected);
        assert!(sink.opened().is_empty());
        assert!(sink.browsers.lock().expect("lock").is_empty());
    }
    assert_eq!(connections(&listener), 0);
}

/// 承認済みの credential_use wait も daemon 経路と同じ照合を通る。policy が変わった承認は launcher に
/// 接続する前に拒否し、承認を消費しない。
#[tokio::test]
async fn launcher_stale_credential_approval_is_refused_before_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("launcher.sock");
    let listener = counting_listener(&sock);
    let credd = dir.path().join("credd");
    let sup = supervisor(&credd);
    let policy = credential_policy();
    let mut approved = registered_wait(&policy);
    approved.reason = BrowserWaitReason::WaitingForApproval;
    approved.state = BrowserWaitState::Approved;
    approved.operation = Some(task_core::browser_wait::OperationIntent {
        intent_id: "intent-wait-reg-1".into(),
        action: "credential_use".into(),
        args_digest: None,
    });
    approved.trusted_login = Some(trusted_login());
    approved.policy_revision += 1;
    let sink = RegisteredSink::new(vec![approved]);
    let err = run(
        Arc::new(crate::fake::FakeAdapter::default()),
        launcher_run_request(dir.path()),
        "run-y",
        limits(),
        &sink,
        LauncherTarget {
            socket: &sock,
            refuse_test_loopback: false,
            launcher_uid: Some(LAUNCHER_UID),
        },
        &policy,
        Some(&sup),
    )
    .await
    .expect_err("refused");
    assert_eq!(denied_text(err), "approved browser credential use denied");
    assert!(sink.opened().is_empty());
    assert_eq!(connections(&listener), 0);
}

// ---- 2026-10-09 付記「launcher の Authenticate 経路」: 承認後の launcher login ----

mod launcher_login {
    use super::*;
    use crate::browser::sso_tests::ChromeFixture;
    use crate::browser_launcher::backend::{
        LoginSection, begin_login, begin_login_once, run_login,
    };
    use crate::browser_launcher::protocol::AuthenticationStatus;
    use celeris_credentiald::injection_ipc::{Admission, LiveSessionRegistration, process_start};
    use celeris_credentiald::{Broker, CredentialPolicy, ManualProvider, ipc};
    use std::os::unix::fs::PermissionsExt;
    use task_core::browser_isolation::{CdpEndpoint, RuntimeFacts};
    use task_core::browser_wait::{ConsumedBrowserApproval, CredentialRecord};

    const SECRET: &str = "launcher-login-secret-7c1e5a90";
    const POLICY_ID: &str = "sso-fixture";

    /// The launcher-owned side of a session, with a real Chromium controller (the launcher's
    /// `RuntimeSession` minus bwrap / userns). Uses the production `begin_login` / `run_login`.
    struct ChromeSession {
        controller: Arc<Mutex<crate::browser_cdp_sink::CdpController>>,
        login: Option<LoginSection>,
        child: std::process::Child,
        brokers: Arc<Mutex<usize>>,
    }

    impl BackendSession for ChromeSession {
        fn action(&mut self, _verb: Verb, _args: &ActionArgs) -> Result<Observation, ErrorCode> {
            Err(ErrorCode::Unauthorized)
        }
        fn observe(&mut self) -> (SessionState, SessionFacts) {
            (SessionState::Running, good_facts())
        }
        fn isolation_ok(&mut self) -> bool {
            true
        }
        fn auth_begin(&mut self, auth_section_id: &str) -> Result<String, ErrorCode> {
            begin_login_once(&self.controller, &mut self.login, auth_section_id)
        }
        fn authenticate(
            &mut self,
            args: &AuthenticateArgs,
            broker: UnixStream,
        ) -> Result<crate::browser_launcher::protocol::LoginResult, ErrorCode> {
            *self.brokers.lock().expect("lock") += 1;
            let section = self.login.as_mut().ok_or(ErrorCode::Unauthorized)?;
            run_login(
                &self.controller,
                section,
                args,
                &mut crate::browser_cdp_sink::PassedInjectionStream::new(broker),
            )
        }
        fn stop(mut self: Box<Self>) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct ChromeBackend {
        controller: Arc<Mutex<crate::browser_cdp_sink::CdpController>>,
        chrome_pid: i32,
        brokers: Arc<Mutex<usize>>,
    }

    impl SessionBackend for ChromeBackend {
        fn start(&self, _req: &StartRequest) -> Result<Launched, ErrorCode> {
            use std::os::unix::process::CommandExt;
            // The launcher's process group is a private `sleep` (teardown signals it, never the
            // test's own group); the runtime binding is the real Chromium.
            let child = std::process::Command::new("sleep")
                .arg("600")
                .process_group(0)
                .stdin(std::process::Stdio::null())
                .spawn()
                .map_err(|_| ErrorCode::LaunchFailed)?;
            let pid = child.id() as i32;
            let starttime =
                crate::browser_runtime::process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
            Ok(Launched {
                session: Box::new(ChromeSession {
                    controller: Arc::clone(&self.controller),
                    login: None,
                    child,
                    brokers: Arc::clone(&self.brokers),
                }),
                pid,
                pgid: pid,
                starttime,
                runtime_pid: self.chrome_pid,
                runtime_starttime: crate::browser_runtime::process_starttime(self.chrome_pid)
                    .ok_or(ErrorCode::LaunchFailed)?,
                ns_inodes: task_core::browser_isolation::collect_ns_inodes("self")
                    .map_err(|_| ErrorCode::LaunchFailed)?,
            })
        }
    }

    /// Test admission facts (the same-UID harness; isolation evidence lives in the userns suites).
    fn isolated_facts(session_id: &str, _pid: i32) -> RuntimeFacts {
        RuntimeFacts {
            session_id: session_id.into(),
            host_uid: 1000,
            runtime_uid: 1000,
            userns_owner_uid: Some(1001),
            namespaces: task_core::browser_isolation::REQUIRED_NAMESPACES
                .into_iter()
                .collect(),
            root_readonly: true,
            writable_mounts: vec!["/session".into()],
            visible_paths: vec![],
            cdp: CdpEndpoint::Pipe,
            no_new_privs: true,
            capabilities_dropped: true,
            pgid: 4242,
        }
    }

    /// A real credentiald (manual provider, control + injection sockets) whose only control peer is
    /// this test process — the daemon's role.
    fn credentiald(root: &Path, origin: &str, task_id: &str) {
        let policy = CredentialPolicy {
            policy_id: POLICY_ID.into(),
            revision: 1,
            exact_origin: origin.into(),
            task_id: task_id.into(),
            max_ttl_seconds: 60,
            require_approval: true,
            allow_persistence: false,
            login_url: Some(format!("{origin}/entry")),
            password_selector: Some("input[name=j_password]".into()),
            submit_selector: Some("button[name=_eventId_proceed]".into()),
            username_selector: None,
            post_login: None,
            consent: None,
        };
        credentiald_with(root, &policy, "user-1", SECRET);
    }

    /// Start a real credentiald under `root` and register `user` / `password` for `policy`.
    fn credentiald_with(root: &Path, policy: &CredentialPolicy, user: &str, password: &str) {
        for name in ["config", "data", "run"] {
            let path = root.join(name);
            std::fs::create_dir(&path).expect("broker dir");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
        }
        let manual =
            ManualProvider::open(root.join("config/keys"), root.join("data/vault")).expect("vault");
        manual.initialize_key().expect("key");
        let broker = Arc::new(Broker::new(manual, root.join("data/audit")).expect("broker"));
        let run = root.join("run");
        std::thread::spawn(move || {
            ipc::serve_with(
                broker,
                &run,
                vec![std::process::id()],
                Admission::SameUidHarnessFacts(isolated_facts),
            )
        });
        let control = root.join("run/celeris-credentiald/control.sock");
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !(control.exists() && root.join("run/celeris-credentiald/injection.sock").exists()) {
            assert!(std::time::Instant::now() < deadline, "credentiald startup");
            std::thread::sleep(Duration::from_millis(10));
        }
        let reference = celeris_credentiald::CredentialRef {
            credential_id: "cred-1".into(),
            provider: "manual".into(),
            policy_id: POLICY_ID.into(),
        };
        let reply = ipc::call(
            &control,
            serde_json::json!({"op":"register","reference":reference,"policy":policy,
                "revision":1,"secret":{"username":user,"password":password}})
            .to_string()
            .as_bytes(),
        )
        .expect("register");
        assert!(reply.success, "register: {:?}", reply.code);
    }

    /// The approved credential_use wait, consumed (what `browser_approval_consume` returns), for
    /// the fixture's IdP origin. The wait's session is the logical Celeris session.
    fn consumed(origin: &str, task_id: task_core::TaskId) -> ConsumedBrowserApproval {
        let trusted = TrustedLogin {
            policy_id: POLICY_ID.into(),
            revision: 1,
            // Shibboleth shape: the origin root has no form; the login URL starts the flow.
            login_url: format!("{origin}/entry"),
            password_selector: "input[name=j_password]".into(),
            submit_selector: Some("button[name=_eventId_proceed]".into()),
            username_selector: None,
            post_login: None,
            consent: None,
        };
        consumed_with(origin, task_id, trusted)
    }

    /// The consumed approval for `origin` with the pinned `trusted` login.
    fn consumed_with(
        origin: &str,
        task_id: task_core::TaskId,
        trusted: TrustedLogin,
    ) -> ConsumedBrowserApproval {
        let policy = credential_policy();
        let mut wait = registered_wait(&policy);
        wait.task_id = task_id;
        wait.origin = origin.into();
        wait.reason = BrowserWaitReason::WaitingForApproval;
        wait.state = BrowserWaitState::Resumed;
        wait.approval_id = Some("approval-1".into());
        wait.policy_hash = "sha256:00aa11bb".into();
        wait.deadline = time::OffsetDateTime::now_utc() + time::Duration::minutes(5);
        wait.credential = Some(CredentialRef {
            credential_id: "cred-1".into(),
            provider: "manual".into(),
            policy_id: POLICY_ID.into(),
        });
        wait.trusted_login = Some(trusted.clone());
        ConsumedBrowserApproval {
            wait,
            credential: CredentialRecord {
                credential_id: "cred-1".into(),
                provider: "manual".into(),
                policy_id: POLICY_ID.into(),
                credential_revision: 1,
                origin: origin.into(),
                receipt_id: "receipt-1".into(),
            },
            approved_by: "human-1".into(),
            trusted_login: Some(trusted),
        }
    }

    /// Records every store call the login makes (auth section) and any progress text.
    #[derive(Default)]
    struct AuthSink {
        calls: Mutex<Vec<String>>,
    }

    impl EventSink for AuthSink {
        fn browser_auth_section(
            &self,
            run_id: &str,
            session_id: &str,
            active: bool,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .expect("lock")
                .push(format!("auth_section {run_id} {session_id} {active}"));
            Ok(())
        }
        fn progress(&self, msg: &str) {
            self.calls.lock().expect("lock").push(msg.into());
        }
        fn artifact(&self, _artifact: &task_core::ArtifactRef) {}
    }

    /// Registered → approval → Approved → launcher `auth_begin` / `authenticate`: the daemon grants
    /// the lease bound to the launcher session, opens the credentiald section and passes its own
    /// `injection.sock` connection; the launcher navigates to the trusted `login_url` (the fixture
    /// root has no form), injects through credentiald, submits, and the SP receives the password.
    /// The lease is consumed once, the auth section is recorded (released only after stop), and
    /// the secret appears in no daemon-visible value.
    #[tokio::test]
    async fn launcher_credential_login_navigates_injects_submits_and_consumes_the_granted_lease() {
        let chrome = ChromeFixture::start();
        let brokers = Arc::new(Mutex::new(0));
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("launcher.sock");
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![DaemonIds::current().uid],
                limits: LauncherLimits::default(),
            },
            Arc::new(ChromeBackend {
                controller: Arc::clone(&chrome.controller),
                chrome_pid: chrome.chrome_pid as i32,
                brokers: Arc::clone(&brokers),
            }),
            Registry::open(dir.path().join("state"), "inst-login").expect("registry"),
        )
        .expect("bind");
        let _handle = server.spawn().expect("spawn");
        let task_id = task_core::TaskId::new();
        let credd = tempfile::tempdir().expect("credd");
        credentiald(credd.path(), &chrome.origin, &task_id.to_string());
        let run_dir = credd.path().join("run");
        let (runtime, _) = LauncherRuntime::start(&sock, "task-1", "run-login", policy())
            .expect("launcher session");
        let runtime = Arc::new(runtime);
        // What `register_launcher_proof` does after admission: the live session is the
        // launcher-assigned id, controller = this (daemon) process, runtime = the browser.
        crate::browser_cdp_sink::UnixInjectionClient::new(
            celeris_credentiald::injection_ipc::injection_socket(&run_dir),
        )
        .register_live_session(LiveSessionRegistration {
            session_id: runtime.session_id().to_owned(),
            controller_pid: std::process::id(),
            controller_start: process_start(std::process::id()).expect("self start"),
            runtime_pid: chrome.chrome_pid,
            runtime_start: process_start(chrome.chrome_pid).expect("chrome start"),
        })
        .expect("register live session");
        let approval = consumed(&chrome.origin, task_id);
        assert_ne!(runtime.session_id(), approval.wait.session_id);
        let sup = crate::browser_credential::CredentialSupervisor {
            broker: Arc::new(crate::browser_credential::UnixLeaseBroker {
                control_socket: run_dir.join("celeris-credentiald/control.sock"),
            }),
            bridge: PathBuf::from("/nonexistent/celeris-credentiald"),
            runtime_dir: Some(run_dir.clone()),
        };
        let sink = AuthSink::default();
        let login = launcher_credential_login(
            &runtime,
            &approval,
            None,
            &sup,
            &run_dir,
            &task_id.to_string(),
            "run-resumed",
            &approval.wait.session_id,
            &sink,
        )
        .await;
        assert_eq!(
            login,
            CredentialLogin::Recorded(Ok(crate::browser_launcher::protocol::LoginResult::HELD))
        );
        assert_eq!(
            *brokers.lock().expect("lock"),
            1,
            "one passed broker stream"
        );
        // Submitted: the SP received exactly the injected password.
        let received = chrome.directory.path().join("received");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !received.exists() {
            assert!(std::time::Instant::now() < deadline, "SP never received");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&received).expect("received"),
            SECRET
        );
        // The granted lease was consumed by credentiald for the launcher session.
        let journal = std::fs::read_to_string(credd.path().join("data/audit/journal.jsonl"))
            .expect("journal");
        let consumed_lines: Vec<&str> = journal
            .lines()
            .filter(|l| l.contains("\"consumed\""))
            .collect();
        assert_eq!(consumed_lines.len(), 1, "{journal}");
        assert!(consumed_lines[0].contains(runtime.session_id()));
        assert!(journal.contains("\"injected\""), "{journal}");
        // Store: the auth section was recorded on the logical session and is not released while
        // the session lives (ADR-0080 H3); the launcher's controller keeps observation stopped.
        let calls = sink.calls.lock().expect("lock").clone();
        assert_eq!(
            calls,
            vec![format!(
                "auth_section run-resumed {} true",
                approval.wait.session_id
            )]
        );
        {
            let c = chrome.controller.lock().expect("lock");
            assert!(c.auth_section_active());
            assert_eq!(c.redisplay_guards(), 1);
        }
        // A second login in the same session is refused (one auth section per session).
        assert!(runtime.auth_begin("auth-again").is_err());
        // No daemon-visible value carries the secret.
        for seen in [format!("{login:?}"), format!("{calls:?}"), journal] {
            assert!(!seen.contains(SECRET), "{seen}");
        }
        runtime.stop().expect("stop");
        drop(chrome);
    }

    /// 付記 2026-10-10b (launcher protocol 7, real credentiald): the pinned consent button is pressed
    /// once by the launcher's controller and the login resumes on the LMS.
    #[tokio::test]
    async fn launcher_credential_post_login_presses_the_pinned_consent_once_and_resumes() {
        use crate::browser::post_login_tests::{
            FIXTURE, USER, origins, trusted as post_login_trusted,
        };
        const PASSWORD: &str = "launcher-post-login-pw-a81f3c";
        let chrome = ChromeFixture::start_with(&FIXTURE);
        std::fs::write(chrome.directory.path().join("consent"), "1").expect("consent flag");
        let o = origins(&chrome);
        let brokers = Arc::new(Mutex::new(0));
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("launcher.sock");
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![DaemonIds::current().uid],
                limits: LauncherLimits::default(),
            },
            Arc::new(ChromeBackend {
                controller: Arc::clone(&chrome.controller),
                chrome_pid: chrome.chrome_pid as i32,
                brokers: Arc::clone(&brokers),
            }),
            Registry::open(dir.path().join("state"), "inst-post-login-consent-press")
                .expect("registry"),
        )
        .expect("bind");
        let _handle = server.spawn().expect("spawn");
        let task_id = task_core::TaskId::new();
        let mut trusted = post_login_trusted(&o, &[o.lms.as_str()]);
        trusted.policy_id = POLICY_ID.into();
        trusted.consent = Some(task_core::browser_wait::ConsentPolicy {
            selector: "input[name=_eventId_proceed]".into(),
            choice_selector: Some(
                "input[name=_shib_idp_consentOptions][value=_shib_idp_doNotRememberConsent]".into(),
            ),
        });
        let credd = tempfile::tempdir().expect("credd");
        credentiald_with(
            credd.path(),
            &CredentialPolicy {
                policy_id: POLICY_ID.into(),
                revision: 1,
                exact_origin: o.idp.clone(),
                task_id: task_id.to_string(),
                max_ttl_seconds: 60,
                require_approval: true,
                allow_persistence: false,
                login_url: Some(trusted.login_url.clone()),
                password_selector: Some(trusted.password_selector.clone()),
                submit_selector: trusted.submit_selector.clone(),
                username_selector: trusted.username_selector.clone(),
                post_login: trusted.post_login.clone(),
                consent: trusted.consent.clone(),
            },
            USER,
            PASSWORD,
        );
        let run_dir = credd.path().join("run");
        let (runtime, _) =
            LauncherRuntime::start(&sock, "task-1", "run-post-login-consent-press", policy())
                .expect("launcher session");
        assert_eq!(
            runtime.protocol_version().expect("hello"),
            crate::browser_launcher::protocol::PROTOCOL_VERSION
        );
        let runtime = Arc::new(runtime);
        crate::browser_cdp_sink::UnixInjectionClient::new(
            celeris_credentiald::injection_ipc::injection_socket(&run_dir),
        )
        .register_live_session(LiveSessionRegistration {
            session_id: runtime.session_id().to_owned(),
            controller_pid: std::process::id(),
            controller_start: process_start(std::process::id()).expect("self start"),
            runtime_pid: chrome.chrome_pid,
            runtime_start: process_start(chrome.chrome_pid).expect("chrome start"),
        })
        .expect("register live session");
        let approval = consumed_with(&o.idp, task_id, trusted);
        let sup = crate::browser_credential::CredentialSupervisor {
            broker: Arc::new(crate::browser_credential::UnixLeaseBroker {
                control_socket: run_dir.join("celeris-credentiald/control.sock"),
            }),
            bridge: PathBuf::from("/nonexistent/celeris-credentiald"),
            runtime_dir: Some(run_dir.clone()),
        };
        let read = crate::browser::PostLoginRead {
            read_origins: vec![o.lms.clone()],
            actions: ["snapshot", "extract", "screenshot", "download", "click"]
                .map(String::from)
                .to_vec(),
        };
        let sink = AuthSink::default();
        let login = launcher_credential_login(
            &runtime,
            &approval,
            Some(&read),
            &sup,
            &run_dir,
            &task_id.to_string(),
            "run-resumed",
            &approval.wait.session_id,
            &sink,
        )
        .await;
        let CredentialLogin::Recorded(Ok(result)) = &login else {
            panic!("login not recorded: {login:?}");
        };
        assert_eq!(
            result.observation,
            crate::browser_launcher::protocol::LoginObservation::Resumed
        );
        assert!(result.consent_pressed);
        assert_eq!(
            std::fs::read_to_string(chrome.directory.path().join("consent_posts")).expect("posts"),
            "_eventId_proceed _shib_idp_doNotRememberConsent\n",
            "the launcher's controller pressed exactly once"
        );
        assert!(chrome.controller.lock().expect("lock").post_login_active());
        runtime.stop().expect("stop");
        drop(chrome);
    }

    /// 付記 2026-10-10: an IdP attribute-release consent page after the login holds observation and
    /// the launcher (protocol 6) reports `consent_required` through the real protocol.
    #[tokio::test]
    async fn launcher_credential_post_login_consent_page_is_held_with_its_reason() {
        use crate::browser::post_login_tests::{
            FIXTURE, USER, origins, trusted as post_login_trusted,
        };
        const PASSWORD: &str = "launcher-post-login-pw-a81f3c";
        let chrome = ChromeFixture::start_with(&FIXTURE);
        std::fs::write(chrome.directory.path().join("consent"), "1").expect("consent flag");
        let o = origins(&chrome);
        let brokers = Arc::new(Mutex::new(0));
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("launcher.sock");
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![DaemonIds::current().uid],
                limits: LauncherLimits::default(),
            },
            Arc::new(ChromeBackend {
                controller: Arc::clone(&chrome.controller),
                chrome_pid: chrome.chrome_pid as i32,
                brokers: Arc::clone(&brokers),
            }),
            Registry::open(dir.path().join("state"), "inst-post-login-consent").expect("registry"),
        )
        .expect("bind");
        let _handle = server.spawn().expect("spawn");
        let task_id = task_core::TaskId::new();
        let mut trusted = post_login_trusted(&o, &[o.lms.as_str()]);
        trusted.policy_id = POLICY_ID.into();
        let credd = tempfile::tempdir().expect("credd");
        credentiald_with(
            credd.path(),
            &CredentialPolicy {
                policy_id: POLICY_ID.into(),
                revision: 1,
                exact_origin: o.idp.clone(),
                task_id: task_id.to_string(),
                max_ttl_seconds: 60,
                require_approval: true,
                allow_persistence: false,
                login_url: Some(trusted.login_url.clone()),
                password_selector: Some(trusted.password_selector.clone()),
                submit_selector: trusted.submit_selector.clone(),
                username_selector: trusted.username_selector.clone(),
                post_login: trusted.post_login.clone(),
                consent: None,
            },
            USER,
            PASSWORD,
        );
        let run_dir = credd.path().join("run");
        let (runtime, _) =
            LauncherRuntime::start(&sock, "task-1", "run-post-login-consent", policy())
                .expect("launcher session");
        assert_eq!(
            runtime.protocol_version().expect("hello"),
            crate::browser_launcher::protocol::PROTOCOL_VERSION
        );
        let runtime = Arc::new(runtime);
        crate::browser_cdp_sink::UnixInjectionClient::new(
            celeris_credentiald::injection_ipc::injection_socket(&run_dir),
        )
        .register_live_session(LiveSessionRegistration {
            session_id: runtime.session_id().to_owned(),
            controller_pid: std::process::id(),
            controller_start: process_start(std::process::id()).expect("self start"),
            runtime_pid: chrome.chrome_pid,
            runtime_start: process_start(chrome.chrome_pid).expect("chrome start"),
        })
        .expect("register live session");
        let approval = consumed_with(&o.idp, task_id, trusted);
        let sup = crate::browser_credential::CredentialSupervisor {
            broker: Arc::new(crate::browser_credential::UnixLeaseBroker {
                control_socket: run_dir.join("celeris-credentiald/control.sock"),
            }),
            bridge: PathBuf::from("/nonexistent/celeris-credentiald"),
            runtime_dir: Some(run_dir.clone()),
        };
        let read = crate::browser::PostLoginRead {
            read_origins: vec![o.lms.clone()],
            actions: ["snapshot", "extract", "screenshot", "download", "click"]
                .map(String::from)
                .to_vec(),
        };
        let sink = AuthSink::default();
        let login = launcher_credential_login(
            &runtime,
            &approval,
            Some(&read),
            &sup,
            &run_dir,
            &task_id.to_string(),
            "run-resumed",
            &approval.wait.session_id,
            &sink,
        )
        .await;
        // The answer names the condition (fixed code) and the consent form's control names only;
        // observation stays stopped.
        let CredentialLogin::Recorded(Ok(result)) = &login else {
            panic!("login not recorded: {login:?}");
        };
        assert_eq!(
            result.observation,
            crate::browser_launcher::protocol::LoginObservation::Held
        );
        assert_eq!(
            result.held_reason,
            Some(crate::browser_cdp_sink::PostLoginHeld::ConsentRequired)
        );
        assert!(!result.consent_pressed);
        let line = crate::browser_cdp_sink::format_consent_controls(&result.consent_controls);
        assert!(line.contains("_eventId_proceed=Accept(submit)"), "{line}");
        assert!(!line.contains(USER) && !line.contains(PASSWORD), "{line}");
        {
            let c = chrome.controller.lock().expect("lock");
            assert!(c.auth_section_active());
            assert!(!c.post_login_active());
        }
        assert!(!format!("{login:?}").contains(PASSWORD));
        runtime.stop().expect("stop");
        drop(chrome);
    }

    /// ADR 2026-10-09 credential username / post-login acceptance (launcher protocol v5, real
    /// credentiald, real Chromium): one approval → one lease → one pair injection fills the IdP's
    /// username and password on the same form (not at the origin root); after submit the SP lands
    /// on the LMS and the launcher closes the auth section on the post-login conditions. The agent
    /// (through the controller's agent path, as the relay calls it) then reads the assignment list
    /// (snapshot / extract / screenshot), clicks into an assignment and downloads its handout on the
    /// LMS; it is refused on the IdP, on another origin and on a page with a password field, and an
    /// off-origin download is cancelled. The password and the session cookie reach no agent reply or
    /// event; neither the password nor the username reaches any Celeris-written surface (journal,
    /// store calls, results). The username shown by the LMS is readable by the agent (A1).
    #[tokio::test]
    async fn launcher_credential_post_login_pair_login_then_reads_only_the_lms() {
        use crate::browser::post_login_tests::{
            Agent, COOKIE_VALUE, FIXTURE, USER, origins, trusted as post_login_trusted,
        };
        const PASSWORD: &str = "launcher-post-login-pw-a81f3c";
        let chrome = ChromeFixture::start_with(&FIXTURE);
        let o = origins(&chrome);
        let brokers = Arc::new(Mutex::new(0));
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("launcher.sock");
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![DaemonIds::current().uid],
                limits: LauncherLimits::default(),
            },
            Arc::new(ChromeBackend {
                controller: Arc::clone(&chrome.controller),
                chrome_pid: chrome.chrome_pid as i32,
                brokers: Arc::clone(&brokers),
            }),
            Registry::open(dir.path().join("state"), "inst-post-login").expect("registry"),
        )
        .expect("bind");
        let _handle = server.spawn().expect("spawn");
        let task_id = task_core::TaskId::new();
        let mut trusted = post_login_trusted(&o, &[o.lms.as_str()]);
        trusted.policy_id = POLICY_ID.into();
        let credd = tempfile::tempdir().expect("credd");
        credentiald_with(
            credd.path(),
            &CredentialPolicy {
                policy_id: POLICY_ID.into(),
                revision: 1,
                exact_origin: o.idp.clone(),
                task_id: task_id.to_string(),
                max_ttl_seconds: 60,
                require_approval: true,
                allow_persistence: false,
                login_url: Some(trusted.login_url.clone()),
                password_selector: Some(trusted.password_selector.clone()),
                submit_selector: trusted.submit_selector.clone(),
                username_selector: trusted.username_selector.clone(),
                post_login: trusted.post_login.clone(),
                consent: None,
            },
            USER,
            PASSWORD,
        );
        let run_dir = credd.path().join("run");
        let (runtime, _) = LauncherRuntime::start(&sock, "task-1", "run-post-login", policy())
            .expect("launcher session");
        assert_eq!(
            runtime.protocol_version().expect("hello"),
            crate::browser_launcher::protocol::PROTOCOL_VERSION
        );
        let runtime = Arc::new(runtime);
        crate::browser_cdp_sink::UnixInjectionClient::new(
            celeris_credentiald::injection_ipc::injection_socket(&run_dir),
        )
        .register_live_session(LiveSessionRegistration {
            session_id: runtime.session_id().to_owned(),
            controller_pid: std::process::id(),
            controller_start: process_start(std::process::id()).expect("self start"),
            runtime_pid: chrome.chrome_pid,
            runtime_start: process_start(chrome.chrome_pid).expect("chrome start"),
        })
        .expect("register live session");
        let approval = consumed_with(&o.idp, task_id, trusted);
        let sup = crate::browser_credential::CredentialSupervisor {
            broker: Arc::new(crate::browser_credential::UnixLeaseBroker {
                control_socket: run_dir.join("celeris-credentiald/control.sock"),
            }),
            bridge: PathBuf::from("/nonexistent/celeris-credentiald"),
            runtime_dir: Some(run_dir.clone()),
        };
        let read = crate::browser::PostLoginRead {
            read_origins: vec![o.lms.clone()],
            actions: ["snapshot", "extract", "screenshot", "download", "click"]
                .map(String::from)
                .to_vec(),
        };
        let sink = AuthSink::default();
        let login = launcher_credential_login(
            &runtime,
            &approval,
            Some(&read),
            &sup,
            &run_dir,
            &task_id.to_string(),
            "run-resumed",
            &approval.wait.session_id,
            &sink,
        )
        .await;
        assert_eq!(login, CredentialLogin::Recorded(Ok(LoginResult::RESUMED)));
        assert_eq!(*brokers.lock().expect("lock"), 1);
        // The IdP got both fields from the one injection.
        let received = chrome.directory.path().join("received");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !received.exists() {
            assert!(std::time::Instant::now() < deadline, "IdP never received");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            std::fs::read_to_string(&received).expect("received"),
            format!("{USER}\n{PASSWORD}")
        );
        let journal = std::fs::read_to_string(credd.path().join("data/audit/journal.jsonl"))
            .expect("journal");
        assert_eq!(
            journal
                .lines()
                .filter(|l| l.contains("\"consumed\""))
                .count(),
            1,
            "one lease use for both fields: {journal}"
        );
        assert!(journal.contains("\"injected\"") && journal.contains("input[name=j_username]"));
        {
            let c = chrome.controller.lock().expect("lock");
            assert!(!c.auth_section_active());
            assert!(c.post_login_active());
            assert_eq!(c.redisplay_guards(), 1);
        }
        // The agent reads the LMS tab the login left behind.
        let targets = chrome
            .controller
            .lock()
            .expect("lock")
            .agent_command("Target.getTargets", serde_json::json!({}), None)
            .expect("targets");
        let lms_target = targets["result"]["targetInfos"]
            .as_array()
            .expect("targets")
            .iter()
            .find(|t| t["url"].as_str().is_some_and(|u| u.starts_with(&o.lms)))
            .and_then(|t| t["targetId"].as_str())
            .expect("the LMS tab")
            .to_owned();
        let mut agent = Agent::attach(&chrome.controller, &lms_target);
        agent
            .cmd("Network.enable", serde_json::json!({}))
            .expect("network");
        let text = agent.eval("document.body.innerText").expect("extract");
        let text = text.as_str().unwrap_or_default().to_owned();
        assert!(text.contains("Report 1: Fluid dynamics essay"), "{text}");
        assert!(text.contains(&format!("Signed in as {USER}")), "{text}");
        assert!(
            agent
                .cmd("Accessibility.getFullAXTree", serde_json::json!({}))
                .is_ok()
        );
        assert!(
            agent
                .cmd(
                    "Page.captureScreenshot",
                    serde_json::json!({"format":"png"})
                )
                .expect("screenshot")["result"]["data"]
                .as_str()
                .is_some_and(|d| d.len() > 100)
        );
        agent.click("report").expect("click");
        agent.wait_url(&format!("{}/ct/report_1", o.lms));
        assert!(
            agent
                .eval("document.body.innerText")
                .expect("detail")
                .as_str()
                .is_some_and(|t| t.contains("Due 2026-10-20"))
        );
        agent.goto(&format!("{}/ct/home", o.lms), &format!("{}/ct/home", o.lms));
        let downloads = tempfile::tempdir().expect("downloads");
        chrome
            .controller
            .lock()
            .expect("lock")
            .agent_command(
                "Browser.setDownloadBehavior",
                serde_json::json!({"behavior":"allowAndName","downloadPath":downloads.path(),
                    "eventsEnabled":true}),
                None,
            )
            .expect("download behavior");
        let from = agent.seen.len();
        agent.click("dl").expect("download");
        let lms = agent
            .wait_download_begin(from, &[])
            .unwrap_or_else(|| panic!("LMS download began: {:?}", agent.tail(6)));
        assert!(
            agent.wait_download(from, &lms, "completed"),
            "LMS download completed: {:?}",
            agent.tail(6)
        );
        let mut tab2 = crate::browser::post_login_tests::Agent::new_tab(&chrome.controller);
        assert_eq!(
            tab2.cmd(
                "Page.navigate",
                serde_json::json!({"url":format!("{}/files/other.bin", o.other)})
            )
            .err()
            .map(|e| e.code()),
            Some("observation_origin_denied")
        );
        tab2.goto(&format!("{}/ct/home", o.lms), &format!("{}/ct/home", o.lms));
        tab2.download_by_script("dl-other", "canceled")
            .expect("other-origin download cancelled");
        assert_eq!(
            std::fs::read_dir(downloads.path()).expect("dir").count(),
            1,
            "only the LMS handout"
        );
        let denied = |r: Result<serde_json::Value, crate::browser_cdp_sink::InjectionError>| {
            r.err().map(|e| e.code())
        };
        agent.goto(&format!("{}/ct/go_idp", o.lms), &format!("{}/", o.idp));
        assert_eq!(
            denied(agent.eval("document.body.innerText")),
            Some("observation_origin_denied")
        );
        assert_eq!(
            denied(agent.cmd("Page.captureScreenshot", serde_json::json!({}))),
            Some("observation_origin_denied")
        );
        agent.goto(
            &format!("{}/ct/go_other", o.lms),
            &format!("{}/page", o.other),
        );
        assert_eq!(
            denied(agent.eval("document.title")),
            Some("observation_origin_denied")
        );
        agent.goto(
            &format!("{}/ct/settings", o.lms),
            &format!("{}/ct/settings", o.lms),
        );
        assert_eq!(
            denied(agent.eval("document.title")),
            Some("password_field_present")
        );
        assert_eq!(
            denied(agent.cmd("Page.captureScreenshot", serde_json::json!({}))),
            Some("password_field_present")
        );
        // The relay refuses cookie / storage reads outright (no page check needed).
        for m in [
            "Network.getAllCookies",
            "Storage.getCookies",
            "DOMStorage.getDOMStorageItems",
        ] {
            assert!(crate::browser_shared_cdp::denied_method(m), "{m}");
        }
        // A second login in this session is refused.
        assert!(runtime.auth_begin("auth-again").is_err());
        let seen = agent.seen.join("\n");
        assert!(!seen.contains(PASSWORD), "password reached the agent");
        assert!(
            !seen.contains(COOKIE_VALUE),
            "session cookie reached the agent"
        );
        let calls = sink.calls.lock().expect("lock").clone();
        assert_eq!(
            calls,
            vec![format!(
                "auth_section run-resumed {} true",
                approval.wait.session_id
            )]
        );
        // Celeris-written surfaces carry neither the password nor the username.
        for written in [format!("{login:?}"), format!("{calls:?}"), journal] {
            assert!(!written.contains(PASSWORD), "{written}");
            assert!(!written.contains(USER), "{written}");
        }
        runtime.stop().expect("stop");
        drop(chrome);
    }

    /// A forged broker on the passed stream cannot use the sink for anything but the fixed
    /// injection call: a cookie read never reaches Chrome and the login fails closed.
    #[test]
    fn launcher_credential_login_forged_sink_frame_never_reaches_chrome() {
        struct Forged(Arc<Mutex<Option<isize>>>);
        struct Done(std::thread::JoinHandle<()>);
        impl crate::browser_cdp_sink::PendingInjection for Done {
            fn finish(
                self: Box<Self>,
            ) -> Result<serde_json::Value, crate::browser_cdp_sink::InjectionError> {
                let _ = self.0.join();
                Ok(serde_json::json!({"v":1,"request_id":"","ok":false,"code":"sink_failed"}))
            }
        }
        impl crate::browser_cdp_sink::BrokerClient for Forged {
            fn start(
                &mut self,
                request: serde_json::Value,
                sink: std::os::fd::OwnedFd,
            ) -> Result<
                Box<dyn crate::browser_cdp_sink::PendingInjection>,
                crate::browser_cdp_sink::InjectionError,
            > {
                use std::os::fd::AsRawFd;
                let seen = Arc::clone(&self.0);
                Ok(Box::new(Done(std::thread::spawn(move || {
                    let mut frame = serde_json::to_vec(&serde_json::json!({
                        "id": request["cdp_command_id"], "sessionId": request["cdp_session_id"],
                        "method": "Network.getAllCookies", "params": {}
                    }))
                    .expect("json");
                    frame.push(0);
                    // SAFETY: a connected seqpacket FD and valid buffers.
                    let sent = unsafe {
                        nix::libc::send(sink.as_raw_fd(), frame.as_ptr().cast(), frame.len(), 0)
                    };
                    assert_eq!(sent, frame.len() as isize);
                    let mut reply = [0u8; 4096];
                    // SAFETY: as above. The controller closes its end without answering.
                    let n = unsafe {
                        nix::libc::recv(sink.as_raw_fd(), reply.as_mut_ptr().cast(), reply.len(), 0)
                    };
                    *seen.lock().expect("lock") = Some(n);
                }))))
            }
        }
        let chrome = ChromeFixture::start();
        let mut section = begin_login(&chrome.controller, "auth-forged").expect("begin");
        let reply = Arc::new(Mutex::new(None));
        let args = AuthenticateArgs {
            session_id: "launcher-session".into(),
            lease_id: "lease".into(),
            auth_section_id: "auth-forged".into(),
            credential_lease_id: "credlease".into(),
            origin: chrome.origin.clone(),
            login_url: format!("{}/entry", chrome.origin),
            password_selector: "input[name=j_password]".into(),
            submit_selector: Some("button[name=_eventId_proceed]".into()),
            username_selector: None,
            post_login: None,
            report_held_reason: false,
            consent: None,
            report_consent_controls: false,
        };
        let result = run_login(
            &chrome.controller,
            &mut section,
            &args,
            &mut Forged(Arc::clone(&reply)),
        );
        assert!(result.is_err());
        assert_eq!(
            *reply.lock().expect("lock"),
            Some(0),
            "the sink closed without a CDP reply"
        );
        assert!(!chrome.directory.path().join("received").exists());
        // The section is single-use.
        assert_eq!(
            run_login(
                &chrome.controller,
                &mut section,
                &args,
                &mut Forged(Arc::new(Mutex::new(None)))
            ),
            Err(ErrorCode::Unauthorized)
        );
    }

    #[test]
    fn launcher_credential_login_old_launcher_message_is_explicit() {
        let text = denied_text(launcher_too_old(3, 4));
        assert!(text.contains("protocol 3"), "{text}");
        assert!(text.contains("rebuild and replace celeris-browser-launcher"));
        assert!(text.contains(&format!("protocol {CREDENTIAL_LOGIN_PROTOCOL} required")));
        let _ = AuthenticationStatus::Rejected;
    }
}

// ---- ADR 2026-10-09 credential username / post-login ----

fn multi_domain_policy(
    actions: &[task_core::BrowserAction],
) -> crate::browser_policy::PreparedBrowserPolicy {
    let domains: Vec<String> = ["idp.example.com", "lms.example.com", "other.example.com"]
        .into_iter()
        .map(String::from)
        .collect();
    let grant = task_core::BrowserCapability {
        allowed_domains: domains.clone(),
        allowed_actions: Some(actions.to_vec()),
        approval_actions: vec![],
        credential_policy_ids: vec!["pol-example".into()],
        ..Default::default()
    };
    let task = task_core::BrowserTaskPolicy {
        policy_id: "post-login".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: domains,
        allowed_actions: actions.to_vec(),
        approval_actions: vec![],
        credential_policy_ids: vec!["pol-example".into()],
        artifact_policy_id: None,
    };
    crate::browser_policy::prepare(&grant, Some(&task), super::super::SUPPORTED_VERSION)
        .expect("policy")
}

/// D2-1・D2-5: the effective read is the site policy's opt-in narrowed by the task policy (task ∩
/// grant): read origins outside the allowed domains and actions the task does not allow drop out,
/// and the harness policy gains only those actions (never auth / the credential plugin).
#[test]
fn post_login_read_is_the_site_opt_in_narrowed_by_the_task_policy() {
    use task_core::BrowserAction as B;
    use task_core::browser_wait::{PostLogin, PostLoginAction as P};
    let policy = multi_domain_policy(&[
        B::Navigate,
        B::Snapshot,
        B::Extract,
        B::Click,
        B::Scroll,
        B::CredentialUse,
    ]);
    let mut trusted = trusted_login();
    assert_eq!(
        super::super::post_login_read(&policy, &trusted).expect("ok"),
        None
    );
    trusted.post_login = Some(PostLogin {
        read_origins: vec![
            "https://lms.example.com".into(),
            "https://not-allowed.example.net".into(),
        ],
        actions: vec![P::Snapshot, P::Extract, P::Download, P::Click],
    });
    let (read, bytes) = super::super::post_login_read(&policy, &trusted)
        .expect("ok")
        .expect("opt-in");
    assert_eq!(
        read.read_origins,
        vec!["https://lms.example.com".to_string()]
    );
    assert_eq!(read.actions, vec!["snapshot", "extract", "click"]);
    let allow = serde_json::from_slice::<task_core::AgentBrowserActionPolicy>(&bytes)
        .expect("policy")
        .allow;
    for a in [
        "snapshot", "gettext", "click", "navigate", "scroll", "launch", "close",
    ] {
        assert!(allow.iter().any(|x| x == a), "{a} in {allow:?}");
    }
    for a in [
        "screenshot",
        "download",
        "auth_login",
        task_core::browser::CREDENTIAL_PLUGIN_ACTION,
    ] {
        assert!(!allow.iter().any(|x| x == a), "{a} in {allow:?}");
    }
    // Opted-in actions the task does not allow, or origins outside the task, mean no opt-in.
    trusted.post_login = Some(PostLogin {
        read_origins: vec!["https://lms.example.com".into()],
        actions: vec![P::Screenshot, P::Download],
    });
    assert_eq!(
        super::super::post_login_read(&policy, &trusted).expect("ok"),
        None
    );
    trusted.post_login = Some(PostLogin {
        read_origins: vec!["https://not-allowed.example.net".into()],
        actions: vec![P::Snapshot],
    });
    assert_eq!(
        super::super::post_login_read(&policy, &trusted).expect("ok"),
        None
    );
}

/// D1-5: a pinned login with a username field or a post-login read needs launcher protocol 5;
/// the refusal (before the approval is spent) names the required version.
#[test]
fn launcher_credential_v5_required_for_username_or_post_login_with_explicit_message() {
    let policy = credential_policy();
    let mut wait = registered_wait(&policy);
    wait.trusted_login = Some(trusted_login());
    assert_eq!(required_launcher_protocol(&wait), 4);
    let mut t = trusted_login();
    t.username_selector = Some("#user".into());
    wait.trusted_login = Some(t);
    assert_eq!(required_launcher_protocol(&wait), 5);
    let mut t = trusted_login();
    t.username_selector = Some("#user".into());
    t.post_login = Some(task_core::browser_wait::PostLogin {
        read_origins: vec!["https://lms.example.com".into()],
        actions: vec![task_core::browser_wait::PostLoginAction::Snapshot],
    });
    wait.trusted_login = Some(t);
    assert_eq!(required_launcher_protocol(&wait), 6);
    // 付記 2026-10-10b: a pinned consent button needs protocol 7.
    if let Some(t) = wait.trusted_login.as_mut() {
        t.consent = Some(task_core::browser_wait::ConsentPolicy {
            selector: "input[name=_eventId_proceed]".into(),
            choice_selector: None,
        });
    }
    assert_eq!(required_launcher_protocol(&wait), 7);
    assert!(
        denied_text(launcher_too_old(6, 7))
            .contains("rebuild and replace celeris-browser-launcher (protocol 7 required)")
    );
    let text = denied_text(launcher_too_old(5, 6));
    assert!(text.contains("protocol 5"), "{text}");
    assert!(
        text.contains("rebuild and replace celeris-browser-launcher (protocol 6 required)"),
        "{text}"
    );
}

/// D2-5: the prompt says what may be read after login, and the H3 wording stays for held sessions.
#[test]
fn post_login_prompt_names_origins_and_actions_only_when_resumed() {
    let mut context = super::super::BrowserContext {
        run: BrowserRun {
            task_id: task_core::TaskId::new(),
            run_id: "r".into(),
            session_id: "celeris-s".into(),
            state: BrowserRunState::Running,
            live_view_url: None,
            policy: None,
        },
        cli: PathBuf::from("/w/celeris-browser.py"),
        credential_used: true,
        approval_actions: vec![],
        approved_operation: None,
        post_login: None,
    };
    let held = super::super::prompt(&context);
    assert!(
        held.contains("disabled for the rest of this session"),
        "{held}"
    );
    context.post_login = Some(super::super::PostLoginRead {
        read_origins: vec!["https://lms.example.com".into()],
        actions: vec!["snapshot".into(), "extract".into()],
    });
    let resumed = super::super::prompt(&context);
    assert!(
        resumed.contains("you may use snapshot, extract only on pages of https://lms.example.com"),
        "{resumed}"
    );
    assert!(resumed.contains("password field cannot be"), "{resumed}");
    assert!(!resumed.contains("disabled for the rest of this session"));
}

/// 付記 2026-10-10: the production task policy shape (task 01M4GYJ3XGJNWZQDF35F1MDE0H: wildcard
/// network domain `https://*.tsukuba.ac.jp`, all business actions) with the manaba site policy's
/// opt-in re-enables snapshot / extract / click / screenshot / download once the section closes.
/// While held, the credential harness policy keeps them off ("not permitted by the task browser
/// policy" in that run was the held state, not a policy gap).
#[test]
fn post_login_read_with_the_production_task_policy_shape_enables_reading() {
    use task_core::BrowserAction as B;
    use task_core::browser_wait::{PostLogin, PostLoginAction as P};
    let actions = [
        B::Navigate,
        B::Click,
        B::Snapshot,
        B::Extract,
        B::Screenshot,
        B::Download,
        B::Scroll,
        B::CredentialUse,
    ];
    let domains = vec!["https://*.tsukuba.ac.jp".to_string()];
    let grant = task_core::BrowserCapability {
        allowed_domains: domains.clone(),
        allowed_actions: Some(actions.to_vec()),
        approval_actions: vec![],
        credential_policy_ids: vec!["manaba-tsukuba".into()],
        ..Default::default()
    };
    let task = task_core::BrowserTaskPolicy {
        policy_id: "auto".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: domains,
        allowed_actions: actions.to_vec(),
        approval_actions: vec![],
        credential_policy_ids: vec!["manaba-tsukuba".into()],
        artifact_policy_id: None,
    };
    let policy =
        crate::browser_policy::prepare(&grant, Some(&task), super::super::SUPPORTED_VERSION)
            .expect("policy");
    let mut trusted = trusted_login();
    trusted.post_login = Some(PostLogin {
        read_origins: vec!["https://manaba.tsukuba.ac.jp".into()],
        actions: vec![
            P::Snapshot,
            P::Extract,
            P::Click,
            P::Screenshot,
            P::Download,
        ],
    });
    let (read, bytes) = super::super::post_login_read(&policy, &trusted)
        .expect("ok")
        .expect("opt-in is effective");
    assert_eq!(
        read.read_origins,
        vec!["https://manaba.tsukuba.ac.jp".to_string()]
    );
    let allow = serde_json::from_slice::<task_core::AgentBrowserActionPolicy>(&bytes)
        .expect("policy")
        .allow;
    for a in ["snapshot", "gettext", "click", "screenshot", "download"] {
        assert!(allow.iter().any(|x| x == a), "{a} in {allow:?}");
    }
    let held = serde_json::from_slice::<task_core::AgentBrowserActionPolicy>(
        &super::super::credential_harness_policy(&policy.action_policy).expect("held"),
    )
    .expect("held policy")
    .allow;
    assert!(!held.iter().any(|x| x == "snapshot" || x == "gettext"));
}

// ---- 付記 2026-10-10e: protocol v8 の screenshot / download artifact transfer ----

fn artifact_policy() -> SessionPolicy {
    session_policy(
        &[
            "navigate".into(),
            "snapshot".into(),
            "screenshot".into(),
            "download".into(),
        ],
        &["example.com".into()],
        Duration::from_secs(600),
    )
}

fn shim_name(prefix: &str) -> String {
    format!(
        "{prefix}-{}.{}",
        crate::browser_launcher::random_id().expect("id"),
        if prefix == "screenshot" { "png" } else { "bin" }
    )
}

/// A PDF larger than several transfer chunks, with a marker that must not leak anywhere but the file.
fn big_pdf() -> Vec<u8> {
    let mut body = b"%PDF-1.4\n".to_vec();
    while body.len() < 3 * crate::browser_launcher::protocol::ARTIFACT_CHUNK + 777 {
        body.extend_from_slice(b"SLIDE-BODY-MARKER ");
    }
    body
}

/// shim と同じ形の要求（artifact 名付き）を action socket に送る。
fn shim_artifact_request(
    sock: &std::path::Path,
    verb: &str,
    args: &[&str],
    artifact: &str,
) -> serde_json::Value {
    let mut stream = UnixStream::connect(sock).expect("connect");
    let req = serde_json::json!({"verb": verb, "args": args, "artifact": artifact});
    stream.write_all(req.to_string().as_bytes()).expect("write");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    let mut out = String::new();
    stream.read_to_string(&mut out).expect("read");
    serde_json::from_str(&out).expect("json")
}

#[test]
fn launcher_artifacts_reach_the_run_output_through_the_v8_transfer() {
    let launcher = fake_launcher(good_facts());
    let pdf = big_pdf();
    launcher.log.lock().expect("lock").artifact_body = Some(Ok(pdf.clone()));
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-a1", artifact_policy())
            .expect("start");
    let runtime = Arc::new(runtime);
    let dir = tempfile::tempdir().expect("tempdir");
    let output = dir.path().join("output");
    std::fs::create_dir(&output).expect("output");
    let executor = Arc::new(LauncherExecutor::new(
        Arc::clone(&runtime),
        output.clone(),
        crate::browser_launcher::PROTOCOL_VERSION,
    ));
    let sock = dir.path().join("browser.action.sock");
    let server = ActionServer::start_with(
        &sock,
        Arc::clone(&executor) as Arc<dyn crate::browser_action::ActionExecutor>,
        vec!["example.com".into()],
        vec!["snapshot".into(), "screenshot".into(), "download".into()],
        Vec::new(),
        Arc::new(InMemoryGate::new()),
    )
    .expect("action server");
    // download: the shim's generated name receives the launcher's file, byte for byte.
    let name = shim_name("download");
    let reply = shim_artifact_request(&sock, "download", &["@e3"], &name);
    assert_eq!(reply["status"], 0, "{reply}");
    let stdout: serde_json::Value =
        serde_json::from_str(reply["stdout"].as_str().expect("stdout")).expect("json");
    assert_eq!(stdout["success"], true);
    assert_eq!(stdout["data"]["media_type"], "application/pdf");
    assert_eq!(stdout["data"]["bytes"], pdf.len());
    assert!(!reply.to_string().contains("SLIDE-BODY-MARKER"));
    let written = output.join(&name);
    assert_eq!(std::fs::read(&written).expect("delivered"), pdf);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&written)
            .expect("meta")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    // screenshot: PNG only.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[7u8; 1000]);
    launcher.log.lock().expect("lock").artifact_body = Some(Ok(png.clone()));
    let shot = shim_name("screenshot");
    let reply = shim_artifact_request(&sock, "screenshot", &[], &shot);
    assert_eq!(reply["status"], 0, "{reply}");
    assert_eq!(std::fs::read(output.join(&shot)).expect("shot"), png);
    drop(server);
    assert!(executor.take_refusals().is_empty());
    assert_eq!(
        executor.delivered.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
}

#[test]
fn launcher_artifacts_refuse_types_sizes_counts_and_canceled_downloads() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-a2", artifact_policy())
            .expect("start");
    let dir = tempfile::tempdir().expect("tempdir");
    let exec = LauncherExecutor::new(
        Arc::new(runtime),
        dir.path().to_path_buf(),
        crate::browser_launcher::PROTOCOL_VERSION,
    );
    let download = |exec: &LauncherExecutor| {
        let req = ActionRequest {
            verb: "download".into(),
            args: vec!["@e1".into()],
            artifact: Some(shim_name("download")),
        };
        exec.run(1, &req).expect("answered")
    };
    let screenshot = |exec: &LauncherExecutor| {
        let req = ActionRequest {
            verb: "screenshot".into(),
            args: vec![],
            artifact: Some(shim_name("screenshot")),
        };
        exec.run(1, &req).expect("answered")
    };
    let set = |body: Option<Result<Vec<u8>, ()>>| {
        launcher.log.lock().expect("lock").artifact_body = body;
    };
    // HTML (a login page saved as a "download"), an executable and an unknown blob: not handed over.
    for body in [
        b"<html><form><input type=password></form>".to_vec(),
        b"\x7fELF\x02\x01\x01".to_vec(),
        vec![0u8; 64],
    ] {
        set(Some(Ok(body)));
        let out = download(&exec);
        assert_eq!(out["reason"], ARTIFACT_REJECTED, "{out}");
    }
    // A screenshot must be PNG even when the type is otherwise allowed.
    set(Some(Ok(b"%PDF-1.4 x".to_vec())));
    assert_eq!(screenshot(&exec)["reason"], ARTIFACT_REJECTED);
    // Over 10 MiB: refused by the launcher before any chunk.
    let mut huge = b"%PDF-1.7\n".to_vec();
    huge.resize(
        crate::browser_launcher::protocol::MAX_ARTIFACT_BYTES as usize + 1,
        b' ',
    );
    set(Some(Ok(huge)));
    assert_eq!(download(&exec)["reason"], ARTIFACT_TOO_LARGE);
    // The launcher's verb failed (an other-origin download it canceled): no file, fixed reason.
    set(Some(Err(())));
    assert_eq!(download(&exec)["reason"], ARTIFACT_ACTION_FAILED);
    // The launcher produced no file name.
    set(None);
    assert_eq!(download(&exec)["reason"], ARTIFACT_FAILED);
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("dir").count(),
        0,
        "nothing refused is left in the output"
    );
    // Count limit: the run's quota is spent before the launcher is asked again.
    set(Some(Ok(b"%PDF-1.4 ok".to_vec())));
    exec.delivered.store(
        crate::browser_launcher::protocol::MAX_ARTIFACTS,
        std::sync::atomic::Ordering::SeqCst,
    );
    let before = launcher.log.lock().expect("lock").actions.len();
    assert_eq!(download(&exec)["reason"], ARTIFACT_LIMIT);
    assert_eq!(launcher.log.lock().expect("lock").actions.len(), before);
    let refusals = exec.take_refusals();
    assert!(refusals.contains(&ARTIFACT_REJECTED) && refusals.contains(&ARTIFACT_LIMIT));
}

#[test]
fn launcher_artifacts_fail_closed_with_a_reason_below_protocol_8() {
    let launcher = fake_launcher(good_facts());
    launcher.log.lock().expect("lock").artifact_body = Some(Ok(b"%PDF-1.4 x".to_vec()));
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-a3", artifact_policy())
            .expect("start");
    let dir = tempfile::tempdir().expect("tempdir");
    let exec = LauncherExecutor::new(Arc::new(runtime), dir.path().to_path_buf(), 7);
    for (verb, args) in [
        ("download", vec!["@e1".to_string()]),
        ("screenshot", vec![]),
    ] {
        let req = ActionRequest {
            verb: verb.into(),
            args,
            artifact: Some(shim_name(verb)),
        };
        let out = exec.run(1, &req).expect("answered");
        assert_eq!(out["status"], 1);
        assert_eq!(out["reason"], ARTIFACTS_REQUIRE_V8);
    }
    // Nothing reached the old launcher and nothing was written.
    assert!(launcher.log.lock().expect("lock").actions.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 0);
    assert_eq!(
        exec.take_refusals(),
        vec![ARTIFACTS_REQUIRE_V8, ARTIFACTS_REQUIRE_V8]
    );
    assert_eq!(ARTIFACT_PROTOCOL, 8);
    const { assert!(crate::browser_launcher::PROTOCOL_VERSION >= ARTIFACT_PROTOCOL) };
}

#[test]
fn launcher_serves_only_names_its_session_produced() {
    let launcher = fake_launcher(good_facts());
    launcher.log.lock().expect("lock").artifact_body = Some(Ok(b"%PDF-1.4 x".to_vec()));
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-a4", artifact_policy())
            .expect("start");
    // A well-formed name this session never produced.
    let name = format!("download-{}.bin", "b".repeat(32));
    assert_eq!(
        runtime.fetch_artifact(&name, Verb::Download),
        Err(ARTIFACT_FAILED)
    );
    // A session whose policy has no screenshot / download cannot fetch at all.
    let (plain, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-a5", policy()).expect("start");
    let (_, obs) = runtime
        .action(
            Verb::Download,
            ActionArgs {
                selector: Some("@e1".into()),
                ..ActionArgs::default()
            },
        )
        .expect("download");
    let produced = obs.artifact.expect("name");
    assert!(plain.fetch_artifact(&produced, Verb::Download).is_err());
    // The producing session gets it.
    assert_eq!(
        runtime.fetch_artifact(&produced, Verb::Download),
        Ok((
            crate::browser_launcher::protocol::ArtifactKind::Pdf,
            b"%PDF-1.4 x".to_vec()
        ))
    );
}

#[test]
fn artifact_responses_carry_no_secrets_and_diagnostics_withhold_bytes() {
    use crate::browser_launcher::protocol::{ArtifactKind, decode_request};
    let resp = Response::Artifact {
        name: format!("download-{}.bin", "c".repeat(32)),
        kind: ArtifactKind::Pdf,
        size: 3,
        offset: 0,
        data: "JVBE".into(),
    };
    let json = serde_json::to_value(&resp).expect("json");
    let mut keys: Vec<_> = json.as_object().expect("object").keys().cloned().collect();
    keys.sort();
    // Only the fixed framing: no URL, header, cookie or path field exists in the type.
    assert_eq!(keys, ["data", "kind", "name", "offset", "size", "type"]);
    // Unknown fields (e.g. a cookie) are refused by decode.
    let sneaky = br#"{"type":"fetch_artifact","session_id":"s","lease_id":"l","name":"download-cccccccccccccccccccccccccccccccc.bin","offset":0,"cookie":"x"}"#;
    assert!(decode_request(sneaky).is_err());
    // Paths and foreign names are refused before any backend sees them.
    for name in [
        "../download-cccccccccccccccccccccccccccccccc.bin",
        "download-cccccccccccccccccccccccccccccccc.pdf",
        "extract-cccccccccccccccccccccccccccccccc.json",
        "/etc/passwd",
    ] {
        let body = serde_json::json!({"type":"fetch_artifact","session_id":"s","lease_id":"l","name":name,"offset":0});
        assert!(
            decode_request(body.to_string().as_bytes()).is_err(),
            "{name}"
        );
    }
}

/// The real shim (`browser_cli.py`) → daemon action server → v8 launcher → `browser/output`: the
/// agent gets the PDF under a `.pdf` name it can read, the event names only the generated file,
/// and an old launcher answers with the fixed reason (付記 2026-10-10e).
#[test]
fn launcher_shim_download_lands_in_the_run_output_as_a_readable_pdf() {
    let policy = prepared_with(
        "example.com",
        vec![
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Snapshot,
            task_core::BrowserAction::Screenshot,
            task_core::BrowserAction::Download,
        ],
    );
    let launcher = fake_launcher(good_facts());
    let pdf = big_pdf();
    launcher.log.lock().expect("lock").artifact_body = Some(Ok(pdf.clone()));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime_dir = dir.path().join("runs").join("run-pdf").join("browser");
    let (cli, action_socket) = write_shim_files(
        &runtime_dir,
        "celeris-test-session",
        &policy,
        &policy.action_policy,
        &[],
        false,
    )
    .expect("shim files");
    let output = runtime_dir.join("output");
    let allowed: task_core::AgentBrowserActionPolicy =
        serde_json::from_slice(&policy.action_policy).expect("allow");
    assert!(
        allowed.allow.iter().any(|a| a == "download"),
        "{:?}",
        allowed.allow
    );
    let (runtime, _) = LauncherRuntime::start(
        &launcher.sock,
        "task-1",
        "run-pdf",
        session_policy(
            &allowed.allow,
            policy.allowed_domains(),
            Duration::from_secs(600),
        ),
    )
    .expect("start");
    let runtime = Arc::new(runtime);
    for (protocol, expect_file) in [
        (crate::browser_launcher::PROTOCOL_VERSION, true),
        (7, false),
    ] {
        let server = ActionServer::start_with(
            &action_socket,
            Arc::new(LauncherExecutor::new(
                Arc::clone(&runtime),
                output.clone(),
                protocol,
            )),
            policy.allowed_domains().to_vec(),
            allowed.allow.clone(),
            Vec::new(),
            Arc::new(InMemoryGate::new()),
        )
        .expect("action server");
        let out = run_shim(&cli, &["download", "@e5"]);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(!stdout.contains("SLIDE-BODY-MARKER"), "{stdout}");
        drop(server);
        if expect_file {
            assert!(out.status.success(), "{stdout}");
            let reply: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
            let file = std::path::PathBuf::from(reply["file"].as_str().expect("file"));
            assert_eq!(file.parent(), Some(output.as_path()));
            assert_eq!(file.extension().and_then(|e| e.to_str()), Some("pdf"));
            assert_eq!(std::fs::read(&file).expect("pdf"), pdf);
            let artifact = reply["artifact"].as_str().expect("artifact");
            assert_eq!(std::fs::read(output.join(artifact)).expect("bin"), pdf);
        } else {
            assert_eq!(out.status.code(), Some(1), "{stdout}");
            assert!(stdout.contains(ARTIFACTS_REQUIRE_V8), "{stdout}");
        }
    }
    // The event log the daemon forwards carries the operation, status and generated name only.
    let events = std::fs::read_to_string(runtime_dir.join("events.jsonl")).expect("events");
    assert!(
        !events.contains("SLIDE-BODY-MARKER") && !events.contains("@e5"),
        "{events}"
    );
    assert!(
        events.contains("\"download\"") && events.contains("download-"),
        "{events}"
    );
    // Only the delivered pair (.bin and its .pdf link) is in the output.
    assert_eq!(std::fs::read_dir(&output).expect("dir").count(), 2);
}

/// 付記 2026-10-10f: a failed launcher screenshot / download says which side refused it, with fixed
/// reasons only (the shim's `browser_[a-z0-9_]` vocabulary).
#[test]
fn launcher_artifact_action_failures_keep_the_launcher_code_as_a_fixed_reason() {
    use crate::browser_launcher::ErrorCode as E;
    let cases = [
        (Some(E::Unauthorized), ARTIFACT_ACTION_REFUSED),
        (Some(E::Timeout), ARTIFACT_ACTION_TIMEOUT),
        (Some(E::Limit), ARTIFACT_LIMIT),
        (Some(E::IsolationFailed), ARTIFACT_ISOLATION_FAILED),
        (Some(E::BadRequest), ARTIFACT_ACTION_FAILED),
        (Some(E::LaunchFailed), ARTIFACT_ACTION_FAILED),
        (None, ARTIFACT_LAUNCHER_UNAVAILABLE),
    ];
    let shim_reason = |r: &str| {
        r.strip_prefix("browser_").is_some_and(|rest| {
            (1..=64).contains(&rest.len())
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
    };
    for (code, reason) in cases {
        assert_eq!(artifact_action_reason(code), reason, "{code:?}");
        assert!(shim_reason(reason), "{reason}");
    }
}

/// A fake credentiald control socket answering one `describe_policy` with `reply`.
fn fake_credentiald_reply(runtime: &Path, reply: serde_json::Value) -> std::thread::JoinHandle<()> {
    let sock = runtime.join("celeris-credentiald/control.sock");
    std::fs::create_dir_all(sock.parent().expect("parent")).expect("mkdir");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    let reply = serde_json::to_vec(&reply).expect("json");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = Vec::new();
            stream.read_to_end(&mut request).expect("read");
            stream.write_all(&reply).expect("write");
        }
    })
}

/// Every describe refusal (including temporary unavailability) returns to manual registration.
#[test]
fn registered_credential_describe_failures_reopen_manual_registration() {
    let policy = credential_policy();
    let wait = registered_wait(&policy);
    for code in ["denied", "vault_locked", "not_found", "invalid_request"] {
        let dir = tempfile::tempdir().unwrap();
        let answered =
            fake_credentiald_reply(dir.path(), serde_json::json!({"success":false,"code":code}));
        let sink = RegisteredSink::new(vec![wait.clone()]);
        let out = crate::browser::registered_credential_approval(
            wait.task_id,
            "run-next",
            std::slice::from_ref(&wait),
            &policy,
            Some(&supervisor(dir.path())),
            &sink,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(out.terminal, crate::Terminal::Question { .. }));
        assert_eq!(sink.opened()[0].reason, BrowserWaitReason::WaitingForAuth);
        assert!(sink.opened()[0].credential.is_none());
        answered.join().unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let sink = RegisteredSink::new(vec![wait.clone()]);
    assert!(
        crate::browser::registered_credential_approval(
            wait.task_id,
            "run-next",
            std::slice::from_ref(&wait),
            &policy,
            Some(&supervisor(dir.path())),
            &sink
        )
        .unwrap()
        .is_some()
    );
    assert_eq!(sink.opened()[0].reason, BrowserWaitReason::WaitingForAuth);
}

#[test]
fn trusted_login_difference_names_the_changed_field_only() {
    use crate::browser::trusted_login_difference;
    let pinned = trusted_login();
    assert_eq!(trusted_login_difference(&pinned, &pinned), None);
    let mut current = pinned.clone();
    current.consent = Some(task_core::browser_wait::ConsentPolicy {
        selector: "#ok".into(),
        choice_selector: None,
    });
    assert_eq!(
        trusted_login_difference(&current, &pinned),
        Some("pinned_differs_consent")
    );
    let mut current = pinned.clone();
    current.login_url = "https://example.com/other".into();
    assert_eq!(
        trusted_login_difference(&current, &pinned),
        Some("pinned_differs_login_url")
    );
}

/// 付記 2026-10-10j: a download the launcher's runner could not produce reaches the agent with its
/// fixed diagnostic tokens (the agent asked for them); text outside the token shape is dropped.
#[test]
fn launcher_shim_download_failure_carries_the_fixed_tokens_only() {
    let policy = prepared_with(
        "example.com",
        vec![
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Download,
        ],
    );
    let launcher = fake_launcher(good_facts());
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime_dir = dir.path().join("runs").join("run-fail").join("browser");
    let (cli, action_socket) = write_shim_files(
        &runtime_dir,
        "celeris-test-session",
        &policy,
        &policy.action_policy,
        &[],
        false,
    )
    .expect("shim files");
    let output = runtime_dir.join("output");
    let allowed: task_core::AgentBrowserActionPolicy =
        serde_json::from_slice(&policy.action_policy).expect("allow");
    let (runtime, _) = LauncherRuntime::start(
        &launcher.sock,
        "task-1",
        "run-fail",
        session_policy(
            &allowed.allow,
            policy.allowed_domains(),
            Duration::from_secs(600),
        ),
    )
    .expect("start");
    let runtime = Arc::new(runtime);
    let server = ActionServer::start_with(
        &action_socket,
        Arc::new(LauncherExecutor::new(
            Arc::clone(&runtime),
            output.clone(),
            crate::browser_launcher::PROTOCOL_VERSION,
        )),
        policy.allowed_domains().to_vec(),
        allowed.allow.clone(),
        Vec::new(),
        Arc::new(InMemoryGate::new()),
    )
    .expect("action server");
    let tokens = "code=bad_request status=1 runner_reason=exec_timeout error_class=none \
                  link=target_blank,no_download_attr,href_same_origin,path_pdf \
                  after=window_open_origin_denied,new_tab_from_page gate=none";
    for (text, expect_detail) in [
        (tokens.to_owned(), true),
        (format!("{tokens} url=https://lms.test/a.pdf?sid=1"), false),
    ] {
        launcher.log.lock().expect("lock").failure_text = Some(text);
        let out = run_shim(&cli, &["download", "@e5"]);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(out.status.code(), Some(1), "{stdout}");
        let reply: serde_json::Value = serde_json::from_str(stdout.trim()).expect("json");
        assert_eq!(reply["error"], ARTIFACT_ACTION_FAILED, "{stdout}");
        if expect_detail {
            assert_eq!(reply["detail"], tokens, "{stdout}");
        } else {
            assert!(reply.get("detail").is_none(), "{stdout}");
            assert!(
                !stdout.contains("sid=") && !stdout.contains("lms.test"),
                "{stdout}"
            );
        }
    }
    drop(server);
    assert_eq!(std::fs::read_dir(&output).expect("dir").count(), 0);
}

/// Full metadata path: encrypted vault -> admitted control IPC -> shim request -> approval wait.
/// A subsequent task/run needs a distinct approval; changed policies, other owners, and deletion
/// take the identical request back to manual registration without disclosing either secret.
#[test]
fn saved_credential_reuse_waits_every_run_and_falls_back_without_secrets() {
    use celeris_credentiald::{Broker, ManualProvider, SecretEnvelope, ipc};
    use std::os::unix::fs::PermissionsExt;
    struct SavedSink<'a> {
        recorded: RecordingWaitSink,
        sup: &'a crate::browser_credential::CredentialSupervisor,
        owner: &'a str,
        login: TrustedLogin,
    }
    impl EventSink for SavedSink<'_> {
        fn browser_saved_credential(
            &self,
            policy: &str,
            origin: &str,
        ) -> Option<crate::browser_credential::SavedCredential> {
            if policy != self.login.policy_id {
                return None;
            }
            crate::browser_credential::find_saved_with(
                self.sup,
                self.owner,
                self.login.clone(),
                origin,
            )
        }
        fn browser_wait_open(&self, wait: &NewBrowserWait) -> Result<(), String> {
            self.recorded.browser_wait_open(wait)
        }
        fn progress(&self, _: &str) {}
        fn artifact(&self, _: &task_core::ArtifactRef) {}
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        dir.path(),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )
    .unwrap();
    let manual = ManualProvider::open(dir.path().join("keys"), dir.path().join("vault")).unwrap();
    manual.initialize_key().unwrap();
    let login = trusted_login();
    let reference = celeris_credentiald::CredentialRef {
        credential_id: "saved-login".into(),
        provider: "manual".into(),
        policy_id: login.policy_id.clone(),
    };
    let vault_policy = celeris_credentiald::CredentialPolicy {
        policy_id: login.policy_id.clone(),
        revision: 1,
        exact_origin: "https://example.com".into(),
        task_id: "old-task".into(),
        max_ttl_seconds: 60,
        require_approval: true,
        allow_persistence: false,
        login_url: Some(login.login_url.clone()),
        password_selector: Some(login.password_selector.clone()),
        submit_selector: login.submit_selector.clone(),
        username_selector: None,
        post_login: None,
        consent: None,
    };
    manual
        .register_owned(
            &reference,
            &vault_policy,
            1,
            &SecretEnvelope {
                username: "SECRET-USER".into(),
                password: "SECRET-PASSWORD".into(),
            },
            Some("owner"),
        )
        .unwrap();
    let broker = Arc::new(Broker::new(manual.clone(), dir.path().join("audit")).unwrap());
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let _ = ipc::serve(broker, &path, vec![std::process::id()]);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !dir.path().join("celeris-credentiald/control.sock").exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let sup = supervisor(dir.path());
    let runtime = dir.path().join("browser");
    write_request(
        &runtime,
        "credential-request.json",
        &credential_request_json("pol-example", "https://example.com", "Read course"),
    );
    let policy = credential_policy();
    let mut keys = Vec::new();
    let original_task = task_core::TaskId::new();
    for (index, (owner, changed, expected)) in [
        ("owner", false, BrowserWaitReason::WaitingForApproval),
        ("owner", false, BrowserWaitReason::WaitingForApproval),
        ("other", false, BrowserWaitReason::WaitingForAuth),
        ("owner", true, BrowserWaitReason::WaitingForAuth),
    ]
    .into_iter()
    .enumerate()
    {
        let mut current = login.clone();
        if changed {
            current.password_selector = "#changed".into();
        }
        let sink = SavedSink {
            recorded: RecordingWaitSink::new(true),
            sup: &sup,
            owner,
            login: current,
        };
        let (outcome, _) = super::super::shim_request_wait(
            &runtime,
            if index < 2 {
                original_task
            } else {
                task_core::TaskId::new()
            },
            &format!("next-run-{index}"),
            "session",
            &policy,
            &sink,
            done_outcome(),
        );
        assert!(matches!(
            outcome.unwrap().terminal,
            crate::Terminal::Question { .. }
        ));
        let wait = sink.recorded.opened().remove(0);
        wait.validate().unwrap();
        assert_eq!(wait.reason, expected);
        if expected == BrowserWaitReason::WaitingForApproval {
            assert_eq!(wait.operation.unwrap().action, "credential_use");
            assert_eq!(wait.trusted_login, Some(login.clone()));
            assert_eq!(wait.owner_id.as_deref(), Some("owner"));
            keys.push(wait.resume_key);
        }
        for secret in ["SECRET-USER", "SECRET-PASSWORD"] {
            assert!(!sink.recorded.recorded().contains(secret));
        }
    }
    assert_ne!(keys[0], keys[1], "each task/run asks again");
    let sink = SavedSink {
        recorded: RecordingWaitSink::new(true),
        sup: &sup,
        owner: "owner",
        login: login.clone(),
    };
    let saved = sink
        .browser_saved_credential("pol-example", "https://example.com")
        .unwrap();
    assert!(crate::browser_credential::saved_is_valid_with(
        &sup,
        "owner",
        &saved.reference,
        &login,
        "https://example.com"
    ));
    assert!(!crate::browser_credential::saved_is_valid_with(
        &sup,
        "other",
        &saved.reference,
        &login,
        "https://example.com"
    ));
    let mut changed = login.clone();
    changed.login_url = "https://example.com/changed".into();
    assert!(!crate::browser_credential::saved_is_valid_with(
        &sup,
        "owner",
        &saved.reference,
        &changed,
        "https://example.com"
    ));
    let mut previous = registered_wait(&policy);
    previous.credential = Some(saved.reference.clone());
    let outcome = super::super::credential_failure_reentry(
        previous.task_id,
        "failed-login",
        &previous,
        &policy,
        &sink,
        &sup,
        true,
    )
    .unwrap();
    assert!(matches!(outcome.terminal, crate::Terminal::Question { .. }));
    assert_eq!(
        sink.recorded.opened()[0].reason,
        BrowserWaitReason::WaitingForAuth
    );
    assert!(sink.recorded.opened()[0].credential.is_none());
    assert!(!crate::browser_credential::saved_is_valid_with(
        &sup,
        "owner",
        &saved.reference,
        &login,
        "https://example.com"
    ));
    let (_, state) = super::super::shim_request_wait(
        &runtime,
        task_core::TaskId::new(),
        "after-failure",
        "session",
        &policy,
        &sink,
        done_outcome(),
    );
    assert_eq!(state, Some(BrowserRunState::WaitingForAuth));
    assert!(manual.list_owned("owner").unwrap().is_empty());
    drop(worker); // Test broker lives until process exit; tempdir removal removes its sockets.
}
