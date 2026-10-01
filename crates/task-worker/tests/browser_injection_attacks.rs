//! ADR-0089 D5: attack matrix A1-A17 against a real broker, real CDP pipe and real
//! chrome-headless-shell in a network namespace with no external route.
//! Missing prerequisites fail (no skip).
use celeris_credentiald::injection_ipc::{
    self, Admission, AuthSectionRegistration, CdpSink, InjectionService, LiveRegistry,
    LiveSessionRegistration, PeerCred, SinkFailed, process_start,
};
use celeris_credentiald::ipc;
use celeris_credentiald::{Broker, CredentialPolicy, CredentialRef, LeaseRequest, ManualProvider};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use task_worker::browser_cdp_sink::{CdpController, InjectionRequest, UnixInjectionClient};
use task_worker::browser_runtime::{DEFAULT_MAX_EGRESS, EgressRelay, IsolatedRuntime, RuntimeSpec};

const INNER: &str = "CELERIS_INJECTION_ATTACKS_INNER";
const FIXTURE_IP: &str = "93.184.216.34";
const ORIGIN: &str = "https://fixture.example.com";
const SECRET: &str = "sentinel-38fc8240a717425e9c27d1cc90116a0d";
/// A4 OOPIF: a host whose registrable domain differs from ORIGIN's (another site).
const CROSS_SITE: &str = "https://login.example.net";
const POLICY: &str = r#"{"allow":["fixture.example.com:443","other.example.com:443","login.example.net:443"],"resolver":"127.0.0.1","allow_ipv6":false}"#;

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
            "fixture.example.com" | "other.example.com" | "login.example.net"
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
    let page = |name: &str, body: &str| {
        std::fs::write(dir.join(name), body).expect("fixture page");
    };
    // The page source never contains the sentinel; it records length and a checksum
    // of what reached the input so the test can prove the secret arrived.
    let record = "function h(e){var v=e.value;if(!v)return;var s=0;for(var i=0;i<v.length;i++)s+=v.charCodeAt(i);document.body.dataset.len=String(v.length);document.body.dataset.sum=String(s);document.body.dataset.injected='yes';";
    page(
        "login.html",
        &format!(
            "<html><body><input id=\"pass\" type=\"password\" oninput=\"h(this)\"><script>{record}}}</script></body></html>"
        ),
    );
    page(
        "login2.html",
        &format!(
            "<html><body><input id=\"pass\" type=\"password\" oninput=\"h(this)\"><script>{record}}}</script></body></html>"
        ),
    );
    page(
        "index.html",
        &format!(
            "<html><body><input id=\"pass\" type=\"password\" oninput=\"h(this)\"><iframe src=\"https://other.example.com/other.html\"></iframe><script>{record}}}</script></body></html>"
        ),
    );
    // A4 OOPIF: cross-site child frame; site-per-process puts it in its own renderer.
    page(
        "oopif.html",
        "<html><body><iframe src=\"https://login.example.net/login.html\"></iframe></body></html>",
    );
    page(
        "other.html",
        r#"<html><body><input id="other" type="password"></body></html>"#,
    );
    page(
        "redir.html",
        r#"<html><head><meta http-equiv="refresh" content="0;url=https://fixture.example.com/login.html"></head><body>r</body></html>"#,
    );
    page(
        "rev.html",
        r#"<html><body><iframe src="https://fixture.example.com/login.html"></iframe></body></html>"#,
    );
    page(
        "redisplay.html",
        &format!(
            "<html><body><input id=\"pass\" type=\"text\" oninput=\"h(this)\"><script>{record}}}</script></body></html>"
        ),
    );
    // Hostile page: copies the value to a visible div, console.log and throws it.
    page(
        "leak.html",
        &format!(
            "<html><body><input id=\"pass\" type=\"password\" oninput=\"h(this)\"><div id=\"copy\"></div><script>{record}document.getElementById('copy').textContent=v;console.log(v);setTimeout(function(){{throw new Error('leak '+v)}},0);}}</script></body></html>"
        ),
    );
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
fn grant(root: &tempfile::TempDir, n: u32) -> String {
    let sock = root.path().join("run/celeris-credentiald/control.sock");
    let policy = CredentialPolicy {
        policy_id: format!("site-{n}"),
        revision: 1,
        exact_origin: ORIGIN.into(),
        task_id: "task-1".into(),
        max_ttl_seconds: 60,
        require_approval: true,
        allow_persistence: false,
        login_url: Some(format!("{ORIGIN}/login.html")),
        password_selector: Some("#pass".into()),
        submit_selector: None,
    };
    let reference = CredentialRef {
        credential_id: format!("login-{n}"),
        provider: "manual".into(),
        policy_id: format!("site-{n}"),
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
        approval_id: format!("approval-{n}"),
        approved_by: "human-1".into(),
        policy_hash: format!("hash-{n}"),
        idempotency_key: format!("key-{n}"),
        ttl_seconds: 60,
        approval_expires_at: now() + 300,
        session_expires_at: now() + 300,
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

// ---- sentinel surfaces -------------------------------------------------------

fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in input.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unb64(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for ch in input.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    out
}

fn sentinel_forms() -> Vec<(&'static str, Vec<u8>)> {
    let raw = SECRET.as_bytes();
    let mut forms = vec![
        ("raw", raw.to_vec()),
        (
            "percent",
            raw.iter()
                .map(|b| format!("%{b:02x}"))
                .collect::<String>()
                .into_bytes(),
        ),
        (
            "hex",
            raw.iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                .into_bytes(),
        ),
        (
            "utf16le",
            SECRET.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ),
    ];
    for (name, offset) in [("b64@0", 0usize), ("b64@1", 1), ("b64@2", 2)] {
        let mut v = vec![0u8; offset];
        v.extend_from_slice(raw);
        let e = b64(&v);
        // drop the characters that depend on neighbouring bytes
        forms.push((name, e.as_bytes()[4..e.len() - 4].to_vec()));
    }
    forms
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn assert_no_sentinel(surface: &str, bytes: &[u8]) {
    for (form, needle) in sentinel_forms() {
        assert!(
            !contains(bytes, &needle),
            "sentinel ({form}) found in surface {surface}"
        );
    }
}

fn mark(attack: &str, detail: &str) {
    eprintln!("ATTACK-{attack}-OK {detail}");
}

// ---- harness -------------------------------------------------------------------

struct Ids {
    frame: String,
    loader: String,
    child: Option<String>,
}

struct Ctx {
    root: tempfile::TempDir,
    broker: UnixInjectionClient,
    cdp: CdpController,
    sid: String,
    target: String,
    rt_pid: u32,
    n: u32,
}

struct DummySink;
impl CdpSink for DummySink {
    fn exchange(&mut self, _frame: &[u8]) -> Result<Vec<u8>, SinkFailed> {
        Err(SinkFailed)
    }
}

impl Ctx {
    fn inj_sock(&self) -> PathBuf {
        self.root
            .path()
            .join("run/celeris-credentiald/injection.sock")
    }
    fn journal(&self) -> String {
        std::fs::read_to_string(self.root.path().join("data/audit/journal.jsonl"))
            .unwrap_or_default()
    }
    fn check_journal(&self, what: &str) {
        assert_no_sentinel(
            &format!("audit journal after {what}"),
            self.journal().as_bytes(),
        );
    }

    /// Agent command whose successful reply must not carry the sentinel.
    fn agent(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, task_worker::browser_cdp_sink::InjectionError> {
        let sid = self.sid.clone();
        let r = self.cdp.agent_command(method, params, Some(&sid))?;
        assert_no_sentinel(&format!("CDP reply {method}"), r.to_string().as_bytes());
        Ok(r)
    }

    fn eval(&mut self, expression: &str) -> Value {
        let r = self
            .agent(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true,"awaitPromise":true}),
            )
            .expect("evaluate");
        r["result"]["result"]["value"].clone()
    }

    fn goto(&mut self, url: &str, expect: &str, child: Option<&str>) -> Ids {
        let nav = self
            .agent("Page.navigate", json!({"url":url}))
            .expect("navigate");
        assert!(
            nav["result"]["errorText"].is_null(),
            "navigation failed: {nav}"
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(Instant::now() < deadline, "page {expect} did not load");
            let tree = self
                .agent("Page.getFrameTree", json!({}))
                .expect("frame tree");
            let root = &tree["result"]["frameTree"];
            let frame = &root["frame"];
            let cf = &root["childFrames"][0]["frame"];
            let child_ok = match child {
                None => true,
                Some(u) => cf["url"] == u,
            };
            if frame["url"] == expect && child_ok {
                return Ids {
                    frame: frame["id"].as_str().expect("frame id").to_owned(),
                    loader: frame["loaderId"].as_str().expect("loader").to_owned(),
                    child: cf["id"].as_str().map(str::to_owned),
                };
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Fresh lease + broker section + controller section.
    fn begin(&mut self) -> (String, String) {
        self.n += 1;
        let n = self.n;
        let lease = grant(&self.root, n);
        let auth = format!("auth-a{n}");
        self.broker
            .open_auth_section(AuthSectionRegistration {
                session_id: "wire-sink".into(),
                auth_section_id: auth.clone(),
                lease_id: lease.clone(),
                exact_origin: ORIGIN.into(),
                cdp_target_id: self.target.clone(),
            })
            .expect("open auth section");
        self.cdp.open_auth_section(auth.clone());
        (lease, auth)
    }

    fn end(&mut self, auth: &str) {
        self.cdp.close_auth_section().expect("close local section");
        self.broker
            .close_auth_section("wire-sink", auth)
            .expect("close broker section");
    }

    fn req(&mut self, ids: &Ids, lease: &str, auth: &str) -> InjectionRequest {
        self.n += 1;
        InjectionRequest {
            request_id: format!("req-{}", self.n),
            session_id: "wire-sink".into(),
            cdp_target_id: self.target.clone(),
            frame_id: ids.frame.clone(),
            loader_id: ids.loader.clone(),
            exact_origin: ORIGIN.into(),
            redirect_chain: vec![ORIGIN.into()],
            selector: "#pass".into(),
            field: "password".into(),
            auth_section_id: auth.into(),
            lease_id: lease.into(),
        }
    }

    fn inject(&mut self, req: &InjectionRequest) -> Result<Value, &'static str> {
        let sid = self.sid.clone();
        match self.cdp.inject(req, &sid, &mut self.broker) {
            Ok(v) => {
                assert_no_sentinel("worker receipt", v.to_string().as_bytes());
                Ok(v)
            }
            Err(e) => Err(e.code()),
        }
    }

    /// A raw, well-formed broker request from this process (the registered controller).
    fn wire(&mut self, lease: &str, auth: &str) -> Value {
        self.n += 1;
        json!({"v":1,"request_id":format!("raw-{}", self.n),"session_id":"wire-sink",
            "cdp_target_id":self.target,"cdp_session_id":self.sid,
            "frame_id":"FRAME","loader_id":"LOADER","frame_chain":[ORIGIN],
            "redirect_chain":[ORIGIN],"selector":"#pass","object_id":"OBJ",
            "field":"password","input_type":"password","auth_section_id":auth,
            "lease_id":lease,"cdp_command_id":999})
    }

    /// Sends the request on injection.sock with a seqpacket sink FD. Returns the broker's
    /// decision code (None on success) and asserts that no frame reached the sink.
    fn raw(&self, body: &Value, expect_sink_empty: bool) -> Option<String> {
        let mut fds = [0i32; 2];
        // SAFETY: fds is a valid 2-int array.
        let rc = unsafe {
            nix::libc::socketpair(
                nix::libc::AF_UNIX,
                nix::libc::SOCK_SEQPACKET,
                0,
                fds.as_mut_ptr(),
            )
        };
        assert_eq!(rc, 0, "socketpair");
        let reply =
            injection_ipc::call_raw(&self.inj_sock(), body.to_string().as_bytes(), &[fds[0]])
                .expect("raw injection call");
        assert_no_sentinel("broker IPC reply", &reply);
        if expect_sink_empty {
            let mut buf = [0u8; 16];
            // SAFETY: fds[1] is our own open socket; non-blocking read of a few bytes.
            let n = unsafe {
                nix::libc::recv(
                    fds[1],
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    nix::libc::MSG_DONTWAIT,
                )
            };
            assert!(n <= 0, "a frame reached the sink FD");
        }
        // SAFETY: closing descriptors opened above.
        unsafe {
            nix::libc::close(fds[0]);
            nix::libc::close(fds[1]);
        }
        let v: Value = serde_json::from_slice(&reply).expect("reply json");
        if v["ok"] == true {
            None
        } else {
            Some(v["code"].as_str().unwrap_or("?").to_owned())
        }
    }

    fn assert_injected(&mut self) {
        assert_eq!(
            self.eval("document.body.dataset.injected"),
            "yes",
            "value did not reach page"
        );
        assert_eq!(
            self.eval("document.body.dataset.len"),
            SECRET.len().to_string()
        );
        let sum: u32 = SECRET.bytes().map(u32::from).sum();
        assert_eq!(self.eval("document.body.dataset.sum"), sum.to_string());
        assert_eq!(self.eval("document.querySelector('#pass').value"), "");
    }
}

const WORKER_PY: &str = r#"
import socket, struct, sys, json
inj, resolve, body = sys.argv[1], sys.argv[2], sys.argv[3]
sys.stdin.readline()
a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(inj)
raw = body.encode()
socket.send_fds(s, [struct.pack('>I', len(raw)) + raw], [b.fileno()])
s.shutdown(socket.SHUT_WR)
data = b''
while True:
    c = s.recv(65536)
    if not c:
        break
    data += c
print('INJ ' + data[4:].decode())
try:
    a.setblocking(False)
    print('SINK ' + repr(a.recv(16)))
except BlockingIOError:
    print('SINK empty')
r = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
r.connect(resolve)
r.sendall(json.dumps({"op":"resolve","lease_id":json.loads(body)["lease_id"],"session_id":"wire-sink","origin":"https://fixture.example.com"}).encode())
r.shutdown(socket.SHUT_WR)
out = b''
while True:
    c = r.recv(65536)
    if not c:
        break
    out += c
print('RES ' + out.decode())
"#;

/// Runs WORKER_PY as a separate process. `controller_of` registers that process as the
/// controller of another live session first (A14).
fn run_worker(ctx: &Ctx, body: &Value, controller_of: Option<&str>) -> (String, String) {
    let mut child = Command::new(tool("python3"))
        .arg("-c")
        .arg(WORKER_PY)
        .arg(ctx.inj_sock())
        .arg(ctx.root.path().join("run/celeris-credentiald/resolve.sock"))
        .arg(body.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("worker child");
    let pid = child.id();
    if let Some(session) = controller_of {
        UnixInjectionClient::new(ctx.inj_sock())
            .register_live_session(LiveSessionRegistration {
                session_id: session.into(),
                controller_pid: pid,
                controller_start: process_start(pid).expect("child start"),
                runtime_pid: ctx.rt_pid,
                runtime_start: process_start(ctx.rt_pid).expect("runtime start"),
            })
            .expect("register other session");
    }
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"go\n")
        .expect("go");
    let out = child.wait_with_output().expect("worker output");
    assert!(
        out.status.success(),
        "worker failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_no_sentinel("worker stdout", &out.stdout);
    assert_no_sentinel("worker stderr", &out.stderr);
    if let Some(session) = controller_of {
        UnixInjectionClient::new(ctx.inj_sock())
            .unregister_live_session(session)
            .expect("unregister other session");
    }
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn worker_code(stdout: &str) -> String {
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("INJ "))
        .expect("INJ line");
    let v: Value = serde_json::from_str(line).expect("INJ json");
    assert_eq!(v["ok"], false, "worker injection must fail: {line}");
    v["code"].as_str().expect("code").to_owned()
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
            "--site-per-process",
            "--remote-debugging-pipe",
            "--user-data-dir=/session/profile",
            "--proxy-server=http://127.0.0.1:3128",
            "about:blank",
        ]
        .map(OsString::from),
    );
    argv.insert(1, browser.clone().into_os_string());
    let spec = RuntimeSpec {
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
    let mut ctx = Ctx {
        root: broker_dir,
        broker: UnixInjectionClient::new(PathBuf::new()),
        cdp,
        sid: session_id,
        target,
        rt_pid: rt.inner_pid() as u32,
        n: 0,
    };
    ctx.broker = UnixInjectionClient::new(ctx.inj_sock());

    // Positive path first: the secret really reaches the fixture input.
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    let receipt = ctx.inject(&req).expect("positive injection");
    assert_eq!(receipt["ok"], true);
    ctx.end(&auth);
    ctx.assert_injected();
    mark("A0", "positive injection reached the fixture input");

    a15_outside_section(&mut ctx);
    a1_a2_stale_document(&mut ctx);
    a4_a5_iframes(&mut ctx);
    a6_redisplay_field(&mut ctx);
    a3_a11_a14_a16_a17_same_lease(&mut ctx);
    a7_a8_a9_a10_observation(&mut ctx);
    a12_browser_reach(&mut ctx);
    a13_other_uid(&ctx);
    a1_target_changed_hook(&mut ctx);
    a4_oopif(&mut ctx);
    a8_guard_negative_control();

    ctx.check_journal("all attacks");
    let journal = ctx.journal();
    assert!(
        journal.contains("lease_used"),
        "audit lacks the lease_used decision"
    );
    eprintln!("ATTACKS-AUDIT-LINES {}", journal.lines().count());
    eprintln!("ATTACKS-ALL-DONE");
    rt.kill();
    let _ = tls.kill();
    let _ = tls.wait();
}

fn a15_outside_section(ctx: &mut Ctx) {
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    ctx.n += 1;
    let lease = grant(&ctx.root, ctx.n);
    // before open: controller refuses locally, broker refuses too
    let mut req = ctx.req(&ids, &lease, "auth-none");
    assert_eq!(ctx.inject(&req), Err("auth_section_required"));
    let body = ctx.wire(&lease, "auth-none");
    assert_eq!(
        ctx.raw(&body, true).as_deref(),
        Some("auth_section_required")
    );
    // after close
    let (l2, auth) = ctx.begin();
    ctx.end(&auth);
    req.lease_id = l2.clone();
    req.auth_section_id = auth.clone();
    assert_eq!(ctx.inject(&req), Err("auth_section_required"));
    let body = ctx.wire(&l2, &auth);
    assert_eq!(
        ctx.raw(&body, true).as_deref(),
        Some("auth_section_required")
    );
    // the un-consumed lease is still valid afterwards (new section, real injection)
    ctx.n += 1;
    let auth3 = format!("auth-a{}", ctx.n);
    ctx.broker
        .open_auth_section(AuthSectionRegistration {
            session_id: "wire-sink".into(),
            auth_section_id: auth3.clone(),
            lease_id: l2.clone(),
            exact_origin: ORIGIN.into(),
            cdp_target_id: ctx.target.clone(),
        })
        .expect("reopen");
    ctx.cdp.open_auth_section(auth3.clone());
    let req = ctx.req(&ids, &l2, &auth3);
    ctx.inject(&req)
        .expect("lease was not consumed by the refusals");
    ctx.end(&auth3);
    ctx.check_journal("A15");
    mark(
        "A15",
        "auth_section_required before open and after close (controller and broker); lease unused",
    );
}

fn a1_a2_stale_document(ctx: &mut Ctx) {
    // A1: ids captured, then the top document moves to another origin.
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    ctx.goto(
        "https://other.example.com/other.html",
        "https://other.example.com/other.html",
        None,
    );
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    let code = ctx.inject(&req).expect_err("origin changed");
    assert!(
        matches!(code, "target_mismatch" | "target_changed"),
        "A1 code {code}"
    );
    // the refusal happened before the broker saw the request (lease unconsumed).
    ctx.end(&auth);
    assert_eq!(ctx.eval("document.querySelector('#other').value"), "");
    ctx.check_journal("A1");
    mark(
        "A1",
        &format!(
            "stale ids after cross-origin navigation rejected: {code}; foreign page input empty"
        ),
    );

    // A2: same-origin new document (new loader) after ids captured.
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    let fresh = ctx.goto(
        &format!("{ORIGIN}/login2.html"),
        &format!("{ORIGIN}/login2.html"),
        None,
    );
    assert_ne!(ids.loader, fresh.loader);
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    let code = ctx.inject(&req).expect_err("document replaced");
    assert!(
        matches!(code, "target_changed" | "target_mismatch"),
        "A2 code {code}"
    );
    ctx.end(&auth);
    assert_eq!(ctx.eval("document.querySelector('#pass').value"), "");
    assert_eq!(
        ctx.eval("String(document.body.dataset.injected)"),
        "undefined"
    );
    // document.open() replacement: stale object ids must not carry a value over.
    ctx.eval("document.open();document.write('<input id=pass type=password>');document.close();1");
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&fresh, &lease, &auth);
    let outcome = ctx.inject(&req);
    ctx.end(&auth);
    eprintln!(
        "A2-document-open outcome: {:?}",
        outcome.as_ref().map(|_| "injected").map_err(|e| *e)
    );
    ctx.check_journal("A2");
    mark(
        "A2",
        &format!("same-origin replacement rejected: {code}; new document input empty"),
    );
}

/// A1 (deterministic): the controller's checks pass, then the test hook moves the page
/// to another origin before the broker's `Runtime.callFunctionOn` reaches CDP.
fn a1_target_changed_hook(ctx: &mut Ctx) {
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    let before = ctx.journal().matches("target_changed").count();
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    ctx.cdp
        .retarget_before_sink_for_test("https://other.example.com/other.html".into());
    let code = ctx
        .inject(&req)
        .expect_err("target changed after the checks");
    assert_eq!(code, "target_changed", "A1 hook code");
    ctx.end(&auth);
    let after = ctx.journal().matches("target_changed").count();
    assert!(after > before, "broker did not record target_changed");
    assert_eq!(
        ctx.eval("location.origin"),
        "https://other.example.com",
        "hook did not move the page"
    );
    assert_eq!(ctx.eval("document.querySelector('#other').value"), "");
    // The broker consumed the lease before the sink: it cannot be replayed.
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    ctx.n += 1;
    let auth2 = format!("auth-a{}", ctx.n);
    ctx.broker
        .open_auth_section(AuthSectionRegistration {
            session_id: "wire-sink".into(),
            auth_section_id: auth2.clone(),
            lease_id: lease.clone(),
            exact_origin: ORIGIN.into(),
            cdp_target_id: ctx.target.clone(),
        })
        .expect("reopen with the used lease");
    ctx.cdp.open_auth_section(auth2.clone());
    let req = ctx.req(&ids, &lease, &auth2);
    assert_eq!(ctx.inject(&req), Err("lease_used"));
    ctx.end(&auth2);
    assert_eq!(ctx.eval("document.querySelector('#pass').value"), "");
    assert_eq!(
        ctx.eval("String(document.body.dataset.injected)"),
        "undefined"
    );
    ctx.check_journal("A1 hook");
    mark(
        "A1-TARGET-CHANGED",
        "target moved after controller checks, before callFunctionOn: broker target_changed; lease_used on replay; other-origin input empty",
    );
}

/// A4 (OOPIF): a cross-site child frame is its own CDP target (type iframe). Injection
/// aimed at that frame is refused as target_mismatch by the controller and the broker.
fn a4_oopif(ctx: &mut Ctx) {
    let sid = ctx.sid.clone();
    ctx.cdp
        .agent_command(
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}),
            Some(&sid),
        )
        .expect("auto-attach");
    let ids = ctx.goto(
        &format!("{ORIGIN}/oopif.html"),
        &format!("{ORIGIN}/oopif.html"),
        None,
    );
    let login = format!("{CROSS_SITE}/login.html");
    let deadline = Instant::now() + Duration::from_secs(30);
    let (child, oopif_sid) = loop {
        assert!(Instant::now() < deadline, "OOPIF target did not attach");
        let found = ctx.cdp.take_agent_events().into_iter().find_map(|e| {
            let info = &e["params"]["targetInfo"];
            (e["method"] == "Target.attachedToTarget" && info["type"] == "iframe")
                .then(|| {
                    Some((
                        info["targetId"].as_str()?.to_owned(),
                        e["params"]["sessionId"].as_str()?.to_owned(),
                    ))
                })
                .flatten()
        });
        if let Some(found) = found {
            break found;
        }
        ctx.agent("Runtime.evaluate", json!({"expression":"1"}))
            .expect("pump events");
        thread::sleep(Duration::from_millis(100));
    };
    let info = ctx
        .agent("Target.getTargetInfo", json!({"targetId":child}))
        .expect("OOPIF target info");
    assert_eq!(
        info["result"]["targetInfo"]["type"], "iframe",
        "child frame is not out-of-process: {info}"
    );
    let oopif = |ctx: &mut Ctx, method: &str, params: Value| {
        let r = ctx
            .cdp
            .agent_command(method, params, Some(&oopif_sid))
            .expect("OOPIF command");
        assert_no_sentinel(&format!("OOPIF reply {method}"), r.to_string().as_bytes());
        r
    };
    let tree = oopif(ctx, "Page.getFrameTree", json!({}));
    let frame = &tree["result"]["frameTree"]["frame"];
    assert_eq!(frame["id"], child.as_str());
    let deadline = Instant::now() + Duration::from_secs(30);
    while oopif(ctx, "Page.getFrameTree", json!({}))["result"]["frameTree"]["frame"]["url"]
        != login.as_str()
    {
        assert!(Instant::now() < deadline, "OOPIF login page did not load");
        thread::sleep(Duration::from_millis(100));
    }
    let tree = oopif(ctx, "Page.getFrameTree", json!({}));
    let frame = &tree["result"]["frameTree"]["frame"];
    let loader = frame["loaderId"].as_str().expect("OOPIF loader").to_owned();

    let (lease, auth) = ctx.begin();
    let mut codes = Vec::new();
    for exact in [CROSS_SITE, ORIGIN] {
        let mut req = ctx.req(&ids, &lease, &auth);
        req.cdp_target_id = child.clone();
        req.frame_id = child.clone();
        req.loader_id = loader.clone();
        req.exact_origin = exact.into();
        req.redirect_chain = vec![exact.into()];
        let code = match ctx.cdp.inject(&req, &oopif_sid, &mut ctx.broker) {
            Ok(v) => panic!("OOPIF injection succeeded: {v}"),
            Err(e) => e.code(),
        };
        assert_eq!(code, "target_mismatch", "OOPIF controller code ({exact})");
        codes.push(code);
    }
    // broker: the real OOPIF target and session instead of the registered page target
    let mut body = ctx.wire(&lease, &auth);
    body["cdp_target_id"] = json!(child);
    body["cdp_session_id"] = json!(oopif_sid);
    body["frame_id"] = json!(child);
    body["loader_id"] = json!(loader);
    assert_eq!(ctx.raw(&body, true).as_deref(), Some("target_mismatch"));
    ctx.end(&auth);
    let value = oopif(
        ctx,
        "Runtime.evaluate",
        json!({"expression":"document.querySelector('#pass').value+'|'+String(document.body.dataset.injected)","returnByValue":true}),
    );
    assert_eq!(value["result"]["result"]["value"], "|undefined");
    ctx.cdp
        .agent_command(
            "Target.setAutoAttach",
            json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}),
            Some(&sid),
        )
        .expect("auto-attach off");
    let _ = ctx.cdp.agent_command(
        "Target.detachFromTarget",
        json!({"sessionId":oopif_sid}),
        None,
    );
    ctx.cdp.take_agent_events();
    ctx.check_journal("A4 OOPIF");
    mark(
        "A4-OOPIF",
        &format!(
            "cross-site OOPIF target {child}: controller {codes:?}, broker target_mismatch; OOPIF input empty"
        ),
    );
}

