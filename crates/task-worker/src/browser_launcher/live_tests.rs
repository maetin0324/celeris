//! Protocol v8 Live View (ADR 2026-10-10-browser-launcher-live-view-frames 付記 2026-10-10b) の試験。
//! 実 browser・userns は使わない: CDP は pipe の上の偽 Chrome、session は偽 backend で決定的に組む。

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};

use super::client::{ClientError, LauncherClient, LiveRead};
use super::live::{LiveFeed, LiveImage, LiveNext, LiveTap, ScreencastFeed};
use super::protocol::*;
use super::registry::Registry;
use super::server::*;
use crate::browser_cdp_sink::CdpController;
use crate::browser_runtime::process_starttime;

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn pipe() -> (File, File) {
    let mut fds = [0i32; 2];
    // SAFETY: fds has room for two descriptors.
    assert_eq!(
        unsafe { nix::libc::pipe2(fds.as_mut_ptr(), nix::libc::O_CLOEXEC) },
        0
    );
    // SAFETY: new descriptors owned only here.
    unsafe {
        (
            File::from(OwnedFd::from_raw_fd(fds[0])),
            File::from(OwnedFd::from_raw_fd(fds[1])),
        )
    }
}

// ---- fake Chrome on the CDP pipe ----

/// Commands the launcher wrote to the fake Chrome (method, sessionId, params).
type CdpLog = Arc<Mutex<Vec<(String, Option<String>, Value)>>>;

const SECRET_URL: &str = "https://secret.example.test/account?token=SECRET-URL-MARKER";
const CDP_SECRET: &str = "CDP-RESPONSE-SECRET-MARKER";

fn send(w: &mut File, v: Value) {
    let mut b = serde_json::to_vec(&v).expect("json");
    b.push(0);
    w.write_all(&b).expect("write cdp");
}

fn huge_frame() -> String {
    b64(&vec![b'x'; MAX_LIVE_BODY + 16])
}

fn fake_chrome(mut input: File, mut out: File, log: CdpLog) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut acks = 0;
    loop {
        let n = match input.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        while let Some(end) = buf.iter().position(|b| *b == 0) {
            let msg: Vec<u8> = buf.drain(..=end).collect();
            let v: Value = serde_json::from_slice(&msg[..end]).expect("cdp json");
            let id = v["id"].clone();
            let method = v["method"].as_str().unwrap_or_default().to_owned();
            let session = v["sessionId"].as_str().map(str::to_owned);
            log.lock()
                .expect("log")
                .push((method.clone(), session.clone(), v["params"].clone()));
            let mut reply = json!({"id":id,"result":{}});
            if let Some(s) = &session {
                reply["sessionId"] = s.clone().into();
            }
            match method.as_str() {
                "Target.getTargets" => {
                    reply["result"] = json!({"targetInfos":[
                        {"targetId":"P0","type":"service_worker","url":SECRET_URL},
                        {"targetId":"P1","type":"page","url":SECRET_URL,"title":CDP_SECRET}
                    ]});
                }
                "Target.attachToTarget" => {
                    send(
                        &mut out,
                        json!({"method":"Target.attachedToTarget","params":{"sessionId":"LIVE","targetInfo":{"targetId":"P1","type":"page","url":SECRET_URL},"waitingForDebugger":false}}),
                    );
                    reply["result"] = json!({"sessionId":"LIVE"});
                }
                "Page.startScreencast" => {
                    send(&mut out, reply.clone());
                    // The agent's own session (screencast from an agent connection) and a root event
                    // still reach the controller; only the launcher's session is taken out.
                    send(
                        &mut out,
                        json!({"method":"Page.screencastFrame","sessionId":"AGENT","params":{"data":b64(b"AGENT-FRAME"),"sessionId":1,"metadata":{"deviceWidth":10,"deviceHeight":10}}}),
                    );
                    send(
                        &mut out,
                        json!({"method":"Target.targetCreated","params":{"targetInfo":{"targetId":"P2","type":"page"}}}),
                    );
                    send(
                        &mut out,
                        json!({"method":"Page.frameNavigated","sessionId":"LIVE","params":{"frame":{"url":SECRET_URL}}}),
                    );
                    send(
                        &mut out,
                        json!({"method":"Page.screencastFrame","sessionId":"LIVE","params":{"data":b64(b"FRAME-1"),"sessionId":7,"metadata":{"deviceWidth":800.0,"deviceHeight":600.4}}}),
                    );
                    continue;
                }
                "Page.screencastFrameAck" => {
                    acks += 1;
                    send(&mut out, reply.clone());
                    if acks == 1 {
                        send(
                            &mut out,
                            json!({"method":"Page.screencastFrame","sessionId":"LIVE","params":{"data":huge_frame(),"sessionId":8,"metadata":{"deviceWidth":800,"deviceHeight":600}}}),
                        );
                        send(
                            &mut out,
                            json!({"method":"Page.screencastFrame","sessionId":"LIVE","params":{"data":b64(b"FRAME-2"),"sessionId":9,"metadata":{"deviceWidth":800,"deviceHeight":600}}}),
                        );
                    }
                    continue;
                }
                "Target.detachFromTarget" => {
                    send(
                        &mut out,
                        json!({"method":"Target.detachedFromTarget","params":{"sessionId":"LIVE","targetId":"P1"}}),
                    );
                }
                _ => {}
            }
            send(&mut out, reply);
        }
    }
}

