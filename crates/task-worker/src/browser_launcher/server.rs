//! ADR-0116 D2/D6: launcher の Unix socket server。
//!
//! accept 直後に `SO_PEERCRED` を読み、uid が `allowed_uids` に無ければ応答せずに閉じる。接続ごとに
//! 1 本の thread で要求を 1 個ずつ処理する（未応答要求は 1 接続 1 個）。session 操作は lease id・
//! lease 期限・接続の持ち主（接続 id）が全部一致したときだけ通す。切断・lease 失効・`stop`・
//! shutdown のどれでも [`BackendSession::stop`] と process group の停止（starttime 照合つき）で回収する。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nix::libc;

use super::protocol::{
    ActionArgs, ErrorCode, Observation, Outcome, Receipt, Request, Response, SessionFacts,
    SessionPolicy, SessionState, Verb, decode_request, write_message,
};
use super::registry::{Registry, SessionRecord, stop_group};
use super::{now_unix_ms, random_id};
use crate::browser_runtime::process_starttime;

/// launcher の上限（ADR-0116 D2 の既定値）。
#[derive(Debug, Clone)]
pub struct LauncherLimits {
    pub max_frame: usize,
    pub max_connections: usize,
    pub max_sessions: usize,
    /// frame の長さを読んでから本体を読み切るまでの期限。
    pub request_read: Duration,
    pub start: Duration,
    pub action: Duration,
    /// 要求の無い接続を閉じるまでの時間。
    pub idle: Duration,
    /// 停止時の SIGTERM から SIGKILL までの猶予。
    pub stop_grace: Duration,
}

impl Default for LauncherLimits {
    fn default() -> Self {
        Self {
            max_frame: super::protocol::DEFAULT_MAX_FRAME,
            max_connections: 4,
            max_sessions: 2,
            request_read: Duration::from_secs(5),
            start: Duration::from_secs(60),
            action: Duration::from_secs(120),
            idle: Duration::from_secs(300),
            stop_grace: Duration::from_secs(5),
        }
    }
}

/// launcher の固定設定のうち server が使う部分（root 所有の設定ファイルから決まる）。
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub allowed_uids: Vec<u32>,
    pub limits: LauncherLimits,
}

/// backend に渡す起動要求（launcher が採番した id と policy だけ）。
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub session_id: String,
    pub instance_id: String,
    pub task_id: String,
    pub run_id: String,
    pub policy: SessionPolicy,
}

/// 起動できた session。`pid` は process group の leader（bwrap）。
pub struct Launched {
    pub session: Box<dyn BackendSession>,
    pub pid: i32,
    pub pgid: i32,
    pub starttime: u64,
}

/// session の起動の中身（userns・bwrap・Chrome）。単体試験では偽の実装を使う。
pub trait SessionBackend: Send + Sync + 'static {
    fn start(&self, req: &StartRequest) -> Result<Launched, ErrorCode>;
}

/// 起動済みの 1 session。
pub trait BackendSession: Send + 'static {
    fn action(&mut self, verb: Verb, args: &ActionArgs) -> Result<Observation, ErrorCode>;
    fn observe(&mut self) -> (SessionState, SessionFacts);
    /// `verify_isolation`（ADR-0115 の owner / map 検査を含む）が Ok か。
    fn isolation_ok(&mut self) -> bool;
    /// CDP と channel を閉じ、process group を止めて回収する。
    fn stop(self: Box<Self>);
}

struct Entry {
    record: SessionRecord,
    owner_conn: u64,
    lease_deadline: Instant,
    policy: SessionPolicy,
    backend: Mutex<Option<Box<dyn BackendSession>>>,
}

#[derive(Default)]
struct State {
    sessions: HashMap<String, Arc<Entry>>,
    starting: usize,
}

