use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use super::*;
use crate::browser_launcher::protocol::{
    LiveEncoding, MAX_LIVE_BODY, Request, Response, read_frame, write_frame, write_message,
};
use crate::browser_launcher::{
    BackendSession, ErrorCode, Launched, LauncherLimits, LauncherServer, LiveFeed, LiveImage,
    LiveNext, Registry, ServerConfig, ServerHandle, SessionBackend, StartRequest,
};
use crate::browser_live::{
    CollectingSink, InMemoryGate, LIVE_NO_FRAMES_REASON, LiveEmitter, LiveFrameRegistration,
};
use task_core::browser_isolation::{LiveSessionRegistry, LiveSessions};
use task_core::browser_live::LiveEvent;

/// 長い保険（出来事待ちが来なかったときだけ効く）。
const INSURANCE: Duration = Duration::from_secs(30);
const LAUNCHER_UID: u32 = 4_000_001;
const SUBUID: u32 = 5_000_000;
const MARKER: &[u8] = b"CREDENTIAL-INPUT-hunter2-PIXELS";

type Feeds = Arc<Mutex<Vec<Option<mpsc::Sender<LiveNext>>>>>;
/// harness が run 中に見たもの: frame 購読口の有無・live key・slot で受け取った frame。
type Seen = (bool, Option<(String, String)>, Option<Vec<u8>>);

#[derive(Default)]
struct LiveBackend {
    feeds: Feeds,
    live_opened: Arc<AtomicUsize>,
    actions: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
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
    live_opened: Arc<AtomicUsize>,
    actions: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
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
        let starttime =
            crate::browser_runtime::process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
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
                live_opened: self.live_opened.clone(),
                actions: self.actions.clone(),
                stopped: self.stopped.clone(),
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

impl BackendSession for LiveSession {
    fn action(&mut self, verb: Verb, _args: &ActionArgs) -> Result<Observation, ErrorCode> {
        self.actions.fetch_add(1, Ordering::SeqCst);
        Ok(Observation {
            text: Some(format!(
                r#"{{"success":true,"data":{{"verb":"{verb:?}"}}}}"#
            )),
            artifact: None,
        })
    }
    fn live(&mut self) -> Result<Box<dyn LiveFeed>, ErrorCode> {
        let (tx, rx) = mpsc::channel();
        self.feeds.lock().expect("lock")[self.index] = Some(tx);
        self.live_opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ChannelFeed(rx)))
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        (SessionState::Running, good_facts())
    }
    fn isolation_ok(&mut self) -> bool {
        true
    }
    fn auth_begin(&mut self, _auth_section_id: &str) -> Result<String, ErrorCode> {
        Ok("TARGET-LOGIN".into())
    }
    fn stop(mut self: Box<Self>) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    dir: tempfile::TempDir,
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
            allowed_uids: vec![DaemonIds::current().uid],
            limits: LauncherLimits::default(),
        },
        backend.clone(),
        registry,
    )
    .expect("bind");
    let handle = server.spawn().expect("spawn");
    Fixture {
        dir,
        sock,
        backend,
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

/// 出来事待ち: 偽 backend の frame 接続（`live()`）が開くまで。
fn feed(f: &Fixture, session: usize) -> mpsc::Sender<LiveNext> {
    let deadline = std::time::Instant::now() + INSURANCE;
    loop {
        if let Some(tx) = f.backend.feeds.lock().expect("lock")[session].clone() {
            return tx;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "live stream never opened"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn push(f: &Fixture, session: usize, body: &[u8]) {
    feed(f, session)
        .send(LiveNext::Frame(LiveImage::new(
            640,
            480,
            LiveEncoding::Jpeg,
            body.to_vec(),
        )))
        .expect("send frame");
}

async fn next_body(slot: &task_core::browser_live_frame::LatestFrameSlot) -> Option<Vec<u8>> {
    tokio::time::timeout(INSURANCE, slot.next())
        .await
        .expect("slot event")
        .map(|frame| frame.body().to_vec())
}

fn registry_entry(
    reg: &LiveSessions,
    session: &str,
) -> Arc<dyn task_core::browser_isolation::LiveSessionEntry> {
    reg.get(session).expect("registered")
}

// ---- v7 launcher: `hello` の版だけを書き換える proxy ----

/// 偽 launcher の前に置く proxy。全接続の要求の種類を順に記録し、`rewrite` があれば `hello` の
/// `protocol_version` をその値にする（v7 launcher の再現）。
struct Proxy {
    sock: PathBuf,
    seen: Arc<Mutex<Vec<String>>>,
}

fn proxy(upstream: &Path, dir: &Path, rewrite: Option<u32>) -> Proxy {
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
            let (mut c_in, mut s_out) = (
                client.try_clone().expect("clone"),
                server.try_clone().expect("clone"),
            );
            let seen = Arc::clone(&seen_t);
            std::thread::spawn(move || {
                while let Ok(body) = read_frame(&mut c_in, 4 * MAX_LIVE_BODY) {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
                        let kind = v["type"].as_str().unwrap_or("?").to_owned();
                        seen.lock().expect("lock").push(kind);
                    }
                    if write_frame(&mut s_out, &body, 4 * MAX_LIVE_BODY).is_err() {
                        break;
                    }
                }
                let _ = s_out.shutdown(std::net::Shutdown::Both);
            });
            let (mut s_in, mut c_out) = (server, client);
            std::thread::spawn(move || {
                while let Ok(mut body) = read_frame(&mut s_in, 4 * MAX_LIVE_BODY) {
                    if let (Some(v), Ok(mut json)) =
                        (rewrite, serde_json::from_slice::<serde_json::Value>(&body))
                        && json["type"] == "hello"
                    {
                        json["protocol_version"] = v.into();
                        body = serde_json::to_vec(&json).expect("json");
                    }
                    if write_frame(&mut c_out, &body, 4 * MAX_LIVE_BODY).is_err() {
                        break;
                    }
                }
                let _ = c_out.shutdown(std::net::Shutdown::Both);
            });
        }
    });
    Proxy { sock, seen }
}

