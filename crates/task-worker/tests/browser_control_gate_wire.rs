//! ADR-0094: the real shim (`browser_cli.py`) → `ActionServer` → store-backed `StoreGate` path.
//! A fake action child stands in for sandboxd; no browser and no network are involved.
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use task_core::SqliteStore;
use task_core::browser_control::{ControlCommand, ControlPhase, ControlRequest};
use task_core::browser_store::BrowserSessionKey;
use task_worker::browser_action::ActionServer;
use task_worker::browser_live::StoreGate;

const SHIM: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/browser_cli.py"));

fn key() -> BrowserSessionKey<'static> {
    BrowserSessionKey {
        task_id: "task-gate",
        run_id: "run-gate",
        session_id: "session-gate",
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: PathBuf,
    store: Arc<SqliteStore>,
    /// Verbs the fake action child received (= reached the browser).
    seen: Arc<Mutex<Vec<String>>>,
    /// When set, the child holds the next `click` until a message arrives.
    hold: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    _server: ActionServer,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let output = dir.path().join("output");
        std::fs::create_dir_all(runtime.join("actions")).unwrap();
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(runtime.join("celeris-browser.py"), SHIM).unwrap();
        let policy = br#"{"default":"deny","allow":["launch","close","click","snapshot"]}"#;
        std::fs::write(runtime.join("policy.json"), policy).unwrap();
        std::fs::write(
            runtime.join("config.json"),
            serde_json::to_vec(&serde_json::json!({
                "session_id": "session-gate",
                "allowed_domains": ["example.test"],
                "output": output,
                "policy_sha256": format!("{:x}", Sha256::digest(policy)),
                "credential_policy_ids": [],
                "credential_use": false,
            }))
            .unwrap(),
        )
        .unwrap();
        let store = Arc::new(SqliteStore::open(&dir.path().join("celeris.db")).unwrap());
        let gate = Arc::new(StoreGate::new(
            store.clone(),
            key().task_id,
            key().run_id,
            key().session_id,
        ));
        let server = ActionServer::start(
            &runtime.with_extension("action.sock"),
            &runtime,
            vec!["example.test".into()],
            ["launch", "close", "click", "snapshot"]
                .map(String::from)
                .to_vec(),
            gate,
        )
        .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hold: Arc<Mutex<Option<mpsc::Receiver<()>>>> = Arc::new(Mutex::new(None));
        let (actions, seen2, hold2) = (runtime.join("actions"), seen.clone(), hold.clone());
        std::thread::spawn(move || {
            loop {
                let Ok(entries) = std::fs::read_dir(&actions) else {
                    return;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_none_or(|e| e != "request") {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    // The server creates the request file before writing it; retry a
                    // half-written request on the next poll instead of panicking.
                    let Ok(req) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                        continue;
                    };
                    let verb = req["verb"].as_str().unwrap().to_string();
                    let _ = std::fs::remove_file(&path);
                    seen2.lock().unwrap().push(verb.clone());
                    if verb == "click"
                        && let Some(rx) = hold2.lock().unwrap().take()
                    {
                        let _ = rx.recv_timeout(Duration::from_secs(30));
                    }
                    let stdout = r#"{"success":true,"data":{"ok":true}}"#;
                    // Publish the result atomically: the server polls for the file and
                    // would otherwise read it half-written under load.
                    let tmp = path.with_extension("result.tmp");
                    std::fs::write(
                        &tmp,
                        serde_json::json!({"status":0,"stdout":stdout}).to_string(),
                    )
                    .unwrap();
                    std::fs::rename(&tmp, path.with_extension("result")).unwrap();
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            _dir: dir,
            runtime,
            store,
            seen,
            hold,
            _server: server,
        }
    }

    fn shim(&self, args: &[&str]) -> bool {
        Command::new("python3")
            .arg(self.runtime.join("celeris-browser.py"))
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn phase(&self) -> ControlPhase {
        self.store.browser_control_get(key()).unwrap().phase()
    }

    fn command(&self, command: ControlCommand, idem: &str) {
        let version = self.store.browser_control_get(key()).unwrap().version();
        let req = ControlRequest {
            command,
            expected_version: version,
            idempotency_key: idem.into(),
        };
        self.store.browser_control_apply(key(), &req, now()).unwrap();
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn resume(holder: &str) -> ControlCommand {
    ControlCommand::Resume {
        holder: holder.into(),
        fresh_snapshot: true,
        policy_origin_ok: true,
    }
}

fn takeover(holder: &str) -> ControlCommand {
    ControlCommand::Takeover {
        holder: holder.into(),
        ttl_secs: Some(60),
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}


#[test]
fn gate_wire_running_actions_reach_the_browser() {
    let f = Fixture::new();
    assert!(f.shim(&["snapshot"]));
    assert!(f.shim(&["click", "@e1"]));
    assert_eq!(f.seen(), ["snapshot", "click"]);
    assert_eq!(f.store.browser_control_get(key()).unwrap().in_flight(), 0);
}

#[test]
fn gate_wire_human_control_blocks_agent_until_resume() {
    let f = Fixture::new();
    f.command(ControlCommand::Pause, "p1");
    f.command(takeover("human-a"), "t1");
    assert_eq!(f.phase(), ControlPhase::HumanControl);
    assert!(!f.shim(&["snapshot"]), "agent action during human control");
    assert!(f.seen().is_empty(), "blocked action reached the browser");
    // The lease is exclusive: another controller cannot act or resume.
    let mut state = f.store.browser_control_get(key()).unwrap();
    assert!(state.authorize_human_action("human-b", now()).is_err());
    let req = ControlRequest {
        command: resume("human-b"),
        expected_version: state.version(),
        idempotency_key: "r-other".into(),
    };
    assert!(f.store.browser_control_apply(key(), &req, now()).is_err());
    assert!(!f.shim(&["snapshot"]));
    assert!(f.seen().is_empty());
    f.command(resume("human-a"), "r1");
    assert_eq!(f.phase(), ControlPhase::AgentRunning);
    assert!(f.shim(&["snapshot"]));
    assert_eq!(f.seen(), ["snapshot"]);
}

#[test]
fn gate_wire_pause_converges_after_in_flight_then_blocks() {
    let f = Fixture::new();
    let (release, rx) = mpsc::channel();
    *f.hold.lock().unwrap() = Some(rx);
    let runtime = f.runtime.clone();
    let in_flight = std::thread::spawn(move || {
        Command::new("python3")
            .arg(runtime.join("celeris-browser.py"))
            .args(["click", "@e2"])
            .output()
            .unwrap()
            .status
            .success()
    });
    wait_until("click in flight", || f.seen() == ["click"]);
    assert_eq!(f.store.browser_control_get(key()).unwrap().in_flight(), 1);
    f.command(ControlCommand::Pause, "p1");
    assert_eq!(f.phase(), ControlPhase::Pausing);
    release.send(()).unwrap();
    assert!(in_flight.join().unwrap(), "in-flight action completes");
    wait_until("paused", || f.phase() == ControlPhase::Paused);
    assert!(!f.shim(&["snapshot"]), "agent action after pause");
    assert_eq!(f.seen(), ["click"]);
    f.command(takeover("human-a"), "t1");
    f.command(resume("human-a"), "r1");
    assert!(f.shim(&["snapshot"]));
    assert_eq!(f.seen(), ["click", "snapshot"]);
}

#[test]
fn gate_wire_auth_section_blocks_agent_until_left() {
    let f = Fixture::new();
    f.store.browser_control_auth_section(key(), true).unwrap();
    assert_eq!(f.phase(), ControlPhase::AgentRunning);
    assert!(!f.shim(&["snapshot"]), "agent action inside auth section");
    assert!(!f.shim(&["click", "@e1"]));
    assert!(f.seen().is_empty());
    // H3 stays: no takeover while the auth section is active.
    let state = f.store.browser_control_get(key()).unwrap();
    let req = ControlRequest {
        command: takeover("human-a"),
        expected_version: state.version(),
        idempotency_key: "t-auth".into(),
    };
    assert!(f.store.browser_control_apply(key(), &req, now()).is_err());
    f.store.browser_control_auth_section(key(), false).unwrap();
    assert!(f.shim(&["snapshot"]));
    assert_eq!(f.seen(), ["snapshot"]);
}

#[test]
fn gate_wire_stopped_closes_session_once_and_never_runs_actions() {
    let f = Fixture::new();
    assert!(f.shim(&["snapshot"]));
    assert_eq!(f.store.browser_control_stop_task(key().task_id, now()).unwrap(), 1);
    assert_eq!(f.phase(), ControlPhase::Stopped);
    assert!(!f.shim(&["click", "@e1"]), "agent action after stop");
    assert!(!f.shim(&["snapshot"]));
    assert_eq!(f.seen(), ["snapshot", "close"], "stop closes once, runs nothing");
}
