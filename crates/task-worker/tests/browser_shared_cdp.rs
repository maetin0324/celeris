//! A real bwrap/Chromium pipe shared with an agent-style WebSocket client.
//! The fixture and DNS exist only in the test network namespace.
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use task_worker::browser_cdp_sink::{
    BrokerClient, CdpController, InjectionError, InjectionRequest, PendingInjection,
};
use task_worker::browser_runtime::{DEFAULT_MAX_EGRESS, EgressRelay, RuntimeSpec};
use task_worker::browser_shared_cdp::{AUTH_ERROR, SharedCdp};
use task_worker::browser_supervisor::{Supervisor, SupervisorOptions};

const INNER: &str = "CELERIS_SHARED_CDP_INNER";
const IP: &str = "93.184.216.34";
const ORIGIN: &str = "https://fixture.example.com";
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECRET: &str = "shared-cdp-secret-sentinel";
/// `CELERIS_ISOLATION_TESTS`: `skip` は飛ばす、`require` は preflight で飛ばさない（release gate）。
const ISOLATION: &str = "CELERIS_ISOLATION_TESTS";
/// preflight の 1 段の時間の上限（環境が無い sandbox で何十秒も待たない）。
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(20);
// ADR-0079 付記「R7-12」D4: WebSocket の frame は読み切ってから判定する（1 回の `recv` は header だけを返すことが
// ある。高負荷時・並走時に `b'\x81~\x00\xdf'` だけを読んで assert に落ち、host は 60 秒待ってから失敗していた）。
// probe の例外は `/session/probe.err` に書き、host はそれを見たら待たずにその内容で失敗する。
// 接続〜CDP 応答までを 1 試行とし、EOF・拒否は期限（host の 60 秒より短い 55 秒）内で再試行する。
const TCP_PROBE: &str = r#"import socket, time, sys, traceback
from pathlib import Path
def _report(t, v, tb):
    Path('/session/probe.err').write_text(''.join(traceback.format_exception(t, v, tb)))
sys.excepthook = _report
deadline = time.monotonic() + 55
request = b'GET /aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa HTTP/1.1\r\nHost: 127.0.0.1:9223\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n'
payload = b'{"id":1,"method":"Target.getTargets","params":{}}'
frame = bytes([0x81, 0x80 | len(payload), 1, 2, 3, 4]) + bytes(c ^ [1,2,3,4][i%4] for i,c in enumerate(payload))
def recv_exact(sock, n):
    buf = b''
    while len(buf) < n:
        if time.monotonic() >= deadline:
            raise TimeoutError('CDP relay deadline exceeded after %r' % buf)
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError('relay closed after %r' % buf)
        buf += chunk
    return buf
last_error = None
while True:
    try:
        with socket.create_connection(('127.0.0.1', 9223), timeout=2) as sock:
            sock.settimeout(min(2, max(.1, deadline - time.monotonic())))
            sock.sendall(request)
            head = b''
            while not head.endswith(b'\r\n\r\n'):
                head += recv_exact(sock, 1)
                if len(head) > 8192:
                    raise ValueError('WebSocket upgrade header too large')
            if not head.startswith(b'HTTP/1.1 101'):
                raise ValueError('WebSocket upgrade rejected: ' + repr(head))
            sock.sendall(frame)
            header = recv_exact(sock, 2)
            if header[0] != 0x81:
                raise ValueError('unexpected WebSocket frame header: %r' % header)
            size = header[1] & 0x7f
            if size == 126:
                size = int.from_bytes(recv_exact(sock, 2), 'big')
            elif size == 127:
                size = int.from_bytes(recv_exact(sock, 8), 'big')
            if size > 65536:
                raise ValueError('CDP response too large: %d' % size)
            reply = recv_exact(sock, size)
            if b'targetInfos' not in reply:
                raise ValueError('CDP response without targetInfos: %r' % reply)
            Path('/session/tcp-probe.ok').write_text('connected')
            time.sleep(90)
        break
    except (OSError, ValueError) as error:
        last_error = error
        if time.monotonic() >= deadline:
            raise RuntimeError('CDP forwarding unavailable') from last_error
        time.sleep(.02)
