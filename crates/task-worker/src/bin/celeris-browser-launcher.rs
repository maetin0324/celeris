//! Dedicated browser launcher service. The service manager starts this as celeris-browser.
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::libc;
use task_worker::browser_launcher::backend::{BackendConfig, RuntimeBackend};
use task_worker::browser_launcher::{LauncherLimits, LauncherServer, Registry, ServerConfig};

fn config_path() -> Result<PathBuf, String> {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next(), args.next()) {
        (Some(flag), Some(path), None) if flag == "--config" => Ok(PathBuf::from(path)),
        _ => Err("usage: celeris-browser-launcher --config <toml>".into()),
    }
}

fn load(path: &Path) -> Result<BackendConfig, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 || !meta.is_file() {
        return Err("launcher config must be a root-owned, non-writable regular file".into());
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cfg: BackendConfig = toml::from_str(&contents).map_err(|e| e.to_string())?;
    if cfg.allowed_uids.is_empty() || cfg.allowed_uids.contains(&unsafe { libc::geteuid() }) {
        return Err("allowed_uids must name a distinct daemon UID".into());
    }
    ensure_session_root(&cfg.state_dir, &cfg.session_root)?;
    for dir in [&cfg.state_dir, &cfg.session_root] {
        let meta = std::fs::metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if !dir.is_absolute()
            || !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
        {
            return Err(format!(
                "launcher directory must be private: {}",
                dir.display()
            ));
        }
    }
    for tool in [
        &cfg.bwrap,
        &cfg.sandboxd,
        &cfg.egress,
        &cfg.chrome,
        &cfg.agent_browser,
    ] {
        if !tool.is_absolute() || !tool.is_file() {
            return Err(format!("launcher executable missing: {}", tool.display()));
        }
    }
    Ok(cfg)
}

/// session_root が state_dir の直下にあり未作成なら 0700 で作る。
/// state_dir は systemd の StateDirectory が launcher の所有で用意する。
fn ensure_session_root(state_dir: &Path, session_root: &Path) -> Result<(), String> {
    if session_root.parent() != Some(state_dir) || session_root.symlink_metadata().is_ok() {
        return Ok(());
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(session_root)
        .map_err(|e| format!("{}: {e}", session_root.display()))
}

fn activation_listener() -> Result<Option<UnixListener>, String> {
    let fds = std::env::var("LISTEN_FDS").ok();
    let pid = std::env::var("LISTEN_PID").ok();
    match (fds, pid) {
        (None, None) => Ok(None),
        (Some(n), Some(p)) if n == "1" && p.parse::<u32>().ok() == Some(std::process::id()) => {
            // SAFETY: systemd socket activation transfers ownership of descriptor 3.
            let listener = unsafe { UnixListener::from_raw_fd(3 as RawFd) };
            Ok(Some(listener))
        }
        _ => Err("invalid systemd socket activation".into()),
    }
}

fn run() -> Result<(), String> {
    let path = config_path()?;
    let cfg = load(&path)?;
    let registry = Registry::open(
        cfg.state_dir.join("registry"),
        task_worker::browser_launcher::random_id().map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", cfg.state_dir.join("registry").display()))?;
    let server_cfg = ServerConfig {
        allowed_uids: cfg.allowed_uids.clone(),
        limits: LauncherLimits::default(),
    };
    let backend = Arc::new(RuntimeBackend::new(cfg.clone()));
    let server = match activation_listener()? {
        Some(listener) => LauncherServer::from_listener(listener, server_cfg, backend, registry),
        None => {
            let socket = cfg.socket.as_ref().ok_or("socket path missing")?;
            let server = LauncherServer::bind(socket, server_cfg, backend, registry)
                .map_err(|e| e.to_string())?;
            std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            server
        }
    };
    // Block before spawning server threads, so SIGTERM/SIGINT reaches sigwait
    // and every session is stopped through ServerHandle::shutdown.
    let mut signals = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
    unsafe {
        libc::sigemptyset(signals.as_mut_ptr());
        libc::sigaddset(signals.as_mut_ptr(), libc::SIGTERM);
        libc::sigaddset(signals.as_mut_ptr(), libc::SIGINT);
    }
    let signals = unsafe { signals.assume_init() };
    if unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut()) } != 0 {
        return Err("cannot block termination signals".into());
    }
    let mut handle = server.spawn().map_err(|e| e.to_string())?;
    let mut received = 0;
    if unsafe { libc::sigwait(&signals, &mut received) } != 0 {
        return Err("cannot wait for termination signal".into());
    }
    handle.shutdown();
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("celeris-browser-launcher: {e}");
        std::process::exit(1);
    }
}