struct Inner {
    cfg: ServerConfig,
    backend: Arc<dyn SessionBackend>,
    registry: Registry,
    state: Mutex<State>,
    connections: AtomicUsize,
    next_conn: AtomicU64,
    conns: Mutex<HashMap<u64, UnixStream>>,
    shutdown: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// launcher の server。[`LauncherServer::spawn`] で accept loop と lease の timer を起こす。
pub struct LauncherServer {
    listener: UnixListener,
    inner: Arc<Inner>,
}

/// 動いている server。drop か [`ServerHandle::shutdown`] で全 session を回収して止まる。
pub struct ServerHandle {
    inner: Arc<Inner>,
    threads: Vec<JoinHandle<()>>,
}

impl LauncherServer {
    /// `path` に socket を作る。既存の path は socket のときだけ消す。
    pub fn bind(
        path: &Path,
        cfg: ServerConfig,
        backend: Arc<dyn SessionBackend>,
        registry: Registry,
    ) -> std::io::Result<Self> {
        if let Ok(m) = std::fs::symlink_metadata(path) {
            if !m.file_type().is_socket() {
                return Err(std::io::Error::other(
                    "socket path exists and is not a socket",
                ));
            }
            std::fs::remove_file(path)?;
        }
        Ok(Self::from_listener(
            UnixListener::bind(path)?,
            cfg,
            backend,
            registry,
        ))
    }

    /// systemd の socket activation などで受け取った listener から作る。
    pub fn from_listener(
        listener: UnixListener,
        cfg: ServerConfig,
        backend: Arc<dyn SessionBackend>,
        registry: Registry,
    ) -> Self {
        Self {
            listener,
            inner: Arc::new(Inner {
                cfg,
                backend,
                registry,
                state: Mutex::new(State::default()),
                connections: AtomicUsize::new(0),
                next_conn: AtomicU64::new(1),
                conns: Mutex::new(HashMap::new()),
                shutdown: AtomicBool::new(false),
            }),
        }
    }

    /// 前の instance の孤児を回収してから accept loop と lease timer を起こす。
    pub fn spawn(self) -> std::io::Result<ServerHandle> {
        let reaped = self.inner.registry.reap_orphans()?;
        if !reaped.is_empty() {
            tracing::info!(
                count = reaped.len(),
                "browser launcher: reaped orphan sessions"
            );
        }
        self.listener.set_nonblocking(true)?;
        let inner = self.inner.clone();
        let listener = self.listener;
        let accept = std::thread::Builder::new()
            .name("celeris-launcher-accept".into())
            .spawn(move || accept_loop(&inner, &listener))?;
        let inner = self.inner.clone();
        let timer = std::thread::Builder::new()
            .name("celeris-launcher-lease".into())
            .spawn(move || lease_loop(&inner))?;
        Ok(ServerHandle {
            inner: self.inner,
            threads: vec![accept, timer],
        })
    }
}

impl ServerHandle {
    pub fn session_count(&self) -> usize {
        lock(&self.inner.state).sessions.len()
    }