struct FakeCdp {
    tap: Arc<LiveTap>,
    controller: Arc<Mutex<CdpController>>,
    log: CdpLog,
}

fn fake_cdp() -> FakeCdp {
    let (chrome_out_r, chrome_out_w) = pipe();
    let (chrome_in_r, chrome_in_w) = pipe();
    let (ctrl_read, tap) = LiveTap::interpose(chrome_out_r).expect("interpose");
    let controller = Arc::new(Mutex::new(CdpController::new(chrome_in_w, ctrl_read)));
    let log: CdpLog = Arc::default();
    let l2 = log.clone();
    std::thread::spawn(move || fake_chrome(chrome_in_r, chrome_out_w, l2));
    FakeCdp {
        tap,
        controller,
        log,
    }
}

fn next_image(feed: &mut dyn LiveFeed) -> LiveImage {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match feed.next_frame(Duration::from_millis(200)) {
            LiveNext::Frame(img) => return img,
            LiveNext::Idle => {}
            LiveNext::Ended => panic!("feed ended"),
        }
    }
    panic!("no frame within 20 s");
}

/// Agent-visible events after a round trip (every event Chrome wrote before the reply is queued).
fn agent_events(cdp: &FakeCdp) -> String {
    let mut c = cdp.controller.lock().expect("controller");
    c.controller_command("Browser.getVersion", json!({}), None)
        .expect("barrier");
    serde_json::to_string(&c.take_agent_events()).expect("json")
}

fn methods(log: &CdpLog) -> Vec<(String, Option<String>, Value)> {
    log.lock().expect("log").clone()
}

/// D1/D2/D4: the launcher-owned controller starts, acks and stops the screencast on its own CDP
/// session; the frames, the session's events and its attach announcement never reach the agent
/// queue; another session's frames and non-frame CDP data never reach the feed; nothing but the
/// screencast commands (no `Input.*`) is sent.
#[test]
fn browser_launcher_live_frame_cdp_screencast_start_ack_stop_on_fake_cdp() {
    let cdp = fake_cdp();
    let mut feed = ScreencastFeed::new(cdp.tap.clone(), cdp.controller.clone());
    let first = next_image(&mut feed);
    assert_eq!(first.body(), b"FRAME-1");
    assert_eq!((first.width(), first.height()), (800, 600));
    assert_eq!(first.encoding(), LiveEncoding::Jpeg);

    let events = agent_events(&cdp);
    assert!(
        events.contains("AGENT"),
        "agent's own events stay: {events}"
    );
    assert!(events.contains("Target.targetCreated"), "{events}");
    assert!(
        !events.contains("LIVE"),
        "screencast session leaked: {events}"
    );
    assert!(!events.contains(&b64(b"FRAME-1")), "{events}");

    // The oversized frame (> 2 MiB decoded) is dropped; the next one arrives.
    let second = next_image(&mut feed);
    assert_eq!(second.body(), b"FRAME-2");
    drop(feed);
    let events = agent_events(&cdp);
    assert!(
        !events.contains("LIVE"),
        "detach announcement leaked: {events}"
    );

    let log = methods(&cdp.log);
    let on_live = |m: &str| {
        log.iter()
            .any(|(method, s, _)| method == m && s.as_deref() == Some("LIVE"))
    };
    assert!(on_live("Page.startScreencast"));
    assert!(on_live("Page.stopScreencast"));
    assert!(log.iter().any(|(m, s, p)| m == "Page.screencastFrameAck"
        && s.as_deref() == Some("LIVE")
        && p["sessionId"] == 7));
    assert!(
        log.iter()
            .any(|(m, _, p)| m == "Target.attachToTarget" && p["targetId"] == "P1")
    );
    assert!(
        log.iter()
            .any(|(m, _, p)| m == "Target.detachFromTarget" && p["sessionId"] == "LIVE")
    );
    let allowed = [
        "Target.getTargets",
        "Target.attachToTarget",
        "Target.detachFromTarget",
        "Page.startScreencast",
        "Page.screencastFrameAck",
        "Page.stopScreencast",
        "Browser.getVersion",
    ];
    for (m, _, _) in &log {
        assert!(allowed.contains(&m.as_str()), "unexpected CDP command {m}");
    }
    assert!(cdp.tap.frames_seen() >= 2);
}

