//! 2026-10-10 (production, launcher protocol 8): screenshot / download run by the real
//! agent-browser 0.38.1 through the sandbox's action runner (`browser_action.py`) inside the real
//! bwrap runtime (`celeris-browser-sandboxd --shared-cdp`), against a fixture in a private network
//! namespace (no external route). The files must land in `/session/output` under the runner's
//! names, readable by another host UID (the launcher reads them for the daemon).
mod userns_gate;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use task_worker::browser_cdp_sink::CdpController;
use task_worker::browser_runtime::{DEFAULT_MAX_EGRESS, EgressRelay, IsolatedRuntime, RuntimeSpec};

const INNER: &str = "CELERIS_SANDBOX_ARTIFACTS_INNER";
const FIXTURE_IP: &str = "93.184.216.34";
const ORIGIN: &str = "https://fixture.example.com";
const POLICY: &str = r#"{"allow":["fixture.example.com:443","files.example.com:443"],"resolver":"127.0.0.1","allow_ipv6":false}"#;
const PDF: &[u8] =
    b"%PDF-1.4\n% celeris artifact fixture\n1 0 obj << >> endobj\ntrailer << >>\n%%EOF\n";

fn tool(name: &str) -> PathBuf {
    let p = PathBuf::from("/usr/bin").join(name);
    assert!(p.exists(), "{name} not found at {p:?}");
    p
}

fn browser() -> PathBuf {
    if let Ok(p) = std::env::var("CELERIS_TEST_BROWSER") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME");
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

/// The agent-browser 0.38.1 entry point (resolved like the daemon does) and its package dir.
fn agent_browser() -> Option<(PathBuf, PathBuf)> {
    let candidates = [
        std::env::var_os("CELERIS_TEST_AGENT_BROWSER").map(PathBuf::from),
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join(".local/celeris/npm/agent-browser-0.38.1/node_modules/.bin/agent-browser")
        }),
    ];
    let exe = candidates.into_iter().flatten().find(|p| {
        Command::new(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "agent-browser 0.38.1")
    })?;
    let real = exe.canonicalize().ok()?;
    let package = real.parent()?.parent()?.to_path_buf();
    Some((real, package))
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
        let name = labels.join(".");
        let answer = (name == "fixture.example.com" || name == "files.example.com") && qtype == 1;
        let mut response = q[..pos + 5].to_vec();
        response[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        response[6..8].copy_from_slice(&u16::from(answer).to_be_bytes());
        if answer {
            response.extend([0xc0, 0x0c]);
            response.extend(qtype.to_be_bytes());
            response.extend([0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 216, 34]);
        }
        let _ = stream.write_all(&(response.len() as u16).to_be_bytes());
        let _ = stream.write_all(&response);
    }
}

