//! task-api 層の層横断 Live View 試験（ADR 2026-10-10-browser-launcher-live-view-frames D1〜D3・D7）。
//!
//! 経路: 偽 Chrome（CDP pipe）→ launcher の `LiveTap`・`ScreencastFeed` → 実の `LauncherServer`
//! （偽 `SessionBackend`）→ Unix socket → task-worker の daemon relay（`DaemonLauncherSession`・
//! `open_frame_relay`）→ `LiveFrameRegistration`（`LiveSessions` の entry）→ task-api の
//! `POST /api/v1/tasks/{id}/browser/live/{run}/{session}/frames`。userns・実 browser は使わない。
//! 待ちは出来事待ち（stream の次の chunk）と長い保険の期限だけ。
//!
//! 偽部品は `crates/task-worker/tests/browser_live_cross.rs` の `FakeCdp`・`FakeLauncher` から、この
//! 試験に要る分だけを写した（v7 proxy は使わない）。Unix socket の SUN_LEN のため `TMPDIR=/tmp` で回す。

mod common;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::body::BodyDataStream;
use base64::Engine as _;
use common::*;
use futures_util::StreamExt;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::BrowserApiConfig;
use task_core::browser_isolation::{LiveSessionEntry as _, LiveSessions};
use task_core::browser_live::LiveEvent;
use task_core::{RunIndexRole, RunIndexStatus, RunRow, Status, Task, TaskKind, TaskStore};
use task_worker::browser::launcher_cross_test_support::DaemonLauncherSession;
use task_worker::browser_cdp_sink::CdpController;
use task_worker::browser_launcher::live::{LiveTap, ScreencastFeed};
use task_worker::browser_launcher::protocol::{
    AuthenticateArgs, AuthenticationStatus, LoginObservation, LoginResult,
};
use task_worker::browser_launcher::{
    ActionArgs, BackendSession, ErrorCode, Launched, LauncherLimits, LauncherServer, LiveFeed,
    Observation, Registry, ServerConfig, ServerHandle, SessionBackend, SessionFacts, SessionPolicy,
    SessionState, StartRequest, Verb,
};
use task_worker::browser_live::{CollectingSink, LiveEmitter, LiveFrameRegistration};
use time::OffsetDateTime;
use tower::ServiceExt;

/// 長い保険（出来事待ちが来なかったときだけ効く）。
const INSURANCE: Duration = Duration::from_secs(30);
/// 偽 launcher の UID（daemon と別 UID に見せる。`verify_isolation` を通すため）。
const LAUNCHER_UID: u32 = 4_000_001;
const SUBUID: u32 = 5_000_000;
/// 偽 Chrome が答える頁 target（ScreencastFeed が選ぶ）。
const PAGE_TARGET: &str = "P1";
/// 偽 Chrome が screencast の attach に返す CDP session。
const LIVE_CDP_SESSION: &str = "LIVE";
/// 本人の Live View の browser session id（`LiveSessions` の鍵）。
const BROWSER: &str = "browser-1";
const RUN: &str = "r1";
const OWNER: &str = "owner";

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn pipe() -> (File, File) {
    let (r, w) = std::io::pipe().expect("pipe");
    (File::from(OwnedFd::from(r)), File::from(OwnedFd::from(w)))
}

// ---- 偽 Chrome（CDP pipe）----

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
struct FakeCdp {
    tap: Arc<LiveTap>,
    controller: Arc<Mutex<CdpController>>,
    methods: Arc<Mutex<Vec<String>>>,
    chrome: Arc<Mutex<ChromeState>>,
}

impl Default for FakeCdp {
    fn default() -> Self {
        let (chrome_out_r, chrome_out_w) = pipe();
        let (chrome_in_r, chrome_in_w) = pipe();
        let (ctrl_read, tap) = LiveTap::interpose(chrome_out_r).expect("interpose");
        let controller = Arc::new(Mutex::new(CdpController::new(chrome_in_w, ctrl_read)));
        let methods = Arc::default();
        let chrome = Arc::new(Mutex::new(ChromeState {
            out: chrome_out_w,
            screencasting: false,
            awaiting_ack: false,
            queue: VecDeque::new(),
            ack: 0,
        }));
        let (methods_t, chrome_t) = (Arc::clone(&methods), Arc::clone(&chrome));
        std::thread::spawn(move || fake_chrome(chrome_in_r, chrome_t, methods_t));
        Self {
            tap,
            controller,
            methods,
            chrome,
        }
    }
}

