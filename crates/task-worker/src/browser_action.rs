//! ADR-0108 D4: host shim requests enter a private socket; only validated actions are
//! written into the isolated runtime's `/session` for sandboxd's action child.
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::browser_live::{ControlGate, GatedOutcome, SessionCloser, run_gated};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActionRequest {
    verb: String,
    args: Vec<String>,
    artifact: Option<String>,
}

pub struct ActionServer {
    socket: PathBuf,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ActionServer {
    pub fn start(
        socket: &Path,
        session: &Path,
        allowed_domains: Vec<String>,
        allowed: Vec<String>,
        gate: Arc<dyn ControlGate>,
    ) -> std::io::Result<Self> {
        let socket = socket.to_path_buf();
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (stop, rx) = mpsc::channel();
        let root = session.to_path_buf();
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
                                root: &root,
                                domains: &allowed_domains,
                                actions: &allowed,
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
            stop,
            thread: Some(thread),
        })
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

fn allowed(req: &ActionRequest, domains: &[String], actions: &[String]) -> bool {
    let action = match req.verb.as_str() {
        "open" => "navigate",
        "click" => "click",
        "snapshot" => "snapshot",
        "extract" => "gettext",
        "screenshot" => "screenshot",
        "download" => "download",
        "scroll" => "scroll",
        "close" => "close",
        "__version__" => "launch",
        _ => return false,
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
            let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
            !host.is_empty()
                && !rest.contains('@')
                && !rest.contains('?')
                && !rest.contains('#')
                && domains.iter().any(|d| {
                    d.strip_prefix("*.")
                        .map_or(host == d, |base| host.ends_with(&format!(".{base}")))
                })
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
    root: &'a Path,
    domains: &'a [String],
    actions: &'a [String],
    gate: &'a dyn ControlGate,
    closed: &'a AtomicBool,
}

/// ADR-0114 D4: `Stopped` closes the session once through the upstream `close` action
/// (the same close the cancel path issues).
struct CloseOnce<'a> {
    root: &'a Path,
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
            let _ = run_action(self.root, self.sequence, &close);
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
    let response = match req.filter(|r| allowed(r, ctx.domains, ctx.actions)) {
        // The supervisor's own version probe is trusted and not an agent action.
        Some(req) if req.verb == "__version__" => {
            run_action(ctx.root, sequence, &req).unwrap_or_else(|_| failed())
        }
        // ADR-0113 D1: agent actions reach the browser only while the store's control state
        // lets the agent act; otherwise nothing is written for the action child.
        Some(req) => {
            let closer = CloseOnce {
                root: ctx.root,
                sequence,
                closed: ctx.closed,
            };
            match run_gated(ctx.gate, &closer, || run_action(ctx.root, sequence, &req)) {
                GatedOutcome::Ran(out) => out.unwrap_or_else(|_| failed()),
                GatedOutcome::Blocked(_) | GatedOutcome::Closed => {
                    serde_json::json!({"status":3,"stdout":""})
                }
            }
        }
        None => serde_json::json!({"status":2,"stdout":""}),
    };
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
