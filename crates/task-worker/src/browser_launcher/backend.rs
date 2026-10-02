//! Launcher-owned browser runtime. Paths come only from the root-owned service config.
use std::io::Write;
use std::net::IpAddr;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use nix::libc;
use serde::Deserialize;
use serde_json::json;

use super::protocol::{ActionArgs, ErrorCode, Observation, SessionFacts, SessionState, Verb};
use super::server::{BackendSession, Launched, SessionBackend, StartRequest};
use super::userns;
use crate::browser_cdp_sink::CdpController;
use crate::browser_runtime::{EgressRelay, RuntimeSpec, UsernsMode, listening_tcp};
use crate::browser_shared_cdp::SharedCdp;
use crate::browser_supervisor::{Supervisor, SupervisorOptions};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    pub socket: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub session_root: PathBuf,
    pub allowed_uids: Vec<u32>,
    pub bwrap: PathBuf,
    pub sandboxd: PathBuf,
    pub egress: PathBuf,
    pub chrome: PathBuf,
    pub agent_browser: PathBuf,
    pub resolver: IpAddr,
}

pub struct RuntimeBackend {
    cfg: BackendConfig,
}
impl RuntimeBackend {
    pub fn new(cfg: BackendConfig) -> Self {
        Self { cfg }
    }
}

/// 起動失敗の段と原因を launcher の stderr（journal）に出し、daemon には固定の code だけ返す。
fn fail<'a, E: std::fmt::Display>(
    session: &'a str,
    stage: &'static str,
    code: ErrorCode,
) -> impl FnOnce(E) -> ErrorCode + 'a {
    move |e| {
        eprintln!("celeris-browser-launcher: start {session}: {stage}: {e}");
        code
    }
}

fn write_shared(path: &Path, data: &[u8]) -> Result<(), ErrorCode> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(path)?;
        f.write_all(data)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o644))
    };
    write().map_err(|e| {
        eprintln!("celeris-browser-launcher: write {}: {e}", path.display());
        ErrorCode::LaunchFailed
    })
}

fn ns_owner(f: &std::fs::File) -> std::io::Result<u32> {
    let mut owner = 0u32;
    // NS_GET_OWNER_UID = _IO(0xb7, 0x4); Linux nsfs.h.
    if unsafe { libc::ioctl(f.as_raw_fd(), 0xb704, &mut owner) } < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(owner)
    }
}

/// Chrome の userns の owner（bwrap が内側 1000 = subuid として作った入れ子の userns）。
fn owner_uid(pid: i32) -> std::io::Result<u32> {
    ns_owner(&std::fs::File::open(format!("/proc/{pid}/ns/user"))?)
}

/// Chrome の userns の親（launcher が 2 map で作った userns）の owner。
fn parent_owner_uid(pid: i32) -> std::io::Result<u32> {
    use std::os::fd::FromRawFd;
    let f = std::fs::File::open(format!("/proc/{pid}/ns/user"))?;
    // NS_GET_PARENT = _IO(0xb7, 0x2)。成功時は親 namespace の新しい fd。
    let parent = unsafe { libc::ioctl(f.as_raw_fd(), 0xb702) };
    if parent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    ns_owner(&unsafe { std::fs::File::from_raw_fd(parent) })
}

/// launcher が作った userns（親）の owner が launcher、その中で bwrap が内側 1000（subuid）として
/// 作った userns（Chrome）の owner が subuid であること（ADR-0116 付記）。
fn owners_match(pid: i32, uid: u32, subuid: u32) -> Result<(), String> {
    let owner = owner_uid(pid).map_err(|e| format!("NS_GET_OWNER_UID: {e}"))?;
    let parent = parent_owner_uid(pid).map_err(|e| format!("NS_GET_PARENT owner: {e}"))?;
    if owner == subuid && parent == uid {
        Ok(())
    } else {
        Err(format!(
            "namespace owner {owner} (want {subuid}), parent owner {parent} (want {uid})"
        ))
    }
}

fn maps_match(pid: i32, uid: userns::Mapping, gid: userns::Mapping, forbidden: &[u32]) -> bool {
    let uid_map = std::fs::read_to_string(format!("/proc/{pid}/uid_map"));
    let gid_map = std::fs::read_to_string(format!("/proc/{pid}/gid_map"));
    let (Ok(u), Ok(g)) = (uid_map, gid_map) else {
        return false;
    };
    let normalize = |s: &str| -> Vec<Vec<u32>> {
        s.lines()
            .map(|l| {
                l.split_whitespace()
                    .filter_map(|x| x.parse().ok())
                    .collect()
            })
            .collect()
    };
    // launcher から見た入れ子の map: 内側 1000 → subuid / subgid の 1 行だけ。
    // launcher 自身（host_id）は Chrome の userns には map されない。
    let expected_u = vec![vec![1000, uid.sub_id, 1]];
    let expected_g = vec![vec![1000, gid.sub_id, 1]];
    normalize(&u) == expected_u
        && normalize(&g) == expected_g
        && forbidden
            .iter()
            .all(|id| ![uid.host_id, uid.sub_id, gid.host_id, gid.sub_id].contains(id))
}