"#;

struct FakeBroker;
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
        assert!(!request.to_string().contains(SECRET));
        Ok(Box::new(Pending(thread::spawn(move || {
            let mut command = serde_json::to_vec(&json!({
                "id":request["cdp_command_id"], "sessionId":request["cdp_session_id"],
                "method":"Runtime.callFunctionOn", "params":{
                    "objectId":request["object_id"],
                    "functionDeclaration":"function(value){if(!this.isConnected||this.type!=='password')return 'target_changed';Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(this,value);return 'ok';}",
                    "arguments":[{"value":SECRET}], "returnByValue":true, "silent":true
                }
            })).map_err(|_| InjectionError::SinkFailed)?;
            command.push(0);
            // SAFETY: the connected seqpacket fd is valid and command is readable.
            let sent = unsafe {
                nix::libc::send(
                    sink.as_raw_fd(),
                    command.as_ptr().cast(),
                    command.len(),
                    nix::libc::MSG_NOSIGNAL,
                )
            };
            if sent != command.len() as isize {
                return Err(InjectionError::SinkFailed);
            }
            let mut reply = [0; 4096];
            // SAFETY: the connected seqpacket fd is valid and reply is writable.
            let len = unsafe {
                nix::libc::recv(sink.as_raw_fd(), reply.as_mut_ptr().cast(), reply.len(), 0)
            };
            if len <= 0 {
                return Err(InjectionError::SinkFailed);
            }
            let value: Value = serde_json::from_slice(&reply[..len as usize])
                .map_err(|_| InjectionError::SinkFailed)?;
            if value["result"]["result"]["value"] != "ok" {
                return Err(InjectionError::TargetChanged);
            }
            Ok(
                json!({"v":1,"request_id":request["request_id"],"ok":true,"receipt":{
                    "lease_id":request["lease_id"],"auth_section_id":request["auth_section_id"],
                    "session_id":request["session_id"],"cdp_target_id":request["cdp_target_id"],
                    "frame_id":request["frame_id"],"loader_id":request["loader_id"],
                    "field":request["field"],"injected_at":1
                },"redisplay_guard":celeris_credentiald::injection::RedisplayGuard::new(SECRET).to_wire()}),
            )
        }))))
    }
}

fn tool(name: &str) -> PathBuf {
    let path = PathBuf::from("/usr/bin").join(name);
    assert!(path.is_file(), "required tool missing: {}", path.display());
    path
}

fn find_browser() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("CELERIS_TEST_BROWSER") {
        let path = PathBuf::from(path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "CELERIS_TEST_BROWSER {} is not a file",
                path.display()
            ))
        };
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let cache = Path::new(&home).join(".cache/ms-playwright");
    let mut found: Vec<_> = std::fs::read_dir(&cache)
        .map_err(|e| format!("Playwright cache {}: {e}", cache.display()))?
        .filter_map(Result::ok)
        .map(|e| {
            e.path()
                .join("chrome-headless-shell-linux64/chrome-headless-shell")
        })
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found
        .pop()
        .ok_or_else(|| format!("no chrome-headless-shell under {}", cache.display()))
}

fn browser() -> PathBuf {
    find_browser().expect("chrome-headless-shell")
}

/// `command` を `timeout` まで待つ（出力は捨てる。stderr の末尾だけ理由に使う）。時間切れは kill して `Err`。
fn run_bounded(mut command: Command, what: &str, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{what}: cannot start: {e}"))?;
    let mut stderr = child.stderr.take();
    let reader = thread::spawn(move || {
        let mut text = String::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_string(&mut text);
        }
        text
    });
    let until = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{what}: no answer within {timeout:?}"));
            }
            Err(e) => return Err(format!("{what}: wait failed: {e}")),
        }
    };
    let text = reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let tail: String = text.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        Err(format!("{what}: {status}: {tail}"))
    }
}