fn a4_a5_iframes(ctx: &mut Ctx) {
    let ids = ctx.goto(
        &format!("{ORIGIN}/index.html"),
        &format!("{ORIGIN}/index.html"),
        Some("https://other.example.com/other.html"),
    );
    let child = ids.child.clone().expect("child frame");
    let (lease, auth) = ctx.begin();
    let mut req = ctx.req(&ids, &lease, &auth);
    req.frame_id = child.clone();
    assert_eq!(ctx.inject(&req), Err("cross_origin_frame"));
    // broker-level check of the same: frame chain with a foreign origin
    let mut body = ctx.wire(&lease, &auth);
    body["frame_chain"] = json!([ORIGIN, "https://other.example.com"]);
    assert_eq!(ctx.raw(&body, true).as_deref(), Some("cross_origin_frame"));
    // wrong CDP target (OOPIF-style target confusion)
    let mut body = ctx.wire(&lease, &auth);
    body["cdp_target_id"] = json!("SOMEOTHERTARGET");
    assert_eq!(ctx.raw(&body, true).as_deref(), Some("target_mismatch"));
    let mut body = ctx.wire(&lease, &auth);
    body["frame_chain"] = json!([]);
    assert_eq!(ctx.raw(&body, true).as_deref(), Some("empty_frame_chain"));
    ctx.end(&auth);
    assert_eq!(
        ctx.eval("String(document.querySelector('iframe').contentWindow.length)"),
        "0"
    );
    ctx.check_journal("A4");
    mark(
        "A4",
        "cross_origin_frame (controller and broker), target_mismatch, empty_frame_chain",
    );

    // A5: foreign top document, correct-origin login iframe.
    let ids = ctx.goto(
        "https://other.example.com/rev.html",
        "https://other.example.com/rev.html",
        Some(&format!("{ORIGIN}/login.html")),
    );
    let child = ids.child.clone().expect("child");
    let (lease, auth) = ctx.begin();
    let mut req = ctx.req(&ids, &lease, &auth);
    req.frame_id = child.clone();
    let c1 = ctx.inject(&req).expect_err("foreign top");
    let mut req2 = req.clone();
    req2.exact_origin = "https://other.example.com".into();
    let c2 = ctx
        .inject(&req2)
        .expect_err("foreign top, top origin expected");
    assert!(matches!(c1, "target_mismatch" | "cross_origin_frame"));
    assert_eq!(c2, "cross_origin_frame");
    ctx.end(&auth);
    ctx.check_journal("A5");
    mark(
        "A5",
        &format!("reverse iframe: expecting fixture origin -> {c1}; expecting top origin -> {c2}"),
    );
}

