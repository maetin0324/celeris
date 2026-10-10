//! 層横断の Live View 試験（ADR 2026-10-10-browser-launcher-live-view-frames D4・D5、付記 2026-10-10b
//! の protocol 互換表）。
//!
//! 経路: 偽 Chrome（CDP pipe）→ launcher の `LiveTap`・`ScreencastFeed` → 実の `LauncherServer`
//! （偽 `SessionBackend`）→ Unix socket → daemon 側の `LauncherClient`・`open_frame_relay` →
//! `LatestFrameSlot`。userns・実 browser は使わない。待ちは出来事待ち（slot の `next`）と長い保険の
//! 期限だけ。v7 の launcher は、実 launcher の前に置いた proxy が `hello` の版を書き換えて再現する
//! （proxy は `authenticate` の `SCM_RIGHTS` FD も中継する）。
//!
//! fixture（[`FakeCdp`]・[`FakeLauncher`]・[`Proxy`]・[`next_body`] など）は同じ file の他の試験も
//! 使う前提で、file 内 helper として置く。Unix socket の SUN_LEN のため `TMPDIR=/tmp` で回す。

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use task_core::browser_isolation::LiveSessionEntry as _;
use task_core::browser_live_frame::LatestFrameSlot;
use task_worker::browser::launcher_cross_test_support::{DaemonLauncherSession, open_frame_relay};
use task_worker::browser_cdp_sink::CdpController;
use task_worker::browser_launcher::live::{LiveTap, ScreencastFeed};
use task_worker::browser_launcher::protocol::{
    AuthenticateArgs, AuthenticationStatus, DEFAULT_MAX_FRAME, FrameError, LoginObservation,
    LoginResult, read_frame, write_frame, write_message,
};
use task_worker::browser_launcher::{
    ActionArgs, BackendSession, ErrorCode, LIVE_FRAME_PROTOCOL, Launched, LauncherClient,
    LauncherLimits, LauncherServer, LiveFeed, MAX_LIVE_BODY, Observation, PROTOCOL_VERSION,
    Registry, Request, Response, ServerConfig, ServerHandle, SessionBackend, SessionFacts,
    SessionPolicy, SessionState, StartRequest, Verb,
};
use task_worker::browser_live::{LIVE_NO_FRAMES_REASON, LiveFrameRegistration};

/// 長い保険（出来事待ちが来なかったときだけ効く）。
const INSURANCE: Duration = Duration::from_secs(30);
/// 偽 launcher の UID（daemon と別 UID に見せる。`verify_isolation` を通すため）。
const LAUNCHER_UID: u32 = 4_000_001;
const SUBUID: u32 = 5_000_000;
/// 偽 Chrome が答える頁 target（ScreencastFeed が選ぶ）。
const PAGE_TARGET: &str = "P1";
/// 偽 Chrome が screencast の attach に返す CDP session。
const LIVE_CDP_SESSION: &str = "LIVE";

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn pipe() -> (File, File) {
    let mut fds = [0i32; 2];
    // SAFETY: fds has room for the two descriptors pipe2 writes.
    assert_eq!(
        unsafe { nix::libc::pipe2(fds.as_mut_ptr(), nix::libc::O_CLOEXEC) },
        0
    );
    // SAFETY: the descriptors are new and owned only here.
    unsafe {
        (
            File::from(OwnedFd::from_raw_fd(fds[0])),
            File::from(OwnedFd::from_raw_fd(fds[1])),
        )
    }
}

// ---- 偽 Chrome（CDP pipe）----

/// launcher が偽 Chrome に書いた CDP command（method, sessionId, params）。
pub type CdpLog = Arc<Mutex<Vec<(String, Option<String>, Value)>>>;

/// 偽 Chrome の screencast 状態。frame は ack を待ってから 1 枚ずつ送る（Chrome と同じ背圧）。
struct ChromeState {
    out: File,
    screencasting: bool,
    awaiting_ack: bool,
    queue: VecDeque<Vec<u8>>,
    ack: i64,
}

impl ChromeState {
    fn send(&mut self, v: Value) {
        let mut b = serde_json::to_vec(&v).expect("json");
        b.push(0);
        // launcher 側が閉じた後の書き込み失敗は無視する（session の終わり）。
        let _ = self.out.write_all(&b);
    }

