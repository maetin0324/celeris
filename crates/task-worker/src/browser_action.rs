//! ADR-0108 D4: host shim requests enter a private socket; only validated actions are
//! written into the isolated runtime's `/session` for sandboxd's action child.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::browser_live::{ControlGate, GatedOutcome, SessionCloser, run_gated};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionRequest {
    pub(crate) verb: String,
    pub(crate) args: Vec<String>,
    pub(crate) artifact: Option<String>,
}

/// 検査と gate を通った action を実際に出す先。既定は isolated runtime の `/session/actions`
/// への request file（[`FileExecutor`]）。ADR-0116 D5 の launcher 経路は launcher の client に頼む。
pub(crate) trait ActionExecutor: Send + Sync {
    fn run(&self, sequence: u64, req: &ActionRequest) -> std::io::Result<serde_json::Value>;
}

struct FileExecutor {
    root: PathBuf,
}

impl ActionExecutor for FileExecutor {
    fn run(&self, sequence: u64, req: &ActionRequest) -> std::io::Result<serde_json::Value> {
        run_action(&self.root, sequence, req)
    }
}

/// `sockaddr_un.sun_path` は NUL 込みで 108 byte。bind できる path はこの長さまで。
pub(crate) const SUN_PATH_MAX: usize = 107;
/// action socket を置く短い base（ADR 2026-10-06-browser-action-socket-path）。
const ACTION_SOCKET_BASE: &str = "/tmp";

/// shim と daemon の間の action socket の path。daemon 経路と launcher 経路で共通。
/// workspace の深さに依らない固定長（`/tmp/celeris-browser-<uid>/<hash 16 桁>.sock`）にし、
/// 上限を超えるなら bind の前に path と長さを含む誤りにする。
pub(crate) fn action_socket_path(session_dir: &Path) -> Result<PathBuf, crate::AdapterError> {
    action_socket_path_in(Path::new(ACTION_SOCKET_BASE), session_dir)
}

pub(crate) fn action_socket_path_in(
    base: &Path,
    session_dir: &Path,
) -> Result<PathBuf, crate::AdapterError> {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
    let uid = nix::unistd::geteuid().as_raw();
    let dir = base.join(format!("celeris-browser-{uid}"));
    let digest = Sha256::digest(session_dir.as_os_str().as_bytes());
    let socket = dir.join(format!("{:x}", digest)[..16].to_string() + ".sock");
    let len = socket.as_os_str().len();
    if len > SUN_PATH_MAX {
        return Err(crate::AdapterError::Other(format!(
            "browser action socket path is {len} bytes (unix socket limit {SUN_PATH_MAX}): {}",
            socket.display()
        )));
    }
    let unusable = |why: &str| {
        crate::AdapterError::Other(format!(
            "browser action socket dir {} is unusable: {why}",
            dir.display()
        ))
    };
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(unusable(&e.to_string())),
    }
    // 共有の /tmp なので、他人の dir・symlink・緩い mode は使わない。
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| unusable(&e.to_string()))?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(unusable("not a private directory owned by this user"));
    }
    // 前の run が残した同じ名前の socket は消す（名前は session dir ごとに決まる）。
    if let Ok(old) = std::fs::symlink_metadata(&socket) {
        if !old.file_type().is_socket() {
            return Err(unusable("socket name is taken by a non-socket"));
        }
        std::fs::remove_file(&socket).map_err(|e| unusable(&e.to_string()))?;
    }
    Ok(socket)
}

pub struct ActionServer {
    socket: PathBuf,
    /// この session で許す upstream action（serve と共有）。
    allowed: Arc<Mutex<Vec<String>>>,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ActionServer {
    pub fn start(
        socket: &Path,
        session: &Path,
        allowed_domains: Vec<String>,
        allowed: Vec<String>,
        single_use: Vec<String>,
        gate: Arc<dyn ControlGate>,
    ) -> std::io::Result<Self> {
        let executor = Arc::new(FileExecutor {
            root: session.to_path_buf(),
        });
        Self::start_with(socket, executor, allowed_domains, allowed, single_use, gate)
    }

    /// 同じ検査と gate で、action を `executor` に出す。`single_use` の upstream action
    /// （ADR 2026-10-08 D2: 人が一回だけ承認した click / download）は、browser に届いた最初の 1 回で
    /// allow から外れる。
    pub(crate) fn start_with(
        socket: &Path,
        executor: Arc<dyn ActionExecutor>,
        allowed_domains: Vec<String>,
        allowed: Vec<String>,
        single_use: Vec<String>,
        gate: Arc<dyn ControlGate>,
    ) -> std::io::Result<Self> {
        let socket = socket.to_path_buf();
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (stop, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(allowed));
        let allowed = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("celeris-browser-actions".into())
            .spawn(move || {
                let mut sequence = 0u64;
                let closed = AtomicBool::new(false);
                while rx.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            sequence += 1;
                            let ctx = Serve {
                                executor: executor.as_ref(),
                                domains: &allowed_domains,
                                actions: allowed.as_ref(),
                                single_use: &single_use,
                                gate: gate.as_ref(),
                                closed: &closed,
                            };
                            serve(stream, &ctx, sequence);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            socket,
            allowed: shared,
            stop,
            thread: Some(thread),
        })
    }

    /// ADR 2026-10-09 credential username / post-login D2-5: replace the allowed upstream actions
    /// (the post-login policy once the auth section closed). Takes effect for the next request.
    pub fn replace_allowed(&self, allowed: Vec<String>) {
        let mut actions = self.allowed.lock().unwrap_or_else(|e| e.into_inner());
        *actions = allowed;
    }
}

impl Drop for ActionServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// shim の verb に対応する upstream action 名。
fn upstream_action(verb: &str) -> Option<&'static str> {
    Some(match verb {
        "open" => "navigate",
        "click" => "click",
        "snapshot" => "snapshot",
        "extract" => "gettext",
        "screenshot" => "screenshot",
        "download" => "download",
        "scroll" => "scroll",
        "close" => "close",
        "__version__" => "launch",
        _ => return None,
    })
}