/// ADR-0079 付記「R7-12」D4: このテストが要る環境（道具・browser・unprivileged な user + net namespace・その中の
/// loopback の TCP・bwrap の入れ子の namespace）が使えるか。worker の sandbox（codex workspace-write の seccomp、
/// ADR-0095 の namespace の中の WU の check）では使えないことがある。どの段も [`PREFLIGHT_TIMEOUT`] で打ち切る。
fn preflight() -> Result<(), String> {
    for name in ["unshare", "ip", "openssl", "bwrap", "python3"] {
        let path = PathBuf::from("/usr/bin").join(name);
        if !path.is_file() {
            return Err(format!("required tool missing: {}", path.display()));
        }
    }
    find_browser()?;
    for var in [
        "CARGO_BIN_EXE_celeris-browser-sandboxd",
        "CARGO_BIN_EXE_celeris-browser-egress",
    ] {
        if std::env::var_os(var).is_none() {
            return Err(format!(
                "{var} is not set (run through cargo test / nextest)"
            ));
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("test binary: {e}"))?;
    let mut command = Command::new("/usr/bin/unshare");
    command
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(exe)
        .args(["--exact", "inner_shared_cdp", "--test-threads=1"])
        .env(INNER, "preflight");
    run_bounded(
        command,
        "user+net namespace with loopback TCP and nested bwrap",
        PREFLIGHT_TIMEOUT,
    )
}

/// preflight の netns の中（`CELERIS_SHARED_CDP_INNER=preflight`）: loopback を上げ、TCP の bind / connect / 1 byte の
/// 往復、bwrap の入れ子の user / net namespace が動くかを見る。
fn inner_preflight() {
    assert!(
        Command::new(tool("ip"))
            .args(["link", "set", "lo", "up"])
            .status()
            .expect("ip")
            .success(),
        "ip link set lo up"
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback TCP bind");
    let addr = listener.local_addr().expect("local addr");
    let mut client = TcpStream::connect(addr).expect("loopback TCP connect");
    let (mut server, _) = listener.accept().expect("loopback TCP accept");
    client.write_all(b"x").expect("loopback TCP write");
    let mut byte = [0];
    server
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    server.read_exact(&mut byte).expect("loopback TCP read");
    assert!(
        Command::new(tool("bwrap"))
            .args([
                "--unshare-user",
                "--unshare-pid",
                "--unshare-net",
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--",
                "/usr/bin/true",
            ])
            .status()
            .expect("bwrap")
            .success(),
        "nested bwrap namespaces"
    );
}

fn dns(listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        let mut length = [0; 2];
        if s.read_exact(&mut length).is_err() {
            continue;
        }
        let mut query = vec![0; u16::from_be_bytes(length) as usize];
        if s.read_exact(&mut query).is_err() || query.len() < 17 {
            continue;
        }
        let mut pos = 12;
        while pos < query.len() && query[pos] != 0 {
            pos += query[pos] as usize + 1;
        }
        if pos + 4 >= query.len() {
            continue;
        }
        let qtype = u16::from_be_bytes([query[pos + 1], query[pos + 2]]);
        let mut response = query[..pos + 5].to_vec();
        response[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        response[6..8].copy_from_slice(&u16::from(qtype == 1).to_be_bytes());
        if qtype == 1 {
            response.extend([0xc0, 0x0c]);
            response.extend(qtype.to_be_bytes());
            response.extend([0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 216, 34]);
        }
        let _ = s.write_all(&(response.len() as u16).to_be_bytes());
        let _ = s.write_all(&response);
    }
}

fn fixture(dir: &Path) -> Child {
    std::fs::write(
        dir.join("index.html"),
        "<html><head><title>shared-cdp-fixture</title></head><body>same page<input id=pass type=password></body></html>",
    )
    .expect("html");
    let status = Command::new(tool("openssl"))
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
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("cert");
    assert!(status.success(), "certificate");
    let child = Command::new(tool("openssl"))
        .args([
            "s_server", "-quiet", "-WWW", "-cert", "cert.pem", "-key", "key.pem", "-accept",
        ])
        .arg(format!("{IP}:443"))
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("TLS fixture");
    let until = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect((IP, 443)).is_err() {
        assert!(Instant::now() < until, "TLS fixture never listened");
        thread::sleep(Duration::from_millis(50));
    }
    child
}

struct Agent {
    stream: UnixStream,
    next: u64,
}

impl Agent {
    fn connect(path: &Path) -> Self {
        let mut stream = UnixStream::connect(path).expect("relay socket");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("timeout");
        write!(stream, "GET /{TOKEN} HTTP/1.1\r\nHost: 127.0.0.1:9223\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").expect("handshake");
        let mut response = Vec::new();
        let mut b = [0];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut b).expect("upgrade response");
            response.push(b[0]);
            assert!(response.len() < 8192);
        }
        assert!(
            String::from_utf8_lossy(&response).starts_with("HTTP/1.1 101"),
            "upgrade: {response:?}"
        );
        Self { stream, next: 1 }
    }

    fn send(&mut self, value: Value) {
        let bytes = value.to_string().into_bytes();
        let mut frame = vec![0x81];
        if bytes.len() < 126 {
            frame.push(0x80 | bytes.len() as u8);
        } else {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        }
        frame.extend([1, 2, 3, 4]);
        for (i, byte) in bytes.iter().enumerate() {
            frame.push(byte ^ [1, 2, 3, 4][i % 4]);
        }
        self.stream.write_all(&frame).expect("WebSocket send");
    }

    fn recv(&mut self) -> Value {
        let mut head = [0; 2];
        self.stream.read_exact(&mut head).expect("WebSocket header");
        assert_eq!(head[0], 0x81);
        let mut n = (head[1] & 0x7f) as usize;
        if n == 126 {
            let mut size = [0; 2];
            self.stream.read_exact(&mut size).expect("size");
            n = u16::from_be_bytes(size) as usize;
        }
        if n == 127 {
            let mut size = [0; 8];
            self.stream.read_exact(&mut size).expect("size");
            n = u64::from_be_bytes(size) as usize;
        }
        let mut body = vec![0; n];
        self.stream.read_exact(&mut body).expect("WebSocket body");
        serde_json::from_slice(&body).expect("CDP JSON")
    }

    fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Value {
        let id = self.next;
        self.next += 1;
        let mut command = json!({"id":id,"method":method,"params":params});
        if let Some(s) = session {
            command["sessionId"] = s.into();
        }
        self.send(command);
        loop {
            let reply = self.recv();
            if reply["id"] == id {
                return reply;
            }
        }
    }
}