    /// screencast 中で ack 待ちでなければ、待っている frame を 1 枚送る。
    fn pump(&mut self) {
        if !self.screencasting || self.awaiting_ack {
            return;
        }
        let Some(body) = self.queue.pop_front() else {
            return;
        };
        self.ack += 1;
        self.awaiting_ack = true;
        let ack = self.ack;
        self.send(
            json!({"method":"Page.screencastFrame","sessionId":LIVE_CDP_SESSION,
            "params":{"data":b64(&body),"sessionId":ack,
                "metadata":{"deviceWidth":640,"deviceHeight":480}}}),
        );
    }
}

/// 偽 Chrome と launcher 側の CDP 部品（`LiveTap`・`CdpController`）。session ごとに 1 つ。
pub struct FakeCdp {
    pub tap: Arc<LiveTap>,
    pub controller: Arc<Mutex<CdpController>>,
    pub log: CdpLog,
    chrome: Arc<Mutex<ChromeState>>,
}

impl FakeCdp {
    pub fn new() -> Self {
        let (chrome_out_r, chrome_out_w) = pipe();
        let (chrome_in_r, chrome_in_w) = pipe();
        let (ctrl_read, tap) = LiveTap::interpose(chrome_out_r).expect("interpose");
        let controller = Arc::new(Mutex::new(CdpController::new(chrome_in_w, ctrl_read)));
        let log: CdpLog = Arc::default();
        let chrome = Arc::new(Mutex::new(ChromeState {
            out: chrome_out_w,
            screencasting: false,
            awaiting_ack: false,
            queue: VecDeque::new(),
            ack: 0,
        }));
        let (log_t, chrome_t) = (Arc::clone(&log), Arc::clone(&chrome));
        std::thread::spawn(move || fake_chrome(chrome_in_r, chrome_t, log_t));
        Self {
            tap,
            controller,
            log,
            chrome,
        }
    }

    /// 次の screencast frame（画素の代わりの bytes）を積む。screencast 中なら ack に合わせて流れる。
    pub fn push_frame(&self, body: &[u8]) {
        let mut st = self.chrome.lock().expect("chrome");
        st.queue.push_back(body.to_vec());
        st.pump();
    }

    /// 偽 Chrome が受けた CDP method の一覧（順）。
    pub fn methods(&self) -> Vec<String> {
        self.log
            .lock()
            .expect("log")
            .iter()
            .map(|(m, _, _)| m.clone())
            .collect()
    }
}

impl Default for FakeCdp {
    fn default() -> Self {
        Self::new()
    }
}

fn fake_chrome(mut input: File, chrome: Arc<Mutex<ChromeState>>, log: CdpLog) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match input.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        while let Some(end) = buf.iter().position(|b| *b == 0) {
            let msg: Vec<u8> = buf.drain(..=end).collect();
            let v: Value = serde_json::from_slice(&msg[..end]).expect("cdp json");
            let method = v["method"].as_str().unwrap_or_default().to_owned();
            let session = v["sessionId"].as_str().map(str::to_owned);
            log.lock()
                .expect("log")
                .push((method.clone(), session.clone(), v["params"].clone()));
            let mut reply = json!({"id":v["id"].clone(),"result":{}});
            if let Some(s) = &session {
                reply["sessionId"] = s.clone().into();
            }
            let mut st = chrome.lock().expect("chrome");
            match method.as_str() {
                "Target.getTargets" => {
                    reply["result"] = json!({"targetInfos":[
                        {"targetId":PAGE_TARGET,"type":"page","url":"https://example.com/"}
                    ]});
                }
                "Target.attachToTarget" => {
                    st.send(json!({"method":"Target.attachedToTarget","params":{
                        "sessionId":LIVE_CDP_SESSION,
                        "targetInfo":{"targetId":PAGE_TARGET,"type":"page","url":"https://example.com/"},
                        "waitingForDebugger":false}}));
                    reply["result"] = json!({"sessionId":LIVE_CDP_SESSION});
                }
                "Page.startScreencast" => {
                    st.send(reply);
                    st.screencasting = true;
                    st.awaiting_ack = false;
                    st.pump();
                    continue;
                }
                "Page.screencastFrameAck" => {
                    st.send(reply);
                    st.awaiting_ack = false;
                    st.pump();
                    continue;
                }
                "Page.stopScreencast" => {
                    st.screencasting = false;
                }
                "Target.detachFromTarget" => {
                    st.send(json!({"method":"Target.detachedFromTarget",
                        "params":{"sessionId":LIVE_CDP_SESSION,"targetId":PAGE_TARGET}}));
                }
                _ => {}
            }
            st.send(reply);
        }
    }
}

