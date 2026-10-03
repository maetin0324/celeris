//! ADR-0109 D4: real bwrap/browser CDP injection against a fixture in a
//! network namespace with no external route. Missing prerequisites fail.
mod userns_gate;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use task_worker::browser_cdp_sink::{
    BrokerClient, CdpController, InjectionError, InjectionRequest, PendingInjection,
};
use task_worker::browser_runtime::{DEFAULT_MAX_EGRESS, EgressRelay, IsolatedRuntime, RuntimeSpec};

const INNER: &str = "CELERIS_CDP_SINK_TEST_INNER";
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
    std::fs::write(dir.join("index.html"), r#"<html><body><input id="pass" type="password" oninput="document.body.dataset.injected='yes'"><iframe src="https://other.example.com/other.html"></iframe></body></html>"#).expect("fixture html");
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
    let deadline = Instant::now() + Duration::from_secs(60);
    while TcpStream::connect((FIXTURE_IP, 443)).is_err() {
        assert!(
            Instant::now() < deadline,
            "fixture TLS server did not listen"
        );
        thread::sleep(Duration::from_millis(50));
    }
    child
}

fn packet(fd: &OwnedFd, data: &[u8]) -> Result<(), InjectionError> {
    // SAFETY: data is readable and fd is a connected seqpacket socket.
    let n = unsafe {
        nix::libc::send(
            fd.as_raw_fd(),
            data.as_ptr().cast(),
            data.len(),
            nix::libc::MSG_NOSIGNAL,
        )
    };
    if n != data.len() as isize {
        return Err(InjectionError::SinkFailed);
    }
    Ok(())
}

fn response(fd: &OwnedFd) -> Result<Value, InjectionError> {
    let mut bytes = [0u8; 4096];
    // SAFETY: bytes is writable and fd is a connected seqpacket socket.
    let n = unsafe { nix::libc::recv(fd.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if n <= 0 {
        return Err(InjectionError::SinkFailed);
    }
    serde_json::from_slice(&bytes[..n as usize]).map_err(|_| InjectionError::SinkFailed)
}

struct FakeBroker {
    value: String,
}
struct Pending(thread::JoinHandle<Result<Value, InjectionError>>);
impl PendingInjection for Pending {
    fn finish(self: Box<Self>) -> Result<Value, InjectionError> {
        self.0.join().map_err(|_| InjectionError::SinkFailed)?
    }
}
impl BrokerClient for FakeBroker {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        assert!(
            !request.to_string().contains(&self.value),
            "secret in worker request"
        );
        let value = self.value.clone();
        Ok(Box::new(Pending(thread::spawn(move || {
            // Fixed broker function: origin, all ancestor frames and input type
            // are checked atomically with the setter in an isolated world.
            let function = r#"function(expected,depth,field,value){try{if(!this.isConnected||!this.ownerDocument.defaultView)return 'target_changed';let w=this.ownerDocument.defaultView,n=0;while(true){if(w.location.origin!==expected)return 'target_changed';if(w===w.parent)break;w=w.parent;n++;}if(n!==depth)return 'target_changed';if(!(this instanceof HTMLInputElement))return 'target_changed';if(field==='password'&&this.type!=='password')return 'target_changed';if(field==='username'&&this.type!=='text'&&this.type!=='email')return 'target_changed';Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(this,value);this.dispatchEvent(new Event('input',{bubbles:true}));this.dispatchEvent(new Event('change',{bubbles:true}));return 'ok';}catch(_){return 'target_changed';}}"#;
            let mut frame = serde_json::to_vec(&json!({"id":request["cdp_command_id"],"sessionId":request["cdp_session_id"],"method":"Runtime.callFunctionOn","params":{"objectId":request["object_id"],"functionDeclaration":function,"arguments":[{"value":ORIGIN},{"value":0},{"value":request["field"]},{"value":value}],"returnByValue":true,"silent":true}})).map_err(|_| InjectionError::SinkFailed)?;
            frame.push(0);
            packet(&sink, &frame)?;
            let reply = response(&sink)?;
            if reply["result"]["result"]["value"] != "ok" {
                return Err(InjectionError::TargetChanged);
            }
            Ok(
                json!({"v":1,"request_id":request["request_id"],"ok":true,"receipt":{"lease_id":request["lease_id"],"auth_section_id":request["auth_section_id"],"session_id":request["session_id"],"cdp_target_id":request["cdp_target_id"],"frame_id":request["frame_id"],"loader_id":request["loader_id"],"field":request["field"],"injected_at":1},"redisplay_guard":celeris_credentiald::injection::RedisplayGuard::new(&value).to_wire()}),
            )
        }))))
    }
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
        session_id: "cdp-sink".into(),
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
    let mut cdp = CdpController::new(
        rt.cdp_write.take().expect("CDP write"),
        rt.cdp_read.take().expect("CDP read"),
    );
    cdp.response_timeout_for_test(Duration::from_secs(60));
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
        session_id: "cdp-sink".into(),
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
    let mut broker = FakeBroker {
        value: SECRET.into(),
    };
    assert_eq!(
        cdp.inject(&base, &session_id, &mut broker)
            .expect_err("H3 required")
            .code(),
        "auth_section_required"
    );
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
        .expect("broker sink injection");
    assert!(receipt["ok"] == true && receipt["receipt"].is_object());
    assert!(
        !receipt.to_string().contains(SECRET),
        "worker receipt contains secret"
    );
    cdp.close_auth_section()
        .expect("clear before observation resumes");
    let marker = cdp
        .agent_command(
            "Runtime.evaluate",
            json!({"expression":"document.body.dataset.injected","returnByValue":true}),
            Some(&session_id),
        )
        .expect("input event marker");
    assert_eq!(marker["result"]["result"]["value"], "yes");
    eprintln!("CDP-SINK-EVIDENCE real browser injection, receipt only, origin and iframe rejected");
    rt.kill();
    let _ = tls.kill();
    let _ = tls.wait();
}

#[test]
fn real_browser_injection_receipt_and_origin_guards() {
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
            "inner_cdp_sink",
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
        String::from_utf8_lossy(&out.stderr).contains("CDP-SINK-EVIDENCE"),
        "inner test did not execute: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn inner_cdp_sink() {
    if std::env::var(INNER).is_ok() {
        inner();
    }
}