/// A manaba-like page linking a PDF served as an attachment and one served inline.
fn fixture(dir: &Path) -> Child {
    std::fs::write(dir.join("slides.pdf"), PDF).expect("pdf");
    std::fs::write(
        dir.join("server.py"),
        r#"
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer as HTTPServer
from pathlib import Path
PDF = Path('slides.pdf').read_bytes()
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == '/page.html':
            body = (b'<html><body><h1>Week 3</h1><p>Material for the lab.</p>'
                    b'<a href="/ct/attach_Slides1.pdf">Attachment PDF</a> '
                    b'<a href="/ct/inline_Slides2.pdf">Inline PDF</a> '
                    b'<a href="/ct/redirect_Slides4.pdf">Redirect PDF</a> '
                    b'<a href="/ct/elsewhere_Slides5.pdf">Elsewhere PDF</a></body></html>')
            self.send_response(200); self.send_header('Content-Type', 'text/html')
        elif self.path == '/slow.html':
            # A course page whose PDF answers after 7 s (付記 2026-10-10i), linked directly and through
            # a mousedown handler that starts the navigation before the click ends.
            body = (b'<html><body><h1>Slow</h1>'
                    b'<a href="/ct/slow_Slides6.pdf">Slow PDF</a> '
                    b'<a href="/ct/slow_Slides6.pdf" onmousedown="location.href=this.href">MouseDown PDF</a>'
                    b'</body></html>')
            self.send_response(200); self.send_header('Content-Type', 'text/html')
        elif self.path == '/blank.html':
            # manaba-like file links (付記 2026-10-10j): a new tab (target=_blank) to the file, fast and
            # slow, and an intermediate page that moves on to the file by script.
            body = (b'<html><body><h1>Materials</h1>'
                    b'<a href="/ct/page_9_file/Slides1.pdf?view=full" target="_blank">Blank PDF</a> '
                    b'<a href="/ct/slow_Slides6.pdf" target="_blank">Blank Slow PDF</a> '
                    b'<a href="/ct/viewer_page.html">Viewer PDF</a> '
                    b'<a href="https://files.example.com/ct/inline_Slides2.pdf" target="_blank" onclick="void 0">Elsewhere Blank PDF</a>'
                    b'</body></html>')
            self.send_response(200); self.send_header('Content-Type', 'text/html')
        elif self.path.startswith('/ct/page_9_file/Slides1.pdf'):
            body = PDF; self.send_response(200); self.send_header('Content-Type', 'application/pdf')
        elif self.path == '/ct/viewer_page.html':
            body = (b'<html><body><p>Opening the file</p><script>'
                    b'setTimeout(function(){location.href="/ct/page_9_file/Slides1.pdf?view=full"},300)'
                    b'</script></body></html>')
            self.send_response(200); self.send_header('Content-Type', 'text/html')
        elif self.path == '/ct/slow_Slides6.pdf':
            import time; time.sleep(7)
            body = PDF; self.send_response(200); self.send_header('Content-Type', 'application/pdf')
        elif self.path == '/dialogs.html':
            # Course-page links whose click opens a JavaScript dialog (付記 2026-10-10h).
            body = (b'<html><body><h1>Dialogs</h1>'
                    b'<a href="/ct/page_x/Slides1.pdf?view=full" onclick="alert(\'Starting\')">Alert PDF</a> '
                    b'<a href="/ct/page_x/Slides1.pdf?view=full" onclick="return confirm(\'Download?\')">Confirm PDF</a>'
                    b'</body></html>')
            self.send_response(200); self.send_header('Content-Type', 'text/html')
        elif self.path == '/unload.html':
            body = (b'<html><body><script>window.addEventListener("beforeunload", function(e){'
                    b'e.preventDefault(); e.returnValue="";});</script>'
                    b'<a href="/ct/page_x/Slides1.pdf?view=full">Unload PDF</a></body></html>')
            self.send_response(200); self.send_header('Content-Type', 'text/html')
        elif self.path.startswith('/ct/page_x/Slides1.pdf'):
            body = PDF; self.send_response(200); self.send_header('Content-Type', 'application/pdf')
        elif self.path == '/ct/attach_Slides1.pdf':
            body = PDF; self.send_response(200); self.send_header('Content-Type', 'application/pdf')
            self.send_header('Content-Disposition', 'attachment; filename="Slides1.pdf"')
        elif self.path == '/ct/elsewhere_Slides5.pdf':
            body = b''; self.send_response(302)
            self.send_header('Location', 'https://files.example.com/ct/inline_Slides2.pdf')
        elif self.path == '/ct/redirect_Slides4.pdf':
            body = b''; self.send_response(302); self.send_header('Location', '/ct/inline_Slides2.pdf')
        elif self.path == '/ct/inline_Slides2.pdf':
            body = PDF; self.send_response(200); self.send_header('Content-Type', 'application/pdf')
        else:
            body = b'missing'; self.send_response(404)
        self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
import ssl
server = HTTPServer(('93.184.216.34', 443), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain('cert.pem', 'key.pem')
server.socket = ctx.wrap_socket(server.socket, server_side=True)
server.serve_forever()
"#,
    )
    .expect("server");
    sh(&format!(
        "cd {} && openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 1 -subj /CN=fixture.example.com -addext subjectAltName=DNS:fixture.example.com,DNS:files.example.com >/dev/null 2>&1",
        dir.display()
    ));
    let child = Command::new(tool("python3"))
        .arg("server.py")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("fixture starts");
    let deadline = Instant::now() + Duration::from_secs(60);
    while TcpStream::connect((FIXTURE_IP, 443)).is_err() {
        assert!(Instant::now() < deadline, "fixture did not listen");
        thread::sleep(Duration::from_millis(50));
    }
    child
}

/// One request through the action runner, exactly as the daemon / launcher writes it.
fn action(
    session: &Path,
    seq: &mut u64,
    verb: &str,
    args: &[&str],
    artifact: Option<&str>,
) -> Value {
    *seq += 1;
    let request = session
        .join("actions")
        .join(format!("{:016x}.request", *seq));
    std::fs::write(
        &request,
        serde_json::to_vec(&json!({"verb":verb,"args":args,"artifact":artifact})).expect("json"),
    )
    .expect("request");
    let result = request.with_extension("result");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Ok(body) = std::fs::read(&result) {
            let _ = std::fs::remove_file(&result);
            return serde_json::from_slice(&body).expect("result json");
        }
        assert!(Instant::now() < deadline, "{verb}: no result");
        thread::sleep(Duration::from_millis(20));
    }
}