/// D3: the owner's view follows the auth section's login target (the preferred target).
#[test]
fn browser_launcher_live_frame_follows_preferred_target() {
    let cdp = fake_cdp();
    cdp.tap.set_preferred_target(Some("P1".into()));
    let mut feed = ScreencastFeed::new(cdp.tap.clone(), cdp.controller.clone());
    assert_eq!(next_image(&mut feed).body(), b"FRAME-1");
    let attach = methods(&cdp.log)
        .into_iter()
        .filter(|(m, _, _)| m == "Target.attachToTarget")
        .count();
    assert_eq!(attach, 1);
}

// ---- protocol v8 wire ----

#[test]
fn browser_launcher_protocol_v8_frames() {
    assert_eq!(PROTOCOL_VERSION, 8);
    assert_eq!(PROTOCOL_VERSION, LIVE_FRAME_PROTOCOL);
    assert_eq!(MAX_LIVE_BODY, 2_097_152);
    const { assert!(CONSENT_PROTOCOL < LIVE_FRAME_PROTOCOL) };

    let start = Request::LiveStart {
        session_id: "s1".into(),
        lease_id: "l1".into(),
    };
    let body = serde_json::to_string(&start).expect("json");
    assert_eq!(
        body,
        r#"{"type":"live_start","session_id":"s1","lease_id":"l1"}"#
    );
    assert_eq!(decode_request(body.as_bytes()), Ok(start));
    let stop = Request::LiveStop {
        session_id: "s1".into(),
        lease_id: "l1".into(),
    };
    let body = serde_json::to_vec(&stop).expect("json");
    assert_eq!(decode_request(&body), Ok(stop));

    let frame = Response::LiveFrame {
        session_id: "s1".into(),
        seq: 3,
        width: 800,
        height: 600,
        encoding: LiveEncoding::Jpeg,
        body_len: 1234,
    };
    let text = serde_json::to_string(&frame).expect("json");
    assert_eq!(
        text,
        r#"{"type":"live_frame","session_id":"s1","seq":3,"width":800,"height":600,"encoding":"jpeg","body_len":1234}"#
    );
    assert_eq!(
        serde_json::from_str::<Response>(&text).expect("decode"),
        frame
    );
}