// ---- 1. frame 接続の束縛と後始末 ----

/// 偽の frame 接続: `live_started` は正しい session に答え、1 枚目は正しい session、2 枚目は
/// 別 session の frame を流す。daemon は 1 枚目だけを slot に入れ、2 枚目で stream を終えて
/// slot を閉じる（別 session の frame は誰にも届かない）。
#[tokio::test]
async fn browser_live_frame_session_mismatch_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("raw.sock");
    let listener = UnixListener::bind(&sock).expect("bind");
    // 2 枚目は 1 枚目を受け取った後に送る（close は保持中の frame を捨てるので順を固定する）。
    let (taken_tx, taken_rx) = mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        let req = read_frame(&mut s, 1 << 20).expect("live_start");
        let req: Request = serde_json::from_slice(&req).expect("request");
        assert!(
            matches!(&req, Request::LiveStart { session_id, lease_id }
                if session_id == "sess-A" && lease_id == "lease-A"),
            "{req:?}"
        );
        let send = |s: &mut UnixStream, session: &str, seq: u64, body: &[u8]| {
            write_message(
                s,
                &Response::LiveFrame {
                    session_id: session.into(),
                    seq,
                    width: 2,
                    height: 2,
                    encoding: LiveEncoding::Png,
                    body_len: body.len() as u64,
                },
                1 << 20,
            )
            .expect("meta");
            // The daemon closes the relay as soon as it rejects the foreign-session
            // metadata. That can race this body write, so BrokenPipe is the expected
            // outcome for the deliberately invalid second frame.
            let _ = write_frame(s, body, MAX_LIVE_BODY);
        };
        write_message(
            &mut s,
            &Response::LiveStarted {
                session_id: "sess-A".into(),
                max_body: MAX_LIVE_BODY as u64,
            },
            1 << 20,
        )
        .expect("started");
        send(&mut s, "sess-A", 1, b"OWN-FRAME");
        taken_rx.recv_timeout(INSURANCE).expect("first frame taken");
        send(&mut s, "sess-B", 2, b"OTHER-SESSION-FRAME");
        // 接続は開いたまま: 終わりは daemon 側の判定による。
        let mut rest = Vec::new();
        let _ = std::io::Read::read_to_end(&mut s, &mut rest);
    });
    let relay = tokio::task::spawn_blocking({
        let sock = sock.clone();
        move || open_frame_relay(&sock, 9, "sess-A", "lease-A")
    })
    .await
    .expect("join")
    .unwrap_or_else(|reason| panic!("relay: {reason}"));
    let slot = relay.slot();
    assert_eq!(next_body(&slot).await.as_deref(), Some(&b"OWN-FRAME"[..]));
    taken_tx.send(()).expect("signal");
    assert_eq!(
        next_body(&slot).await,
        None,
        "other session's frame closes the slot"
    );
    assert!(slot.is_closed());
    drop(relay);
    server.join().expect("server");
}