// ---- 偽 launcher（実の LauncherServer ＋ 偽 SessionBackend）----

/// 1 session の中で起きたこと（backend への到達の数）と、その session の偽 Chrome。
#[derive(Default)]
pub struct SessionProbe {
    pub cdp: FakeCdp,
    pub actions: AtomicUsize,
    pub live_opened: AtomicUsize,
    pub auth_begun: AtomicUsize,
    pub logins: AtomicUsize,
    pub stopped: AtomicUsize,
}

/// 起動した session を順に持つ偽 backend。
#[derive(Default)]
pub struct FakeBackend {
    pub sessions: Mutex<Vec<Arc<SessionProbe>>>,
}

struct FakeSession {
    child: Option<std::process::Child>,
    probe: Arc<SessionProbe>,
}

/// daemon の `verify_isolation` を通る launcher の観測（daemon と別 UID・owner は launcher）。
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
            task_worker::browser_runtime::process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
        let probe = Arc::new(SessionProbe::default());
        self.sessions
            .lock()
            .expect("sessions")
            .push(Arc::clone(&probe));
        Ok(Launched {
            session: Box::new(FakeSession {
                child: Some(child),
                probe,
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
}

impl BackendSession for FakeSession {
    fn action(&mut self, verb: Verb, _args: &ActionArgs) -> Result<Observation, ErrorCode> {
        self.probe.actions.fetch_add(1, Ordering::SeqCst);
        Ok(Observation {
            text: Some(format!(
                r#"{{"success":true,"data":{{"verb":"{verb:?}"}}}}"#
            )),
            artifact: None,
        })
    }
    fn auth_begin(&mut self, _auth_section_id: &str) -> Result<String, ErrorCode> {
        self.probe.auth_begun.fetch_add(1, Ordering::SeqCst);
        // 本番と同じく、本人の Live View は login target を追う（D3）。
        self.probe
            .cdp
            .tap
            .set_preferred_target(Some(PAGE_TARGET.into()));
        Ok(PAGE_TARGET.into())
    }
    fn authenticate(
        &mut self,
        _args: &AuthenticateArgs,
        _broker: UnixStream,
    ) -> Result<LoginResult, ErrorCode> {
        self.probe.logins.fetch_add(1, Ordering::SeqCst);
        Ok(LoginResult::RESUMED)
    }
    fn live(&mut self) -> Result<Box<dyn LiveFeed>, ErrorCode> {
        self.probe.live_opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ScreencastFeed::new(
            Arc::clone(&self.probe.cdp.tap),
            Arc::clone(&self.probe.cdp.controller),
        )))
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        (SessionState::Running, good_facts())
    }
    fn isolation_ok(&mut self) -> bool {
        true
    }
    fn stop(mut self: Box<Self>) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.probe.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

/// 実の `LauncherServer`（daemon の UID を許可）と偽 backend。
pub struct FakeLauncher {
    pub dir: tempfile::TempDir,
    pub sock: PathBuf,
    pub backend: Arc<FakeBackend>,
    pub handle: ServerHandle,
}

impl FakeLauncher {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("l.sock");
        let registry = Registry::open(dir.path().join("state"), "inst-cross").expect("registry");
        let backend = Arc::new(FakeBackend::default());
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![nix::unistd::getuid().as_raw()],
                limits: LauncherLimits::default(),
            },
            backend.clone(),
            registry,
        )
        .expect("bind");
        let handle = server.spawn().expect("spawn");
        Self {
            dir,
            sock,
            backend,
            handle,
        }
    }

    /// `index` 番目に起動した session（session の起動は `start_session` の応答より前に終わる）。
    pub fn session(&self, index: usize) -> Arc<SessionProbe> {
        Arc::clone(&self.backend.sessions.lock().expect("sessions")[index])
    }

    /// 新しい proxy 用の dir（socket 名の衝突を避ける）。
    pub fn subdir(&self, name: &str) -> PathBuf {
        let d = self.dir.path().join(name);
        std::fs::create_dir(&d).expect("subdir");
        d
    }
}