#[test]
fn browser_launcher_live_frame_unknown_fields_and_verbs_rejected() {
    for bad in [
        r#"{"type":"live_start","session_id":"s","lease_id":"l","url":"https://x"}"#,
        r#"{"type":"live_start","session_id":"s","lease_id":"l","input":true}"#,
        r#"{"type":"live_stop","session_id":"s"}"#,
        r#"{"type":"live_start","session_id":"../s","lease_id":"l"}"#,
        r#"{"type":"live_input","session_id":"s","lease_id":"l","x":1,"y":2}"#,
        r#"{"type":"live_control","session_id":"s","lease_id":"l"}"#,
        r#"{"type":"dispatch_mouse_event","session_id":"s","lease_id":"l"}"#,
        r#"{"type":"dispatch_key_event","session_id":"s","lease_id":"l","text":"a"}"#,
        r#"{"type":"live_frame","session_id":"s","seq":1,"width":1,"height":1,"encoding":"jpeg","body_len":1}"#,
    ] {
        assert!(decode_request(bad.as_bytes()).is_err(), "{bad}");
    }
    // A notification with anything beyond the fixed metadata does not decode on the daemon side.
    for bad in [
        r#"{"type":"live_frame","session_id":"s","seq":1,"width":1,"height":1,"encoding":"jpeg","body_len":1,"url":"https://x"}"#,
        r#"{"type":"live_frame","session_id":"s","seq":1,"width":1,"height":1,"encoding":"gif","body_len":1}"#,
        r#"{"type":"live_frame","session_id":"s","seq":1,"width":1,"height":1,"encoding":"jpeg","body_len":1,"cdp":{}}"#,
    ] {
        assert!(serde_json::from_str::<Response>(bad).is_err(), "{bad}");
    }
}

// ---- launcher server with a fake backend ----

fn uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

/// Frames a test pushes into one session's feed.
type FeedTx = mpsc::Sender<LiveNext>;

/// One sender per started session, replaced by every `live_start` (the feed of the open stream).
type Feeds = Arc<Mutex<Vec<Option<FeedTx>>>>;

#[derive(Default)]
struct LiveBackend {
    feeds: Feeds,
    actions: Arc<AtomicUsize>,
}

struct ChannelFeed(mpsc::Receiver<LiveNext>);

impl LiveFeed for ChannelFeed {
    fn next_frame(&mut self, wait: Duration) -> LiveNext {
        match self.0.recv_timeout(wait) {
            Ok(n) => n,
            Err(mpsc::RecvTimeoutError::Timeout) => LiveNext::Idle,
            Err(mpsc::RecvTimeoutError::Disconnected) => LiveNext::Ended,
        }
    }
}

struct LiveSession {
    child: Option<std::process::Child>,
    index: usize,
    feeds: Feeds,
    actions: Arc<AtomicUsize>,
}

impl SessionBackend for LiveBackend {
    fn start(&self, _req: &StartRequest) -> Result<Launched, ErrorCode> {
        use std::os::unix::process::CommandExt;
        let child = std::process::Command::new("sleep")
            .arg("600")
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .spawn()
            .map_err(|_| ErrorCode::LaunchFailed)?;
        let pid = child.id() as i32;
        let starttime = process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
        let index = {
            let mut feeds = self.feeds.lock().expect("lock");
            feeds.push(None);
            feeds.len() - 1
        };
        Ok(Launched {
            session: Box::new(LiveSession {
                child: Some(child),
                index,
                feeds: self.feeds.clone(),
                actions: self.actions.clone(),
            }),
            pid,
            pgid: pid,
            starttime,
            runtime_pid: pid,
            runtime_starttime: starttime,
            ns_inodes: task_core::browser_isolation::collect_ns_inodes(&pid.to_string())
                .map_err(|_| ErrorCode::LaunchFailed)?,
        })
    }
}

impl BackendSession for LiveSession {
    fn action(&mut self, verb: Verb, _args: &ActionArgs) -> Result<Observation, ErrorCode> {
        self.actions.fetch_add(1, Ordering::SeqCst);
        Ok(Observation {
            text: Some(format!("{verb:?}")),
            artifact: None,
        })
    }
    fn live(&mut self) -> Result<Box<dyn LiveFeed>, ErrorCode> {
        let (tx, rx) = mpsc::channel();
        self.feeds.lock().expect("lock")[self.index] = Some(tx);
        Ok(Box::new(ChannelFeed(rx)))
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        (SessionState::Running, SessionFacts::default())
    }
    fn isolation_ok(&mut self) -> bool {
        true
    }
    fn stop(mut self: Box<Self>) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    sock: PathBuf,
    backend: Arc<LiveBackend>,
    _handle: ServerHandle,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("l.sock");
    let registry = Registry::open(dir.path().join("state"), "inst-live").expect("registry");
    let backend = Arc::new(LiveBackend::default());
    let server = LauncherServer::bind(
        &sock,
        ServerConfig {
            allowed_uids: vec![uid()],
            limits: LauncherLimits::default(),
        },
        backend.clone(),
        registry,
    )
    .expect("bind");
    let handle = server.spawn().expect("spawn");
    Fixture {
        _dir: dir,
        sock,
        backend,
        _handle: handle,
    }
}