impl FakeCdp {
    /// 次の screencast frame（画素の代わりの bytes）を積む。screencast 中なら ack に合わせて流れる。
    fn push_frame(&self, body: &[u8]) {
        let mut st = self.chrome.lock().expect("chrome");
        st.queue.push_back(body.to_vec());
        st.pump();
    }

    fn methods(&self) -> Vec<String> {
        self.methods.lock().expect("methods").clone()
    }
}

fn fake_chrome(mut input: File, chrome: Arc<Mutex<ChromeState>>, methods: Arc<Mutex<Vec<String>>>) {
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
            methods.lock().expect("methods").push(method.clone());
            let mut reply = json!({"id":v["id"].clone(),"result":{}});
            if let Some(s) = v["sessionId"].as_str() {
                reply["sessionId"] = s.into();
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

#[derive(Default)]
struct SessionProbe {
    cdp: FakeCdp,
    auth_begun: AtomicUsize,
    logins: AtomicUsize,
}

#[derive(Default)]
struct FakeBackend {
    sessions: Mutex<Vec<Arc<SessionProbe>>>,
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
    }
}

/// 実の `LauncherServer`（daemon の UID を許可）と偽 backend。
struct FakeLauncher {
    dir: tempfile::TempDir,
    sock: PathBuf,
    backend: Arc<FakeBackend>,
    _handle: ServerHandle,
}

impl FakeLauncher {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("l.sock");
        // daemon（この process）の UID: 自分で作った dir の持ち主。
        let daemon_uid = std::fs::metadata(dir.path()).expect("tempdir meta").uid();
        let registry =
            Registry::open(dir.path().join("state"), "inst-api-cross").expect("registry");
        let backend = Arc::new(FakeBackend::default());
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![daemon_uid],
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
            _handle: handle,
        }
    }

    fn session(&self, index: usize) -> Arc<SessionProbe> {
        Arc::clone(&self.backend.sessions.lock().expect("sessions")[index])
    }
}

fn policy() -> SessionPolicy {
    SessionPolicy {
        allowed_domains: vec!["example.com".into()],
        allowed_actions: vec![Verb::Open, Verb::Click, Verb::Snapshot],
        lease_seconds: 600,
    }
}

/// 同期の daemon 呼び出しを blocking pool で回す（launcher client は同期 I/O）。
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.expect("join")
}

/// daemon 側の credential login（auth_begin → injection.sock の代わりの socketpair → authenticate）。
async fn daemon_credential_login(
    session: &Arc<DaemonLauncherSession>,
    auth_section_id: &str,
) -> (AuthenticationStatus, LoginResult) {
    let (s, id) = (Arc::clone(session), auth_section_id.to_owned());
    blocking(move || {
        let target = s.auth_begin(&id).expect("auth_begin");
        assert_eq!(target, PAGE_TARGET);
        let (broker, _credentiald) = UnixStream::pair().expect("socketpair");
        let args = AuthenticateArgs {
            session_id: String::new(),
            lease_id: String::new(),
            auth_section_id: id.clone(),
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
        };
        s.authenticate(args, OwnedFd::from(broker))
            .expect("authenticate")
    })
    .await
}

// ---- tracing の捕捉（全 thread。daemon relay・launcher の thread も含む）----

/// span・event の全 field を TRACE まで文字列で書き溜める subscriber（tracing-subscriber の fmt と
/// 同じく field を Debug で書く。依存を足さないために手で組む）。
struct Capture {
    buf: Arc<Mutex<Vec<u8>>>,
    next_id: AtomicU64,
}

struct FieldWriter<'a>(&'a mut Vec<u8>);