impl Default for FakeLauncher {
    fn default() -> Self {
        Self::new()
    }
}

pub fn policy() -> SessionPolicy {
    SessionPolicy {
        allowed_domains: vec!["example.com".into()],
        allowed_actions: vec![Verb::Open, Verb::Click, Verb::Snapshot],
        lease_seconds: 600,
    }
}

// ---- v7 launcher の再現: hello の版を書き換える proxy（SCM_RIGHTS も中継）----

/// 実 launcher の前に置く proxy。全接続の要求の種類を順に記録し、`rewrite` があれば `hello` の
/// `protocol_version` をその値にする。
pub struct Proxy {
    pub sock: PathBuf,
    pub seen: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    pub fn requests(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }
}

/// `recvmsg` で読み、付いていた `SCM_RIGHTS` の FD を集める。0 は切断。
fn recv_with_fds(s: &UnixStream, buf: &mut [u8], fds: &mut Vec<OwnedFd>) -> std::io::Result<usize> {
    use nix::libc;
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    let mut control = [0u64; 8];
    // SAFETY: msghdr は全 field 0 で有効。iov・control は有効な buffer を指す。
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&control) as _;
    // SAFETY: msg は上の有効な buffer を指す。
    let n = unsafe { libc::recvmsg(s.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: recvmsg が埋めた control buffer を CMSG_* で辿る。
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let payload = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let p = libc::CMSG_DATA(c).cast::<libc::c_int>();
                for i in 0..payload / std::mem::size_of::<libc::c_int>() {
                    let raw = std::ptr::read_unaligned(p.add(i));
                    if raw >= 0 {
                        fds.push(OwnedFd::from_raw_fd(raw));
                    }
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
    }
    Ok(n as usize)
}

/// 長さ前置きの frame を書く。`fd` があれば先頭に `SCM_RIGHTS` で付ける。
fn send_with_fd(s: &UnixStream, body: &[u8], fd: Option<&OwnedFd>) -> std::io::Result<()> {
    use nix::libc;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    let Some(fd) = fd else {
        return (&mut &*s).write_all(&frame);
    };
    let mut iov = libc::iovec {
        iov_base: frame.as_mut_ptr().cast(),
        iov_len: frame.len(),
    };
    let mut control = [0u64; 4];
    // SAFETY: msghdr・iov・整列した control buffer は FD 1 本に足りる。
    let sent = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::c_int>(), fd.as_raw_fd());
        libc::sendmsg(s.as_raw_fd(), &msg, libc::MSG_NOSIGNAL)
    };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rest = frame.get(sent as usize..).unwrap_or_default();
    (&mut &*s).write_all(rest)
}

/// client → launcher: frame ごとに種類を記録し、受けた FD を付けて転送する。
fn forward_requests(client: UnixStream, server: UnixStream, seen: Arc<Mutex<Vec<String>>>) {
    let mut buf = Vec::new();
    let mut fds = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    'outer: loop {
        match recv_with_fds(&client, &mut chunk, &mut fds) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        while buf.len() >= 4 {
            let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if buf.len() < 4 + len {
                break;
            }
            let body: Vec<u8> = buf.drain(..4 + len).skip(4).collect();
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                let kind = v["type"].as_str().unwrap_or("?").to_owned();
                seen.lock().expect("seen").push(kind);
            }
            let fd = (!fds.is_empty()).then(|| fds.remove(0));
            if send_with_fd(&server, &body, fd.as_ref()).is_err() {
                break 'outer;
            }
        }
    }
    let _ = server.shutdown(std::net::Shutdown::Both);
}

/// launcher → client: `rewrite` があれば `hello` の版を書き換えて転送する。
fn forward_responses(mut server: UnixStream, mut client: UnixStream, rewrite: Option<u32>) {
    while let Ok(mut body) = read_frame(&mut server, 4 * MAX_LIVE_BODY) {
        if let (Some(v), Ok(mut json)) = (rewrite, serde_json::from_slice::<Value>(&body))
            && json["type"] == "hello"
        {
            json["protocol_version"] = v.into();
            body = serde_json::to_vec(&json).expect("json");
        }
        if write_frame(&mut client, &body, 4 * MAX_LIVE_BODY).is_err() {
            break;
        }
    }
    let _ = client.shutdown(std::net::Shutdown::Both);
}