    /// accept を止め、全接続を閉じ、全 session を回収する。
    pub fn shutdown(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        for (_, s) in lock(&self.inner.conns).drain() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let all: Vec<_> = lock(&self.inner.state)
            .sessions
            .drain()
            .map(|(_, e)| e)
            .collect();
        for e in all {
            teardown(&self.inner, &e);
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(inner: &Arc<Inner>, listener: &UnixListener) {
    while !inner.shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => admit(inner, stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                tracing::warn!(error = %e, "browser launcher: accept failed");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// `SO_PEERCRED`（pid, uid）。
fn peer_cred(s: &UnixStream) -> Option<(i32, u32)> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred は ucred の大きさの書込み先で、len はその大きさ。
    let rc = unsafe {
        libc::getsockopt(
            s.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (rc == 0 && len as usize == std::mem::size_of::<libc::ucred>()).then_some((cred.pid, cred.uid))
}

fn admit(inner: &Arc<Inner>, mut stream: UnixStream) {
    let Some((peer_pid, peer_uid)) = peer_cred(&stream) else {
        return;
    };
    if !inner.cfg.allowed_uids.contains(&peer_uid) {
        // 応答せずに閉じる（ADR-0116 D2）。
        tracing::warn!(peer_uid, "browser launcher: peer uid not allowed");
        return;
    }
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let limits = &inner.cfg.limits;
    if inner.connections.fetch_add(1, Ordering::SeqCst) >= limits.max_connections {
        inner.connections.fetch_sub(1, Ordering::SeqCst);
        let _ = stream.set_write_timeout(Some(limits.request_read));
        let _ = write_message(
            &mut stream,
            &Response::Error {
                code: ErrorCode::Limit,
            },
            limits.max_frame,
        );
        return;
    }
    let conn_id = inner.next_conn.fetch_add(1, Ordering::SeqCst);
    if let Ok(c) = stream.try_clone() {
        lock(&inner.conns).insert(conn_id, c);
    }
    let inner2 = inner.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("celeris-launcher-conn-{conn_id}"))
        .spawn(move || {
            let peer = Peer {
                conn_id,
                pid: peer_pid,
                starttime: process_starttime(peer_pid).unwrap_or(0),
            };
            serve_conn(&inner2, stream, &peer);
            lock(&inner2.conns).remove(&conn_id);
            close_conn_sessions(&inner2, conn_id);
            inner2.connections.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        lock(&inner.conns).remove(&conn_id);
        inner.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Peer {
    conn_id: u64,
    pid: i32,
    starttime: u64,
}

/// 1 frame を読む。idle の間は `idle`、長さを読んだ後は `request_read` で切る。
/// 上限超過・期限切れ・切断はどれも `None`（接続を閉じる）。
fn read_request(stream: &mut UnixStream, limits: &LauncherLimits) -> Option<Vec<u8>> {
    stream.set_read_timeout(Some(limits.idle)).ok()?;
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).ok()?;
    let n = u32::from_be_bytes(len) as usize;
    if n > limits.max_frame {
        return None;
    }
    stream.set_read_timeout(Some(limits.request_read)).ok()?;
    let mut body = vec![0u8; n];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

fn serve_conn(inner: &Arc<Inner>, mut stream: UnixStream, peer: &Peer) {
    let limits = inner.cfg.limits.clone();
    let _ = stream.set_write_timeout(Some(limits.request_read));
    while !inner.shutdown.load(Ordering::SeqCst) {
        let Some(body) = read_request(&mut stream, &limits) else {
            return;
        };
        let (resp, close) = match decode_request(&body) {
            Ok(req) => (handle(inner, req, peer), false),
            Err(code) => (Response::Error { code }, true),
        };
        if write_message(&mut stream, &resp, limits.max_frame).is_err() || close {
            let _ = stream.flush();
            return;
        }
    }
}

fn receipt(
    rec: &SessionRecord,
    verb: Option<Verb>,
    outcome: Outcome,
    isolation_ok: bool,
) -> Receipt {
    Receipt {
        session_id: rec.session_id.clone(),
        instance_id: rec.instance_id.clone(),
        verb,
        outcome,
        at_unix_ms: now_unix_ms(),
        isolation_ok,
    }
}

fn err(code: ErrorCode) -> Response {
    Response::Error { code }
}

fn handle(inner: &Arc<Inner>, req: Request, peer: &Peer) -> Response {
    match req {
        Request::StartSession {
            task_id,
            run_id,
            lease_id,
            policy,
        } => start_session(inner, peer, task_id, run_id, lease_id, policy),
        Request::Action {
            session_id,
            lease_id,
            verb,
            args,
        } => {
            let entry = match authorize(inner, peer, &session_id, &lease_id) {
                Ok(e) => e,
                Err(code) => return err(code),
            };
            if !action_allowed(&entry.policy, verb, &args) {
                return err(ErrorCode::Unauthorized);
            }
            let e2 = entry.clone();
            let out = run_with_deadline(
                move || {
                    let mut g = lock(&e2.backend);
                    let s = g.as_mut().ok_or(ErrorCode::LeaseMismatch)?;
                    let obs = s.action(verb, &args)?;
                    Ok((obs, s.isolation_ok()))
                },
                inner.cfg.limits.action,
                |_| {},
            );
            match out {
                Some(Ok((observation, iso))) => Response::ActionResult {
                    receipt: receipt(&entry.record, Some(verb), Outcome::Ok, iso),
                    observation,
                },
                Some(Err(code)) => err(code),
                None => {
                    // 期限切れの session は止める（fail closed）。
                    remove_and_teardown(inner, &entry.record.session_id);
                    err(ErrorCode::Timeout)
                }
            }
        }
        Request::Observe {
            session_id,
            lease_id,
        } => {
            let entry = match authorize(inner, peer, &session_id, &lease_id) {
                Ok(e) => e,
                Err(code) => return err(code),
            };
            let e2 = entry.clone();
            let out = run_with_deadline(
                move || lock(&e2.backend).as_mut().map(|s| s.observe()),
                inner.cfg.limits.action,
                |_| {},
            );
            match out {
                Some(Some((state, facts))) => Response::Observed { state, facts },
                Some(None) => err(ErrorCode::LeaseMismatch),
                None => {
                    remove_and_teardown(inner, &entry.record.session_id);
                    err(ErrorCode::Timeout)
                }
            }
        }
        Request::Stop {
            session_id,
            lease_id,
        } => {
            let entry = match authorize(inner, peer, &session_id, &lease_id) {
                Ok(e) => e,
                Err(code) => return err(code),
            };
            remove_and_teardown(inner, &entry.record.session_id);
            Response::Stopped {
                receipt: receipt(&entry.record, None, Outcome::Stopped, true),
            }
        }
    }
}

fn start_session(
    inner: &Arc<Inner>,
    peer: &Peer,
    task_id: String,
    run_id: String,
    lease_id: String,
    policy: SessionPolicy,
) -> Response {
    {
        let mut st = lock(&inner.state);
        if st.sessions.len() + st.starting >= inner.cfg.limits.max_sessions {
            return err(ErrorCode::Limit);
        }
        st.starting += 1;
    }
    let resp = start_reserved(inner, peer, task_id, run_id, lease_id, policy);
    lock(&inner.state).starting -= 1;
    resp
}

fn start_reserved(
    inner: &Arc<Inner>,
    peer: &Peer,
    task_id: String,
    run_id: String,
    lease_id: String,
    policy: SessionPolicy,
) -> Response {
    let Ok(session_id) = random_id() else {
        return err(ErrorCode::LaunchFailed);
    };
    let lease = Duration::from_secs(policy.lease_seconds);
    let req = StartRequest {
        session_id: session_id.clone(),
        instance_id: inner.registry.instance_id().to_owned(),
        task_id,
        run_id,
        policy: policy.clone(),
    };
    let backend = inner.backend.clone();
    let out = run_with_deadline(
        move || {
            let mut l = backend.start(&req)?;
            if !l.session.isolation_ok() {
                l.session.stop();
                return Err(ErrorCode::IsolationFailed);
            }
            Ok(l)
        },
        inner.cfg.limits.start,
        |late: Result<Launched, ErrorCode>| {
            if let Ok(l) = late {
                l.session.stop();
            }
        },
    );
    let launched = match out {
        Some(Ok(l)) => l,
        Some(Err(code)) => return err(code),
        None => return err(ErrorCode::Timeout),
    };
    let record = SessionRecord {
        session_id: session_id.clone(),
        instance_id: inner.registry.instance_id().to_owned(),
        pid: launched.pid,
        pgid: launched.pgid,
        starttime: launched.starttime,
        peer_pid: peer.pid,
        peer_starttime: peer.starttime,
        lease_id,
        lease_deadline_unix_ms: now_unix_ms().saturating_add(lease.as_millis() as u64),
    };
    let entry = Arc::new(Entry {
        record,
        owner_conn: peer.conn_id,
        lease_deadline: Instant::now() + lease,
        policy,
        backend: Mutex::new(Some(launched.session)),
    });
    if inner.registry.write(&entry.record).is_err() {
        teardown(inner, &entry);
        return err(ErrorCode::LaunchFailed);
    }
    lock(&inner.state)
        .sessions
        .insert(session_id.clone(), entry.clone());
    Response::Started {
        session_id,
        instance_id: entry.record.instance_id.clone(),
        receipt: receipt(&entry.record, None, Outcome::Started, true),
    }
}

/// lease id・期限・接続の持ち主が全部一致したときだけ session を返す。
/// session の有無は区別して返さない（どれも `lease_mismatch`）。
fn authorize(
    inner: &Arc<Inner>,
    peer: &Peer,
    session_id: &str,
    lease_id: &str,
) -> Result<Arc<Entry>, ErrorCode> {
    let st = lock(&inner.state);
    let e = st
        .sessions
        .get(session_id)
        .ok_or(ErrorCode::LeaseMismatch)?;
    if e.owner_conn != peer.conn_id
        || e.record.lease_id != lease_id
        || Instant::now() >= e.lease_deadline
    {
        return Err(ErrorCode::LeaseMismatch);
    }
    Ok(e.clone())
}

/// launcher 側での policy の再検査（verb と URL の domain）。
fn action_allowed(policy: &SessionPolicy, verb: Verb, args: &ActionArgs) -> bool {
    if !policy.allowed_actions.contains(&verb) {
        return false;
    }
    match &args.url {
        None => verb != Verb::Open,
        Some(u) => {
            let Ok(url) = url::Url::parse(u) else {
                return false;
            };
            if !matches!(url.scheme(), "http" | "https") {
                return false;
            }
            let Some(host) = url.host_str() else {
                return false;
            };
            policy
                .allowed_domains
                .iter()
                .any(|d| host == d || host.ends_with(&format!(".{d}")))
        }
    }
}

fn remove_and_teardown(inner: &Arc<Inner>, session_id: &str) {
    let e = lock(&inner.state).sessions.remove(session_id);
    if let Some(e) = e {
        teardown(inner, &e);
    }
}

fn close_conn_sessions(inner: &Arc<Inner>, conn_id: u64) {
    let owned: Vec<_> = {
        let mut st = lock(&inner.state);
        let ids: Vec<_> = st
            .sessions
            .iter()
            .filter(|(_, e)| e.owner_conn == conn_id)
            .map(|(k, _)| k.clone())
            .collect();
        ids.iter().filter_map(|k| st.sessions.remove(k)).collect()
    };
    for e in owned {
        teardown(inner, &e);
    }
}

fn lease_loop(inner: &Arc<Inner>) {
    while !inner.shutdown.load(Ordering::SeqCst) {
        let now = Instant::now();
        let expired: Vec<_> = {
            let mut st = lock(&inner.state);
            let ids: Vec<_> = st
                .sessions
                .iter()
                .filter(|(_, e)| now >= e.lease_deadline)
                .map(|(k, _)| k.clone())
                .collect();
            ids.iter().filter_map(|k| st.sessions.remove(k)).collect()
        };
        for e in expired {
            teardown(inner, &e);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// backend の stop を猶予つきで待ち、その後 process group を（leader が本人なら）止め、記録を消す。
fn teardown(inner: &Arc<Inner>, entry: &Arc<Entry>) {
    let grace = inner.cfg.limits.stop_grace;
    let e2 = entry.clone();
    let _ = run_with_deadline(
        move || {
            let s = lock(&e2.backend).take();
            if let Some(s) = s {
                s.stop();
            }
        },
        grace,
        |_| {},
    );
    stop_group(&entry.record, grace);
    if let Err(e) = inner.registry.remove(&entry.record.session_id) {
        tracing::warn!(error = %e, "browser launcher: failed to remove session record");
    }
}

struct Slot<T> {
    done: Option<T>,
    abandoned: bool,
}

/// `f` を別 thread で走らせ、`timeout` 内に終われば値を返す。間に合わなければ `None` を返し、
/// 後から出た値は `on_late` に渡す（起動が遅れて成功した session を止めるため）。
fn run_with_deadline<T, F, L>(f: F, timeout: Duration, on_late: L) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
    L: FnOnce(T) + Send + 'static,
{
    let shared = Arc::new((
        Mutex::new(Slot {
            done: None,
            abandoned: false,
        }),
        Condvar::new(),
    ));
    let s2 = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("celeris-launcher-op".into())
        .spawn(move || {
            let v = f();
            let (m, cv) = &*s2;
            let mut g = lock(m);
            if g.abandoned {
                drop(g);
                on_late(v);
            } else {
                g.done = Some(v);
                cv.notify_all();
            }
        });
    if spawned.is_err() {
        return None;
    }
    let (m, cv) = &*shared;
    let g = lock(m);
    let (mut g, _) = cv
        .wait_timeout_while(g, timeout, |s| s.done.is_none())
        .unwrap_or_else(|p| p.into_inner());
    match g.done.take() {
        Some(v) => Some(v),
        None => {
            g.abandoned = true;
            None
        }
    }
}
