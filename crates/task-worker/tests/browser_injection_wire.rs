//! ADR-0109 D1-D4: real broker and browser CDP injection against a fixture in a
//! network namespace with no external route. Missing prerequisites fail.
mod userns_gate;
use celeris_credentiald::injection_ipc::{
    Admission, AuthSectionRegistration, LiveSessionRegistration, process_start,
};
use celeris_credentiald::ipc;
use celeris_credentiald::{Broker, CredentialPolicy, CredentialRef, LeaseRequest, ManualProvider};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use task_worker::browser_cdp_sink::{CdpController, InjectionRequest, UnixInjectionClient};
use task_worker::browser_runtime::{DEFAULT_MAX_EGRESS, EgressRelay, IsolatedRuntime, RuntimeSpec};

const INNER: &str = "CELERIS_INJECTION_WIRE_INNER";
const FIXTURE_IP: &str = "93.184.216.34";
const ORIGIN: &str = "https://fixture.example.com";
const SECRET: &str = "sentinel-38fc8240a717425e9c27d1cc90116a0d";
const POLICY: &str = r#"{"allow":["fixture.example.com:443","other.example.com:443"],"resolver":"127.0.0.1","allow_ipv6":false}"#;

fn tool(name: &str) -> PathBuf {
    let p = PathBuf::from("/usr/bin").join(name);
    assert!(
        p.exists(),
        "{name} not found at {p:?}; real CDP sink test requires it"
    );
    p
}

fn browser() -> PathBuf {
    if let Ok(p) = std::env::var("CELERIS_TEST_BROWSER") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME is needed to find Playwright browser");
    let base = Path::new(&home).join(".cache/ms-playwright");
    let mut found: Vec<_> = std::fs::read_dir(&base)
        .expect("Playwright browser cache is required")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("chromium_headless_shell-"))
        })
        .map(|p| p.join("chrome-headless-shell-linux64/chrome-headless-shell"))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.pop().expect("chrome-headless-shell not installed")
}

fn sh(command: &str) {
    let status = Command::new("/bin/sh")
        .args(["-c", command])
        .status()
        .expect("shell starts");
    assert!(status.success(), "{command} failed");
}

fn dns_server(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut len = [0u8; 2];
        if stream.read_exact(&mut len).is_err() {
            continue;
        }
        let mut q = vec![0u8; usize::from(u16::from_be_bytes(len))];
        if stream.read_exact(&mut q).is_err() || q.len() < 17 {
            continue;
        }
        let mut pos = 12;
        let mut labels = Vec::new();
        while pos < q.len() && q[pos] != 0 {
            let n = usize::from(q[pos]);
            if pos + 1 + n >= q.len() {
                break;
            }
            labels.push(String::from_utf8_lossy(&q[pos + 1..pos + 1 + n]).into_owned());
            pos += n + 1;
        }
        if pos + 4 >= q.len() {
            continue;
        }
        let qtype = u16::from_be_bytes([q[pos + 1], q[pos + 2]]);
        let answer = if matches!(
            labels.join(".").as_str(),
            "fixture.example.com" | "other.example.com"
        ) && qtype == 1
        {
            Some([93u8, 184, 216, 34])
        } else {
            None
        };
        let mut response = q[..pos + 5].to_vec();
        response[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        response[6..8].copy_from_slice(&u16::from(answer.is_some()).to_be_bytes());
        if let Some(a) = answer {
            response.extend([0xc0, 0x0c]);
            response.extend(qtype.to_be_bytes());
            response.extend([0, 1, 0, 0, 0, 30, 0, 4]);
            response.extend(a);
        }
        let _ = stream.write_all(&(response.len() as u16).to_be_bytes());
        let _ = stream.write_all(&response);
    }
}