impl SessionBackend for RuntimeBackend {
    fn start(&self, req: &StartRequest) -> Result<Launched, ErrorCode> {
        let cfg = &self.cfg;
        let sid = req.session_id.as_str();
        let ns = userns::create().map_err(fail(sid, "create userns", ErrorCode::LaunchFailed))?;
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let subuid = userns::subordinate_id(
            &std::fs::read_to_string("/etc/subuid").map_err(fail(
                sid,
                "read /etc/subuid",
                ErrorCode::LaunchFailed,
            ))?,
            "celeris-browser",
            uid,
        )
        .map_err(fail(sid, "subuid", ErrorCode::LaunchFailed))?;
        let subgid = userns::subordinate_id(
            &std::fs::read_to_string("/etc/subgid").map_err(fail(
                sid,
                "read /etc/subgid",
                ErrorCode::LaunchFailed,
            ))?,
            "celeris-browser",
            gid,
        )
        .map_err(fail(sid, "subgid", ErrorCode::LaunchFailed))?;
        if cfg
            .allowed_uids
            .iter()
            .any(|id| [uid, gid, subuid, subgid].contains(id))
        {
            eprintln!("celeris-browser-launcher: start {sid}: launcher ids overlap allowed_uids");
            return Err(ErrorCode::IsolationFailed);
        }
        let dir = cfg.session_root.join(&req.session_id);
        std::fs::create_dir(&dir).map_err(fail(
            sid,
            "create session dir",
            ErrorCode::LaunchFailed,
        ))?;
        let setup = (|| {
            for child in ["output", "home", "run", "actions", "profile"] {
                std::fs::create_dir(dir.join(child)).map_err(fail(
                    sid,
                    "create session subdir",
                    ErrorCode::LaunchFailed,
                ))?;
            }
            // The browser is host subuid: the spawn child switches to internal UID 1000 before bwrap.
            use std::os::unix::fs::PermissionsExt;
            // The parent session_root is 0700 and owned only by the dedicated
            // launcher UID. Within the bind, subuid S needs to create profile and
            // action files. Sticky session root protects B-owned configuration.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1777))
                .map_err(fail(sid, "chmod session dir", ErrorCode::LaunchFailed))?;
            for child in ["output", "home", "run", "actions", "profile"] {
                std::fs::set_permissions(dir.join(child), std::fs::Permissions::from_mode(0o777))
                    .map_err(fail(sid, "chmod session subdir", ErrorCode::LaunchFailed))?;
            }
            write_shared(
                &dir.join("browser_action.py"),
                include_str!("../browser_action.py").as_bytes(),
            )?;
            write_shared(
                &dir.join("upstream.json"),
                br#"{"idleTimeout":"5m","noWebmcp":true}"#,
            )?;
            let allowed: Vec<&str> = req
                .policy
                .allowed_actions
                .iter()
                .map(|v| action_name(*v))
                .collect();
            write_shared(
                &dir.join("policy.json"),
                &serde_json::to_vec(&json!({"allow":allowed}))
                    .map_err(|_| ErrorCode::LaunchFailed)?,
            )?;
            let token = format!(
                "{}{}",
                super::random_id().map_err(|_| ErrorCode::LaunchFailed)?,
                super::random_id().map_err(|_| ErrorCode::LaunchFailed)?
            );
            write_shared(
                &dir.join("action-config.json"),
                &serde_json::to_vec(&json!({
                    "executable":cfg.agent_browser, "session_id":req.session_id,
                    "allowed_domains":req.policy.allowed_domains,
                "browser_cache":cfg.chrome.parent().and_then(|p| p.parent()).and_then(|p| p.parent()),
                    "cdp_endpoint":format!("ws://127.0.0.1:9223/{token}")
                }))
                .map_err(|_| ErrorCode::LaunchFailed)?,
            )?;
            let egress_policy = task_core::browser_isolation::EgressPolicy {
                allow: req
                    .policy
                    .allowed_domains
                    .iter()
                    .map(|d| format!("{d}:443"))
                    .collect(),
                resolver: cfg.resolver,
                allow_ipv6: false,
            };
            let dirs = [&cfg.sandboxd, &cfg.chrome, &cfg.agent_browser];
            let mut ro_dirs: Vec<PathBuf> = dirs
                .iter()
                .filter_map(|p| p.parent().map(Path::to_path_buf))
                .collect();
            if let Some(install) = cfg.chrome.parent().and_then(|p| p.parent()) {
                ro_dirs.push(install.to_path_buf());
            }
            ro_dirs.sort();
            ro_dirs.dedup();
            let spec = RuntimeSpec {
                bwrap: cfg.bwrap.clone(),
                userns: UsernsMode::Fd(ns.as_raw_fd()),
                session_id: req.session_id.clone(),
                session_dir: dir.clone(),
                ro_dirs,
                argv: vec![
                    cfg.sandboxd.clone().into_os_string(),
                    "--shared-cdp".into(),
                    cfg.chrome.clone().into_os_string(),
                    "python3".into(),
                    "/session/browser_action.py".into(),
                ],
                cdp_pipe: true,
                egress: Some(EgressRelay {
                    proxy: cfg.egress.clone(),
                    policy: serde_json::to_vec(&egress_policy)
                        .map_err(|_| ErrorCode::LaunchFailed)?,
                    max_concurrent: crate::browser_runtime::DEFAULT_MAX_EGRESS,
                }),
            };
            let mut sup = Supervisor::start(
                spec,
                SupervisorOptions::new(cfg.state_dir.join("supervisor")),
            )
            .map_err(fail(sid, "start bwrap", ErrorCode::LaunchFailed))?;
            // bwrap has entered the namespace; releasing the holder cannot change its owner.
            drop(ns);
            let pid = sup.runtime_pid();
            owners_match(pid, uid, subuid).map_err(fail(
                sid,
                "namespace owner",
                ErrorCode::IsolationFailed,
            ))?;
            if !maps_match(
                pid,
                userns::Mapping {
                    host_id: uid,
                    sub_id: subuid,
                },
                userns::Mapping {
                    host_id: gid,
                    sub_id: subgid,
                },
                &cfg.allowed_uids,
            ) {
                eprintln!(
                    "celeris-browser-launcher: start {sid}: id map mismatch: uid_map={:?} gid_map={:?}",
                    std::fs::read_to_string(format!("/proc/{pid}/uid_map")).unwrap_or_default(),
                    std::fs::read_to_string(format!("/proc/{pid}/gid_map")).unwrap_or_default()
                );
                return Err(ErrorCode::IsolationFailed);
            }
            let (Some(write), Some(read)) = (sup.cdp_write.take(), sup.cdp_read.take()) else {
                eprintln!("celeris-browser-launcher: start {sid}: CDP pipe missing");
                return Err(ErrorCode::LaunchFailed);
            };
            let shared = SharedCdp::start_with_mode(
                CdpController::new(write, read),
                &dir.join("cdp-relay.sock"),
                token,
                req.policy.allowed_domains.clone(),
                0o666,
            )
            .map_err(fail(sid, "start CDP relay", ErrorCode::LaunchFailed))?;
            sup.attach_controller(shared.controller());
            let facts = crate::browser_runtime::collect_facts(
                &req.session_id,
                pid,
                sup.processes().first().map_or(0, |p| p.pid),
            )
            .map_err(fail(sid, "collect facts", ErrorCode::IsolationFailed))?;
            task_core::browser_isolation::verify_isolation(&facts).map_err(|v| {
                eprintln!("celeris-browser-launcher: start {sid}: verify_isolation: {v:?}");
                ErrorCode::IsolationFailed
            })?;
            let leader = sup
                .processes()
                .first()
                .ok_or(ErrorCode::LaunchFailed)?
                .clone();
            Ok(Launched {
                pid: leader.pid,
                pgid: leader.pid,
                starttime: leader.starttime,
                session: Box::new(RuntimeSession {
                    sup,
                    _shared: shared,
                    sequence: 0,
                    dir: dir.clone(),
                    uid,
                    gid,
                    subuid,
                    subgid,
                    forbidden: cfg.allowed_uids.clone(),
                }),
            })
        })();
        if setup.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        setup
    }
}