impl tracing::field::Visit for FieldWriter<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={:?}", field.name(), value);
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let _ = write!(self.0, " {}={}", field.name(), value);
    }
}

impl Capture {
    fn write_line(&self, meta: &tracing::Metadata<'_>, f: impl FnOnce(&mut FieldWriter<'_>)) {
        let mut buf = self.buf.lock().expect("capture");
        let _ = write!(buf, "{} {}:", meta.level(), meta.target());
        f(&mut FieldWriter(&mut buf));
        buf.push(b'\n');
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        self.write_line(attrs.metadata(), |w| attrs.record(w));
        tracing::span::Id::from_u64(self.next_id.fetch_add(1, Ordering::SeqCst))
    }
    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut buf = self.buf.lock().expect("capture");
        values.record(&mut FieldWriter(&mut buf));
        buf.push(b'\n');
    }
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        self.write_line(event.metadata(), |w| event.record(w));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// process 全体の tracing を TRACE で捕捉する buffer（最初の呼び出しで global subscriber を入れる）。
fn log_capture() -> Arc<Mutex<Vec<u8>>> {
    static CAPTURE: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    Arc::clone(CAPTURE.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Capture {
            buf: Arc::clone(&buf),
            next_id: AtomicU64::new(1),
        };
        tracing::subscriber::set_global_default(subscriber).expect("global subscriber");
        buf
    }))
}

// ---- API 側の helper ----

fn sign(
    key: &ring::signature::Ed25519KeyPair,
    task: &str,
    run: &str,
    browser: &str,
    owner: &str,
    is_owner: bool,
) -> Value {
    sign_with_origin(key, task, run, browser, owner, is_owner, true)
}