/// session の後始末: registry 登録の drop（run の終わり・早期 return）は entry を外して slot を
/// 閉じ、session の stop は launcher 側の stream 終了で slot を閉じる。閉じた slot に後から来た
/// frame は捨てる。
#[tokio::test]
async fn browser_live_frame_cleanup_closes_slot_on_drop_and_stop() {
    let f = fixture();
    let registry = Arc::new(LiveSessions::default());
    // (a) registration の drop。
    let (runtime, _) = LauncherRuntime::start(&f.sock, "t1", "r1", policy()).expect("start");
    let runtime = Arc::new(runtime);
    let reg = live_registration(
        &runtime,
        &f.sock,
        Some(Arc::clone(&registry)),
        "logical-1",
        ("t1".into(), "r1".into()),
    )
    .await
    .expect("registered");
    let entry = registry_entry(&registry, "logical-1");
    assert_eq!(entry.live_key(), Some(("t1".into(), "r1".into())));
    assert!(
        !entry.accepts_state(),
        "launcher path takes no identity state"
    );
    let slot = entry.live_frames().expect("v9 frames");
    push(&f, 0, b"FRAME-1");
    assert_eq!(next_body(&slot).await.as_deref(), Some(&b"FRAME-1"[..]));
    drop(reg);
    assert!(registry.get("logical-1").is_none(), "entry removed");
    assert!(slot.is_closed());
    assert!(entry.live_frames().is_none());
    assert!(
        entry.current_attestation().is_err(),
        "ended session has no attestation"
    );
    let _ = feed(&f, 0).send(LiveNext::Frame(LiveImage::new(
        1,
        1,
        LiveEncoding::Jpeg,
        b"LATE".to_vec(),
    )));
    assert!(slot.try_take().is_none(), "no frame after close");
    let stop = Arc::clone(&runtime);
    tokio::task::spawn_blocking(move || stop.stop())
        .await
        .expect("join")
        .expect("stop");

    // (b) session の stop（registration はまだ持っている）。
    let (runtime, _) = LauncherRuntime::start(&f.sock, "t2", "r2", policy()).expect("start");
    let runtime = Arc::new(runtime);
    let reg = live_registration(
        &runtime,
        &f.sock,
        Some(Arc::clone(&registry)),
        "logical-2",
        ("t2".into(), "r2".into()),
    )
    .await
    .expect("registered");
    let slot = registry_entry(&registry, "logical-2")
        .live_frames()
        .expect("v9 frames");
    push(&f, 1, b"FRAME-2");
    assert_eq!(next_body(&slot).await.as_deref(), Some(&b"FRAME-2"[..]));
    let stop = Arc::clone(&runtime);
    tokio::task::spawn_blocking(move || stop.stop())
        .await
        .expect("join")
        .expect("stop");
    assert_eq!(next_body(&slot).await, None, "session stop closes the slot");
    drop(reg);
    assert!(registry.is_empty());
}

/// auth section（credential 注入中）の daemon 側: `LiveEmitter` は auth guard の間 event を捨て、
/// H3 の観測停止（restored の旗）は frame が流れても解けない。一方、本人向けの slot には auth
/// section の login 画面の frame が届く（D3）。frame は emitter の sink に一切入らない。
#[tokio::test]
async fn browser_live_frame_auth_section_owner_only() {
    let f = fixture();
    let registry = Arc::new(LiveSessions::default());
    let (runtime, _) = LauncherRuntime::start(&f.sock, "t1", "r1", policy()).expect("start");
    let runtime = Arc::new(runtime);
    let reg = live_registration(
        &runtime,
        &f.sock,
        Some(Arc::clone(&registry)),
        "logical-auth",
        ("t1".into(), "r1".into()),
    )
    .await
    .expect("registered");
    let observation_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let emitter = LiveEmitter::with_observation_stop(
        CollectingSink::default(),
        Arc::clone(&observation_stop),
    );
    let guard = emitter.auth_section();
    let auth = Arc::clone(&runtime);
    let target = tokio::task::spawn_blocking(move || auth.auth_begin("auth-1"))
        .await
        .expect("join")
        .expect("auth_begin");
    assert_eq!(target, "TARGET-LOGIN");
    observation_stop.store(true, Ordering::SeqCst);
    let slot = registry_entry(&registry, "logical-auth")
        .live_frames()
        .expect("owner slot stays open in the auth section");
    push(&f, 0, MARKER);
    assert_eq!(next_body(&slot).await.as_deref(), Some(MARKER));
    assert!(!emitter.emit(&LiveEvent::Status {
        state: "typing".into()
    }));
    drop(guard);
    assert!(
        emitter.in_auth_section(),
        "H3 observation stop is not lifted by frames"
    );
    assert!(!emitter.emit(&LiveEvent::Status {
        state: "after".into()
    }));
    assert!(
        emitter.sink().events().is_empty(),
        "no event reached the persisted sink"
    );
    drop(reg);
    let stop = Arc::clone(&runtime);
    let _ = tokio::task::spawn_blocking(move || stop.stop()).await;
}