fn connect(sock: &Path) -> LauncherClient {
    LauncherClient::connect(sock, Duration::from_secs(10)).expect("connect")
}

fn policy() -> SessionPolicy {
    SessionPolicy {
        allowed_domains: vec!["example.com".into()],
        allowed_actions: vec![Verb::Open, Verb::Click, Verb::Snapshot],
        lease_seconds: 60,
    }
}

fn push(f: &Fixture, session: usize, body: &[u8]) {
    let tx = f.backend.feeds.lock().expect("lock")[session]
        .clone()
        .expect("live stream open");
    tx.send(LiveNext::Frame(LiveImage::new(
        640,
        480,
        LiveEncoding::Jpeg,
        body.to_vec(),
    )))
    .expect("send frame");
}

fn read_image(stream: &mut super::client::LiveStream) -> (u64, LiveImage) {
    match stream.next_frame().expect("frame") {
        LiveRead::Frame(seq, img) => (seq, img),
        LiveRead::Stopped => panic!("stopped"),
    }
}

/// The v8 frame connection: `live_start` on a dedicated connection of the session's daemon,
/// bounded frames bound to the session, `live_stop`; the control connection keeps working.
#[test]
fn browser_launcher_live_frame_stream_roundtrip() {
    let f = fixture();
    let mut c = connect(&f.sock);
    let (version, _) = c.hello().expect("hello");
    let s = c
        .start_session("t1", "r1", "lease1", policy())
        .expect("start");
    let mut live = LauncherClient::open_live(
        &f.sock,
        Duration::from_secs(10),
        version,
        &s.session_id,
        "lease1",
    )
    .expect("open live");
    assert_eq!(live.session_id(), s.session_id);
    push(&f, 0, b"IMG-1");
    // An oversized image from the backend is never written.
    push(&f, 0, &vec![0u8; MAX_LIVE_BODY + 1]);
    push(&f, 0, b"IMG-2");
    let (seq, img) = read_image(&mut live);
    assert_eq!((seq, img.body(), img.width()), (1, &b"IMG-1"[..], 640));
    let (seq, img) = read_image(&mut live);
    assert_eq!((seq, img.body()), (2, &b"IMG-2"[..]));
    assert!(live.responder().is_some());
    live.stop().expect("live stop");
    // The session and its control connection are unaffected.
    c.observe(&s.session_id, "lease1").expect("observe");
    c.action(
        &s.session_id,
        "lease1",
        Verb::Snapshot,
        ActionArgs::default(),
    )
    .expect("action");
}

/// 互換表: below v8 the client refuses before connecting (no `live_start` reaches a v7
/// launcher) with the fixed reason, and the session continues.
#[test]
fn browser_launcher_v7_continues_without_live_view() {
    let f = fixture();
    let mut c = connect(&f.sock);
    let s = c
        .start_session("t1", "r1", "lease1", policy())
        .expect("start");
    let missing = f._dir.path().join("no-such.sock");
    for v in [4, 7] {
        // Even a socket that does not exist is not touched: the refusal precedes any connect.
        let e =
            LauncherClient::open_live(&missing, Duration::from_secs(1), v, &s.session_id, "lease1")
                .expect_err("v7 has no live frames");
        assert!(matches!(e, ClientError::NoLiveFrames(n) if n == v), "{e}");
        assert!(e.to_string().contains(ClientError::NO_LIVE_FRAMES));
    }
    c.observe(&s.session_id, "lease1").expect("observe");
    c.action(
        &s.session_id,
        "lease1",
        Verb::Snapshot,
        ActionArgs::default(),
    )
    .expect("action");
    assert!(f.backend.feeds.lock().expect("lock")[0].is_none());
    // A v8 stream can still start later.
    LauncherClient::open_live(
        &f.sock,
        Duration::from_secs(10),
        LIVE_FRAME_PROTOCOL,
        &s.session_id,
        "lease1",
    )
    .expect("v8 live");
}