fn inner() {
    let Some((agent, package)) = agent_browser() else {
        panic!("agent-browser 0.38.1 is required for this test");
    };
    sh(&format!(
        "ip link set lo up && ip addr add {FIXTURE_IP}/32 dev lo"
    ));
    let dns = TcpListener::bind("127.0.0.1:53").expect("fixture DNS port");
    thread::spawn(move || dns_server(dns));
    let site = tempfile::tempdir().expect("fixture dir");
    let mut server = fixture(site.path());
    let session = tempfile::tempdir().expect("session dir");
    let s = session.path();
    for d in ["output", "home", "run", "actions", "profile"] {
        std::fs::create_dir(s.join(d)).expect("session subdir");
    }
    std::fs::write(
        s.join("browser_action.py"),
        include_str!("../src/browser_action.py"),
    )
    .expect("runner");
    std::fs::write(
        s.join("upstream.json"),
        br#"{"idleTimeout":"5m","noWebmcp":true}"#,
    )
    .expect("upstream");
    std::fs::write(
        s.join("policy.json"),
        serde_json::to_vec(&json!({"default":"deny","allow":[
            "launch","close","navigate","snapshot","gettext","click","screenshot","download","scroll",
            "getattribute","url"]}))
        .expect("policy"),
    )
    .expect("policy");
    let token = "cd".repeat(32);
    std::fs::write(
        s.join("action-config.json"),
        serde_json::to_vec(&json!({
            "executable": agent, "session_id": "artifacts",
            "allowed_domains": [ORIGIN], "browser_cache": null,
            "cdp_endpoint": format!("ws://127.0.0.1:9223/{token}"),
        }))
        .expect("config"),
    )
    .expect("config");
    let chrome = browser();
    // The fixture's certificate is self-signed: the test-only sandboxd build trusts it
    // (`h3-e2e-insecure-cert`, as in task-api's browser_h3_injection).
    let sandboxd = PathBuf::from(std::env::var("CELERIS_TEST_SANDBOXD").expect("sandboxd"));
    let proxy = PathBuf::from(
        std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("egress binary path"),
    );
    let spec = RuntimeSpec {
        userns: task_worker::browser_runtime::UsernsMode::Unshare,
        bwrap: tool("bwrap"),
        session_id: "artifacts".into(),
        session_dir: s.to_path_buf(),
        ro_dirs: vec![
            chrome
                .parent()
                .and_then(Path::parent)
                .expect("chrome install")
                .to_path_buf(),
            sandboxd.parent().expect("sandboxd parent").to_path_buf(),
            package,
        ],
        argv: vec![
            sandboxd.clone().into_os_string(),
            "--shared-cdp".into(),
            chrome.clone().into_os_string(),
            "python3".into(),
            "/session/browser_action.py".into(),
        ],
        cdp_pipe: true,
        egress: Some(EgressRelay {
            proxy,
            policy: POLICY.as_bytes().to_vec(),
            max_concurrent: DEFAULT_MAX_EGRESS,
        }),
    };
    let mut rt = IsolatedRuntime::launch(&spec).expect("isolated browser launches");
    let (cdp_read, live_tap) = task_worker::browser_launcher::live::LiveTap::interpose(
        rt.cdp_read.take().expect("CDP read"),
    )
    .expect("live tap");
    let mut controller = CdpController::new(rt.cdp_write.take().expect("CDP write"), cdp_read);
    // As after a credential login whose auth section closed on the post-login conditions: every
    // agent command passes the post-login gate (read origin, no live password field, guard).
    if std::env::var_os("CELERIS_TEST_POST_LOGIN").is_some() {
        let deadline = Instant::now() + Duration::from_secs(60);
        while controller
            .controller_command("Browser.getVersion", json!({}), None)
            .is_err()
        {
            assert!(Instant::now() < deadline, "browser CDP not ready");
            thread::sleep(Duration::from_millis(50));
        }
        controller.open_auth_section("auth-test".into());
        controller
            .resume_after_login("controller-own", vec![ORIGIN.to_string()])
            .expect("post-login mode");
        assert!(controller.post_login_active());
    }
    let relay = task_worker::browser_shared_cdp::SharedCdp::start(
        controller,
        &s.join("cdp-relay.sock"),
        token,
        vec![ORIGIN.to_string()],
    )
    .expect("relay");
    // As in the launcher during the 2026-10-10 15:48 run: the owner's Live View is streaming
    // (screencast through the controller, frames taken and acked at the viewer's pace).
    let live_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let live_frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let live = {
        use task_worker::browser_launcher::live::{LiveFeed, LiveNext, ScreencastFeed};
        let mut feed = ScreencastFeed::new(live_tap, relay.controller());
        let (stop, frames) = (live_stop.clone(), live_frames.clone());
        thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                if let LiveNext::Frame(_) = feed.next_frame(Duration::from_millis(200)) {
                    frames.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        })
    };
    let mut seq = 0;
    let version = action(s, &mut seq, "__version__", &[], None);
    assert_eq!(version["status"], 0, "{version}");
    let page = format!("{ORIGIN}/page.html");
    let opened = action(s, &mut seq, "open", &[&page], None);
    assert_eq!(opened["status"], 0, "open: {opened}");
    let snap = action(s, &mut seq, "snapshot", &[], None);
    assert_eq!(snap["status"], 0, "snapshot: {snap}");
    let tree: Value = serde_json::from_str(snap["stdout"].as_str().unwrap_or_default().trim())
        .expect("snapshot json");
    let reference = |name: &str| {
        tree["data"]["refs"]
            .as_object()
            .expect("refs")
            .iter()
            .find(|(_, r)| r["name"] == name)
            .map(|(k, _)| format!("@{k}"))
            .unwrap_or_else(|| panic!("{name} ref in {tree}"))
    };
    let shot = format!("screenshot-{}.png", "a".repeat(32));
    let taken = action(s, &mut seq, "screenshot", &[], Some(&shot));
    let out = s.join("output");
    let listing = || {
        std::fs::read_dir(&out)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        taken["status"],
        0,
        "screenshot: {taken}; output {:?}",
        listing()
    );
    let png = std::fs::read(out.join(&shot)).expect("screenshot file");
    assert!(png.starts_with(b"\x89PNG"), "screenshot is a PNG");
    for (link, name) in [
        ("Attachment PDF", format!("download-{}.bin", "b".repeat(32))),
        ("Inline PDF", format!("download-{}.bin", "c".repeat(32))),
        ("Redirect PDF", format!("download-{}.bin", "e".repeat(32))),
    ] {
        let got = action(s, &mut seq, "download", &[&reference(link)], Some(&name));
        assert_eq!(
            got["status"],
            0,
            "download {link}: {got}; output {:?}",
            listing()
        );
        let bytes = std::fs::read(out.join(&name)).expect("download file");
        assert_eq!(bytes, PDF, "{link}");
    }
    // The tab stayed on the page (an inline PDF became a download, not the PDF viewer).
    let shot2 = format!("screenshot-{}.png", "f".repeat(32));
    let again = action(s, &mut seq, "screenshot", &[], Some(&shot2));
    assert_eq!(
        again["status"],
        0,
        "screenshot after downloads: {again}; output {:?}",
        listing()
    );
    // A PDF link that redirects to another origin (not in read_origins): after login the relay
    // cancels the download and records why; the tab stays usable.
    if std::env::var_os("CELERIS_TEST_POST_LOGIN").is_some() {
        let elsewhere = format!("download-{}.bin", "d".repeat(32));
        let start = Instant::now();
        let got = action(
            s,
            &mut seq,
            "download",
            &[&reference("Elsewhere PDF")],
            Some(&elsewhere),
        );
        eprintln!("ELSEWHERE {:?} {got}", start.elapsed());
        // agent-browser fails; if Chrome still wrote the file before the cancel, the runner may
        // adopt it, and the launcher's check refuses it (the controller never let it through).
        match got["adopted_guid"].as_str() {
            None => {
                assert_ne!(got["status"], 0, "{got}");
                assert_eq!(got["runner_reason"], "agent_browser_exit", "{got}");
            }
            Some(guid) => {
                assert!(
                    !relay
                        .controller()
                        .lock()
                        .expect("controller")
                        .download_completed_allowed(guid),
                    "an adopted file from another origin must not pass the launcher's check: {got}"
                );
                std::fs::remove_file(out.join(&elsewhere)).expect("remove refused file");
            }
        }
        let denials = relay
            .controller()
            .lock()
            .expect("controller")
            .take_agent_denials();
        assert!(
            denials
                .iter()
                .any(|(m, c)| m == "Browser.downloadWillBegin" && *c == "download_origin_denied"),
            "{denials:?}"
        );
        let shot3 = format!("screenshot-{}.png", "e".repeat(32));
        let after = action(s, &mut seq, "screenshot", &[], Some(&shot3));
        assert_eq!(
            after["status"], 0,
            "screenshot after a refused download: {after}"
        );
    }
    // 付記 2026-10-10h (production 2026-10-10 14:40, `gate=Input.dispatchMouseEvent!sink_failed`): a
    // click that opens a JavaScript dialog does not answer until the dialog closes, and the relay
    // runs one agent command at a time. The controller answers the dialog: alert / beforeunload
    // accepted (the download goes on), confirm dismissed (the tab is not left blocked).
    let ref_on = |seq: &mut u64, page: &str, link: &str| {
        let opened = action(s, seq, "open", &[&format!("{ORIGIN}/{page}")], None);
        assert_eq!(opened["status"], 0, "open {page}: {opened}");
        let snap = action(s, seq, "snapshot", &[], None);
        let tree: Value = serde_json::from_str(snap["stdout"].as_str().unwrap_or_default().trim())
            .expect("snapshot");
        tree["data"]["refs"]
            .as_object()
            .expect("refs")
            .iter()
            .find(|(_, r)| r["name"] == link)
            .map(|(k, _)| format!("@{k}"))
            .unwrap_or_else(|| panic!("{link} ref"))
    };
    let denials = || {
        relay
            .controller()
            .lock()
            .expect("controller")
            .take_agent_denials()
    };
    // 付記 2026-10-10i (production 2026-10-10 15:48, `gate=Input.dispatchMouseEvent!sink_failed`, Live
    // View streaming): while a page waits for a navigation's response, Chrome answers commands on
    // that page only after the response arrives. A PDF that answers after 7 s failed the click (or
    // its post-login check) at the controller's 5 s; agent commands now wait up to 25 s.
    // 付記 2026-10-10j (production 2026-10-10 18:05, `runner_reason=exec_timeout`): a file link that
    // opens a new tab started its download where agent-browser does not look. During a download the
    // controller (as the launcher sets it up) closes the new tab and follows the link in the opener.
    let watch = |download: bool| {
        relay
            .controller()
            .lock()
            .expect("controller")
            .begin_action_watch(download)
    };
    let watched = || {
        relay
            .controller()
            .lock()
            .expect("controller")
            .take_action_watch()
    };
    for (link, letter) in [
        ("Blank PDF", "6"),
        ("Blank Slow PDF", "7"),
        ("Viewer PDF", "8"),
    ] {
        let r = ref_on(&mut seq, "blank.html", link);
        watch(true);
        let name = format!("download-{}.bin", letter.repeat(32));
        let got = action(s, &mut seq, "download", &[&r], Some(&name));
        let after = watched();
        assert_eq!(got["status"], 0, "download {link}: {got}; after {after:?}");
        assert!(
            got.get("adopted_guid").is_none(),
            "agent-browser saw it: {got}"
        );
        assert_eq!(std::fs::read(out.join(&name)).expect("file"), PDF, "{link}");
        if link != "Viewer PDF" {
            assert!(
                after.contains(&"window_open_followed_in_opener"),
                "{link}: {after:?}"
            );
        }
        let snap = action(s, &mut seq, "snapshot", &[], None);
        assert_eq!(
            snap["status"], 0,
            "the agent's tab still works after {link}: {snap}"
        );
    }
    if std::env::var_os("CELERIS_TEST_POST_LOGIN").is_some() {
        // A new tab to another origin is not followed. If Chrome still wrote the file, the runner
        // adopts it and the launcher's check (the controller never let it through) refuses it.
        let r = ref_on(&mut seq, "blank.html", "Elsewhere Blank PDF");
        watch(true);
        let name = format!("download-{}.bin", "a".repeat(32));
        let got = action(s, &mut seq, "download", &[&r], Some(&name));
        let after = watched();
        assert!(after.contains(&"window_open_origin_denied"), "{after:?}");
        let link = got["link"].as_array().expect("link tokens").clone();
        for token in ["target_blank", "href_other_origin", "path_pdf", "onclick"] {
            assert!(link.iter().any(|t| t == token), "{token} in {link:?}");
        }
        match got["adopted_guid"].as_str() {
            Some(guid) => {
                assert!(
                    !relay
                        .controller()
                        .lock()
                        .expect("controller")
                        .download_completed_allowed(guid),
                    "{got}"
                );
                // As the launcher does with a refused adoption.
                std::fs::remove_file(out.join(&name)).expect("remove refused file");
            }
            None => assert_ne!(got["status"], 0, "{got}"),
        }
    }
    for (link, letter) in [("Slow PDF", "4"), ("MouseDown PDF", "5")] {
        let r = ref_on(&mut seq, "slow.html", link);
        denials();
        let name = format!("download-{}.bin", letter.repeat(32));
        let got = action(s, &mut seq, "download", &[&r], Some(&name));
        assert_eq!(
            got["status"],
            0,
            "download {link}: {got}; gate {:?}",
            denials()
        );
        assert_eq!(std::fs::read(out.join(&name)).expect("file"), PDF, "{link}");
    }
    for (page, link, code, letter) in [
        ("dialogs.html", "Alert PDF", "dialog_accepted_alert", "1"),
        (
            "unload.html",
            "Unload PDF",
            "dialog_accepted_beforeunload",
            "2",
        ),
    ] {
        let r = ref_on(&mut seq, page, link);
        denials();
        let name = format!("download-{}.bin", letter.repeat(32));
        let got = action(s, &mut seq, "download", &[&r], Some(&name));
        assert_eq!(got["status"], 0, "download {link}: {got}");
        assert_eq!(std::fs::read(out.join(&name)).expect("file"), PDF, "{link}");
        assert_eq!(
            denials(),
            vec![("Page.javascriptDialogOpening".to_owned(), code)],
            "{link}"
        );
    }
    let r = ref_on(&mut seq, "dialogs.html", "Confirm PDF");
    denials();
    let clicked = action(s, &mut seq, "click", &[&r], None);
    assert_eq!(clicked["status"], 0, "click Confirm PDF: {clicked}");
    assert_eq!(
        denials(),
        vec![(
            "Page.javascriptDialogOpening".to_owned(),
            "dialog_dismissed_confirm"
        )]
    );
    let shot4 = format!("screenshot-{}.png", "3".repeat(32));
    let after = action(s, &mut seq, "screenshot", &[], Some(&shot4));
    assert_eq!(after["status"], 0, "the tab is not blocked: {after}");
    // 付記 2026-10-10f: a failure carries fixed diagnostics and nothing of agent-browser's text.
    let stale = format!("download-{}.bin", "9".repeat(32));
    let unknown = action(s, &mut seq, "download", &["@e999"], Some(&stale));
    assert_ne!(unknown["status"], 0, "{unknown}");
    assert_eq!(unknown["runner_reason"], "agent_browser_exit", "{unknown}");
    assert_eq!(unknown["error_class"], "unknown_ref", "{unknown}");
    let malformed = action(s, &mut seq, "download", &["not-a-ref"], Some(&stale));
    assert_eq!(malformed["status"], 2, "{malformed}");
    assert_eq!(malformed["runner_reason"], "request_invalid", "{malformed}");
    assert!(!out.join(&stale).exists());
    // Another host UID (the launcher) reads them: the runner left them world-readable.
    use std::os::unix::fs::PermissionsExt;
    for name in listing() {
        let mode = std::fs::metadata(out.join(&name))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o044,
            0o044,
            "{name} readable by the launcher: {mode:o}"
        );
    }
    eprintln!("SANDBOX-ARTIFACTS-EVIDENCE screenshot and two PDF downloads in /session/output");
    let _ = action(s, &mut seq, "close", &[], None);
    live_stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = live.join();
    assert!(
        live_frames.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "the Live View screencast streamed frames during the run"
    );
    rt.kill();
    let _ = server.kill();
    let _ = server.wait();
}