fn sign_with_origin(
    key: &ring::signature::Ed25519KeyPair,
    task: &str,
    run: &str,
    browser: &str,
    owner: &str,
    is_owner: bool,
    origin_ok: bool,
) -> Value {
    let payload = json!({"task_id":task,"run_id":run,"browser_session_id":browser,
        "owner_session_id":owner,"owner_session":is_owner,"origin_ok":origin_ok,
        "expires_at":OffsetDateTime::now_utc().unix_timestamp()+30})
    .to_string();
    let signature: String = key
        .sign(payload.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({"payload":payload,"signature":signature})
}

fn path(task: &str, run: &str, session: &str, method: &str) -> String {
    format!("/api/v1/tasks/{task}/browser/live/{run}/{session}/{method}")
}

fn start_run(env: &TestEnv, task: &str, run: &str) {
    env.store
        .run_index_start(RunRow {
            run_id: run.into(),
            task_id: task.into(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-10-10T00:00:00Z".into(),
            finished_at: None,
        })
        .expect("run index");
}

/// 一時 DB・偽 launcher・daemon の launcher session・`LiveSessions` 登録・task-api の router。
struct Harness {
    env: TestEnv,
    key: ring::signature::Ed25519KeyPair,
    task: Task,
    id: String,
    app: Router,
    launcher: FakeLauncher,
    daemon: Arc<DaemonLauncherSession>,
    reg: Option<LiveFrameRegistration>,
}

impl Harness {
    async fn new() -> Self {
        let env = admin_env();
        let key = {
            let rng = ring::rand::SystemRandom::new();
            let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
            ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("key")
        };
        let task = new_task(TaskKind::Execute, Status::Ready);
        env.seed(&task);
        env.store
            .acquire_lease(task.id, RUN, Duration::from_secs(600))
            .expect("lease");
        let id = task.id.to_string();
        start_run(&env, &id, RUN);
        std::fs::create_dir_all(env.workspace(&task).join("artifacts")).expect("artifacts dir");

        let launcher = FakeLauncher::new();
        let (sock, t) = (launcher.sock.clone(), id.clone());
        let (daemon, attestation) =
            blocking(move || DaemonLauncherSession::start(&sock, &t, RUN, policy()))
                .await
                .expect("launcher session");
        let daemon = Arc::new(daemon);
        let (d, sock) = (Arc::clone(&daemon), launcher.sock.clone());
        let relay = blocking(move || d.open_live_frames(&sock)).await;
        assert!(relay.is_ok(), "v8 frame relay");
        let sessions = Arc::new(LiveSessions::default());
        let reg = LiveFrameRegistration::register(
            Some(Arc::clone(&sessions)),
            BROWSER,
            (id.clone(), RUN.into()),
            attestation,
            relay,
        );
        assert!(reg.entry().live_frames().is_some());
        let app = task_api::router(
            env.state
                .clone()
                .with_browser(BrowserApiConfig {
                    attestation_public_key: Some(key.public_key().as_ref().to_vec()),
                    broker: None,
                })
                .with_live_sessions(sessions),
        );
        Self {
            env,
            key,
            task,
            id,
            app,
            launcher,
            daemon,
            reg: Some(reg),
        }
    }

    /// 本人の grant を取り、frames stream を開く。返り値は (relay body, stream)。
    async fn open_owner_stream(&self) -> (Value, BodyDataStream) {
        let proof = sign(&self.key, &self.id, RUN, BROWSER, OWNER, true);
        let grant = send(
            &self.app,
            post_admin(
                &path(&self.id, RUN, BROWSER, "grant"),
                &json!({"assertion":proof}),
            ),
        )
        .await;
        assert_eq!(grant.status, 200, "{}", grant.text());
        assert_eq!(grant.json()["frames_available"], true);
        let grant_id = grant.json()["grant_id"]
            .as_str()
            .expect("grant id")
            .to_owned();
        let relay = json!({"assertion":proof,"grant_id":grant_id});
        let resp = self
            .app
            .clone()
            .oneshot(post_admin(&path(&self.id, RUN, BROWSER, "frames"), &relay))
            .await
            .expect("infallible");
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers().get("cache-control").expect("cc"), "no-store");
        (relay, resp.into_body().into_data_stream())
    }

    async fn stop(&mut self) {
        drop(self.reg.take());
        let d = Arc::clone(&self.daemon);
        blocking(move || d.stop()).await.expect("stop");
    }
}

/// 出来事待ち: stream の次の frame（4 byte BE 長 ＋ 本体）。
async fn next_frame(stream: &mut BodyDataStream) -> Vec<u8> {
    let chunk = tokio::time::timeout(INSURANCE, stream.next())
        .await
        .expect("frame event")
        .expect("stream open")
        .expect("chunk");
    let (len, body) = chunk.split_at(4);
    let len = u32::from_be_bytes(len.try_into().expect("len")) as usize;
    assert_eq!(len, body.len());
    body.to_vec()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// marker と frame bytes を、生と base64 の両方で探す語の一覧。
fn needles(marker: &[u8], frame: &[u8]) -> Vec<(String, Vec<u8>)> {
    vec![
        ("marker".into(), marker.to_vec()),
        ("marker-b64".into(), b64(marker).into_bytes()),
        ("frame".into(), frame.to_vec()),
        ("frame-b64".into(), b64(frame).into_bytes()),
    ]
}

/// 一時 DB の全表の全行（events 表を含む）で needle を含む表の名前。
fn db_tables_containing(db: &Path, needle: &[u8]) -> (Vec<String>, Vec<String>) {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open db");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .expect("tables")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("names");
    let mut hits = Vec::new();
    for table in &tables {
        let mut stmt = conn
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .expect("select");
        let cols = stmt.column_count();
        let mut rows = stmt.query([]).expect("rows");
        while let Some(row) = rows.next().expect("row") {
            let found = (0..cols).any(|i| match row.get_ref(i).expect("value") {
                rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => {
                    contains(b, needle)
                }
                _ => false,
            });
            if found {
                hits.push(table.clone());
                break;
            }
        }
    }
    (tables, hits)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        let meta = std::fs::symlink_metadata(&path).expect("meta");
        if meta.is_dir() {
            collect_files(&path, out);
        } else if meta.is_file() {
            out.push(path);
        }
    }
}

/// 一時 DB（本体・-wal・-shm）・全表・tracing の捕捉・artifacts dir・tempdir 全体に needle が無い。
fn assert_not_persisted(h: &Harness, needles: &[(String, Vec<u8>)], log: &[u8]) {
    let db = &h.env.db_path;
    let mut db_files = vec![db.clone()];
    for suffix in ["-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", db.display()));
        // store の接続が開いている間は WAL mode の -wal・-shm がある（走査が空振りしない）。
        assert!(p.is_file(), "{p:?} exists");
        db_files.push(p);
    }
    assert!(db.is_file(), "temporary DB exists");
    let artifacts = h.env.workspace(&h.task).join("artifacts");
    assert!(artifacts.is_dir(), "artifacts dir exists");
    let mut files = Vec::new();
    collect_files(h.env.dir.path(), &mut files);
    collect_files(h.launcher.dir.path(), &mut files);
    assert!(
        db_files.iter().all(|p| files.contains(p)),
        "DB files are scanned"
    );
    for (name, needle) in needles {
        let (tables, hits) = db_tables_containing(db, needle);
        assert!(
            tables.iter().any(|t| t == "events"),
            "events table: {tables:?}"
        );
        assert!(hits.is_empty(), "{name} persisted in tables {hits:?}");
        for f in &files {
            let bytes = std::fs::read(f).expect("file");
            assert!(!contains(&bytes, needle), "{name} persisted in {f:?}");
        }
        assert!(!contains(log, needle), "{name} in tracing log");
    }
}