pub fn proxy(upstream: &Path, dir: &Path, rewrite: Option<u32>) -> Proxy {
    let sock = dir.join("p.sock");
    let listener = UnixListener::bind(&sock).expect("bind proxy");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (upstream, seen_t) = (upstream.to_path_buf(), Arc::clone(&seen));
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let Ok(server) = UnixStream::connect(&upstream) else {
                return;
            };
            let (c2, s2) = (
                client.try_clone().expect("clone"),
                server.try_clone().expect("clone"),
            );
            let seen = Arc::clone(&seen_t);
            std::thread::spawn(move || forward_requests(c2, s2, seen));
            std::thread::spawn(move || forward_responses(server, client, rewrite));
        }
    });
    Proxy { sock, seen }
}

// ---- daemon 側の helper ----

/// 出来事待ち: slot の次の frame の bytes（閉じたら `None`）。
pub async fn next_body(slot: &LatestFrameSlot) -> Option<Vec<u8>> {
    tokio::time::timeout(INSURANCE, slot.next())
        .await
        .expect("slot event")
        .map(|frame| frame.body().to_vec())
}

/// 同期の daemon 呼び出しを blocking pool で回す（launcher client は同期 I/O）。
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("join")
}

/// 本番の `credential_login` が組む形の credential login 要求（session・lease は
/// [`DaemonLauncherSession::authenticate`] が埋める）。
pub fn credential_args(auth_section_id: &str) -> AuthenticateArgs {
    AuthenticateArgs {
        session_id: String::new(),
        lease_id: String::new(),
        auth_section_id: auth_section_id.into(),
        credential_lease_id: "cred-lease-1".into(),
        origin: "https://login.example.com".into(),
        login_url: "https://login.example.com/signin".into(),
        password_selector: "#password".into(),
        submit_selector: Some("#submit".into()),
        username_selector: Some("#username".into()),
        post_login: None,
        report_held_reason: false,
        consent: None,
        report_consent_controls: false,
    }
}

/// daemon 側の credential login（auth_begin → injection.sock の代わりの socketpair → authenticate）。
pub async fn daemon_credential_login(
    session: &Arc<DaemonLauncherSession>,
    auth_section_id: &str,
) -> (AuthenticationStatus, LoginResult) {
    let (s, id) = (Arc::clone(session), auth_section_id.to_owned());
    blocking(move || {
        let target = s.auth_begin(&id).expect("auth_begin");
        assert_eq!(target, PAGE_TARGET);
        let (broker, _credentiald) = UnixStream::pair().expect("socketpair");
        s.authenticate(credential_args(&id), OwnedFd::from(broker))
            .expect("authenticate")
    })
    .await
}

fn has_input_command(methods: &[String]) -> bool {
    methods.iter().any(|m| m.starts_with("Input."))
}

// ---- 互換表: v8 daemon × v7 launcher ----