fn inner() {
    assert!(
        Command::new(tool("ip"))
            .args(["link", "set", "lo", "up"])
            .status()
            .expect("ip")
            .success()
    );
    assert!(
        Command::new(tool("ip"))
            .args(["addr", "add", &format!("{IP}/32"), "dev", "lo"])
            .status()
            .expect("ip")
            .success()
    );
    thread::spawn(|| dns(TcpListener::bind("127.0.0.1:53").expect("DNS")));
    let fixture_dir = tempfile::tempdir().expect("fixture dir");
    let mut fixture = fixture(fixture_dir.path());
    let session = tempfile::tempdir().expect("session dir");
    std::fs::write(session.path().join("probe.py"), TCP_PROBE).expect("probe script");
    let browser = browser();
    let sandboxd =
        PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-browser-sandboxd").expect("sandboxd"));
    let proxy =
        PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("egress"));
    let spec = RuntimeSpec {
        userns: task_worker::browser_runtime::UsernsMode::Unshare,
        bwrap: tool("bwrap"), session_id: "shared-cdp".into(), session_dir: session.path().to_path_buf(),
        ro_dirs: vec![browser.parent().expect("browser parent").to_path_buf(), sandboxd.parent().expect("sandboxd parent").to_path_buf()],
        argv: vec![sandboxd.into_os_string(), OsString::from("--shared-cdp"), browser.into_os_string(),
            OsString::from("/usr/bin/python3"), OsString::from("/session/probe.py")],
        cdp_pipe: true,
        egress: Some(EgressRelay { proxy, policy: br#"{"allow":["fixture.example.com:443"],"resolver":"127.0.0.1","allow_ipv6":false}"#.to_vec(), max_concurrent: DEFAULT_MAX_EGRESS }),
    };
    let mut supervisor =
        Supervisor::start(spec, SupervisorOptions::new(session.path().join("records")))
            .expect("supervisor");
    let cdp = CdpController::new(
        supervisor.cdp_write.take().expect("CDP write"),
        supervisor.cdp_read.take().expect("CDP read"),
    );
    let relay = SharedCdp::start(
        cdp,
        &session.path().join("cdp-relay.sock"),
        TOKEN.into(),
        vec!["fixture.example.com".into()],
    )
    .expect("relay");
    // 高負荷時（workspace 全体の test と並走）は sandbox 内の browser 起動が 10 秒を超えるので長めに待つ。
    // ADR-0079 付記「R7-12」D4: probe が例外で終わったら（`probe.err`）待たずにその内容で失敗する。
    let until = Instant::now() + Duration::from_secs(60);
    while !session.path().join("tcp-probe.ok").exists() {
        if let Ok(err) = std::fs::read_to_string(session.path().join("probe.err")) {
            panic!("sandbox TCP probe failed:\n{err}");
        }
        assert!(
            Instant::now() < until,
            "sandbox TCP to controller relay failed"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let controller = relay.controller();
    let target = controller
        .lock()
        .expect("controller lock")
        .agent_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .expect("target")["result"]["targetId"]
        .as_str()
        .expect("target id")
        .to_owned();
    let mut agent = Agent::connect(&session.path().join("cdp-relay.sock"));
    let attached = agent.call(
        "Target.attachToTarget",
        json!({"targetId":target,"flatten":true}),
        None,
    );
    let page = attached["result"]["sessionId"]
        .as_str()
        .expect("agent session")
        .to_owned();
    let own = controller
        .lock()
        .expect("controller lock")
        .controller_command(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )
        .expect("controller attaches to same page")["result"]["sessionId"]
        .as_str()
        .expect("controller session")
        .to_owned();
    assert_ne!(page, own);
    assert!(agent.call("Runtime.enable", json!({}), Some(&page))["error"].is_null());
    assert!(
        agent.call(
            "Security.setIgnoreCertificateErrors",
            json!({"ignore":true}),
            Some(&page)
        )["error"]
            .is_null()
    );
    let nav = agent.call(
        "Page.navigate",
        json!({"url":format!("{ORIGIN}/index.html")}),
        Some(&page),
    );
    assert!(nav["error"].is_null(), "navigation: {nav}");
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let result = agent.call(
            "Runtime.evaluate",
            json!({"expression":"document.title","returnByValue":true}),
            Some(&page),
        );
        if result["result"]["result"]["value"] == "shared-cdp-fixture" {
            break;
        }
        assert!(Instant::now() < until, "fixture never loaded: {result}");
        thread::sleep(Duration::from_millis(100));
    }
    let info = controller
        .lock()
        .expect("controller lock")
        .agent_command("Target.getTargetInfo", json!({"targetId":target}), None)
        .expect("target info");
    assert_eq!(
        info["result"]["targetInfo"]["url"],
        format!("{ORIGIN}/index.html")
    );
    controller
        .lock()
        .expect("controller lock")
        .open_auth_section("h3".into());
    let tree = controller
        .lock()
        .expect("controller lock")
        .controller_command("Page.getFrameTree", json!({}), Some(&own))
        .expect("frame tree");
    let frame = &tree["result"]["frameTree"]["frame"];
    let request = InjectionRequest {
        request_id: "shared-cdp-injection".into(),
        session_id: "shared-cdp".into(),
        cdp_target_id: target.clone(),
        frame_id: frame["id"].as_str().expect("frame id").into(),
        loader_id: frame["loaderId"].as_str().expect("loader id").into(),
        exact_origin: ORIGIN.into(),
        redirect_chain: vec![ORIGIN.into()],
        selector: "#pass".into(),
        field: "password".into(),
        auth_section_id: "h3".into(),
        lease_id: "lease-shared".into(),
    };
    let receipt = controller
        .lock()
        .expect("controller lock")
        .inject(&request, &own, &mut FakeBroker)
        .expect("inject into agent page");
    assert_eq!(receipt["ok"], true);
    assert!(!receipt.to_string().contains(SECRET));
    let injected = controller
        .lock()
        .expect("controller lock")
        .controller_command(
            "Runtime.evaluate",
            json!({"expression":"document.querySelector('#pass').value","returnByValue":true}),
            Some(&own),
        )
        .expect("trusted verification");
    assert_eq!(injected["result"]["result"]["value"], SECRET);
    let blocked = agent.call(
        "Runtime.evaluate",
        json!({"expression":"document.documentElement.outerHTML"}),
        Some(&page),
    );
    assert_eq!(blocked["error"]["message"], AUTH_ERROR);
    let blocked = agent.call("Page.captureScreenshot", json!({}), Some(&page));
    assert_eq!(blocked["error"]["message"], AUTH_ERROR);
    controller
        .lock()
        .expect("controller lock")
        .controller_command(
            "Runtime.evaluate",
            json!({"expression":"console.log('hidden')"}),
            Some(&own),
        )
        .expect("trusted auth operation");
    agent
        .stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("short timeout");
    let mut byte = [0];
    assert!(
        agent.stream.read(&mut byte).is_err(),
        "auth event crossed relay"
    );
    controller
        .lock()
        .expect("controller lock")
        .close_auth_section()
        .expect("close auth");
    let resumed = agent.call(
        "Runtime.evaluate",
        json!({"expression":"document.title","returnByValue":true}),
        Some(&page),
    );
    assert_eq!(resumed["result"]["result"]["value"], "shared-cdp-fixture");
    let cleared = agent.call(
        "Runtime.evaluate",
        json!({"expression":"document.querySelector('#pass').value","returnByValue":true}),
        Some(&page),
    );
    assert_eq!(cleared["result"]["result"]["value"], "");
    eprintln!("SHARED-CDP-EVIDENCE same_target blocked_methods blocked_events resumed");
    drop(relay);
    supervisor.stop();
    let _ = fixture.kill();
    let _ = fixture.wait();
}

#[test]
fn real_shared_cdp_and_auth_section() {
    // ADR-0079 付記「R7-12」D4: 環境が無ければ理由を出して飛ばす（`require` なら飛ばさず失敗する）。
    match std::env::var(ISOLATION).as_deref() {
        Ok("skip") => {
            eprintln!("SKIPPED (not passed): {ISOLATION}=skip");
            return;
        }
        Ok("require") => {
            if let Err(reason) = preflight() {
                panic!("{ISOLATION}=require but the environment is unavailable: {reason}");
            }
        }
        _ => {
            if let Err(reason) = preflight() {
                eprintln!(
                    "SKIPPED (environment unavailable, not passed): {reason} (set {ISOLATION}=require to fail instead)"
                );
                return;
            }
        }
    }
    for name in ["unshare", "ip", "openssl", "bwrap"] {
        tool(name);
    }
    browser();
    let output = Command::new(tool("unshare"))
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "inner_shared_cdp",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .output()
        .expect("netns fixture");
    assert!(
        output.status.success(),
        "inner failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("SHARED-CDP-EVIDENCE"),
        "inner did not run"
    );
}

#[test]
fn inner_shared_cdp() {
    match std::env::var(INNER).as_deref() {
        Ok("preflight") => inner_preflight(),
        Ok(_) => inner(),
        Err(_) => {}
    }
}