fn a6_redisplay_field(ctx: &mut Ctx) {
    let ids = ctx.goto(
        &format!("{ORIGIN}/redisplay.html"),
        &format!("{ORIGIN}/redisplay.html"),
        None,
    );
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    assert_eq!(ctx.inject(&req), Err("redisplay_field"));
    let mut body = ctx.wire(&lease, &auth);
    body["input_type"] = json!("text");
    assert_eq!(ctx.raw(&body, true).as_deref(), Some("redisplay_field"));
    ctx.end(&auth);
    assert_eq!(ctx.eval("document.querySelector('#pass').value"), "");
    // type flipped by a script after the page loaded (before the check)
    let ids = ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    ctx.eval("document.querySelector('#pass').type='text';1");
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    assert_eq!(ctx.inject(&req), Err("redisplay_field"));
    ctx.end(&auth);
    assert_eq!(ctx.eval("document.querySelector('#pass').value"), "");
    ctx.check_journal("A6");
    mark(
        "A6",
        "type=text and script-flipped type rejected as redisplay_field; DOM value empty",
    );
}

fn a3_a11_a14_a16_a17_same_lease(ctx: &mut Ctx) {
    // real redirect: other origin -> meta refresh -> fixture login page
    let ids = ctx.goto(
        "https://other.example.com/redir.html",
        &format!("{ORIGIN}/login.html"),
        None,
    );
    let (lease, auth) = ctx.begin();
    let mut req = ctx.req(&ids, &lease, &auth);
    req.redirect_chain = vec!["https://other.example.com".into(), ORIGIN.into()];
    assert_eq!(ctx.inject(&req), Err("redirected"));
    let mut body = ctx.wire(&lease, &auth);
    body["redirect_chain"] = json!(["https://other.example.com", ORIGIN]);
    assert_eq!(ctx.raw(&body, true).as_deref(), Some("redirected"));
    mark(
        "A3",
        "redirected at controller and broker; lease kept for the next checks",
    );

    // A17: value/length fields
    for extra in ["value", "length"] {
        let mut body = ctx.wire(&lease, &auth);
        body[extra] = if extra == "value" {
            json!("x")
        } else {
            json!(8)
        };
        assert_eq!(
            ctx.raw(&body, true).as_deref(),
            Some("invalid_request"),
            "A17 {extra}"
        );
    }
    mark("A17", "value and length fields -> invalid_request");

    // A11: real worker process with a well-formed request for the live lease/section
    let mut body = ctx.wire(&lease, &auth);
    body["frame_id"] = json!(ids.frame);
    body["loader_id"] = json!(ids.loader);
    let (stdout, _stderr) = run_worker(ctx, &body, None);
    assert_eq!(worker_code(&stdout), "injection_worker_not_allowed");
    assert!(
        stdout.contains("SINK empty"),
        "frame on worker sink: {stdout}"
    );
    let res = stdout
        .lines()
        .find_map(|l| l.strip_prefix("RES "))
        .expect("RES line");
    let rv: Value = serde_json::from_str(res).unwrap_or(Value::Null);
    assert!(
        rv["success"] != true
            && rv["lease_id"].is_null()
            && rv["secret"].is_null()
            && !res.contains("password"),
        "legacy resolve must fail fixed: {res}"
    );
    assert_eq!(
        ipc::bridge_request(b"{}", "t", Path::new("/nonexistent")),
        br#"{"protocol":"agent-browser.plugin.v1","success":false}"#.to_vec()
    );
    mark(
        "A11",
        &format!(
            "worker process -> injection_worker_not_allowed; resolve.sock reply {res}; bridge fixed rejection"
        ),
    );

    // A14: peer is the controller of ANOTHER live session
    let (stdout, _) = run_worker(ctx, &body, Some("other-session"));
    assert_eq!(worker_code(&stdout), "injection_worker_not_allowed");
    let mut missing = ctx.wire(&lease, &auth);
    missing["session_id"] = json!("no-such-session");
    assert_eq!(ctx.raw(&missing, true).as_deref(), Some("session_not_live"));
    mark(
        "A14",
        "other session's controller process -> injection_worker_not_allowed; unknown session -> session_not_live",
    );

    // the lease survived all of the above: real injection with the clean chain succeeds
    req.redirect_chain = vec![ORIGIN.into()];
    let receipt = ctx
        .inject(&req)
        .expect("same lease still valid after rejected attacks");
    assert_eq!(receipt["ok"], true);
    mark(
        "A3b",
        "same lease succeeds after redirected/worker/A17 rejections (not consumed)",
    );

    // A16: second use of the lease
    assert_eq!(ctx.inject(&req), Err("lease_used"));
    let mut body2 = ctx.wire(&lease, &auth);
    body2["frame_id"] = json!(ids.frame);
    assert_eq!(ctx.raw(&body2, true).as_deref(), Some("lease_used"));
    ctx.end(&auth);
    ctx.assert_injected();
    ctx.check_journal("A3/A11/A14/A16/A17");
    let journal = ctx.journal();
    let injected = journal
        .lines()
        .filter(|l| l.contains("\"injected\""))
        .count();
    assert!(injected >= 1);
    mark("A16", "second use -> lease_used, no frame on sink FD");
}