/// 付記 2026-10-10b 互換表 1 行目: launcher の `hello` が 7 なら、daemon は Live View を有効にせず
/// （理由 `launcher_protocol_no_live_frames`）、`live_start` を送らず、frame 接続を開かない。
/// session の開始・action・credential login・stop は従来どおり成功する。
#[tokio::test]
async fn browser_launcher_v8_daemon_v7_launcher_disables_live_view_and_logs_in() {
    let launcher = FakeLauncher::new();
    let p = proxy(&launcher.sock, &launcher.subdir("v7"), Some(7));
    let sock = p.sock.clone();
    let (daemon, attestation) =
        blocking(move || DaemonLauncherSession::start(&sock, "t1", "r1", policy()))
            .await
            .expect("session start on a v7 launcher");
    let daemon = Arc::new(daemon);
    let probe = launcher.session(0);

    let d = Arc::clone(&daemon);
    let version = blocking(move || d.protocol_version()).await.expect("hello");
    assert_eq!(version, 7);
    assert!(version < LIVE_FRAME_PROTOCOL);

    // daemon の Live View 開始: 版確認で止まり、frame 接続は開かない。
    let (d, sock) = (Arc::clone(&daemon), p.sock.clone());
    let relay = blocking(move || d.open_live_frames(&sock)).await;
    assert_eq!(relay.as_ref().err(), Some(&LIVE_NO_FRAMES_REASON));
    let reg = LiveFrameRegistration::register(
        None,
        "logical-v7",
        ("t1".into(), "r1".into()),
        attestation,
        relay,
    );
    assert_eq!(
        reg.entry().unavailable_reason(),
        Some("launcher_protocol_no_live_frames")
    );
    assert!(reg.entry().live_frames().is_none(), "no frame slot");

    // frame 中継に v7 の版を渡しても同じ理由で、接続しない（存在しない socket にも触れない）。
    let missing = launcher.dir.path().join("no-such.sock");
    let sid = daemon.session_id().to_owned();
    let lease = daemon.lease_id().to_owned();
    let direct = blocking(move || open_frame_relay(&missing, 7, &sid, &lease)).await;
    assert_eq!(direct.err(), Some(LIVE_NO_FRAMES_REASON));

    // credential login と action は v7 launcher で従来どおり。
    let (status, login) = daemon_credential_login(&daemon, "auth-v7").await;
    assert_eq!(status, AuthenticationStatus::Success);
    assert_eq!(login.observation, LoginObservation::Resumed);
    let d = Arc::clone(&daemon);
    let (_, obs) = blocking(move || d.action(Verb::Snapshot, ActionArgs::default()))
        .await
        .expect("action");
    assert!(obs.text.is_some());

    let requests = p.requests();
    assert!(requests.iter().any(|t| t == "hello"), "{requests:?}");
    assert!(requests.iter().any(|t| t == "authenticate"), "{requests:?}");
    assert!(
        !requests.iter().any(|t| t == "live_start"),
        "no live_start reaches a v7 launcher: {requests:?}"
    );
    assert_eq!(probe.live_opened.load(Ordering::SeqCst), 0);
    assert_eq!(probe.logins.load(Ordering::SeqCst), 1);
    assert_eq!(probe.auth_begun.load(Ordering::SeqCst), 1);
    assert_eq!(probe.actions.load(Ordering::SeqCst), 1);
    let methods = probe.cdp.methods();
    assert!(
        !methods
            .iter()
            .any(|m| m.starts_with("Page.startScreencast")),
        "{methods:?}"
    );
    assert!(!has_input_command(&methods), "{methods:?}");

    drop(reg);
    let d = Arc::clone(&daemon);
    blocking(move || d.stop()).await.expect("stop");
    assert_eq!(probe.stopped.load(Ordering::SeqCst), 1);
}

// ---- 互換表: v7 daemon × v8 launcher ----

/// 付記 2026-10-10b 互換表 2 行目: v7 の daemon（frame 接続・`live_start` を持たない client）は v8
/// launcher にも frame を要求しない。session の開始・credential login・action・stop は成功し、
/// launcher は screencast を始めない。
#[tokio::test]
async fn browser_launcher_v7_daemon_v8_launcher_never_opens_frames_and_logs_in() {
    let launcher = FakeLauncher::new();
    let p = proxy(&launcher.sock, &launcher.subdir("v8"), None);
    let sock = p.sock.clone();
    let (status, login, version) = blocking(move || {
        // v7 daemon の手順: hello → start_session → auth_begin → authenticate → action → stop。
        let mut c = LauncherClient::connect(&sock, INSURANCE).expect("connect");
        let (version, allow) = c.hello().expect("hello");
        assert!(allow.is_empty());
        let s = c
            .start_session("t1", "r1", "lease-v7", policy())
            .expect("start");
        let target = c
            .auth_begin(&s.session_id, "lease-v7", "auth-v8")
            .expect("auth_begin");
        assert_eq!(target, PAGE_TARGET);
        let mut args = credential_args("auth-v8");
        args.session_id = s.session_id.clone();
        args.lease_id = "lease-v7".into();
        let (broker, _credentiald) = UnixStream::pair().expect("socketpair");
        let (status, login) = c
            .authenticate(args, OwnedFd::from(broker))
            .expect("authenticate");
        c.action(
            &s.session_id,
            "lease-v7",
            Verb::Snapshot,
            ActionArgs::default(),
        )
        .expect("action");
        let (state, _) = c.observe(&s.session_id, "lease-v7").expect("observe");
        assert_eq!(state, SessionState::Running);
        c.stop(&s.session_id, "lease-v7").expect("stop");
        (status, login, version)
    })
    .await;
    assert_eq!(version, PROTOCOL_VERSION);
    assert_eq!(version, LIVE_FRAME_PROTOCOL, "the launcher is v8");
    assert_eq!(status, AuthenticationStatus::Success);
    assert_eq!(login.observation, LoginObservation::Resumed);

    let probe = launcher.session(0);
    let requests = p.requests();
    assert_eq!(
        requests,
        [
            "hello",
            "start_session",
            "auth_begin",
            "authenticate",
            "action",
            "observe",
            "stop"
        ],
        "a v7 daemon sends no frame verb"
    );
    assert!(
        !requests
            .iter()
            .any(|t| t == "live_start" || t == "live_stop"),
        "{requests:?}"
    );
    assert_eq!(
        launcher.handle.session_count(),
        0,
        "stopped session is gone"
    );
    assert_eq!(probe.live_opened.load(Ordering::SeqCst), 0);
    assert_eq!(probe.logins.load(Ordering::SeqCst), 1);
    assert_eq!(probe.actions.load(Ordering::SeqCst), 1);
    assert_eq!(probe.stopped.load(Ordering::SeqCst), 1);
    let methods = probe.cdp.methods();
    assert!(
        !methods.iter().any(|m| m.contains("Screencast")),
        "{methods:?}"
    );
    assert!(!has_input_command(&methods), "{methods:?}");
}

