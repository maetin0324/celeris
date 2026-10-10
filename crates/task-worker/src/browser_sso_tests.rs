//! Real Chromium/HTTPS fixtures exercise the worker completion path without
//! userns. Broker transport is a test sink using credentiald's production JS;
//! isolation/admission evidence remains in the existing H3 wire/attack suite.
use super::*;
use crate::browser_cdp_sink::{
    BrokerClient, CdpController, InjectionError, InjectionRequest, PendingInjection,
};
use serde_json::{Value, json};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

const SECRET: &str = "sso-test-password-394ea91";

pub(crate) struct Children(Vec<Child>);
impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct Fixture {
    controller: Arc<Mutex<CdpController>>,
    target: String,
    session: String,
    origin: String,
    // Stop fixture/browser before deleting the profile and HTTPS files.
    _children: Children,
    directory: tempfile::TempDir,
}

/// The Shibboleth-shaped HTTPS fixture and a real Chromium on a CDP pipe, with a bare controller
/// (no target, no auth section). Shared with the launcher login tests.
pub(crate) struct ChromeFixture {
    pub(crate) controller: Arc<Mutex<CdpController>>,
    pub(crate) origin: String,
    /// Every origin the fixture serves (the first is `origin`).
    pub(crate) origins: Vec<String>,
    pub(crate) chrome_pid: u32,
    // Stop fixture/browser before deleting the profile and HTTPS files.
    pub(crate) children: Children,
    pub(crate) directory: tempfile::TempDir,
}

impl ChromeFixture {
    pub(crate) fn start() -> Self {
        Self::start_with(include_str!("browser_sso_fixture.py"))
    }