fn a7_a8_a9_a10_observation(ctx: &mut Ctx) {
    let ids = ctx.goto(
        &format!("{ORIGIN}/leak.html"),
        &format!("{ORIGIN}/leak.html"),
        None,
    );
    let (lease, auth) = ctx.begin();
    let req = ctx.req(&ids, &lease, &auth);
    ctx.inject(&req).expect("inject into hostile page");
    // during the section every agent observation is refused
    let during: [(&str, Value); 8] = [
        (
            "Runtime.evaluate",
            json!({"expression":"document.querySelector('#pass').value"}),
        ),
        ("DOM.getOuterHTML", json!({"nodeId":1})),
        ("DOM.getAttributes", json!({"nodeId":1})),
        ("Accessibility.getFullAXTree", json!({})),
        ("Page.captureScreenshot", json!({})),
        ("Page.startScreencast", json!({})),
        (
            "Runtime.callFunctionOn",
            json!({"functionDeclaration":"function(){return 1}"}),
        ),
        ("DOM.getDocument", json!({})),
    ];
    for (method, params) in during {
        let sid = ctx.sid.clone();
        let r = ctx.cdp.agent_command(method, params, Some(&sid));
        assert_eq!(
            r.expect_err(method).code(),
            "auth_section_required",
            "{method}"
        );
    }
    mark(
        "A7a",
        "Runtime.evaluate/DOM.getOuterHTML/DOM.getAttributes/Accessibility refused during section",
    );
    mark(
        "A9a",
        "Page.captureScreenshot/Page.startScreencast refused during section",
    );
    ctx.end(&auth);
    // after close: the input is empty, and the page handler saw the value (A10 fired)
    assert_eq!(ctx.eval("document.querySelector('#pass').value"), "");
    assert_eq!(
        ctx.eval("document.body.dataset.len"),
        SECRET.len().to_string()
    );
    let root = ctx
        .agent("DOM.getDocument", json!({"depth":1}))
        .expect("doc");
    let node = root["result"]["root"]["nodeId"]
        .as_i64()
        .expect("root node");
    let q = ctx
        .agent(
            "DOM.querySelector",
            json!({"nodeId":node,"selector":"#pass"}),
        )
        .expect("query");
    let input = q["result"]["nodeId"].as_i64().expect("input node");
    ctx.agent("DOM.getAttributes", json!({"nodeId":input}))
        .expect("attributes");
    mark(
        "A7",
        "value empty after close; DOM.getAttributes carried no sentinel",
    );
    // A10: the page did console.log(value) and threw an Error containing it.
    // CdpController::read_response drops every unsolicited event, so nothing is forwarded.
    mark(
        "A10",
        "page console.log/throw with value ran (dataset.len matches); no event API on CdpController, all CDP replies clean",
    );
    // A9b: screenshot after the section; the page shows the value in a div (text only check)
    let shot = ctx
        .cdp
        .agent_command(
            "Page.captureScreenshot",
            json!({"format":"png"}),
            Some(&ctx.sid.clone()),
        )
        .expect("screenshot after section");
    let data = shot["result"]["data"].as_str().expect("png data");
    assert_no_sentinel("screenshot base64", data.as_bytes());
    let png = unb64(data);
    assert!(png.starts_with(b"\x89PNG"), "screenshot is a PNG");
    assert_no_sentinel("screenshot bytes", &png);
    mark(
        "A9",
        &format!("screenshot {} bytes free of sentinel text", png.len()),
    );
    // A8: page copied the value to a visible div. Observe exactly what the agent can see.
    a8_redisplay(ctx);
    ctx.check_journal("A7-A10");
}