// ---- D4: launcher 経路の Live View は読み取り専用 ----

/// frame 接続を生で開き、`live_started` を確かめてから `input` を送る。launcher は `bad_request` で
/// 拒否して接続を閉じる。
fn send_input_on_frame_connection(sock: &Path, session_id: &str, lease_id: &str, input: &[u8]) {
    let mut raw = UnixStream::connect(sock).expect("raw");
    raw.set_read_timeout(Some(INSURANCE)).expect("timeout");
    write_message(
        &mut raw,
        &Request::LiveStart {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
        },
        DEFAULT_MAX_FRAME,
    )
    .expect("live_start");
    let started: Response =
        serde_json::from_slice(&read_frame(&mut raw, DEFAULT_MAX_FRAME).expect("started"))
            .expect("decode");
    assert!(
        matches!(&started, Response::LiveStarted { session_id: s, .. } if s == session_id),
        "{started:?}"
    );
    write_frame(&mut raw, input, DEFAULT_MAX_FRAME).expect("input");
    // frame の metadata・body が先に来ることがある。拒否まで読み飛ばす（body は JSON でない）。
    let refused = loop {
        let body = read_frame(&mut raw, MAX_LIVE_BODY).expect("refusal");
        match serde_json::from_slice::<Response>(&body) {
            Ok(Response::LiveFrame { .. }) => {
                read_frame(&mut raw, MAX_LIVE_BODY).expect("frame body");
            }
            Ok(other) => break other,
            Err(_) => {}
        }
    };
    assert_eq!(
        refused,
        Response::Error {
            code: ErrorCode::BadRequest
        },
        "input {}",
        String::from_utf8_lossy(input)
    );
    assert!(matches!(
        read_frame(&mut raw, MAX_LIVE_BODY),
        Err(FrameError::Closed)
    ));
}