/// D2: 秘密 marker 入りの frame を API stream で本人に届けた後、一時 DB 本体・-wal・-shm、events
/// 表（全表）、tracing の捕捉、artifacts dir、tempdir 全体に marker と frame bytes（生・base64）が無い。
#[tokio::test]
async fn browser_live_frame_no_persist_db_events_logs_artifacts_tempdir() {
    let log = log_capture();
    tracing::info!(probe = "api-cross capture probe", "capture");
    let mut h = Harness::new().await;
    let probe = h.launcher.session(0);
    let (relay, mut stream) = h.open_owner_stream().await;

    let marker = b"API_PRIVATE_FRAME_MARKER_5c2d";
    let mut frame = vec![0xff, 0xd8, 0xff, 0xe0];
    frame.extend_from_slice(marker);
    frame.extend_from_slice(&[0x13, 0x37, 0xff, 0xd9]);
    probe.cdp.push_frame(&frame);
    assert_eq!(next_frame(&mut stream).await, frame, "owner got the frame");

    // 本人の /read（永続 live event）にも frame は無い。
    let read = send(
        &h.app,
        post_admin(&path(&h.id, RUN, BROWSER, "read"), &relay),
    )
    .await;
    assert_eq!(read.status, 200, "{}", read.text());
    // worker の event 経路に frame の種類は無い（受理しない）。
    assert_problem(
        &send(
            &h.app,
            post_admin(
                &path(&h.id, RUN, BROWSER, "events"),
                &json!({"kind":"frame","bytes":b64(&frame)}),
            ),
        )
        .await,
        422,
        "browser_body_invalid",
    );
    let status = send(
        &h.app,
        post_admin(
            &path(&h.id, RUN, BROWSER, "events"),
            &json!({"kind":"status","state":"running"}),
        ),
    )
    .await;
    assert_eq!(status.status, 200, "{}", status.text());
    let all = needles(marker, &frame);
    for (name, needle) in &all {
        assert!(!contains(&read.body, needle), "{name} in /read");
    }

    drop(stream);
    h.stop().await;
    let log = log.lock().expect("log").clone();
    assert!(
        contains(&log, b"api-cross capture probe"),
        "tracing capture works"
    );
    assert_not_persisted(&h, &all, &log);
}