fn action_name(v: Verb) -> &'static str {
    match v {
        Verb::Open => "navigate",
        Verb::Click => "click",
        Verb::Snapshot => "snapshot",
        Verb::Extract => "gettext",
        Verb::Screenshot => "screenshot",
        Verb::Download => "download",
        Verb::Scroll => "scroll",
        Verb::Close => "close",
    }
}

struct RuntimeSession {
    sup: Supervisor,
    _shared: SharedCdp,
    sequence: u64,
    dir: PathBuf,
    uid: u32,
    gid: u32,
    subuid: u32,
    subgid: u32,
    forbidden: Vec<u32>,
}

impl RuntimeSession {
    fn facts(&self) -> Option<SessionFacts> {
        let pid = self.sup.runtime_pid();
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name).map(str::trim).map(str::to_owned))
        };
        Some(SessionFacts {
            host_uid: self.uid,
            host_gid: self.gid,
            uid_map: std::fs::read_to_string(format!("/proc/{pid}/uid_map")).ok()?,
            gid_map: std::fs::read_to_string(format!("/proc/{pid}/gid_map")).ok()?,
            ns_owner_uid: owner_uid(pid).ok(),
            cap_eff: field("CapEff:").unwrap_or_default(),
            no_new_privs: field("NoNewPrivs:").as_deref() == Some("1"),
            listen_count: listening_tcp(pid).ok()? as u32,
        })
    }
}