fn allowed(req: &ActionRequest, domains: &[String], actions: &[String]) -> bool {
    let Some(action) = upstream_action(&req.verb) else {
        return false;
    };
    if !actions.iter().any(|a| a == action)
        || req
            .args
            .iter()
            .any(|a| a.len() > 4096 || a.starts_with("--"))
    {
        return false;
    }
    match req.verb.as_str() {
        "open" => {
            let Some(url) = req.args.first().filter(|_| req.args.len() == 1) else {
                return false;
            };
            let Some(rest) = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
            else {
                return false;
            };
            !rest.contains('@')
                && !rest.contains('?')
                && !rest.contains('#')
                && !rest.contains('\\')
                && crate::browser_policy::url_origin_allowed(url, domains)
        }
        "click" | "extract" | "download" => {
            req.args.len() == 1
                && req.args[0].starts_with("@e")
                && req.args[0][2..].bytes().all(|b| b.is_ascii_digit())
        }
        "scroll" => {
            req.args.len() == 2
                && ["up", "down"].contains(&req.args[0].as_str())
                && req.args[1]
                    .parse::<u16>()
                    .is_ok_and(|v| (1..=2000).contains(&v))
        }
        _ => req.args.is_empty(),
    }
}

struct Serve<'a> {
    executor: &'a dyn ActionExecutor,
    domains: &'a [String],
    /// この session で許す upstream action。一回承認の action は最初の実行後に外れる。
    actions: &'a Mutex<Vec<String>>,
    single_use: &'a [String],
    gate: &'a dyn ControlGate,
    closed: &'a AtomicBool,
}

/// ADR-0114 D4: `Stopped` closes the session once through the upstream `close` action
/// (the same close the cancel path issues).
struct CloseOnce<'a> {
    executor: &'a dyn ActionExecutor,
    sequence: u64,
    closed: &'a AtomicBool,
}
impl SessionCloser for CloseOnce<'_> {
    fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            let close = ActionRequest {
                verb: "close".into(),
                args: Vec::new(),
                artifact: None,
            };
            let _ = self.executor.run(self.sequence, &close);
        }
    }
}

fn serve(mut stream: UnixStream, ctx: &Serve<'_>, sequence: u64) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut input = Vec::new();
    let valid = Read::by_ref(&mut stream)
        .take(16_385)
        .read_to_end(&mut input)
        .is_ok()
        && input.len() <= 16_384;
    let req = if valid {
        serde_json::from_slice::<ActionRequest>(&input).ok()
    } else {
        None
    };
    let failed = || serde_json::json!({"status":1,"stdout":""});
    // The allow list is held for the whole request so a single-use action cannot be issued
    // twice by concurrent shim calls.
    let mut actions = ctx.actions.lock().unwrap_or_else(|e| e.into_inner());
    let response = match req.filter(|r| allowed(r, ctx.domains, &actions)) {
        // The supervisor's own version probe is trusted and not an agent action.
        Some(req) if req.verb == "__version__" => ctx
            .executor
            .run(sequence, &req)
            .unwrap_or_else(|_| failed()),
        // ADR-0113 D1: agent actions reach the browser only while the store's control state
        // lets the agent act; otherwise nothing is written for the action child.
        Some(req) => {
            let closer = CloseOnce {
                executor: ctx.executor,
                sequence,
                closed: ctx.closed,
            };
            match run_gated(ctx.gate, &closer, || ctx.executor.run(sequence, &req)) {
                GatedOutcome::Ran(out) => {
                    // ADR 2026-10-08 D2: the approved action was handed to the browser once;
                    // whatever the outcome, the approval is spent.
                    if let Some(action) = upstream_action(&req.verb)
                        && ctx.single_use.iter().any(|a| a == action)
                    {
                        actions.retain(|a| a != action);
                    }
                    out.unwrap_or_else(|_| failed())
                }
                GatedOutcome::Blocked(_) | GatedOutcome::Closed => {
                    serde_json::json!({"status":3,"stdout":""})
                }
            }
        }
        None => serde_json::json!({"status":2,"stdout":""}),
    };
    drop(actions);
    let _ = stream.write_all(response.to_string().as_bytes());
}

fn run_action(
    root: &Path,
    sequence: u64,
    req: &ActionRequest,
) -> std::io::Result<serde_json::Value> {
    let actions = root.join("actions");
    let stem = format!("{sequence:016x}");
    let request = actions.join(format!("{stem}.request"));
    let result = actions.join(format!("{stem}.result"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&request)?;
    file.write_all(&serde_json::to_vec(req)?)?;
    drop(file);
    let deadline = Instant::now() + Duration::from_secs(50);
    loop {
        match std::fs::read(&result) {
            Ok(bytes) => {
                let _ = std::fs::remove_file(&result);
                return serde_json::from_slice(&bytes).map_err(std::io::Error::other);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => {
                let _ = std::fs::remove_file(&request);
                return Err(e);
            }
        }
    }
}