/// A8 observation paths (ADR-0092): snapshot text, full HTML, DOM tree and
/// accessibility tree. Each must be discarded with `redisplay_detected`.
const A8_PATHS: &[(&str, &str)] = &[
    (
        "Runtime.evaluate",
        r#"{"expression":"document.body.innerText","returnByValue":true}"#,
    ),
    (
        "Runtime.evaluate",
        r#"{"expression":"document.documentElement.outerHTML","returnByValue":true}"#,
    ),
    (
        "Runtime.evaluate",
        r#"{"expression":"btoa(document.body.innerText)","returnByValue":true}"#,
    ),
    (
        "Runtime.evaluate",
        r#"{"expression":"encodeURIComponent(document.body.innerText)","returnByValue":true}"#,
    ),
    (
        "Runtime.evaluate",
        r#"{"expression":"JSON.stringify(JSON.stringify(document.body.innerText))","returnByValue":true}"#,
    ),
    (
        "Runtime.evaluate",
        r#"{"expression":"Array.from(document.body.innerText).map(c=>c+'\u0000').join('')","returnByValue":true}"#,
    ),
    ("DOM.getDocument", r#"{"depth":-1,"pierce":true}"#),
    ("Accessibility.getFullAXTree", r#"{}"#),
];

fn a8_redisplay(ctx: &mut Ctx) {
    let sid = ctx.sid.clone();
    assert!(
        ctx.cdp.redisplay_guards() >= 1,
        "controller holds no redisplay guard from the broker"
    );
    // Negative control: the same observations through the unguarded trusted
    // controller path (agent_command minus the guard) do carry the sentinel,
    // so the attack is real and the guard is what stops it.
    let mut leaked = 0;
    for (method, params) in A8_PATHS {
        let params: Value = serde_json::from_str(params).expect("params");
        let raw = ctx
            .cdp
            .controller_command(method, params, Some(&sid))
            .expect("unguarded observation");
        if contains(raw.to_string().as_bytes(), SECRET.as_bytes()) {
            leaked += 1;
        }
    }
    assert!(
        leaked >= 2,
        "negative control: page copy of the value not visible unguarded"
    );
    for (method, params) in A8_PATHS {
        let params: Value = serde_json::from_str(params).expect("params");
        match ctx.cdp.agent_command(method, params, Some(&sid)) {
            Err(e) => assert_eq!(e.code(), "redisplay_detected", "{method}"),
            Ok(reply) => {
                assert_no_sentinel(method, reply.to_string().as_bytes());
                panic!("{method}: observation was not discarded");
            }
        }
    }
    // Screenshot path: pixels are not text; the reply is guarded and sentinel free.
    match ctx.cdp.agent_command(
        "Page.captureScreenshot",
        json!({"format":"png"}),
        Some(&sid),
    ) {
        Ok(shot) => assert_no_sentinel("A8 screenshot", shot.to_string().as_bytes()),
        Err(e) => assert_eq!(e.code(), "redisplay_detected"),
    }
    // Events after the section pass the guard too.
    for e in ctx.cdp.take_agent_events() {
        assert_no_sentinel("A8 event", e.to_string().as_bytes());
    }
    mark(
        "A8",
        &format!(
            "page-copied value discarded with redisplay_detected on {} agent paths (unguarded control leaked on {leaked})",
            A8_PATHS.len()
        ),
    );
}

/// Negative control for the guard itself: an observation that is only
/// guarded by a guard for a different value is let through.
fn a8_guard_negative_control() {
    use celeris_credentiald::injection::RedisplayGuard;
    let right = RedisplayGuard::new(SECRET);
    let wrong = RedisplayGuard::new("some-other-value-entirely");
    let obs = json!({"result":{"result":{"value":format!("<div>{SECRET}</div>")}}});
    assert!(right.exposes_json(&obs));
    assert!(
        !wrong.exposes_json(&obs),
        "a mismatched guard must not detect"
    );
    mark(
        "A8n",
        "guard for another value does not detect (guard is load-bearing)",
    );
}

fn a12_browser_reach(ctx: &mut Ctx) {
    ctx.goto(
        &format!("{ORIGIN}/login.html"),
        &format!("{ORIGIN}/login.html"),
        None,
    );
    let run = ctx.root.path().join("run");
    let sock = run.join("celeris-credentiald/injection.sock");
    let resolve = run.join("celeris-credentiald/resolve.sock");
    for path in [&sock, &resolve] {
        let js = format!(
            "fetch('file://{}').then(()=>'reachable',()=>'blocked')",
            path.display()
        );
        assert_eq!(ctx.eval(&js), "blocked", "page reached {}", path.display());
    }
    let root = format!("/proc/{}/root", ctx.rt_pid);
    assert!(
        Path::new(&format!("{root}/session")).exists(),
        "control: sandbox root should expose /session"
    );
    for path in [&sock, &resolve, &run] {
        assert!(
            !Path::new(&format!("{root}{}", path.display())).exists(),
            "{} visible inside the isolated runtime",
            path.display()
        );
    }
    ctx.check_journal("A12");
    mark(
        "A12",
        "page fetch(file://...sock) blocked; broker run dir absent from the runtime root",
    );
}

fn a13_other_uid(ctx: &Ctx) {
    let dir = tempfile::tempdir().expect("dir");
    for name in ["config", "data"] {
        let p = dir.path().join(name);
        std::fs::create_dir(&p).expect("dir");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).expect("mode");
    }
    let manual = ManualProvider::open(
        dir.path().join("config/keys"),
        dir.path().join("data/vault"),
    )
    .expect("provider");
    manual.initialize_key().expect("key");
    let broker = Arc::new(Broker::new(manual, dir.path().join("data/audit")).expect("broker"));
    let registry = Arc::new(LiveRegistry::default());
    let service = InjectionService::new(broker, registry, Admission::SameUidHarness);
    let mut me = PeerCred {
        uid: 0,
        pid: std::process::id(),
    };
    // SAFETY: geteuid has no preconditions.
    me.uid = unsafe { nix::libc::geteuid() } + 1;
    let body = json!({"v":1,"request_id":"uid-1","session_id":"wire-sink",
        "cdp_target_id":ctx.target,"frame_id":"F","loader_id":"L","frame_chain":[ORIGIN],
        "redirect_chain":[ORIGIN],"selector":"#pass","object_id":"O","field":"password",
        "input_type":"password","auth_section_id":"a","lease_id":"l","cdp_command_id":1});
    let reply = service.handle(me, body.to_string().as_bytes(), &mut DummySink);
    assert!(!reply.ok);
    assert_eq!(reply.code.as_deref(), Some("peer_uid_mismatch"));
    mark(
        "A13",
        "unit-level: PeerCred of another uid -> peer_uid_mismatch (real other-UID process not possible under unshare -r; left unresolved)",
    );
}

#[test]
fn real_browser_injection_attack_matrix() {
    for t in ["unshare", "ip", "openssl", "bwrap", "python3"] {
        tool(t);
    }
    browser();
    let out = Command::new(tool("unshare"))
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "inner_injection_attacks",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .output()
        .expect("netns test starts");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "attack matrix failed: {stdout}\n{stderr}"
    );
    for m in [
        "A0",
        "A1",
        "A2",
        "A3",
        "A3b",
        "A4",
        "A5",
        "A6",
        "A7",
        "A7a",
        "A8",
        "A8n",
        "A9",
        "A9a",
        "A10",
        "A11",
        "A12",
        "A13",
        "A14",
        "A15",
        "A16",
        "A17",
        "A1-TARGET-CHANGED",
        "A4-OOPIF",
    ] {
        assert!(
            stderr.contains(&format!("ATTACK-{m}-OK")),
            "missing ATTACK-{m}-OK\n{stderr}"
        );
    }
    // A8 (ADR-0092): a pass, never a reported gap.
    assert!(
        stderr.contains("ATTACK-A8-OK") && !stderr.contains("ATTACK-A8-GAP"),
        "A8 not passed\n{stderr}"
    );
    assert!(stderr.contains("ATTACKS-ALL-DONE"));
    for line in stderr.lines().filter(|l| {
        l.starts_with("ATTACK-A1-TARGET-CHANGED-OK") || l.starts_with("ATTACK-A4-OOPIF-OK")
    }) {
        println!("{line}");
    }
    // whatever the child printed is itself a surface
    assert_no_sentinel("inner stdout", stdout.as_bytes());
    assert_no_sentinel("inner stderr", stderr.as_bytes());
    eprintln!("{stderr}");
}

#[test]
fn inner_injection_attacks() {
    if std::env::var(INNER).is_ok() {
        inner();
    }
}