fn fixture(dir: &Path) -> Child {
    std::fs::write(dir.join("index.html"), format!(
        "<html><body><input id=\"pass\" type=\"password\" oninput='if(this.value==={:?})document.body.dataset.injected=\"yes\"'><iframe src=\"https://other.example.com/other.html\"></iframe></body></html>", SECRET
    )).expect("fixture html");
    std::fs::write(
        dir.join("other.html"),
        r#"<html><body><input id="other" type="password"></body></html>"#,
    )
    .expect("iframe html");
    sh(&format!(
        "cd {} && openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 1 -subj /CN=fixture.example.com -addext subjectAltName=DNS:fixture.example.com >/dev/null 2>&1",
        dir.display()
    ));
    let child = Command::new(tool("openssl"))
        .args([
            "s_server", "-quiet", "-WWW", "-cert", "cert.pem", "-key", "key.pem", "-accept",
        ])
        .arg(format!("{FIXTURE_IP}:443"))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("fixture TLS server starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect((FIXTURE_IP, 443)).is_err() {
        assert!(
            Instant::now() < deadline,
            "fixture TLS server did not listen"
        );
        thread::sleep(Duration::from_millis(50));
    }
    child
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

#[test]
fn delayed_cdp_page_target_response() {
    // A scripted browser holds the response past the production five-second poll.
    // This exercises the same Target.createTarget path without a user namespace.
    let (controller, mut browser) = UnixStream::pair().expect("CDP pipe pair");
    let read = std::fs::File::from(std::os::fd::OwnedFd::from(
        controller.try_clone().expect("CDP read clone"),
    ));
    let write = std::fs::File::from(std::os::fd::OwnedFd::from(controller));
    let responder = thread::spawn(move || {
        let mut command = Vec::new();
        loop {
            let mut byte = [0];
            browser.read_exact(&mut byte).expect("CDP command byte");
            if byte[0] == 0 {
                break;
            }
            command.push(byte[0]);
        }
        let request: Value = serde_json::from_slice(&command).expect("CDP command");
        assert_eq!(request["method"], "Target.createTarget");
        thread::sleep(Duration::from_secs(6));
        let response = json!({"id":request["id"],"result":{"targetId":"delayed-page"}});
        let mut bytes = serde_json::to_vec(&response).expect("CDP response");
        bytes.push(0);
        let _ = browser.write_all(&bytes);
    });
    let mut cdp = CdpController::new(write, read);
    cdp.response_timeout_for_test(Duration::from_secs(30));
    let target = cdp
        .agent_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .expect("delayed page target");
    assert_eq!(target["result"]["targetId"], "delayed-page");
    responder.join().expect("scripted browser");
}
fn control(root: &tempfile::TempDir, value: Value) {
    let socket = root.path().join("run/celeris-credentiald/control.sock");
    let reply = ipc::call(&socket, value.to_string().as_bytes()).expect("control call");
    assert!(reply.success, "control rejected: {:?}", reply.code);
}
fn start_broker(rt: &IsolatedRuntime) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("broker root");
    for name in ["config", "data", "run"] {
        let path = root.path().join(name);
        std::fs::create_dir(&path).expect("broker dir");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    }
    let manual = ManualProvider::open(
        root.path().join("config/keys"),
        root.path().join("data/vault"),
    )
    .expect("provider");
    manual.initialize_key().expect("key");
    let broker = Arc::new(Broker::new(manual, root.path().join("data/audit")).expect("broker"));
    let run = root.path().join("run");
    thread::spawn(move || {
        ipc::serve_with(
            broker,
            &run,
            vec![std::process::id()],
            Admission::SameUidHarness,
        )
    });
    let socket = root.path().join("run/celeris-credentiald/control.sock");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let pid = rt.inner_pid() as u32;
    UnixInjectionClient::new(root.path().join("run/celeris-credentiald/injection.sock"))
        .register_live_session(LiveSessionRegistration {
            session_id: "wire-sink".into(),
            controller_pid: std::process::id(),
            controller_start: process_start(std::process::id()).expect("self start"),
            runtime_pid: pid,
            runtime_start: process_start(pid).expect("runtime start"),
        })
        .expect("register live session");
    root
}
fn grant(root: &tempfile::TempDir) -> String {
    let sock = root.path().join("run/celeris-credentiald/control.sock");
    let policy = CredentialPolicy {
        policy_id: "site-1".into(),
        revision: 1,
        exact_origin: ORIGIN.into(),
        task_id: "task-1".into(),
        max_ttl_seconds: 60,
        require_approval: true,
        allow_persistence: false,
        login_url: Some(format!("{ORIGIN}/login")),
        password_selector: Some("#pass".into()),
        submit_selector: None,
    };
    let reference = CredentialRef {
        credential_id: "login-1".into(),
        provider: "manual".into(),
        policy_id: "site-1".into(),
    };
    control(
        root,
        json!({"op":"register","reference":reference,"policy":policy,"revision":1,
        "secret":{"username":"user","password":SECRET}}),
    );
    let request = LeaseRequest {
        reference,
        policy,
        credential_revision: 1,
        task_id: "task-1".into(),
        run_id: "run-1".into(),
        session_id: "wire-sink".into(),
        approval_id: "approval-1".into(),
        approved_by: "human-1".into(),
        policy_hash: "hash-1".into(),
        idempotency_key: "key-1".into(),
        ttl_seconds: 60,
        approval_expires_at: now() + 100,
        session_expires_at: now() + 100,
    };
    let reply = ipc::call(
        &sock,
        json!({"op":"grant","request":request})
            .to_string()
            .as_bytes(),
    )
    .expect("grant call");
    assert!(reply.success, "grant rejected: {:?}", reply.code);
    reply.lease_id.expect("lease")
}

