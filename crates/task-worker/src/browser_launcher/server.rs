//! ADR-0116 D2/D6: launcher の Unix socket server。
//!
//! accept 直後に `SO_PEERCRED` を読み、uid が `allowed_uids` に無ければ応答せずに閉じる。接続ごとに
//! 1 本の thread で要求を 1 個ずつ処理する（未応答要求は 1 接続 1 個）。session 操作は lease id・
//! lease 期限・接続の持ち主（接続 id）が全部一致したときだけ通す。切断・lease 失効・`stop`・
//! shutdown のどれでも [`BackendSession::stop`] と process group の停止（starttime 照合つき）で回収する。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nix::libc;

use super::live::{LiveFeed, LiveNext};
use super::protocol::{
    ActionArgs, AuthenticateArgs, AuthenticationStatus, ErrorCode, LoginResult, MAX_LIVE_BODY,
    Observation, Outcome, Receipt, Request, Response, SessionBinding, SessionFacts, SessionPolicy,
    SessionState, Verb, decode_request, write_frame, write_message,
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

/// 起動できた session。`pid` は process group の leader（bwrap）。`runtime_*` と `ns_inodes` は
/// launcher が `verify_isolation` を掛けた runtime process とその namespace の inode で、receipt の
/// 束縛（protocol v3）に載る。
pub struct Launched {
    pub session: Box<dyn BackendSession>,
    pub pid: i32,
    pub pgid: i32,
    pub starttime: u64,
    pub runtime_pid: i32,
    pub runtime_starttime: u64,
    pub ns_inodes: std::collections::BTreeMap<task_core::browser_isolation::Namespace, u64>,
}

/// session の起動の中身（userns・bwrap・Chrome）。単体試験では偽の実装を使う。
/// v8: one chunk of an artifact read by the backend (bytes from `offset`, at most
/// [`super::protocol::ARTIFACT_CHUNK`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactChunk {
    pub kind: super::protocol::ArtifactKind,
    pub size: u64,
    pub data: Vec<u8>,
}

pub trait SessionBackend: Send + Sync + 'static {
    fn start(&self, req: &StartRequest) -> Result<Launched, ErrorCode>;
    /// 試験専用 loopback 許可（`127.0.0.1:<port>`）。`hello` で daemon に申告する。既定は空（off）。
    fn test_loopback_allow(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 起動済みの 1 session。
pub trait BackendSession: Send + 'static {
    fn action(&mut self, verb: Verb, args: &ActionArgs) -> Result<Observation, ErrorCode>;
    /// v4 `auth_begin`: stop agent observation on the launcher-owned controller and create the
    /// login target. Returns only the opaque CDP target id.
    fn auth_begin(&mut self, _auth_section_id: &str) -> Result<String, ErrorCode> {
        Err(ErrorCode::Unauthorized)
    }
    /// Runs the fixed credential login inside the launcher-owned controller. `broker` is the
    /// `injection.sock` connection the daemon opened and passed with `SCM_RIGHTS`.
    /// Implementations must never return credential material or CDP data; the success value says
    /// only whether agent observation resumed (v5, ADR 2026-10-09 credential username / post-login).
    fn authenticate(
        &mut self,
        _args: &AuthenticateArgs,
        _broker: UnixStream,
    ) -> Result<LoginResult, ErrorCode> {
        Err(ErrorCode::Unauthorized)
    }
    /// v8 `fetch_artifact`: one chunk of a screenshot / download this session produced
    /// (付記 2026-10-10e). Implementations serve only names they generated in this session, check
    /// the type, size and count, and never return a path or anything but the file's bytes.
    fn fetch_artifact(&mut self, _name: &str, _offset: u64) -> Result<ArtifactChunk, ErrorCode> {
        Err(ErrorCode::Unauthorized)
    }
    /// v9 `live_start`: the session's screencast for the owner's Live View (D1). The feed must not
    /// hold the session lock: the connection thread streams from it while actions run. Dropping
    /// the feed stops the screencast. Backends without a launcher-owned CDP controller refuse.
    fn live(&mut self) -> Result<Box<dyn LiveFeed>, ErrorCode> {
        Err(ErrorCode::Unauthorized)
    }
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
    /// v8: a frame connection streams this session (at most one).
    live_active: AtomicBool,
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
    /// 接続 thread が回収を終えて `connections` を減らしたとき（`conns` の lock の下で）知らせる。
    conn_exit: Condvar,
    shutdown: AtomicBool,
    #[cfg(test)]
    test_hook: Option<TestHookFn>,
}