    /// Start `script` (an HTTPS fixture that writes its ports to `ports`) and a real Chromium.
    pub(crate) fn start_with(script: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cert = Command::new("openssl")
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
                "/CN=localhost",
            ])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(cert.status.success());
        std::fs::write(directory.path().join("server.py"), script).unwrap();
        let mut children = Children(Vec::new());
        children.0.push(
            Command::new("python3")
                .arg("server.py")
                .current_dir(directory.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let port_file = directory.path().join("ports");
        while !port_file.exists() {
            assert!(Instant::now() < deadline, "fixture startup timeout");
            thread::sleep(Duration::from_millis(10));
        }
        let ports = std::fs::read_to_string(port_file).unwrap();
        let origins: Vec<String> = ports
            .split_whitespace()
            .map(|p| format!("https://127.0.0.1:{p}"))
            .collect();
        let origin = origins[0].clone();
        let browser = shared_browser_executable(&browser_install_dirs())
            .expect("Playwright Chromium installed");
        let (read_end, child_write) = UnixStream::pair().unwrap();
        let (write_end, child_read) = UnixStream::pair().unwrap();
        // Duplicate to high FDs before pre_exec so installing FD 3 cannot
        // overwrite the source for FD 4. All original descriptors are CLOEXEC.
        let dup = |fd| {
            // SAFETY: fcntl duplicates a live descriptor; the return is owned here.
            let n = unsafe { nix::libc::fcntl(fd, nix::libc::F_DUPFD_CLOEXEC, 20) };
            assert!(n >= 0);
            unsafe { OwnedFd::from_raw_fd(n) }
        };
        let input = dup(child_read.as_raw_fd());
        let output = dup(child_write.as_raw_fd());
        let mut cmd = Command::new(browser);
        cmd.args([
            "--no-sandbox",
            "--no-zygote",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-background-networking",
            "--ignore-certificate-errors",
            "--remote-debugging-pipe",
        ])
        .arg(format!(
            "--user-data-dir={}",
            directory.path().join("profile").display()
        ))
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        // SAFETY: only async-signal-safe FD operations run after fork.
        unsafe {
            cmd.pre_exec(move || {
                if nix::libc::dup2(input.as_raw_fd(), 3) < 0
                    || nix::libc::dup2(output.as_raw_fd(), 4) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let chrome = cmd.spawn().unwrap();
        let chrome_pid = chrome.id();
        children.0.push(chrome);
        drop(child_read);
        drop(child_write);
        let mut c = CdpController::new(
            File::from(OwnedFd::from(write_end)),
            File::from(OwnedFd::from(read_end)),
        );
        c.response_timeout_for_test(Duration::from_secs(30));
        c.controller_command("Browser.getVersion", json!({}), None)
            .unwrap();
        Self {
            controller: Arc::new(Mutex::new(c)),
            origin,
            origins,
            chrome_pid,
            children,
            directory,
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let ChromeFixture {
            controller,
            origin,
            children,
            directory,
            ..
        } = ChromeFixture::start();
        let mut c = controller.lock().unwrap();
        let target = c
            .controller_command("Target.createTarget", json!({"url":"about:blank"}), None)
            .unwrap()["result"]["targetId"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = c
            .controller_command(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )
            .unwrap()["result"]["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
        c.open_auth_section("auth".into());
        c.controller_command("Network.enable", json!({}), Some(&session))
            .unwrap();
        drop(c);
        Self {
            controller,
            target,
            session,
            origin,
            directory,
            _children: children,
        }
    }

    async fn login(
        &self,
        path: &str,
        timeout: Duration,
        broker: &mut TestBroker,
    ) -> Result<(), &'static str> {
        let trusted = task_core::browser_wait::TrustedLogin {
            policy_id: "sso-fixture".into(),
            revision: 1,
            login_url: format!("{}{path}", self.origin),
            password_selector: "input[name=j_password]".into(),
            submit_selector: Some("button[name=_eventId_proceed]".into()),
            username_selector: None,
            post_login: None,
            consent: None,
        };
        {
            let mut c = self.controller.lock().unwrap();
            c.begin_login_navigation(&self.session, timeout).unwrap();
            c.controller_command(
                "Page.navigate",
                json!({"url":trusted.login_url}),
                Some(&self.session),
            )
            .map_err(|_| "navigation_failed")?;
        }
        let request = InjectionRequest {
            request_id: "request".into(),
            session_id: "test".into(),
            cdp_target_id: self.target.clone(),
            frame_id: String::new(),
            loader_id: String::new(),
            exact_origin: self.origin.clone(),
            redirect_chain: Vec::new(),
            selector: trusted.password_selector.clone(),
            field: "password".into(),
            auth_section_id: "auth".into(),
            lease_id: "lease".into(),
            username_selector: None,
        };
        complete_trusted_login(&self.controller, broker, &self.session, request, &trusted)
            .await
            .map(|_| ())
    }
}

#[derive(Default)]
struct TestBroker {
    requests: Vec<Value>,
}
struct Pending(thread::JoinHandle<Result<Value, InjectionError>>);
impl PendingInjection for Pending {
    fn finish(self: Box<Self>) -> Result<Value, InjectionError> {
        self.0.join().unwrap()
    }
}
impl BrokerClient for TestBroker {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        assert!(!request.to_string().contains(SECRET));
        self.requests.push(request.clone());
        Ok(Box::new(Pending(thread::spawn(move || {
            let mut frame = serde_json::to_vec(&json!({"id":request["cdp_command_id"],"sessionId":request["cdp_session_id"],"method":"Runtime.callFunctionOn","params":{"objectId":request["object_id"],"functionDeclaration":celeris_credentiald::injection_ipc::INJECT_FUNCTION,"arguments":[{"value":request["frame_chain"][0]},{"value":0},{"value":"password"},{"value":SECRET}],"returnByValue":true,"silent":true}})).unwrap();
            frame.push(0);
            // SAFETY: a connected seqpacket FD and valid buffers are supplied.
            assert_eq!(
                unsafe { nix::libc::send(sink.as_raw_fd(), frame.as_ptr().cast(), frame.len(), 0) },
                frame.len() as isize
            );
            let mut bytes = [0u8; 4096];
            let n = unsafe {
                nix::libc::recv(sink.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0)
            };
            assert!(n > 0);
            let response: Value = serde_json::from_slice(&bytes[..n as usize]).unwrap();
            assert_eq!(response["result"]["result"]["value"], "ok");
            Ok(
                json!({"v":1,"request_id":request["request_id"],"ok":true,"receipt":{"injected_at":1,"lease_id":request["lease_id"],"auth_section_id":request["auth_section_id"],"session_id":request["session_id"],"cdp_target_id":request["cdp_target_id"],"frame_id":request["frame_id"],"loader_id":request["loader_id"],"field":request["field"]},"redisplay_guard":celeris_credentiald::injection::RedisplayGuard::new(SECRET).to_wire()}),
            )
        }))))
    }
}

#[tokio::test]
async fn browser_trusted_login_sso_same_origin_relay_and_sp_return() {
    let fixture = Fixture::new();
    let mut broker = TestBroker::default();
    assert_eq!(
        fixture
            .login("/entry", Duration::from_secs(15), &mut broker)
            .await,
        Ok(())
    );
    assert_eq!(broker.requests.len(), 1);
    let chain = broker.requests[0]["redirect_chain"].as_array().unwrap();
    assert!(chain.len() >= 3, "record entry, 302 destination, JS POST");
    assert!(chain.iter().all(|o| o == &fixture.origin));
    let deadline = Instant::now() + Duration::from_secs(10);
    let received = fixture.directory.path().join("received");
    while !received.exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(std::fs::read_to_string(received).unwrap(), SECRET);
    loop {
        let arrived = {
            let mut c = fixture.controller.lock().unwrap();
            let tree = c
                .controller_command("Page.getFrameTree", json!({}), Some(&fixture.session))
                .unwrap();
            tree["result"]["frameTree"]["frame"]["url"]
                .as_str()
                .is_some_and(|url| url.ends_with("/sp") && !url.starts_with(&fixture.origin))
        };
        if arrived {
            break;
        }
        assert!(Instant::now() < deadline, "SP document did not commit");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut c = fixture.controller.lock().unwrap();
    assert_eq!(c.redisplay_guards(), 1);
    assert!(c.take_agent_events().is_empty());
    assert_eq!(
        c.agent_command(
            "Runtime.evaluate",
            json!({"expression":"document.body.innerText"}),
            Some(&fixture.session)
        )
        .unwrap_err()
        .code(),
        "auth_section_required"
    );
    c.close_auth_section().unwrap();
    let tree = c
        .controller_command("Page.getFrameTree", json!({}), Some(&fixture.session))
        .unwrap();
    assert!(
        !tree["result"]["frameTree"]["frame"]["url"]
            .as_str()
            .unwrap()
            .starts_with(&fixture.origin)
    );
}

#[tokio::test]
async fn browser_trusted_login_sso_cross_origin_redirect_refused() {
    let fixture = Fixture::new();
    let mut broker = TestBroker::default();
    assert_eq!(
        fixture
            .login("/cross", Duration::from_secs(15), &mut broker)
            .await,
        Err("redirected")
    );
    assert!(broker.requests.is_empty());
}

#[tokio::test]
async fn browser_trusted_login_sso_password_timeout() {
    let fixture = Fixture::new();
    let mut broker = TestBroker::default();
    let start = Instant::now();
    assert_eq!(
        fixture
            .login("/never", Duration::from_millis(500), &mut broker)
            .await,
        Err("navigation_failed")
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(broker.requests.is_empty());
}
