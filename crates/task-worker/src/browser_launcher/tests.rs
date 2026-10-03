//! ADR-0116 D7 の単体試験（環境不要）。起動の中身は偽の backend（`sleep` を独自の process group で
//! 起こす）で置き換え、protocol・peer 検査・lease・回収を実 socket と実 process で確かめる。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::client::{ClientError, LauncherClient};
use super::protocol::*;
use super::registry::{Registry, SessionRecord};
use super::server::*;
use crate::browser_runtime::{process_starttime, same_process_alive};

fn uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

#[derive(Default)]
struct FakeBackend {
    /// 起動した (pid, starttime)。
    launched: Mutex<Vec<(i32, u64)>>,
    /// true なら stop で何もしない（registry 側の process group 停止だけで回収されることを見る）。
    leaky_stop: bool,
    isolation_ok: bool,
}

struct FakeSession {
    child: Option<Child>,
    leaky: bool,
    isolation_ok: bool,
}

impl SessionBackend for FakeBackend {
    fn start(&self, _req: &StartRequest) -> Result<Launched, ErrorCode> {
        let child = Command::new("sleep")
            .arg("600")
            .process_group(0)
            .stdin(Stdio::null())
            .spawn()
            .map_err(|_| ErrorCode::LaunchFailed)?;
        let pid = child.id() as i32;
        let starttime = process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
        self.launched.lock().expect("lock").push((pid, starttime));
        Ok(Launched {
            session: Box::new(FakeSession {
                child: Some(child),
                leaky: self.leaky_stop,
                isolation_ok: self.isolation_ok,
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

impl BackendSession for FakeSession {
    fn action(&mut self, verb: Verb, _args: &ActionArgs) -> Result<Observation, ErrorCode> {
        Ok(Observation {
            text: Some(format!("{verb:?}")),
            artifact: None,
        })
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        (
            SessionState::Running,
            SessionFacts {
                host_uid: 4242,
                ..SessionFacts::default()
            },
        )
    }
    fn isolation_ok(&mut self) -> bool {
        self.isolation_ok
    }
    fn stop(mut self: Box<Self>) {
        if self.leaky {
            // 子は持ったまま捨てる（kill も wait もしない）。
            std::mem::forget(self.child.take());
            return;
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    sock: PathBuf,
    state_dir: PathBuf,
    backend: Arc<FakeBackend>,
    handle: ServerHandle,
}

fn fixture_with(allowed: Vec<u32>, limits: LauncherLimits, backend: FakeBackend) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("l.sock");
    let state_dir = dir.path().join("state");
    let registry = Registry::open(&state_dir, "inst-now").expect("registry");
    let backend = Arc::new(backend);
    let server = LauncherServer::bind(
        &sock,
        ServerConfig {
            allowed_uids: allowed,
            limits,
        },
        backend.clone(),
        registry,
    )
    .expect("bind");
    let handle = server.spawn().expect("spawn");
    Fixture {
        _dir: dir,
        sock,
        state_dir,
        backend,
        handle,
    }
}

fn fixture_hooked(hook: TestHookFn) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("l.sock");
    let state_dir = dir.path().join("state");
    let registry = Registry::open(&state_dir, "inst-now").expect("registry");
    let backend = Arc::new(ok_backend());
    let mut server = LauncherServer::bind(
        &sock,
        ServerConfig {
            allowed_uids: vec![uid()],
            limits: LauncherLimits::default(),
        },
        backend.clone(),
        registry,
    )
    .expect("bind");
    server.set_test_hook(hook);
    let handle = server.spawn().expect("spawn");
    Fixture {
        _dir: dir,
        sock,
        state_dir,
        backend,
        handle,
    }
}

fn ok_backend() -> FakeBackend {
    FakeBackend {
        isolation_ok: true,
        ..FakeBackend::default()
    }
}

fn fixture() -> Fixture {
    fixture_with(vec![uid()], LauncherLimits::default(), ok_backend())
}

fn connect(sock: &Path) -> LauncherClient {
    LauncherClient::connect(sock, Duration::from_secs(10)).expect("connect")
}

fn policy(lease_seconds: u64) -> SessionPolicy {
    SessionPolicy {
        allowed_domains: vec!["example.com".into()],
        allowed_actions: vec![Verb::Open, Verb::Snapshot, Verb::Close],
        lease_seconds,
    }
}

fn wait_dead(pid: i32, st: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !same_process_alive(pid, st) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn records(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .filter(|e| {
            e.as_ref()
                .map(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
                .unwrap_or(false)
        })
        .count()
}

/// 生の bytes を 1 frame 送り、応答 frame（あれば）を返す。閉じられたら None。
fn raw_exchange(sock: &Path, frame: &[u8]) -> Option<Vec<u8>> {
    let mut s = UnixStream::connect(sock).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let _ = s.write_all(frame);
    read_frame(&mut s, DEFAULT_MAX_FRAME).ok()
}

fn framed(body: &[u8]) -> Vec<u8> {
    let mut v = (body.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(body);
    v
}

// ---- protocol ----

#[test]
fn protocol_roundtrip_and_unknown_fields_rejected() {
    let req = Request::Observe {
        session_id: "s1".into(),
        lease_id: "l1".into(),
    };
    let body = serde_json::to_vec(&req).expect("json");
    assert_eq!(decode_request(&body), Ok(req));

    for bad in [
        r#"{"type":"observe","session_id":"s","lease_id":"l","extra":1}"#,
        r#"{"type":"spawn","argv":["/bin/sh"]}"#,
        r#"{"type":"start_session","task_id":"t","run_id":"r","lease_id":"l","policy":{"allowed_domains":[],"allowed_actions":[],"lease_seconds":5,"bind":"/"}}"#,
        r#"{"type":"action","session_id":"s","lease_id":"l","verb":"exec"}"#,
        r#"{"type":"action","session_id":"s","lease_id":"l","verb":"open","args":{"url":"https://example.com","env":"X=1"}}"#,
        r#"{"type":"stop","session_id":"../etc","lease_id":"l"}"#,
        "not json",
    ] {
        assert_eq!(
            decode_request(bad.as_bytes()),
            Err(ErrorCode::BadRequest),
            "{bad}"
        );
    }
}

#[test]
fn protocol_oversized_values_and_frames_rejected() {
    let mut p = policy(5);
    p.allowed_domains = (0..MAX_DOMAINS + 1)
        .map(|i| format!("d{i}.example"))
        .collect();
    assert_eq!(p.validate(), Err(ErrorCode::Limit));
    let mut p = policy(5);
    p.lease_seconds = MAX_LEASE_SECONDS + 1;
    assert_eq!(p.validate(), Err(ErrorCode::Limit));
    let long = Request::Action {
        session_id: "s".into(),
        lease_id: "l".into(),
        verb: Verb::Open,
        args: ActionArgs {
            url: Some(format!("https://example.com/{}", "a".repeat(MAX_STR))),
            ..ActionArgs::default()
        },
    };
    assert_eq!(long.validate(), Err(ErrorCode::Limit));

    // 上限を超える長さの frame は本体を読まずに拒否する。
    let mut cur = std::io::Cursor::new(((DEFAULT_MAX_FRAME + 1) as u32).to_be_bytes().to_vec());
    assert!(matches!(
        read_frame(&mut cur, DEFAULT_MAX_FRAME),
        Err(FrameError::TooLarge(_))
    ));
    let mut sink = Vec::new();
    assert!(matches!(
        write_frame(
            &mut sink,
            &vec![0u8; DEFAULT_MAX_FRAME + 1],
            DEFAULT_MAX_FRAME
        ),
        Err(FrameError::TooLarge(_))
    ));
}

// ---- server ----

#[test]
fn foreign_uid_is_closed_without_response() {
    let f = fixture_with(
        vec![uid().wrapping_add(1)],
        LauncherLimits::default(),
        ok_backend(),
    );
    let mut c = connect(&f.sock);
    let r = c.start_session("t1", "r1", "l1", policy(60));
    assert!(
        matches!(r, Err(ClientError::Closed) | Err(ClientError::Io(_))),
        "{r:?}"
    );
    assert!(f.backend.launched.lock().expect("lock").is_empty());
}

#[test]
fn unknown_field_over_socket_gets_bad_request_and_close() {
    let f = fixture();
    let body = br#"{"type":"observe","session_id":"s","lease_id":"l","fd":3}"#;
    let mut s = UnixStream::connect(&f.sock).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    s.write_all(&framed(body)).expect("write");
    let resp: Response =
        serde_json::from_slice(&read_frame(&mut s, DEFAULT_MAX_FRAME).expect("frame"))
            .expect("json");
    assert_eq!(
        resp,
        Response::Error {
            code: ErrorCode::BadRequest
        }
    );
    // 接続は閉じられている。
    let mut buf = [0u8; 1];
    assert_eq!(s.read(&mut buf).expect("read"), 0);
}

#[test]
fn oversized_frame_is_closed_without_reading() {
    let f = fixture();
    let header = ((DEFAULT_MAX_FRAME + 1) as u32).to_be_bytes();
    assert!(raw_exchange(&f.sock, &header).is_none());
    assert!(f.backend.launched.lock().expect("lock").is_empty());
}

#[test]
fn session_lifecycle_and_policy_recheck() {
    let f = fixture();
    let mut c = connect(&f.sock);
    let s = c
        .start_session("t1", "r1", "lease1", policy(60))
        .expect("start");
    assert_eq!(s.instance_id, "inst-now");
    assert_eq!(records(&f.state_dir), 1);
    assert_eq!(f.handle.session_count(), 1);
    // protocol v3: 束縛は検査した runtime process と、その 6 つの namespace の inode を載せる。
    let binding = s.receipt.binding.clone().expect("v3 binding");
    assert!(crate::browser_runtime::same_process_alive(
        binding.pid,
        binding.starttime
    ));
    assert_eq!(
        binding.ns_inodes.keys().copied().collect::<Vec<_>>(),
        task_core::browser_isolation::REQUIRED_NAMESPACES.to_vec()
    );

    let (rcpt, obs) = c
        .action(
            &s.session_id,
            "lease1",
            Verb::Open,
            ActionArgs {
                url: Some("https://www.example.com/a".into()),
                ..ActionArgs::default()
            },
        )
        .expect("open");
    assert_eq!(rcpt.outcome, Outcome::Ok);
    assert!(rcpt.isolation_ok);
    assert_eq!(obs.text.as_deref(), Some("Open"));

    // policy に無い domain・verb は launcher 側で拒否する。
    let bad_url = c.action(
        &s.session_id,
        "lease1",
        Verb::Open,
        ActionArgs {
            url: Some("https://evil.test/".into()),
            ..ActionArgs::default()
        },
    );
    assert!(matches!(
        bad_url,
        Err(ClientError::Remote(ErrorCode::Unauthorized))
    ));
    let bad_verb = c.action(
        &s.session_id,
        "lease1",
        Verb::Download,
        ActionArgs::default(),
    );
    assert!(matches!(
        bad_verb,
        Err(ClientError::Remote(ErrorCode::Unauthorized))
    ));

    let (state, facts) = c.observe(&s.session_id, "lease1").expect("observe");
    assert_eq!(state, SessionState::Running);
    assert_eq!(facts.host_uid, 4242);

    let (pid, st) = f.backend.launched.lock().expect("lock")[0];
    let r = c.stop(&s.session_id, "lease1").expect("stop");
    assert_eq!(r.outcome, Outcome::Stopped);
    assert!(wait_dead(pid, st));
    assert_eq!(records(&f.state_dir), 0);
    assert_eq!(f.handle.session_count(), 0);
}

#[test]
fn lease_mismatch_and_other_connection_rejected() {
    let f = fixture();
    let mut c = connect(&f.sock);
    let s = c
        .start_session("t1", "r1", "lease1", policy(60))
        .expect("start");
    let wrong = c.observe(&s.session_id, "lease2");
    assert!(matches!(
        wrong,
        Err(ClientError::Remote(ErrorCode::LeaseMismatch))
    ));
    // 同じ UID の別接続は lease が一致しても拒否する。
    let mut other = connect(&f.sock);
    let r = other.stop(&s.session_id, "lease1");
    assert!(matches!(
        r,
        Err(ClientError::Remote(ErrorCode::LeaseMismatch))
    ));
    let r = other.observe("nosuchsession", "lease1");
    assert!(matches!(
        r,
        Err(ClientError::Remote(ErrorCode::LeaseMismatch))
    ));
    assert_eq!(f.handle.session_count(), 1);
}

#[test]
fn disconnect_reaps_process_group_even_if_backend_stop_leaks() {
    let f = fixture_with(
        vec![uid()],
        LauncherLimits::default(),
        FakeBackend {
            leaky_stop: true,
            isolation_ok: true,
            ..FakeBackend::default()
        },
    );
    let mut c = connect(&f.sock);
    c.start_session("t1", "r1", "lease1", policy(600))
        .expect("start");
    let (pid, st) = f.backend.launched.lock().expect("lock")[0];
    assert!(same_process_alive(pid, st));
    drop(c);
    assert!(
        wait_dead(pid, st),
        "session process group must be killed on disconnect"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while records(&f.state_dir) > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(records(&f.state_dir), 0);
}

#[test]
fn lease_expiry_reaps_session() {
    let f = fixture();
    let mut c = connect(&f.sock);
    let s = c
        .start_session("t1", "r1", "lease1", policy(1))
        .expect("start");
    let (pid, st) = f.backend.launched.lock().expect("lock")[0];
    assert!(wait_dead(pid, st), "lease expiry must stop the session");
    let r = c.observe(&s.session_id, "lease1");
    assert!(matches!(
        r,
        Err(ClientError::Remote(ErrorCode::LeaseMismatch))
    ));
}

#[test]
fn session_and_connection_limits() {
    let limits = LauncherLimits {
        max_sessions: 1,
        max_connections: 1,
        ..LauncherLimits::default()
    };
    let f = fixture_with(vec![uid()], limits, ok_backend());
    let mut c = connect(&f.sock);
    c.start_session("t1", "r1", "lease1", policy(60))
        .expect("start");
    let r = c.start_session("t1", "r2", "lease2", policy(60));
    assert!(matches!(r, Err(ClientError::Remote(ErrorCode::Limit))));
    assert_eq!(f.backend.launched.lock().expect("lock").len(), 1);
    // 2 本目の接続は limit を返して閉じる。
    let mut second = UnixStream::connect(&f.sock).expect("connect");
    second
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let resp: Response =
        serde_json::from_slice(&read_frame(&mut second, DEFAULT_MAX_FRAME).expect("frame"))
            .expect("json");
    assert_eq!(
        resp,
        Response::Error {
            code: ErrorCode::Limit
        }
    );
}

#[test]
fn isolation_failure_stops_and_reports() {
    let f = fixture_with(
        vec![uid()],
        LauncherLimits::default(),
        FakeBackend::default(),
    );
    let mut c = connect(&f.sock);
    let r = c.start_session("t1", "r1", "lease1", policy(60));
    assert!(matches!(
        r,
        Err(ClientError::Remote(ErrorCode::IsolationFailed))
    ));
    let (pid, st) = f.backend.launched.lock().expect("lock")[0];
    assert!(wait_dead(pid, st));
    assert_eq!(f.handle.session_count(), 0);
    assert_eq!(records(&f.state_dir), 0);
}

#[test]
fn shutdown_reaps_all_sessions() {
    let mut f = fixture();
    let mut c = connect(&f.sock);
    c.start_session("t1", "r1", "lease1", policy(600))
        .expect("start");
    let (pid, st) = f.backend.launched.lock().expect("lock")[0];
    f.handle.shutdown();
    assert!(wait_dead(pid, st));
    assert_eq!(records(&f.state_dir), 0);
}

#[derive(Default)]
struct ShutdownEvents {
    teardown_entered: bool,
    shutdown_waiting: bool,
    shutdown_returned: bool,
}

/// 接続 thread の teardown（切断による回収）が記録を消す前に shutdown が走っても、shutdown が
/// 戻った時点で記録が残らない。フックで接続 thread を「process は止めたが記録は消していない」所に
/// 止め、その間に shutdown を呼ぶ。止めた thread は shutdown が接続の終了待ちに入るか戻るかの
/// どちらかの出来事で放す（時間には頼らない）。
#[test]
fn shutdown_waits_for_inflight_connection_teardown() {
    let ev = Arc::new((
        Mutex::new(ShutdownEvents::default()),
        std::sync::Condvar::new(),
    ));
    let ev2 = ev.clone();
    let hook: TestHookFn = Arc::new(move |point| {
        let (m, cv) = &*ev2;
        let mut g = m.lock().expect("lock");
        match point {
            TestHook::TeardownBeforeRemove => {
                g.teardown_entered = true;
                cv.notify_all();
                let _g = cv
                    .wait_while(g, |e| !(e.shutdown_waiting || e.shutdown_returned))
                    .expect("wait");
            }
            TestHook::ShutdownWaitConns => {
                g.shutdown_waiting = true;
                cv.notify_all();
            }
        }
    });
    let mut f = fixture_hooked(hook);
    let mut c = connect(&f.sock);
    c.start_session("t1", "r1", "lease1", policy(600))
        .expect("start");
    let (pid, st) = f.backend.launched.lock().expect("lock")[0];
    // 切断 → 接続 thread が session を引き取って teardown し、記録を消す前で止まる。
    drop(c);
    {
        let (m, cv) = &*ev;
        let g = m.lock().expect("lock");
        let _g = cv.wait_while(g, |e| !e.teardown_entered).expect("wait");
    }
    f.handle.shutdown();
    let left = records(&f.state_dir);
    {
        let (m, cv) = &*ev;
        m.lock().expect("lock").shutdown_returned = true;
        cv.notify_all();
    }
    assert!(wait_dead(pid, st));
    assert_eq!(left, 0, "shutdown returned while a session record remained");
}

// ---- registry ----

fn spawn_group() -> (Child, i32, u64) {
    let child = Command::new("sleep")
        .arg("600")
        .process_group(0)
        .spawn()
        .expect("spawn sleep");
    let pid = child.id() as i32;
    let st = process_starttime(pid).expect("starttime");
    (child, pid, st)
}

fn rec(session_id: &str, instance_id: &str, pid: i32, starttime: u64) -> SessionRecord {
    SessionRecord {
        session_id: session_id.into(),
        instance_id: instance_id.into(),
        pid,
        pgid: pid,
        starttime,
        peer_pid: 1,
        peer_starttime: 0,
        lease_id: "l".into(),
        lease_deadline_unix_ms: 0,
    }
}

#[test]
fn reap_orphans_kills_only_matching_starttime_of_other_instances() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = Registry::open(dir.path(), "inst-new").expect("registry");
    let (mut reused, reused_pid, reused_st) = spawn_group();
    let (mut orphan, orphan_pid, orphan_st) = spawn_group();
    let (mut mine, mine_pid, mine_st) = spawn_group();
    // PID 再利用: pid は生きているが starttime が記録と違う → 殺さない。
    reg.write(&rec("reused", "inst-old", reused_pid, reused_st + 1))
        .expect("write");
    reg.write(&rec("orphan", "inst-old", orphan_pid, orphan_st))
        .expect("write");
    reg.write(&rec("mine", "inst-new", mine_pid, mine_st))
        .expect("write");
    std::fs::write(dir.path().join("broken.json"), b"{").expect("write");

    let killed = reg.reap_orphans().expect("reap");
    assert_eq!(killed, vec![orphan_pid]);
    assert!(wait_dead(orphan_pid, orphan_st));
    assert!(
        same_process_alive(reused_pid, reused_st),
        "PID reuse must not be killed"
    );
    assert!(
        same_process_alive(mine_pid, mine_st),
        "current instance is untouched"
    );
    let left: Vec<_> = reg
        .read_all()
        .expect("read")
        .into_iter()
        .map(|r| r.session_id)
        .collect();
    assert_eq!(left, vec!["mine".to_owned()]);

    for c in [&mut reused, &mut orphan, &mut mine] {
        let _ = c.kill();
        let _ = c.wait();
    }
}

#[test]
fn registry_rejects_path_like_session_ids_and_writes_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = Registry::open(dir.path().join("s"), "i").expect("registry");
    assert!(reg.write(&rec("../x", "i", 2, 0)).is_err());
    reg.write(&rec("ok", "i", 2, 0)).expect("write");
    let mode = std::fs::metadata(reg.dir().join("ok.json"))
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let dmode = std::fs::metadata(reg.dir())
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(dmode & 0o777, 0o700);
}

/// ADR-0116 付記 D-P: socket 起動と同じ形（listen socket を作った process と応答する process が
/// 別）で、client が見る launcher の身元は応答を書いた process（`SCM_CREDENTIALS`）であって、
/// listen socket を作った process（`SO_PEERCRED`）ではないことを確かめる。本番の socket 起動では
/// 前者が launcher（celeris-browser）、後者が systemd（root）。
#[test]
fn client_identifies_the_responding_process_not_the_listener_creator() {
    use nix::libc;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("launcher.sock");
    // listen socket はこの process（systemd の役）が作る。
    let listener = UnixListener::bind(&path).expect("bind");
    let mut reply = Vec::new();
    write_message(
        &mut reply,
        &Response::Observed {
            state: SessionState::Running,
            facts: SessionFacts::default(),
        },
        DEFAULT_MAX_FRAME,
    )
    .expect("encode reply");
    let lfd = listener.as_raw_fd();
    // 応答は fork した子（launcher の役）が accept して書く。子は async-signal-safe な syscall だけを使う。
    // SAFETY: 子は accept/read/write/close/_exit だけを呼んで終わる。
    let child = match unsafe { nix::unistd::fork() }.expect("fork") {
        nix::unistd::ForkResult::Child => unsafe {
            let c = libc::accept(lfd, std::ptr::null_mut(), std::ptr::null_mut());
            if c >= 0 {
                let mut buf = [0u8; 4096];
                libc::read(c, buf.as_mut_ptr().cast(), buf.len());
                libc::write(c, reply.as_ptr().cast(), reply.len());
                libc::close(c);
            }
            libc::_exit(0)
        },
        nix::unistd::ForkResult::Parent { child } => child,
    };
    drop(listener);

    let mut client = LauncherClient::connect(&path, Duration::from_secs(30)).expect("connect");
    assert_eq!(client.responder(), None, "no response yet");
    let resp = client
        .request(&Request::Observe {
            session_id: "s".into(),
            lease_id: "l".into(),
        })
        .expect("request");
    assert!(matches!(resp, Response::Observed { .. }));
    let me = nix::unistd::getpid().as_raw();
    // SO_PEERCRED は listen した process（この試験 process）を指す = 本番では systemd の uid 0。
    let listener_cred = super::server::peer_cred(client.stream_for_test()).expect("peercred");
    assert_eq!(
        listener_cred.0, me,
        "SO_PEERCRED names the listener creator"
    );
    // 身元は実際に応答を書いた子の資格情報（kernel が付けた値）。
    let responder = client.responder().expect("SCM_CREDENTIALS on the reply");
    assert_eq!(
        responder.pid,
        child.as_raw(),
        "responder is the accepting child"
    );
    assert_ne!(responder.pid, me);
    assert_eq!(responder.uid, uid());
    assert_eq!(client.responder_uid(), Some(uid()));
    nix::sys::wait::waitpid(child, None).expect("reap child");
}

/// 実 launcher（in-process の `LauncherServer`）の応答でも送り手が記録され、全応答で一致する。
#[test]
fn client_records_a_consistent_responder_across_requests() {
    let fx = fixture();
    let mut client = LauncherClient::connect(&fx.sock, Duration::from_secs(30)).expect("connect");
    let lease = super::random_id().expect("lease");
    let started = client
        .start_session("t", "r", &lease, policy(60))
        .expect("start");
    let first = client.responder().expect("responder after start");
    assert_eq!(first.uid, uid());
    assert_eq!(first.pid, nix::unistd::getpid().as_raw());
    client
        .observe(&started.session_id, &lease)
        .expect("observe");
    assert_eq!(client.responder(), Some(first));
    client.stop(&started.session_id, &lease).expect("stop");
    assert_eq!(client.responder_uid(), Some(uid()));
}
