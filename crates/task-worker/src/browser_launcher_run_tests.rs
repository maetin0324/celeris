//! ADR-0116 D5: launcher 経由 runtime の daemon 側試験。偽 launcher は実の
//! [`LauncherServer`] に偽の backend を差したもの（テスト内の Unix socket server）。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
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
    stopped: usize,
}

struct FakeBackend {
    facts: SessionFacts,
    log: Arc<Mutex<Log>>,
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
        Ok(Launched {
            session: Box::new(FakeSession {
                facts: self.facts.clone(),
                log: Arc::clone(&self.log),
                child,
            }),
            pid,
            pgid: pid,
            starttime,
        })
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
    // capability が残る・CDP が TCP で listen している・owner 不明は通らない。
    let caps = SessionFacts {
        cap_eff: "000001ffffffffff".into(),
        ..good_facts()
    };
    assert!(verify_isolation(&runtime_facts("s1", &daemon, &caps, true).expect("f")).is_err());
    let tcp = SessionFacts {
        listen_count: 1,
        ..good_facts()
    };
    assert!(verify_isolation(&runtime_facts("s1", &daemon, &tcp, true).expect("f")).is_err());
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
    });
    assert!(super::super::isolated_runtime_ready(Some(&launcher)).is_ok());
}