impl BackendSession for RuntimeSession {
    fn action(&mut self, verb: Verb, args: &ActionArgs) -> Result<Observation, ErrorCode> {
        if !self.isolation_ok() {
            return Err(ErrorCode::IsolationFailed);
        }
        let name = match verb {
            Verb::Open => "open",
            Verb::Click => "click",
            Verb::Snapshot => "snapshot",
            Verb::Extract => "extract",
            Verb::Screenshot => "screenshot",
            Verb::Download => "download",
            Verb::Scroll => "scroll",
            Verb::Close => "close",
        };
        let mut argv = Vec::new();
        match verb {
            Verb::Open => argv.push(args.url.clone().ok_or(ErrorCode::BadRequest)?),
            Verb::Click | Verb::Extract | Verb::Download => {
                argv.push(args.selector.clone().ok_or(ErrorCode::BadRequest)?)
            }
            Verb::Scroll => {
                argv.push(
                    if args.y.unwrap_or(0) < 0 {
                        "up"
                    } else {
                        "down"
                    }
                    .into(),
                );
                argv.push(args.y.unwrap_or(0).unsigned_abs().to_string());
            }
            _ => {}
        }
        let artifact = if matches!(verb, Verb::Screenshot | Verb::Download) {
            Some(format!(
                "{}-{}.{}",
                name,
                super::random_id().map_err(|_| ErrorCode::LaunchFailed)?,
                if verb == Verb::Screenshot {
                    "png"
                } else {
                    "bin"
                }
            ))
        } else {
            None
        };
        self.sequence += 1;
        let request = self
            .dir
            .join("actions")
            .join(format!("{:016x}.request", self.sequence));
        let result = request.with_extension("result");
        write_shared(
            &request,
            &serde_json::to_vec(&json!({"verb":name,"args":argv,"artifact":artifact}))
                .map_err(|_| ErrorCode::BadRequest)?,
        )?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(50);
        let body = loop {
            match std::fs::read(&result) {
                Ok(body) => break body,
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&request);
                    return Err(ErrorCode::Timeout);
                }
            }
        };
        let _ = std::fs::remove_file(result);
        let reply: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| ErrorCode::LaunchFailed)?;
        if reply["status"].as_i64() != Some(0) {
            return Err(ErrorCode::BadRequest);
        }
        let text = reply["stdout"]
            .as_str()
            .map(|s| s.chars().take(16000).collect());
        Ok(Observation { text, artifact })
    }
    fn observe(&mut self) -> (SessionState, SessionFacts) {
        match self.facts() {
            Some(f) => (SessionState::Running, f),
            None => (SessionState::Failed, SessionFacts::default()),
        }
    }
    fn isolation_ok(&mut self) -> bool {
        let pid = self.sup.runtime_pid();
        owners_match(pid, self.uid, self.subuid).is_ok()
            && maps_match(
                pid,
                userns::Mapping {
                    host_id: self.uid,
                    sub_id: self.subuid,
                },
                userns::Mapping {
                    host_id: self.gid,
                    sub_id: self.subgid,
                },
                &self.forbidden,
            )
            && crate::browser_runtime::collect_facts(
                self.sup.session_id(),
                pid,
                self.sup.processes().first().map_or(0, |p| p.pid),
            )
            .is_ok_and(|f| task_core::browser_isolation::verify_isolation(&f).is_ok())
    }
    fn stop(self: Box<Self>) {
        let RuntimeSession {
            sup, _shared, dir, ..
        } = *self;
        drop(_shared);
        sup.stop();
        let _ = std::fs::remove_dir_all(dir);
    }
}