/// A frame connection is bound to exactly one session: another session's frames never appear on
/// it, a wrong lease or the session's control connection is refused, and one stream per session.
#[test]
fn browser_launcher_live_frame_other_session_rejected() {
    let f = fixture();
    let mut a = connect(&f.sock);
    let sa = a.start_session("t1", "r1", "leaseA", policy()).expect("a");
    let mut b = connect(&f.sock);
    let sb = b.start_session("t2", "r2", "leaseB", policy()).expect("b");
    let open = |sid: &str, lease: &str| {
        LauncherClient::open_live(&f.sock, Duration::from_secs(10), 8, sid, lease)
    };
    assert!(matches!(
        open(&sb.session_id, "leaseA"),
        Err(ClientError::Remote(ErrorCode::LeaseMismatch))
    ));
    assert!(matches!(
        open("0123456789abcdef", "leaseA"),
        Err(ClientError::Remote(ErrorCode::LeaseMismatch))
    ));
    // Frames never flow on the control connection.
    assert!(matches!(
        a.request(&Request::LiveStart {
            session_id: sa.session_id.clone(),
            lease_id: "leaseA".into(),
        }),
        Err(ClientError::Remote(ErrorCode::Unauthorized))
    ));
    let mut live_a = open(&sa.session_id, "leaseA").expect("live a");
    assert!(matches!(
        open(&sa.session_id, "leaseA"),
        Err(ClientError::Remote(ErrorCode::Limit))
    ));
    // B's stream on a raw connection (the launcher allows 4 connections: a, b, live a, this).
    let mut raw = UnixStream::connect(&f.sock).expect("raw");
    raw.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write_message(
        &mut raw,
        &Request::LiveStart {
            session_id: sb.session_id.clone(),
            lease_id: "leaseB".into(),
        },
        DEFAULT_MAX_FRAME,
    )
    .expect("start");
    let started: Response =
        serde_json::from_slice(&read_frame(&mut raw, DEFAULT_MAX_FRAME).expect("started"))
            .expect("decode");
    assert!(
        matches!(started, Response::LiveStarted { .. }),
        "{started:?}"
    );
    push(&f, 1, b"B-FRAME");
    push(&f, 0, b"A-FRAME");
    let (_, img) = read_image(&mut live_a);
    assert_eq!(img.body(), b"A-FRAME");
    let meta: Response =
        serde_json::from_slice(&read_frame(&mut raw, DEFAULT_MAX_FRAME).expect("meta"))
            .expect("decode");
    assert!(
        matches!(&meta, Response::LiveFrame { session_id, seq: 1, body_len: 7, .. } if *session_id == sb.session_id),
        "{meta:?}"
    );
    assert_eq!(
        read_frame(&mut raw, MAX_LIVE_BODY).expect("body"),
        b"B-FRAME"
    );
    // `live_stop` naming another session is refused and ends the stream.
    write_message(
        &mut raw,
        &Request::LiveStop {
            session_id: sa.session_id.clone(),
            lease_id: "leaseA".into(),
        },
        DEFAULT_MAX_FRAME,
    )
    .expect("stop");
    let refused: Response =
        serde_json::from_slice(&read_frame(&mut raw, DEFAULT_MAX_FRAME).expect("refused"))
            .expect("decode");
    assert_eq!(
        refused,
        Response::Error {
            code: ErrorCode::LeaseMismatch
        }
    );
    assert!(matches!(
        read_frame(&mut raw, DEFAULT_MAX_FRAME),
        Err(FrameError::Closed)
    ));
}