// ---- 2. 版確認と v7 互換 ----

#[derive(Default)]
struct RecordingSink {
    browsers: Mutex<Vec<BrowserRun>>,
    text: Mutex<String>,
}

impl RecordingSink {
    fn note(&self, s: &str) {
        let mut t = self.text.lock().expect("lock");
        t.push_str(s);
        t.push('\n');
    }
}

impl EventSink for RecordingSink {
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<Arc<dyn crate::browser_live::ControlGate>> {
        Some(Arc::new(InMemoryGate::new()))
    }
    fn browser_updated(&self, browser: &BrowserRun) {
        self.note(&format!("{browser:?}"));
        self.browsers.lock().expect("lock").push(browser.clone());
    }
    fn browser_live(
        &self,
        _run_id: &str,
        _session_id: &str,
        event: &task_core::browser_live::ScrubbedLiveEvent,
    ) {
        self.note(&format!("{:?}", event.as_persisted()));
    }
    fn progress(&self, msg: &str) {
        self.note(msg);
    }
    fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
        self.note(&format!("{msg} {fields:?}"));
    }
    fn artifact(&self, artifact: &task_core::ArtifactRef) {
        self.note(&format!("{artifact:?}"));
    }
    fn comment(&self, body: &str) {
        self.note(body);
    }
}

/// harness の代わり: run 中に registry の entry を見て、frame 購読口があれば偽 backend に
/// MARKER の frame を流し、本人向けの slot で受け取れたかを記録する。agent への応答（outcome）
/// には frame を入れない。
struct ProbeAdapter {
    registry: Arc<LiveSessions>,
    feeds: Feeds,
    seen: Arc<Mutex<Option<Seen>>>,
}