/// 試験専用の遅延フック（agent-docs/guides/testing.md）。競合の窓で止めて順序を決定的にする。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TestHook {
    /// teardown が process を止めた後、記録を消す前。
    TeardownBeforeRemove,
    /// shutdown が接続 thread の終了を待ち始める直前。
    ShutdownWaitConns,
}

#[cfg(test)]
pub(super) type TestHookFn = Arc<dyn Fn(TestHook) + Send + Sync>;

#[cfg(test)]
fn test_hook(inner: &Inner, point: TestHook) {
    if let Some(h) = &inner.test_hook {
        h(point);
    }
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
                conn_exit: Condvar::new(),
                shutdown: AtomicBool::new(false),
                #[cfg(test)]
                test_hook: None,
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn set_test_hook(&mut self, hook: TestHookFn) {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.test_hook = Some(hook);
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
    ///
    /// 接続 thread は切断時に自分の session を引き取って teardown するので、その thread が
    /// 全部終わる（記録の削除まで済む）のを待ってから戻る。待たないと、shutdown が戻った後に
    /// 接続 thread が記録を消す・process を止めることになり、回収の取りこぼしに見える。
    pub fn shutdown(&mut self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        // 先に accept と lease timer を止める。以後は接続も増えず、`conns` に全接続が揃う。
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        for (_, s) in lock(&self.inner.conns).drain() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        #[cfg(test)]
        test_hook(&self.inner, TestHook::ShutdownWaitConns);
        {
            let g = lock(&self.inner.conns);
            let _g = self
                .inner
                .conn_exit
                .wait_while(g, |_| self.inner.connections.load(Ordering::SeqCst) > 0)
                .unwrap_or_else(|p| p.into_inner());
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
pub(super) fn peer_cred(s: &UnixStream) -> Option<(i32, u32)> {
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
    // shutdown が閉じられない接続は受けない（終了待ちが idle まで延びるため）。
    let Ok(c) = stream.try_clone() else {
        inner.connections.fetch_sub(1, Ordering::SeqCst);
        return;
    };
    lock(&inner.conns).insert(conn_id, c);
    let inner2 = inner.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("celeris-launcher-conn-{conn_id}"))
        .spawn(move || {
            let peer = Peer {
                conn_id,
                pid: peer_pid,
                uid: peer_uid,
                starttime: process_starttime(peer_pid).unwrap_or(0),
            };
            serve_conn(&inner2, stream, &peer);
            lock(&inner2.conns).remove(&conn_id);
            close_conn_sessions(&inner2, conn_id);
            let _g = lock(&inner2.conns);
            inner2.connections.fetch_sub(1, Ordering::SeqCst);
            inner2.conn_exit.notify_all();
        });
    if spawned.is_err() {
        lock(&inner.conns).remove(&conn_id);
        inner.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Peer {
    conn_id: u64,
    pid: i32,
    uid: u32,
    starttime: u64,
}

/// FD の上限（`authenticate` は 1 本だけ使う。余分は受けてから閉じる）。
const MAX_PASSED_FDS: usize = 4;

/// `recvmsg` で `buf` を読み、付いていた `SCM_RIGHTS` の FD を `fds` に集める（CLOEXEC）。
/// 読めた byte 数（0 = 切断）。control の切り詰めは失敗にする。
fn recv_with_fds(
    stream: &UnixStream,
    buf: &mut [u8],
    fds: &mut Vec<OwnedFd>,
) -> std::io::Result<usize> {
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
    let n = loop {
        // SAFETY: msg は上の有効な buffer を指す。
        let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        break n as usize;
    };
    // SAFETY: recvmsg が埋めた control buffer を CMSG_* で辿る。受けた FD は必ず OwnedFd にする。
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
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(std::io::Error::other("control data truncated"));
    }
    Ok(n)
}

/// 1 frame を読む。idle の間は `idle`、長さを読んだ後は `request_read` で切る。
/// 上限超過・期限切れ・切断はどれも `None`（接続を閉じる）。frame の先頭（長さ）に付いた
/// `SCM_RIGHTS` の FD も返す（v4 の `authenticate` だけが 1 本使う）。
fn read_request(
    stream: &mut UnixStream,
    limits: &LauncherLimits,
) -> Option<(Vec<u8>, Vec<OwnedFd>)> {
    stream.set_read_timeout(Some(limits.idle)).ok()?;
    let mut len = [0u8; 4];
    let mut fds = Vec::new();
    let mut got = 0;
    while got < len.len() {
        let n = recv_with_fds(stream, &mut len[got..], &mut fds).ok()?;
        if n == 0 || fds.len() > MAX_PASSED_FDS {
            return None;
        }
        got += n;
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > limits.max_frame {
        return None;
    }
    stream.set_read_timeout(Some(limits.request_read)).ok()?;
    let mut body = vec![0u8; n];
    stream.read_exact(&mut body).ok()?;
    Some((body, fds))
}

/// `authenticate` に付いた FD: 接続済みの unix stream socket で、相手（credentiald）の UID が
/// 要求元 daemon 接続の UID と同じこと（credentiald は daemon と同じ UID で動く）。
fn passed_broker_stream(fd: OwnedFd, peer: &Peer) -> Result<UnixStream, ErrorCode> {
    let mut ty: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: ty は c_int の書込み先で、len はその大きさ。
    let rc = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut ty).cast(),
            &mut len,
        )
    };
    if rc != 0 || ty != libc::SOCK_STREAM {
        return Err(ErrorCode::BadRequest);
    }
    let stream = UnixStream::from(fd);
    // unix socket でなければ local_addr が失敗する。未接続なら peer_cred が採れない。
    stream.local_addr().map_err(|_| ErrorCode::BadRequest)?;
    match peer_cred(&stream) {
        Some((pid, uid)) if pid > 0 && uid == peer.uid => Ok(stream),
        _ => Err(ErrorCode::Unauthorized),
    }
}

fn serve_conn(inner: &Arc<Inner>, mut stream: UnixStream, peer: &Peer) {
    let limits = inner.cfg.limits.clone();
    let _ = stream.set_write_timeout(Some(limits.request_read));
    while !inner.shutdown.load(Ordering::SeqCst) {
        let Some((body, mut fds)) = read_request(&mut stream, &limits) else {
            return;
        };
        // FD は `authenticate` にちょうど 1 本だけ。それ以外の組み合わせは bad_request で閉じる。
        let (resp, close) = match decode_request(&body) {
            Ok(Request::Authenticate { args }) => match (fds.pop(), fds.is_empty()) {
                (Some(fd), true) => (handle_authenticate(inner, args, fd, peer), false),
                _ => (
                    Response::Error {
                        code: ErrorCode::BadRequest,
                    },
                    true,
                ),
            },
            Ok(Request::LiveStart {
                session_id,
                lease_id,
            }) if fds.is_empty() => {
                // A started stream makes this the session's frame connection; it closes with the
                // stream. A refusal (wrong lease, the control connection, …) keeps serving.
                match serve_live(inner, &mut stream, peer, &session_id, &lease_id) {
                    Ok(()) => return,
                    Err(code) => (err(code), false),
                }
            }
            Ok(_) if !fds.is_empty() => (
                Response::Error {
                    code: ErrorCode::BadRequest,
                },
                true,
            ),
            Ok(req) => (handle(inner, req, peer), false),
            Err(code) => (Response::Error { code }, true),
        };
        drop(fds);
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
        binding: None,
    }
}

fn err(code: ErrorCode) -> Response {
    Response::Error { code }
}

fn handle(inner: &Arc<Inner>, req: Request, peer: &Peer) -> Response {
    match req {
        Request::Hello {} => Response::Hello {
            protocol_version: super::PROTOCOL_VERSION,
            test_loopback_allow: inner.backend.test_loopback_allow(),
        },
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
                // Fixed tokens only (付記 2026-10-10f).
                eprintln!(
                    "celeris-browser-launcher: action {verb:?} refused: session={} code=unauthorized launcher_reason=session_policy",
                    entry.record.session_id
                );
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
                    eprintln!(
                        "celeris-browser-launcher: action {verb:?} failed: session={} code=timeout launcher_reason=action_deadline",
                        entry.record.session_id
                    );
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
                Some(Some((state, facts))) => {
                    if state == SessionState::Failed {
                        remove_and_teardown(inner, &entry.record.session_id);
                    }
                    Response::Observed { state, facts }
                }
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
        Request::AuthBegin {
            session_id,
            lease_id,
            auth_section_id,
        } => {
            let entry = match authorize(inner, peer, &session_id, &lease_id) {
                Ok(e) => e,
                Err(code) => return err(code),
            };
            let e2 = entry.clone();
            let out = run_with_deadline(
                move || {
                    let mut g = lock(&e2.backend);
                    let session = g.as_mut().ok_or(ErrorCode::LeaseMismatch)?;
                    if !session.isolation_ok() {
                        return Err(ErrorCode::IsolationFailed);
                    }
                    session.auth_begin(&auth_section_id)
                },
                inner.cfg.limits.action,
                |_| {},
            );
            match out {
                Some(Ok(cdp_target_id)) => Response::AuthBegun { cdp_target_id },
                Some(Err(code)) => err(code),
                None => {
                    remove_and_teardown(inner, &entry.record.session_id);
                    err(ErrorCode::Timeout)
                }
            }
        }
        Request::FetchArtifact {
            session_id,
            lease_id,
            name,
            offset,
        } => {
            let entry = match authorize(inner, peer, &session_id, &lease_id) {
                Ok(e) => e,
                Err(code) => return err(code),
            };
            // Only a session whose policy allows the producing verb can hand its files out.
            match super::protocol::artifact_name_verb(&name) {
                Some(verb) if entry.policy.allowed_actions.contains(&verb) => {}
                _ => return err(ErrorCode::Unauthorized),
            }
            let e2 = entry.clone();
            let name2 = name.clone();
            let out = run_with_deadline(
                move || {
                    let mut g = lock(&e2.backend);
                    let session = g.as_mut().ok_or(ErrorCode::LeaseMismatch)?;
                    if !session.isolation_ok() {
                        return Err(ErrorCode::IsolationFailed);
                    }
                    session.fetch_artifact(&name2, offset)
                },
                inner.cfg.limits.action,
                |_| {},
            );
            match out {
                Some(Ok(chunk))
                    if chunk.data.len() <= super::protocol::ARTIFACT_CHUNK
                        && chunk.size <= super::protocol::MAX_ARTIFACT_BYTES =>
                {
                    use base64::Engine as _;
                    Response::Artifact {
                        name,
                        kind: chunk.kind,
                        size: chunk.size,
                        offset,
                        data: base64::engine::general_purpose::STANDARD.encode(&chunk.data),
                    }
                }
                Some(Ok(_)) => err(ErrorCode::Limit),
                Some(Err(code)) => err(code),
                None => {
                    remove_and_teardown(inner, &entry.record.session_id);
                    err(ErrorCode::Timeout)
                }
            }
        }
        // `serve_conn` routes `authenticate` (with its FD) to `handle_authenticate`.
        // `serve_conn` routes `authenticate` (with its FD) to `handle_authenticate` and
        // `live_start` to `serve_live`; `live_stop` belongs on a frame connection only.
        Request::Authenticate { .. } | Request::LiveStart { .. } | Request::LiveStop { .. } => {
            err(ErrorCode::BadRequest)
        }
    }
}

/// v4 `authenticate`: the request and the daemon's `injection.sock` connection. The answer is a
/// fixed status only; no credential material, CDP data or reason crosses back.
fn handle_authenticate(
    inner: &Arc<Inner>,
    args: AuthenticateArgs,
    fd: OwnedFd,
    peer: &Peer,
) -> Response {
    let rejected = Response::AuthenticateResult {
        status: AuthenticationStatus::Rejected,
        observation: None,
        held_reason: None,
        consent_pressed: false,
        consent_controls: Vec::new(),
    };
    let entry = match authorize(inner, peer, &args.session_id, &args.lease_id) {
        Ok(e) => e,
        Err(code) => return err(code),
    };
    let broker = match passed_broker_stream(fd, peer) {
        Ok(stream) => stream,
        Err(_) => return rejected,
    };
    // A v4-shaped request (no username field, no post-login read) gets the v4 answer without
    // `observation`, so a daemon that predates protocol 5 still decodes it while the launcher is
    // replaced first (ADR 2026-10-09 credential username / post-login D1-5).
    let v5 = args.username_selector.is_some() || args.post_login.is_some();
    let report_held_reason = args.report_held_reason;
    let report_consent = args.report_consent_controls;
    let e2 = entry.clone();
    let out = run_with_deadline(
        move || {
            let mut g = lock(&e2.backend);
            let session = g.as_mut().ok_or(ErrorCode::LeaseMismatch)?;
            if !session.isolation_ok() {
                return Err(ErrorCode::IsolationFailed);
            }
            session.authenticate(&args, broker)
        },
        inner.cfg.limits.action,
        |_| {},
    );
    match out {
        Some(Ok(result)) => Response::AuthenticateResult {
            status: AuthenticationStatus::Success,
            observation: v5.then_some(result.observation),
            held_reason: result.held_reason.filter(|_| report_held_reason),
            consent_pressed: report_consent && result.consent_pressed,
            consent_controls: if report_consent {
                result.consent_controls
            } else {
                Vec::new()
            },
        },
        Some(Err(_)) => rejected,
        None => {
            // A login that outlived the deadline leaves the session in an unknown H3 state.
            remove_and_teardown(inner, &entry.record.session_id);
            rejected
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
            // receipt の束縛に載せる userns owner（採れなければ None のまま。daemon が拒否する）。
            let (_, facts) = l.session.observe();
            Ok((l, facts.ns_owner_uid))
        },
        inner.cfg.limits.start,
        |late: Result<(Launched, Option<u32>), ErrorCode>| {
            if let Ok((l, _)) = late {
                l.session.stop();
            }
        },
    );
    let (launched, ns_owner_uid) = match out {
        Some(Ok(l)) => l,
        Some(Err(code)) => return err(code),
        None => return err(ErrorCode::Timeout),
    };
    let binding = SessionBinding {
        pid: launched.runtime_pid,
        starttime: launched.runtime_starttime,
        ns_owner_uid,
        ns_inodes: launched.ns_inodes,
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
        live_active: AtomicBool::new(false),
    });
    if inner.registry.write(&entry.record).is_err() {
        teardown(inner, &entry);
        return err(ErrorCode::LaunchFailed);
    }
    lock(&inner.state)
        .sessions
        .insert(session_id.clone(), entry.clone());
    let mut started = receipt(&entry.record, None, Outcome::Started, true);
    started.binding = Some(binding);
    Response::Started {
        session_id,
        instance_id: entry.record.instance_id.clone(),
        receipt: started,
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

/// v9 `live_start`: the session, its lease and its expiry, opened by the same daemon process
/// (`SO_PEERCRED` pid + starttime of the session's owner) on a connection other than the session's
/// control connection (frames never flow on the control connection).
fn authorize_live(
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
    if e.record.lease_id != lease_id || Instant::now() >= e.lease_deadline {
        return Err(ErrorCode::LeaseMismatch);
    }
    if e.owner_conn == peer.conn_id
        || e.record.peer_pid != peer.pid
        || e.record.peer_starttime != peer.starttime
    {
        return Err(ErrorCode::Unauthorized);
    }
    Ok(e.clone())
}

/// How long one wait for a frame lasts before the connection is checked for `live_stop`.
const LIVE_POLL: Duration = Duration::from_millis(200);

/// Whether `entry` is still the registered, unexpired session.
fn live_session_current(inner: &Arc<Inner>, entry: &Arc<Entry>) -> bool {
    Instant::now() < entry.lease_deadline
        && lock(&inner.state)
            .sessions
            .get(&entry.record.session_id)
            .is_some_and(|e| Arc::ptr_eq(e, entry))
}

/// Whether the peer wrote something (or closed) on the frame connection.
fn peer_readable(stream: &UnixStream) -> bool {
    let mut fd = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(&mut fd, 1, 0) > 0 }
}

/// v9: streams `live_frame` notifications of one session on its dedicated frame connection until
/// `live_stop`, disconnect, session end, lease expiry or shutdown. Each notification carries only
/// the session id, a sequence number, the dimensions, the encoding and the body length; the body
/// follows as one bounded binary frame. Any other request on this connection (there is no input
/// verb) ends the stream with `bad_request`. `Err` = refused before the stream started (nothing
/// was written; the caller answers with the code).
fn serve_live(
    inner: &Arc<Inner>,
    stream: &mut UnixStream,
    peer: &Peer,
    session_id: &str,
    lease_id: &str,
) -> Result<(), ErrorCode> {
    let max = inner.cfg.limits.max_frame;
    let entry = authorize_live(inner, peer, session_id, lease_id)?;
    if entry.live_active.swap(true, Ordering::SeqCst) {
        return Err(ErrorCode::Limit);
    }
    struct Active<'a>(&'a AtomicBool);
    impl Drop for Active<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _active = Active(&entry.live_active);
    let feed = {
        let mut g = lock(&entry.backend);
        match g.as_mut() {
            Some(s) => {
                if s.isolation_ok() {
                    s.live()
                } else {
                    Err(ErrorCode::IsolationFailed)
                }
            }
            None => Err(ErrorCode::LeaseMismatch),
        }
    };
    let mut feed = feed?;
    let sid = entry.record.session_id.clone();
    if write_message(
        stream,
        &Response::LiveStarted {
            session_id: sid.clone(),
            max_body: MAX_LIVE_BODY as u64,
        },
        max,
    )
    .is_err()
    {
        return Ok(());
    }
    let mut seq = 0u64;
    let stopped = Response::LiveStopped {
        session_id: sid.clone(),
    };
    while !inner.shutdown.load(Ordering::SeqCst) && live_session_current(inner, &entry) {
        match feed.next_frame(LIVE_POLL) {
            LiveNext::Frame(image) => {
                if image.body().len() > MAX_LIVE_BODY {
                    continue;
                }
                seq += 1;
                let meta = Response::LiveFrame {
                    session_id: sid.clone(),
                    seq,
                    width: image.width(),
                    height: image.height(),
                    encoding: image.encoding(),
                    body_len: image.body().len() as u64,
                };
                if write_message(stream, &meta, max).is_err()
                    || write_frame(stream, image.body(), MAX_LIVE_BODY).is_err()
                {
                    return Ok(());
                }
            }
            LiveNext::Idle => {}
            LiveNext::Ended => break,
        }
        if peer_readable(stream) {
            let Some((body, fds)) = read_request(stream, &inner.cfg.limits) else {
                return Ok(());
            };
            let resp = match decode_request(&body) {
                Ok(Request::LiveStop {
                    session_id,
                    lease_id,
                }) if fds.is_empty() && session_id == sid && lease_id == entry.record.lease_id => {
                    stopped.clone()
                }
                Ok(Request::LiveStop { .. }) => err(ErrorCode::LeaseMismatch),
                Ok(_) => err(ErrorCode::BadRequest),
                Err(code) => err(code),
            };
            drop(fds);
            let _ = write_message(stream, &resp, max);
            return Ok(());
        }
    }
    drop(feed);
    let _ = write_message(stream, &stopped, max);
    Ok(())
}

/// launcher 側での policy の再検査（verb と URL の origin）。
pub(super) fn action_allowed(policy: &SessionPolicy, verb: Verb, args: &ActionArgs) -> bool {
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
            // daemon が渡すのは task-core の正規 origin（`https://host[:port]`・loopback の
            // `http://…`）。scheme・host・port で照合する。`://` の無い旧来の素の host は従来どおり。
            crate::browser_policy::url_origin_allowed(u, &policy.allowed_domains)
                || policy
                    .allowed_domains
                    .iter()
                    .filter(|d| !d.contains("://"))
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
    #[cfg(test)]
    test_hook(inner, TestHook::TeardownBeforeRemove);
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

#[cfg(test)]
mod origin_tests {
    use super::*;

    fn open(policy: &SessionPolicy, url: &str) -> bool {
        action_allowed(
            policy,
            Verb::Open,
            &ActionArgs {
                url: Some(url.into()),
                ..ActionArgs::default()
            },
        )
    }

    /// daemon が渡す正規 origin は scheme・host・port で照合する（素の host 比較では全拒否だった）。
    #[test]
    fn open_matches_canonical_origins_by_scheme_host_and_port() {
        let policy = SessionPolicy {
            allowed_domains: vec![
                "https://example.com".into(),
                "http://127.0.0.1:17730".into(),
            ],
            allowed_actions: vec![Verb::Open],
            lease_seconds: 60,
        };
        assert!(open(&policy, "https://example.com/a"));
        assert!(open(&policy, "http://127.0.0.1:17730/"));
        assert!(!open(&policy, "http://example.com/a"));
        assert!(!open(&policy, "https://example.com:8443/"));
        assert!(!open(&policy, "https://sub.example.com/"));
        assert!(!open(&policy, "http://127.0.0.1:17731/"));
        assert!(!open(&policy, "https://evil.test/"));
    }

    #[test]
    fn open_keeps_bare_host_entries_as_before() {
        let policy = SessionPolicy {
            allowed_domains: vec!["example.com".into()],
            allowed_actions: vec![Verb::Open],
            lease_seconds: 60,
        };
        assert!(open(&policy, "https://example.com/a"));
        assert!(open(&policy, "https://docs.example.com/a"));
        assert!(!open(&policy, "https://evil.test/"));
    }
}