/// D4: the frame connection is read-only. An action, `authenticate`-shaped or unknown input verb
/// on it is refused with `bad_request`, closes the stream and never reaches the session.
#[test]
fn browser_launcher_live_view_rejects_input() {
    let f = fixture();
    let mut c = connect(&f.sock);
    let s = c
        .start_session("t1", "r1", "lease1", policy())
        .expect("start");
    let inputs = [
        serde_json::to_vec(&Request::Action {
            session_id: s.session_id.clone(),
            lease_id: "lease1".into(),
            verb: Verb::Click,
            args: ActionArgs {
                selector: Some("#x".into()),
                ..ActionArgs::default()
            },
        })
        .expect("json"),
        br#"{"type":"live_input","session_id":"s","lease_id":"l","x":1,"y":1}"#.to_vec(),
        br#"{"type":"dispatch_key_event","text":"a"}"#.to_vec(),
    ];
    for input in inputs {
        let mut raw = UnixStream::connect(&f.sock).expect("raw");
        raw.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        write_message(
            &mut raw,
            &Request::LiveStart {
                session_id: s.session_id.clone(),
                lease_id: "lease1".into(),
            },
            DEFAULT_MAX_FRAME,
        )
        .expect("start");
        let started: Response =
            serde_json::from_slice(&read_frame(&mut raw, DEFAULT_MAX_FRAME).expect("started"))
                .expect("decode");
        assert!(matches!(started, Response::LiveStarted { .. }));
        write_frame(&mut raw, &input, DEFAULT_MAX_FRAME).expect("input");
        let refused: Response =
            serde_json::from_slice(&read_frame(&mut raw, DEFAULT_MAX_FRAME).expect("refused"))
                .expect("decode");
        assert_eq!(
            refused,
            Response::Error {
                code: ErrorCode::BadRequest
            }
        );
        assert!(matches!(
            read_frame(&mut raw, DEFAULT_MAX_FRAME),
            Err(FrameError::Closed)
        ));
    }
    assert_eq!(f.backend.actions.load(Ordering::SeqCst), 0);
}

// ---- client against a raw fake launcher ----

/// Serves `live_started` for session `s1` and then `script` verbatim.
fn fake_launcher(script: Vec<Vec<u8>>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("fake.sock");
    let listener = UnixListener::bind(&sock).expect("bind");
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        let _ = read_frame(&mut s, DEFAULT_MAX_FRAME).expect("live_start");
        write_message(
            &mut s,
            &Response::LiveStarted {
                session_id: "s1".into(),
                max_body: MAX_LIVE_BODY as u64,
            },
            DEFAULT_MAX_FRAME,
        )
        .expect("started");
        for chunk in script {
            if s.write_all(&chunk).is_err() {
                return;
            }
        }
        // Keep the connection open until the client gave up.
        let mut b = [0u8; 1];
        let _ = s.read(&mut b);
    });
    (dir, sock)
}

fn meta(session: &str, seq: u64, body_len: u64) -> Vec<u8> {
    let body = serde_json::to_vec(&Response::LiveFrame {
        session_id: session.into(),
        seq,
        width: 1,
        height: 1,
        encoding: LiveEncoding::Png,
        body_len,
    })
    .expect("json");
    let mut v = (body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(&body);
    v
}

fn framed(body: &[u8]) -> Vec<u8> {
    let mut v = (body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

fn client_error(script: Vec<Vec<u8>>) -> String {
    let (_dir, sock) = fake_launcher(script);
    let mut live =
        LauncherClient::open_live(&sock, Duration::from_secs(10), 8, "s1", "l1").expect("open");
    loop {
        match live.next_frame() {
            Ok(LiveRead::Frame(..)) => {}
            Ok(LiveRead::Stopped) => return "stopped".into(),
            Err(e) => return e.to_string(),
        }
    }
}

/// The daemon side bounds and binds every frame: over 2 MiB, a body whose length differs from
/// its metadata, another session's frame and a replayed sequence are protocol errors.
#[test]
fn browser_launcher_live_frame_client_enforces_bounds_and_binding() {
    let ok = [meta("s1", 1, 3), framed(b"abc")];
    let e = client_error(vec![
        ok[0].clone(),
        ok[1].clone(),
        meta("s1", 2, MAX_LIVE_BODY as u64 + 1),
    ]);
    assert!(e.contains("body too large"), "{e}");
    let e = client_error(vec![meta("s1", 1, 4), framed(b"abc")]);
    assert!(e.contains("differs"), "{e}");
    let e = client_error(vec![
        meta("s1", 1, 3),
        framed(&vec![0u8; MAX_LIVE_BODY + 1]),
    ]);
    assert!(e.contains("too large"), "{e}");
    let e = client_error(vec![meta("other", 1, 3), framed(b"abc")]);
    assert!(e.contains("another session"), "{e}");
    let e = client_error(vec![
        ok[0].clone(),
        ok[1].clone(),
        ok[0].clone(),
        ok[1].clone(),
    ]);
    assert!(e.contains("sequence"), "{e}");
    let e = client_error(vec![
        ok[0].clone(),
        ok[1].clone(),
        framed(br#"{"type":"live_stopped","session_id":"s1"}"#),
    ]);
    assert_eq!(e, "stopped");
}