/// D3・D7: launcher の credential login（auth section）中も本人 owner の stream には frame が届く。
/// 別 owner・owner でない session・別 run・別 session・別 task の frames 要求は拒否され、agent 側の
/// `/read`・events（一時 DB の全表）・LiveEmitter・tool result に frame は無い。
#[tokio::test]
async fn browser_live_frame_auth_section_owner_only_api_stream() {
    let mut h = Harness::new().await;
    let probe = h.launcher.session(0);
    let other = new_task(TaskKind::Execute, Status::Ready);
    h.env.seed(&other);
    let other_id = other.id.to_string();
    start_run(&h.env, &other_id, "r-other");
    let (relay, mut stream) = h.open_owner_stream().await;
    let grant_id = relay["grant_id"].clone();

    // worker の auth section: 永続 event は止まる。
    let emitter = LiveEmitter::new(CollectingSink::default());
    let auth = emitter.auth_section();
    let marker = b"AUTH_SECTION_FRAME_MARKER_91af";
    assert!(!emitter.emit(&LiveEvent::Frame {
        bytes: marker.to_vec()
    }));
    let (status, login) = daemon_credential_login(&h.daemon, "auth-api-section").await;
    assert_eq!(status, AuthenticationStatus::Success);
    assert_eq!(login.observation, LoginObservation::Resumed);
    assert_eq!(probe.auth_begun.load(Ordering::SeqCst), 1);
    assert_eq!(probe.logins.load(Ordering::SeqCst), 1);

    // 本人には届く。
    probe.cdp.push_frame(marker);
    assert_eq!(next_frame(&mut stream).await, marker);

    // 別 owner・owner でない session・別 run・別 session・別 task は拒否。
    let frames = |task: &str, run: &str, session: &str| path(task, run, session, "frames");
    let cases = [
        (
            "other owner",
            frames(&h.id, RUN, BROWSER),
            sign(&h.key, &h.id, RUN, BROWSER, "someone-else", true),
            403,
            "not_owner_session",
        ),
        (
            "not owner session",
            frames(&h.id, RUN, BROWSER),
            sign(&h.key, &h.id, RUN, BROWSER, OWNER, false),
            403,
            "not_owner_session",
        ),
        (
            "other run",
            frames(&h.id, "r2", BROWSER),
            sign(&h.key, &h.id, "r2", BROWSER, OWNER, true),
            403,
            "other_task",
        ),
        (
            "other session",
            frames(&h.id, RUN, "browser-2"),
            sign(&h.key, &h.id, RUN, "browser-2", OWNER, true),
            403,
            "other_task",
        ),
        (
            "other task",
            frames(&other_id, "r-other", BROWSER),
            sign(&h.key, &other_id, "r-other", BROWSER, OWNER, true),
            403,
            "other_task",
        ),
    ];
    for (name, p, assertion, code, problem) in cases {
        let resp = send(
            &h.app,
            post_admin(&p, &json!({"assertion":assertion,"grant_id":grant_id})),
        )
        .await;
        assert_eq!(resp.status.as_u16(), code, "{name}: {}", resp.text());
        assert_problem(&resp, code, problem);
        assert!(!contains(&resp.body, marker), "{name} got the frame");
    }
    // owner でない session は grant も取れない。
    assert_problem(
        &send(
            &h.app,
            post_admin(
                &path(&h.id, RUN, BROWSER, "grant"),
                &json!({"assertion":sign(&h.key, &h.id, RUN, BROWSER, OWNER, false)}),
            ),
        )
        .await,
        403,
        "not_owner_session",
    );
    // 別 session の frames は、本人の grant と正しい署名でも entry が無ければ開けない。
    let no_entry = sign(&h.key, &h.id, RUN, "browser-9", OWNER, true);
    let grant9 = send(
        &h.app,
        post_admin(
            &path(&h.id, RUN, "browser-9", "grant"),
            &json!({"assertion":no_entry}),
        ),
    )
    .await;
    assert_eq!(grant9.status, 200, "{}", grant9.text());
    assert_eq!(grant9.json()["frames_available"], false);
    assert_problem(
        &send(
            &h.app,
            post_admin(
                &frames(&h.id, RUN, "browser-9"),
                &json!({"assertion":no_entry,"grant_id":grant9.json()["grant_id"]}),
            ),
        )
        .await,
        403,
        "live_view_disabled",
    );

    // 拒否の後も本人の stream は続く。
    let marker2 = b"AUTH_SECTION_FRAME_MARKER_SECOND";
    probe.cdp.push_frame(marker2);
    assert_eq!(next_frame(&mut stream).await, marker2);

    // agent 側: /read（永続 live event）・events（全表）・LiveEmitter・tool result に frame は無い。
    let read = send(
        &h.app,
        post_admin(&path(&h.id, RUN, BROWSER, "read"), &relay),
    )
    .await;
    assert_eq!(read.status, 200, "{}", read.text());
    let d = Arc::clone(&h.daemon);
    let (_, tool) = blocking(move || d.action(Verb::Snapshot, ActionArgs::default()))
        .await
        .expect("action");
    let tool_text = tool.text.unwrap_or_default();
    for m in [&marker[..], &marker2[..]] {
        for (name, needle) in needles(m, m) {
            assert!(!contains(&read.body, &needle), "{name} in /read");
            assert!(
                !contains(tool_text.as_bytes(), &needle),
                "{name} in tool result"
            );
            let (tables, hits) = db_tables_containing(&h.env.db_path, &needle);
            assert!(tables.iter().any(|t| t == "events"));
            assert!(hits.is_empty(), "{name} in tables {hits:?}");
        }
    }
    drop(auth);
    assert!(
        emitter.sink().events().is_empty(),
        "auth section persisted no event"
    );
    assert!(
        !probe.cdp.methods().iter().any(|m| m.starts_with("Input.")),
        "no input reached the browser"
    );

    drop(stream);
    h.stop().await;
}

