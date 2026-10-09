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
    BackendSession, ErrorCode, Launched, LauncherLimits, LauncherServer, Registry, ServerConfig,
    ServerHandle, SessionBackend, StartRequest,
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
        Ok(Observation {
            text: Some(format!(
                r#"{{"success":true,"data":{{"verb":"{verb:?}"}}}}"#
            )),
            artifact: None,
        })
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        (SessionState::Running, self.facts.clone())
    }
    fn isolation_ok(&mut self) -> bool {
        true
    }
    fn authenticate(&mut self, args: &AuthenticateArgs) -> Result<(), ErrorCode> {
        if args.session_id.is_empty()
            || args.auth_section_id.is_empty()
            || args.lease_id.is_empty()
            || args.origin.is_empty()
            || args.target.is_empty()
        {
            return Err(ErrorCode::BadRequest);
        }
        Ok(())
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
    let status = runtime
        .authenticate(AuthenticateArgs {
            session_id: runtime.session_id.clone(),
            auth_section_id: "credential-ref".into(),
            lease_id: runtime.lease_id.clone(),
            origin: "https://example.com".into(),
            target: "input[type=password]".into(),
        })
        .expect("fake broker accepted authentication");
    assert_eq!(status, AuthenticationStatus::Success);
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
        Arc::new(LauncherExecutor {
            runtime: Arc::clone(&runtime),
        }),
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
        Arc::new(LauncherExecutor {
            runtime: Arc::clone(&runtime),
        }),
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
fn launcher_executor_refuses_artifact_verbs() {
    let launcher = fake_launcher(good_facts());
    let (runtime, _) =
        LauncherRuntime::start(&launcher.sock, "task-1", "run-4", policy()).expect("start");
    let exec = LauncherExecutor {
        runtime: Arc::new(runtime),
    };
    for verb in ["screenshot", "download", "__version__"] {
        let req = ActionRequest {
            verb: verb.into(),
            args: vec![],
            artifact: None,
        };
        assert!(exec.run(1, &req).is_err(), "{verb}");
    }
    assert!(launcher.log.lock().expect("lock").actions.is_empty());
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
        allowed_actions: vec![
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Snapshot,
        ],
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
            Arc::new(LauncherExecutor {
                runtime: Arc::clone(&runtime),
            }),
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