fn inner() {
    sh(&format!(
        "ip link set lo up && ip addr add {FIXTURE_IP}/32 dev lo"
    ));
    let dns = TcpListener::bind("127.0.0.1:53").expect("fixture DNS port");
    thread::spawn(move || dns_server(dns));
    let dir = tempfile::tempdir().expect("fixture dir");
    let mut tls = fixture(dir.path());
    let session = tempfile::tempdir().expect("session dir");
    let browser = browser();
    let sandboxd = PathBuf::from(
        std::env::var("CARGO_BIN_EXE_celeris-browser-sandboxd").expect("sandboxd binary path"),
    );
    let proxy = PathBuf::from(
        std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("egress binary path"),
    );
    let mut argv = vec![sandboxd.clone().into_os_string()];
    argv.extend(
        [
            "--headless",
            "--no-sandbox",
            "--no-zygote",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-background-networking",
            "--ignore-certificate-errors",
            "--remote-debugging-pipe",
            "--user-data-dir=/session/profile",
            "--proxy-server=http://127.0.0.1:3128",
            "about:blank",
        ]
        .map(OsString::from),
    );
    argv.insert(1, browser.clone().into_os_string());
    let spec = RuntimeSpec {
        userns: task_worker::browser_runtime::UsernsMode::Unshare,
        bwrap: tool("bwrap"),
        session_id: "wire-sink".into(),
        session_dir: session.path().to_path_buf(),
        ro_dirs: vec![
            browser.parent().expect("browser parent").to_path_buf(),
            sandboxd.parent().expect("sandboxd parent").to_path_buf(),
        ],
        argv,
        cdp_pipe: true,
        egress: Some(EgressRelay {
            proxy,
            policy: POLICY.as_bytes().to_vec(),
            max_concurrent: DEFAULT_MAX_EGRESS,
        }),
    };
    let mut rt = IsolatedRuntime::launch(&spec).expect("isolated browser launches");
    assert!(rt.facts().is_ok(), "runtime facts");
    let broker_dir = start_broker(&rt);
    let mut cdp = CdpController::new(
        rt.cdp_write.take().expect("CDP write"),
        rt.cdp_read.take().expect("CDP read"),
    );
    cdp.response_timeout_for_test(Duration::from_secs(30));
    let ready_deadline = Instant::now() + Duration::from_secs(60);
    let mut last_reply = String::from("no CDP response yet");
    loop {
        if !rt.is_running() || Instant::now() >= ready_deadline {
            let running = rt.is_running();
            let bwrap_status = std::fs::read_to_string(format!("/proc/{}/status", rt.bwrap_pid()))
                .unwrap_or_else(|error| format!("unavailable: {error}"));
            let inner_status = std::fs::read_to_string(format!("/proc/{}/status", rt.inner_pid()))
                .unwrap_or_else(|error| format!("unavailable: {error}"));
            rt.kill();
            let stderr = rt.wait_stderr(Duration::ZERO);
            panic!(
                "browser CDP not ready: running={running}, last reply={last_reply}, bwrap={bwrap_status}, sandbox init={inner_status}, stderr={stderr}"
            );
        }
        match cdp.agent_command("Browser.getVersion", json!({}), None) {
            Ok(reply)
                if reply["result"]["product"].is_string()
                    && reply["result"]["protocolVersion"].is_string() =>
            {
                break;
            }
            Ok(reply) => last_reply = format!("invalid Browser.getVersion response: {reply}"),
            Err(error) => last_reply = format!("Browser.getVersion: {error:?}"),
        }
        thread::sleep(Duration::from_millis(50));
    }
    let created = cdp
        .agent_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .expect("page target");
    let target = created["result"]["targetId"]
        .as_str()
        .expect("target ID")
        .to_owned();
    let attached = cdp
        .agent_command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .expect("CDP attach");
    let session_id = attached["result"]["sessionId"]
        .as_str()
        .expect("CDP session ID")
        .to_owned();
    let nav = cdp
        .agent_command(
            "Page.navigate",
            json!({"url":format!("{ORIGIN}/index.html")}),
            Some(&session_id),
        )
        .expect("fixture navigation");
    assert!(
        nav["result"]["errorText"].is_null(),
        "navigation failed: {nav}"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let (frame_id, loader_id, child_id) = loop {
        assert!(
            Instant::now() < deadline,
            "fixture page or iframe did not load"
        );
        let tree = cdp
            .agent_command("Page.getFrameTree", json!({}), Some(&session_id))
            .expect("frame tree");
        let root = &tree["result"]["frameTree"];
        let frame = &root["frame"];
        let child_frame = &root["childFrames"][0]["frame"];
        let child = child_frame["id"].as_str();
        if frame["url"] == format!("{ORIGIN}/index.html")
            && child_frame["url"] == "https://other.example.com/other.html"
            && let Some(child) = child
        {
            break (
                frame["id"].as_str().expect("frame ID").to_owned(),
                frame["loaderId"].as_str().expect("loader ID").to_owned(),
                child.to_owned(),
            );
        }
        thread::sleep(Duration::from_millis(100));
    };
    let base = InjectionRequest {
        request_id: "01M3RMW83EQE46ZKCF8YD2BVY7".into(),
        session_id: "wire-sink".into(),
        cdp_target_id: target,
        frame_id,
        loader_id,
        exact_origin: ORIGIN.into(),
        redirect_chain: vec![ORIGIN.into()],
        selector: "#pass".into(),
        field: "password".into(),
        auth_section_id: "auth-1".into(),
        lease_id: "lease-1".into(),
    };
    let mut broker = UnixInjectionClient::new(
        broker_dir
            .path()
            .join("run/celeris-credentiald/injection.sock"),
    );
    assert_eq!(
        cdp.inject(&base, &session_id, &mut broker)
            .expect_err("H3 required")
            .code(),
        "auth_section_required"
    );
    let lease = grant(&broker_dir);
    broker
        .open_auth_section(AuthSectionRegistration {
            session_id: "wire-sink".into(),
            auth_section_id: "auth-1".into(),
            lease_id: lease.clone(),
            exact_origin: ORIGIN.into(),
            cdp_target_id: base.cdp_target_id.clone(),
        })
        .expect("open auth section");
    let mut base = base;
    base.lease_id = lease;
    cdp.open_auth_section("auth-1".into());
    assert_eq!(
        cdp.agent_command("Page.captureScreenshot", json!({}), Some(&session_id))
            .expect_err("H3 blocks screenshot")
            .code(),
        "auth_section_required"
    );
    let mut wrong = base.clone();
    wrong.exact_origin = "https://evil.example.com".into();
    assert_eq!(
        cdp.inject(&wrong, &session_id, &mut broker)
            .expect_err("origin mismatch")
            .code(),
        "target_mismatch"
    );
    let mut cross = base.clone();
    cross.frame_id = child_id;
    assert_eq!(
        cdp.inject(&cross, &session_id, &mut broker)
            .expect_err("cross-origin iframe")
            .code(),
        "cross_origin_frame"
    );
    let receipt = cdp
        .inject(&base, &session_id, &mut broker)
        .unwrap_or_else(|error| {
            thread::sleep(Duration::from_millis(100));
            let journal =
                std::fs::read_to_string(broker_dir.path().join("data/audit/journal.jsonl"))
                    .unwrap_or_default();
            panic!("broker sink injection: {error:?}; audit: {journal}");
        });
    assert!(receipt["ok"] == true && receipt["receipt"].is_object());
    assert_eq!(receipt.as_object().map(|o| o.len()), Some(4));
    assert!(
        !receipt.to_string().contains(SECRET),
        "worker receipt contains secret"
    );
    assert!(
        !receipt.to_string().contains("#pass"),
        "selector in receipt"
    );
    cdp.close_auth_section()
        .expect("clear before observation resumes");
    broker
        .close_auth_section("wire-sink", "auth-1")
        .expect("close auth section");
    cdp.open_auth_section("auth-2".into());
    let mut outside = base.clone();
    outside.auth_section_id = "auth-2".into();
    assert_eq!(
        cdp.inject(&outside, &session_id, &mut broker)
            .expect_err("broker section closed")
            .code(),
        "auth_section_required"
    );
    cdp.close_auth_section().expect("close local section");
    broker
        .unregister_live_session("wire-sink")
        .expect("unregister live session");
    let marker = cdp
        .agent_command(
            "Runtime.evaluate",
            json!({"expression":"document.body.dataset.injected","returnByValue":true}),
            Some(&session_id),
        )
        .expect("input event marker");
    assert_eq!(marker["result"]["result"]["value"], "yes");
    let cleared = cdp
        .agent_command(
            "Runtime.evaluate",
            json!({"expression":"document.querySelector('#pass').value","returnByValue":true}),
            Some(&session_id),
        )
        .expect("cleared field");
    assert_eq!(cleared["result"]["result"]["value"], "");
    let audit =
        std::fs::read_to_string(broker_dir.path().join("data/audit/journal.jsonl")).expect("audit");
    assert!(!audit.contains(SECRET), "secret in broker audit");
    eprintln!(
        "INJECTION-WIRE-EVIDENCE real browser injection, receipt only, origin and iframe rejected"
    );
    rt.kill();
    let _ = tls.kill();
    let _ = tls.wait();
}

#[test]
fn real_broker_browser_injection_receipt_and_origin_guards() {
    if userns_gate::skip_unless_userns_tests() {
        return;
    }
    for t in ["unshare", "ip", "openssl", "bwrap"] {
        tool(t);
    }
    browser();
    let out = Command::new(tool("unshare"))
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "inner_injection_wire",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .output()
        .expect("netns test starts");
    assert!(
        out.status.success(),
        "real browser injection failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("INJECTION-WIRE-EVIDENCE"),
        "inner test did not execute: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn inner_injection_wire() {
    if std::env::var(INNER).is_ok() {
        inner();
    }
}