/// D4: launcher の Live View 経路は mouse・keyboard・control（takeover）の入力を拒否する。拒否された
/// 入力は session に届かず（action 数 0）、偽 Chrome に `Input.*` は一度も届かず、session の状態
/// （Running・観測・session 数）は変わらない。拒否の後も daemon の Live View が本人の slot に frame
/// を届ける。
#[tokio::test]
async fn browser_launcher_live_view_rejects_input_mouse_keyboard_control() {
    let launcher = FakeLauncher::new();
    let sock = launcher.sock.clone();
    let (daemon, attestation) =
        blocking(move || DaemonLauncherSession::start(&sock, "t1", "r1", policy()))
            .await
            .expect("start");
    let daemon = Arc::new(daemon);
    let probe = launcher.session(0);

    let d = Arc::clone(&daemon);
    let (state_before, facts_before) = blocking(move || d.observe()).await.expect("observe");
    let sessions_before = launcher.handle.session_count();

    let sid = daemon.session_id().to_owned();
    let lease = daemon.lease_id().to_owned();
    let action = |verb: Verb, args: ActionArgs| {
        serde_json::to_vec(&Request::Action {
            session_id: sid.clone(),
            lease_id: lease.clone(),
            verb,
            args,
        })
        .expect("json")
    };
    let raw = |v: Value| serde_json::to_vec(&v).expect("json");
    let inputs: Vec<(&str, Vec<u8>)> = vec![
        // mouse
        (
            "mouse-click",
            action(
                Verb::Click,
                ActionArgs {
                    x: Some(10),
                    y: Some(20),
                    ..ActionArgs::default()
                },
            ),
        ),
        (
            "mouse-raw",
            raw(
                json!({"type":"dispatch_mouse_event","session_id":sid,"lease_id":lease,"x":1,"y":2,"button":"left"}),
            ),
        ),
        (
            "mouse-live-input",
            raw(
                json!({"type":"live_input","session_id":sid,"lease_id":lease,"kind":"mouse","x":1,"y":2}),
            ),
        ),
        // keyboard
        (
            "keyboard-raw",
            raw(
                json!({"type":"dispatch_key_event","session_id":sid,"lease_id":lease,"text":"hunter2"}),
            ),
        ),
        (
            "keyboard-live-input",
            raw(
                json!({"type":"live_input","session_id":sid,"lease_id":lease,"kind":"key","text":"a"}),
            ),
        ),
        // control（takeover）・frame 接続上の他の要求
        (
            "control",
            raw(json!({"type":"live_control","session_id":sid,"lease_id":lease})),
        ),
        (
            "takeover",
            raw(json!({"type":"takeover","session_id":sid,"lease_id":lease})),
        ),
        ("navigate", action(Verb::Open, ActionArgs::default())),
    ];
    for (name, input) in &inputs {
        let (sock, sid, lease, input) = (
            launcher.sock.clone(),
            sid.clone(),
            lease.clone(),
            input.clone(),
        );
        blocking(move || send_input_on_frame_connection(&sock, &sid, &lease, &input)).await;
        assert_eq!(
            probe.actions.load(Ordering::SeqCst),
            0,
            "{name} reached the session"
        );
    }

    // 偽 Chrome に入力の CDP command は一度も届かない（screencast と target の操作だけ）。
    let methods = probe.cdp.methods();
    assert!(!has_input_command(&methods), "{methods:?}");
    let allowed = [
        "Target.getTargets",
        "Target.attachToTarget",
        "Target.detachFromTarget",
        "Page.startScreencast",
        "Page.screencastFrameAck",
        "Page.stopScreencast",
    ];
    for m in &methods {
        assert!(allowed.contains(&m.as_str()), "unexpected CDP command {m}");
    }

    // session の状態は変わらない。
    let d = Arc::clone(&daemon);
    let (state_after, facts_after) = blocking(move || d.observe()).await.expect("observe");
    assert_eq!(state_before, SessionState::Running);
    assert_eq!(state_after, state_before);
    assert_eq!(facts_after, facts_before);
    assert_eq!(launcher.handle.session_count(), sessions_before);
    assert_eq!(probe.stopped.load(Ordering::SeqCst), 0);

    // 本人の frame 経路は続く: 拒否の後に daemon が開く Live View（v8）の slot に偽 Chrome の frame が
    // 届く（frame 接続は session に 1 本なので、拒否された接続が閉じた後に開く）。
    let (d, sock) = (Arc::clone(&daemon), launcher.sock.clone());
    let relay = blocking(move || d.open_live_frames(&sock)).await;
    let reg = LiveFrameRegistration::register(
        None,
        "logical-input",
        ("t1".into(), "r1".into()),
        attestation,
        relay,
    );
    assert_eq!(reg.entry().unavailable_reason(), None);
    assert!(
        task_core::browser_isolation::LiveIsolation::current_attestation(&**reg.entry()).is_ok()
    );
    let slot = reg.entry().live_frames().expect("v8 frame slot");
    probe.cdp.push_frame(b"FRAME-AFTER");
    assert_eq!(next_body(&slot).await.as_deref(), Some(&b"FRAME-AFTER"[..]));
    assert!(!has_input_command(&probe.cdp.methods()));
    // control 接続の action は従来どおり（拒否は frame 接続の入力だけ）。
    let d = Arc::clone(&daemon);
    blocking(move || d.action(Verb::Snapshot, ActionArgs::default()))
        .await
        .expect("action");
    assert_eq!(probe.actions.load(Ordering::SeqCst), 1);

    drop(reg);
    assert!(slot.is_closed());
    let d = Arc::clone(&daemon);
    blocking(move || d.stop()).await.expect("stop");
}