/// ADR-0080 D6 / ADR-0100 D2: Live View grant は本人の task/run/session に限り、
/// frame relay は読み取り専用。launcher run への input/takeover 要求は API で拒否され、
/// CDP の input command へ到達しない。
#[tokio::test]
async fn browser_live_view_existing_takeover_policy() {
    let mut h = Harness::new().await;
    let proof = sign(&h.key, &h.id, RUN, BROWSER, OWNER, true);
    let grant = send(
        &h.app,
        post_admin(
            &path(&h.id, RUN, BROWSER, "grant"),
            &json!({"assertion":proof}),
        ),
    )
    .await;
    assert_eq!(grant.status, 200, "{}", grant.text());
    assert_eq!(grant.json()["frames_available"], true);

    // D6/D2 の既存 owner・Origin 判定は grant の入口で fail closed。
    for (name, assertion, problem) in [
        (
            "not owner session",
            sign(&h.key, &h.id, RUN, BROWSER, OWNER, false),
            "not_owner_session",
        ),
        (
            "origin mismatch",
            sign_with_origin(&h.key, &h.id, RUN, BROWSER, OWNER, true, false),
            "origin_mismatch",
        ),
    ] {
        let denied = send(
            &h.app,
            post_admin(
                &path(&h.id, RUN, BROWSER, "grant"),
                &json!({"assertion":assertion}),
            ),
        )
        .await;
        assert_problem(&denied, 403, problem);
        assert!(
            !contains(&denied.body, b"grant_id"),
            "{name} received a grant"
        );
    }

    // task-api は read/check/events/frames のみを公開し、input/takeover route はない。
    // 既存の grant を添えた要求も 404 で拒否し、launcher/CDP へ転送しない。
    let proof = sign(&h.key, &h.id, RUN, BROWSER, OWNER, true);
    let relay = json!({"assertion":proof,"grant_id":grant.json()["grant_id"]});
    for operation in ["input", "takeover"] {
        let denied = send(
            &h.app,
            post_admin(&path(&h.id, RUN, BROWSER, operation), &relay),
        )
        .await;
        assert_eq!(denied.status, 404, "{operation}: {}", denied.text());
    }
    let methods = h.launcher.session(0).cdp.methods();
    assert!(
        !methods.iter().any(|method| method.starts_with("Input.")),
        "input reached CDP: {methods:?}"
    );
    h.stop().await;
}