#[async_trait::async_trait]
impl WorkerAdapter for ProbeAdapter {
    fn id(&self) -> &str {
        "fake"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let session = req
            .context
            .browser
            .as_ref()
            .map(|b| b.run.session_id.clone())
            .expect("browser context");
        let entry = self
            .registry
            .get(&session)
            .expect("registered during the run");
        let frames = entry.live_frames();
        let mut got = None;
        if let Some(slot) = &frames {
            let deadline = std::time::Instant::now() + INSURANCE;
            let tx = loop {
                if let Some(tx) = self.feeds.lock().expect("lock")[0].clone() {
                    break tx;
                }
                assert!(std::time::Instant::now() < deadline, "live never opened");
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            tx.send(LiveNext::Frame(LiveImage::new(
                8,
                8,
                LiveEncoding::Jpeg,
                MARKER.to_vec(),
            )))
            .expect("send");
            got = next_body(slot).await;
        }
        *self.seen.lock().expect("lock") = Some((frames.is_some(), entry.live_key(), got));
        Ok(RunOutcome {
            terminal: crate::Terminal::Done {
                summary: "harness finished".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn prepared() -> crate::browser_policy::PreparedBrowserPolicy {
    let grant = task_core::BrowserCapability {
        approval_actions: vec![],
        allowed_domains: vec!["example.com".into()],
        ..Default::default()
    };
    let task = task_core::BrowserTaskPolicy {
        policy_id: "launcher-live".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec!["example.com".into()],
        allowed_actions: vec![
            task_core::BrowserAction::Navigate,
            task_core::BrowserAction::Snapshot,
        ],
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    };
    crate::browser_policy::prepare(&grant, Some(&task), super::super::SUPPORTED_VERSION)
        .expect("policy")
}

fn request(workspace: &Path) -> RunRequest {
    let mut task = crate::protocol::tests::sample_task();
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    RunRequest {
        protocol: crate::protocol::PROTOCOL_VERSION,
        task,
        workspace: workspace.into(),
        work_dir: None,
        artifacts_dir: workspace.join("artifacts"),
        context: Default::default(),
        cargo_target_dir: None,
        llm_route: None,
    }
}

struct RunResult {
    outcome: RunOutcome,
    seen: Seen,
    requests: Vec<String>,
    sink: RecordingSink,
    registry: Arc<LiveSessions>,
    workspace: tempfile::TempDir,
}

/// 偽 launcher（`rewrite` があれば v7 に見せる proxy 越し）で launcher 経路の run を最後まで通す。
async fn run_through(f: &Fixture, rewrite: Option<u32>) -> RunResult {
    let p = proxy(&f.sock, f.dir.path(), rewrite);
    let registry = Arc::new(LiveSessions::default());
    let seen = Arc::new(Mutex::new(None));
    let workspace = tempfile::tempdir().expect("workspace");
    let sink = RecordingSink::default();
    let policy = prepared();
    let outcome = run_registered(
        Arc::new(ProbeAdapter {
            registry: Arc::clone(&registry),
            feeds: f.backend.feeds.clone(),
            seen: Arc::clone(&seen),
        }),
        request(workspace.path()),
        "run-live",
        RunLimits {
            wall_clock: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(60),
            kill_grace: Duration::from_secs(1),
        },
        &sink,
        LauncherTarget {
            socket: &p.sock,
            refuse_test_loopback: false,
            launcher_uid: None,
        },
        &policy,
        None,
        Some(Arc::clone(&registry)),
    )
    .await
    .expect("launcher run");
    let seen = seen.lock().expect("lock").take().expect("harness ran");
    let requests = p.seen.lock().expect("lock").clone();
    RunResult {
        outcome,
        seen,
        requests,
        sink,
        registry,
        workspace,
    }
}

fn states(sink: &RecordingSink) -> Vec<BrowserRunState> {
    sink.browsers
        .lock()
        .expect("lock")
        .iter()
        .map(|b| b.state)
        .collect()
}

/// 互換表 v9 daemon × v8 launcher: `hello` が 8 なら `live_start` を送らず（frame 接続を開かず）、
/// entry は frame 購読口なしで理由 `launcher_protocol_no_live_frames` を持つ。session・action・
/// harness・stop は従来どおり。
#[tokio::test]
async fn browser_launcher_v8_continues_without_live_view() {
    let f = fixture();
    let r = run_through(&f, Some(8)).await;
    assert!(
        matches!(r.outcome.terminal, crate::Terminal::Done { .. }),
        "{:?}",
        r.outcome.terminal
    );
    let (has_frames, key, got) = r.seen;
    assert!(!has_frames, "a v8 launcher gives no frame slot");
    assert!(key.is_some(), "the session is still registered for the run");
    assert_eq!(got, None);
    assert!(r.requests.iter().any(|t| t == "hello"));
    assert!(
        !r.requests.iter().any(|t| t == "live_start"),
        "no live_start reaches a v8 launcher: {:?}",
        r.requests
    );
    assert_eq!(f.backend.live_opened.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.backend.stopped.load(Ordering::SeqCst),
        1,
        "session stopped"
    );
    assert_eq!(
        states(&r.sink),
        vec![BrowserRunState::Running, BrowserRunState::Completed]
    );
    assert!(r.registry.is_empty(), "entry removed at the end of the run");

    // 理由と他の機能（同じ v8 launcher の session で action が通る）。
    let again = f.dir.path().join("again");
    std::fs::create_dir(&again).expect("dir");
    let p = proxy(&f.sock, &again, Some(8));
    let (runtime, attestation) =
        LauncherRuntime::start(&p.sock, "t9", "r9", policy()).expect("start");
    let relay = runtime.open_live_frames(&p.sock);
    assert_eq!(relay.as_ref().err(), Some(&LIVE_NO_FRAMES_REASON));
    let reg = LiveFrameRegistration::register(
        None,
        "logical-9",
        ("t9".into(), "r9".into()),
        attestation,
        relay,
    );
    assert_eq!(
        reg.entry().unavailable_reason(),
        Some(LIVE_NO_FRAMES_REASON)
    );
    let (_, obs) = runtime
        .action(Verb::Snapshot, ActionArgs::default())
        .expect("action on a v8 launcher");
    assert!(obs.text.is_some());
    assert!(
        !p.seen
            .lock()
            .expect("lock")
            .iter()
            .any(|t| t == "live_start")
    );
}

/// daemon は Live View を有効にする前に同じ session の control 接続で `hello` の版を確かめる:
/// v9 では `hello` の後に `live_start` が 1 回だけ行き、v8 では `hello` だけで `live_start` は無い。
#[tokio::test]
async fn browser_launcher_daemon_checks_live_protocol_before_enable() {
    for (rewrite, expect_live) in [(None, true), (Some(7), false), (Some(8), false)] {
        let f = fixture();
        let r = run_through(&f, rewrite).await;
        let hello = r.requests.iter().position(|t| t == "hello");
        let starts: Vec<_> = r
            .requests
            .iter()
            .enumerate()
            .filter(|(_, t)| *t == "live_start")
            .map(|(i, _)| i)
            .collect();
        assert!(hello.is_some(), "{:?}", r.requests);
        if expect_live {
            assert_eq!(starts.len(), 1, "{:?}", r.requests);
            assert!(hello < starts.first().copied(), "{:?}", r.requests);
            assert!(r.seen.0, "v8 gives the owner a frame slot");
        } else {
            assert!(starts.is_empty(), "{:?}", r.requests);
            assert!(!r.seen.0);
        }
        assert!(matches!(r.outcome.terminal, crate::Terminal::Done { .. }));
    }
}

fn files_containing(root: &Path, needle: &[u8]) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file()
                && std::fs::read(&path).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle))
            {
                hits.push(path);
            }
        }
    }
    hits
}

/// login 画面の入力が写った frame（MARKER）は本人向けの slot にだけ届き、run の events・
/// progress・browser 更新・live event・comment・outcome（agent への tool result）・run の作業
/// 場所の file（shim の log・events.jsonl・artifacts）・launcher の CDP 応答（action の観測）の
/// どこにも現れない。
#[tokio::test]
async fn launcher_credential_input_not_in_events_cdp_response_or_logs() {
    let f = fixture();
    let r = run_through(&f, None).await;
    let (has_frames, _, got) = &r.seen;
    assert!(has_frames);
    assert_eq!(got.as_deref(), Some(MARKER), "the owner slot got the frame");
    let marker = std::str::from_utf8(MARKER).expect("utf8");
    let recorded = r.sink.text.lock().expect("lock").clone();
    assert!(!recorded.is_empty());
    assert!(!recorded.contains(marker), "{recorded}");
    assert!(!format!("{:?}", r.outcome).contains(marker));
    assert_eq!(
        files_containing(r.workspace.path(), MARKER),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        files_containing(f.dir.path(), MARKER),
        Vec::<PathBuf>::new()
    );

    // CDP 応答（launcher の action の観測）にも frame は混ざらない。
    let (runtime, _) = LauncherRuntime::start(&f.sock, "t3", "r3", policy()).expect("start");
    let runtime = Arc::new(runtime);
    let registry = Arc::new(LiveSessions::default());
    let _reg = live_registration(
        &runtime,
        &f.sock,
        Some(Arc::clone(&registry)),
        "logical-3",
        ("t3".into(), "r3".into()),
    )
    .await
    .expect("registered");
    let session = f.backend.feeds.lock().expect("lock").len() - 1;
    push(&f, session, MARKER);
    let slot = registry_entry(&registry, "logical-3")
        .live_frames()
        .expect("frames");
    assert_eq!(next_body(&slot).await.as_deref(), Some(MARKER));
    let act = Arc::clone(&runtime);
    let (receipt, obs) =
        tokio::task::spawn_blocking(move || act.action(Verb::Snapshot, ActionArgs::default()))
            .await
            .expect("join")
            .expect("action");
    let shown = format!("{receipt:?} {obs:?}");
    assert!(!shown.contains(marker), "{shown}");
    assert!(!format!("{:?}", slot).contains(marker));
    let stop = Arc::clone(&runtime);
    let _ = tokio::task::spawn_blocking(move || stop.stop()).await;
}