/// The sandboxd with `h3-e2e-insecure-cert` (built like task-api's browser_h3_injection does).
fn insecure_sandboxd() -> PathBuf {
    let exe = std::env::current_exe().expect("test executable");
    let path = exe
        .parent()
        .and_then(Path::parent)
        .expect("target directory")
        .join("celeris-browser-sandboxd");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "-q",
            "-p",
            "task-worker",
            "--bins",
            "--features",
            "h3-e2e-insecure-cert",
        ])
        .status()
        .expect("build worker binaries");
    assert!(status.success(), "worker binary build failed");
    assert!(path.is_file(), "sandboxd missing: {}", path.display());
    path
}

#[test]
fn inner_sandbox_artifacts() {
    if std::env::var_os(INNER).is_some() {
        inner();
    }
}

/// Chrome for Testing as the production launcher runs it (`launcher.toml` `chrome`).
const LAUNCHER_CHROME: &str = "/opt/celeris-browser/chrome/chrome";

fn run_outer(post_login: bool, chrome: Option<PathBuf>) {
    if userns_gate::skip_unless_userns_tests() {
        return;
    }
    if agent_browser().is_none() {
        eprintln!("SKIPPED: agent-browser 0.38.1 not installed");
        return;
    }
    for t in ["unshare", "ip", "bwrap", "python3"] {
        tool(t);
    }
    let chrome = chrome.unwrap_or_else(browser);
    let sandboxd = insecure_sandboxd();
    let mut cmd = Command::new(tool("unshare"));
    cmd.args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "inner_sandbox_artifacts",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .env("CELERIS_TEST_BROWSER", &chrome)
        .env("CELERIS_TEST_SANDBOXD", sandboxd);
    if post_login {
        cmd.env("CELERIS_TEST_POST_LOGIN", "1");
    }
    let out = cmd.output().expect("netns test starts");
    assert!(
        out.status.success(),
        "sandbox artifacts failed (post_login={post_login}, chrome={}): {}\n{}",
        chrome.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The production case of 2026-10-10: the launcher's Chrome for Testing (it has the PDF viewer;
/// chrome-headless-shell has none) with the controller in the post-login mode. An inline PDF link
/// must become a download, not open the viewer (the download then waited until it timed out).
#[test]
fn real_sandbox_launcher_chrome_downloads_inline_pdf_after_login() {
    let chrome = std::env::var_os("CELERIS_TEST_LAUNCHER_CHROME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(LAUNCHER_CHROME));
    if !chrome.is_file() {
        eprintln!(
            "SKIPPED: launcher Chrome for Testing not installed at {}",
            chrome.display()
        );
        return;
    }
    run_outer(true, Some(chrome));
}

#[test]
fn real_sandbox_screenshot_and_pdf_download_reach_the_output_dir() {
    run_outer(false, None);
}
