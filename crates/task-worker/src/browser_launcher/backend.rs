//! Launcher-owned browser runtime. Paths come only from the root-owned service config.
use std::collections::BTreeSet;
use std::io::Write;
use std::net::IpAddr;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use nix::libc;
use serde::Deserialize;
use serde_json::json;

use super::protocol::{
    ActionArgs, AuthenticateArgs, ErrorCode, LoginObservation, Observation, SessionFacts,
    SessionState, Verb,
};
use super::server::{BackendSession, Launched, SessionBackend, StartRequest};
use super::userns;
use crate::browser_cdp_sink::CdpController;
use crate::browser_cdp_sink::{BrokerClient, InjectionRequest, PassedInjectionStream};
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
    /// 試験専用 loopback 許可（`127.0.0.1:<port>` だけ。省略時は空 = off）。本番の path では有効に
    /// できない（[`refuse_test_loopback_in_production`]、ADR 2026-10-05-browser-department-web-live-view
    /// 付記 E1/E2）。
    #[serde(default)]
    pub test_loopback_allow: BTreeSet<String>,
}

/// 本番の launcher の固定 path（docs/ops/browser-launcher-host-setup.md）。
pub const PRODUCTION_CONFIG: &str = "/etc/celeris-browser/launcher.toml";
pub const PRODUCTION_SOCKET: &str = "/run/celeris-browser/launcher.sock";
pub const PRODUCTION_STATE_DIR: &str = "/var/lib/celeris-browser";

impl BackendConfig {
    /// `test_loopback_allow` の各項目が `127.0.0.1:<port>`（port は 10 進・先頭 0 なし・0/53/853 以外）
    /// であること。違えば config 読込みで拒否する。
    pub fn validate_test_loopback(&self) -> Result<(), String> {
        for entry in &self.test_loopback_allow {
            let port = entry
                .strip_prefix("127.0.0.1:")
                .filter(|p| {
                    !p.is_empty() && !p.starts_with('0') && p.bytes().all(|b| b.is_ascii_digit())
                })
                .and_then(|p| p.parse::<u16>().ok());
            if !matches!(port, Some(p) if !matches!(p, 0 | 53 | 853)) {
                return Err(format!(
                    "test_loopback_allow entry {entry:?} must be \"127.0.0.1:<port>\" (port 1-65535, not 53/853)"
                ));
            }
        }
        Ok(())
    }
}

/// path を正規化する（存在する祖先までを `canonicalize` し、残りを足す）。正規化できなければ `None`。
fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut rest = Vec::new();
    let mut cur = path;
    loop {
        match std::fs::canonicalize(cur) {
            Ok(base) => {
                let mut out = base;
                for c in rest.iter().rev() {
                    out.push(c);
                }
                return Some(out);
            }
            Err(_) => {
                let name = cur.file_name()?;
                if name == ".." {
                    return None;
                }
                rest.push(name.to_os_string());
                cur = cur.parent()?;
            }
        }
    }
}

/// 2 つの path が正規化後に一致するか。どちらかが正規化できなければ一致とみなす（fail closed）。
fn same_path(a: &Path, b: &Path) -> bool {
    match (normalize(a), normalize(b)) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    }
}

/// 付記 E2（launcher 側）: 試験許可が空でないのに、config path・socket（socket activation なら
/// `getsockname` の path）・state_dir のどれかが本番の固定値と一致したら拒否する。state_dir は本番の
/// 配下・祖先も一致とみなす。試験許可が空（既定）なら何も見ない。
pub fn refuse_test_loopback_in_production(
    config_path: &Path,
    cfg: &BackendConfig,
    activated_socket: Option<&Path>,
) -> Result<(), String> {
    if cfg.test_loopback_allow.is_empty() {
        return Ok(());
    }
    let refuse = |what: &str, p: &Path| {
        Err(format!(
            "test_loopback_allow is set but the {what} {} is the production one; refusing to start",
            p.display()
        ))
    };
    if same_path(config_path, Path::new(PRODUCTION_CONFIG)) {
        return refuse("config", config_path);
    }
    for socket in [cfg.socket.as_deref(), activated_socket]
        .into_iter()
        .flatten()
    {
        if same_path(socket, Path::new(PRODUCTION_SOCKET)) {
            return refuse("socket", socket);
        }
    }
    match (
        normalize(&cfg.state_dir),
        normalize(Path::new(PRODUCTION_STATE_DIR)),
    ) {
        (Some(a), Some(b)) if !a.starts_with(&b) && !b.starts_with(&a) => Ok(()),
        _ => refuse("state_dir", &cfg.state_dir),
    }
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

/// `launch` is agent-browser's CDP attach operation, not a launcher action verb.
/// Keep it in the private agent-browser policy even when no user action is allowed.
fn agent_browser_policy(policy: &super::protocol::SessionPolicy) -> serde_json::Value {
    let mut allow = vec!["launch"];
    for verb in &policy.allowed_actions {
        let name = action_name(*verb);
        if !allow.contains(&name) {
            allow.push(name);
        }
    }
    json!({ "allow": allow })
}

/// The launcher owns the session root, but Chrome creates nested files as its
/// subordinate UID. Remove those contents as that UID if an ordinary removal
/// cannot traverse them, then remove the launcher-owned directory itself.
fn remove_session_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => return Err(e),
        Err(_) => {}
    }
    use std::os::unix::fs::PermissionsExt;
    for child in ["output", "home", "run", "actions", "profile", "tmp"] {
        let path = dir.join(child);
        if path.is_dir() {
            // These immediate children belong to the launcher. In particular,
            // tmp is sticky while the browser runs; it can be relaxed after stop.
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o777))?;
        }
    }
    let mut ns = userns::create()?;
    ns.clear_session_contents(dir)?;
    std::fs::remove_dir_all(dir)
}

struct SessionDir(PathBuf);

impl std::ops::Deref for SessionDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for SessionDir {
    fn drop(&mut self) {
        if let Err(e) = remove_session_dir(&self.0) {
            eprintln!(
                "celeris-browser-launcher: remove session dir {}: {e}",
                self.0.display()
            );
        }
    }
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

/// Chrome の userns の owner（bwrap が内側 1000 = subuid として作った userns）。
fn owner_uid(pid: i32) -> std::io::Result<u32> {
    ns_owner(&std::fs::File::open(format!("/proc/{pid}/ns/user"))?)
}

/// Chrome の userns から launcher 自身の userns の直下までの owner の鎖（Chrome 側が先頭）。
/// bwrap は `--dev` の devpts のために内側 0 で userns を作り、そのあと内側 1000 へ map し直す
/// userns をもう 1 段作るので、鎖は `[S, S, B]` になる（段数に依らず検査する）。
fn owner_chain(pid: i32) -> std::io::Result<Vec<u32>> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    let own = std::fs::metadata("/proc/self/ns/user")?;
    let mut ns = std::fs::File::open(format!("/proc/{pid}/ns/user"))?;
    let mut chain = Vec::new();
    for _ in 0..MAX_USERNS_DEPTH {
        let meta = ns.metadata()?;
        if meta.ino() == own.ino() && meta.dev() == own.dev() {
            return Ok(chain);
        }
        chain.push(ns_owner(&ns)?);
        // NS_GET_PARENT = _IO(0xb7, 0x2)。成功時は親 namespace の新しい fd。
        let parent = unsafe { libc::ioctl(ns.as_raw_fd(), 0xb702) };
        if parent < 0 {
            return Err(std::io::Error::last_os_error());
        }
        ns = unsafe { std::fs::File::from_raw_fd(parent) };
    }
    Err(std::io::Error::other("userns chain too deep"))
}

/// 鎖の上限（Linux の userns の入れ子上限 32 に合わせる）。
const MAX_USERNS_DEPTH: usize = 33;

/// Chrome の userns の owner が subuid、launcher 自身の userns の直下（launcher が 2 map で作った
/// userns）の owner が launcher、その間の owner がすべて subuid か launcher で、daemon UID など
/// `forbidden` が鎖に現れないこと（ADR-0115 の脅威モデル、ADR-0116 付記）。
fn owners_match(pid: i32, uid: u32, subuid: u32, forbidden: &[u32]) -> Result<(), String> {
    let chain = owner_chain(pid).map_err(|e| format!("userns owner chain: {e}"))?;
    if chain_ok(&chain, uid, subuid, forbidden) {
        Ok(())
    } else {
        Err(format!(
            "userns owner chain {chain:?} (want [{subuid}, .., {uid}] without {forbidden:?})"
        ))
    }
}

fn chain_ok(chain: &[u32], uid: u32, subuid: u32, forbidden: &[u32]) -> bool {
    chain.len() >= 2
        && chain.first() == Some(&subuid)
        && chain.last() == Some(&uid)
        && chain.iter().all(|o| *o == subuid || *o == uid)
        && !chain.iter().any(|o| forbidden.contains(o))
}

/// session の egress policy。試験専用 loopback 許可は、session の許可 origin（`host:port`）と launcher
/// config の集合の交わりだけ（付記 E1）。
fn egress_policy(
    domains: &[String],
    resolver: IpAddr,
    test_loopback: &BTreeSet<String>,
) -> task_core::browser_isolation::EgressPolicy {
    let allow: BTreeSet<String> = domains
        .iter()
        .filter_map(|d| crate::browser_policy::origin_host_port(d))
        .map(|(host, port)| format!("{host}:{port}"))
        .collect();
    task_core::browser_isolation::EgressPolicy {
        test_loopback_allow: allow.intersection(test_loopback).cloned().collect(),
        allow,
        resolver,
        allow_ipv6: false,
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
    fn test_loopback_allow(&self) -> Vec<String> {
        self.cfg.test_loopback_allow.iter().cloned().collect()
    }

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
        let cleanup = SessionDir(dir.clone());
        (|| {
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
            write_shared(
                &dir.join("policy.json"),
                &serde_json::to_vec(&agent_browser_policy(&req.policy))
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
            let egress_policy = egress_policy(
                &req.policy.allowed_domains,
                cfg.resolver,
                &cfg.test_loopback_allow,
            );
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
            owners_match(pid, uid, subuid, &cfg.allowed_uids).map_err(fail(
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
            // receipt の束縛（protocol v3）: 検査した runtime process と、その namespace の inode。
            let runtime_starttime =
                crate::browser_runtime::process_starttime(pid).ok_or(ErrorCode::IsolationFailed)?;
            let ns_inodes = task_core::browser_isolation::collect_ns_inodes(&pid.to_string())
                .map_err(fail(sid, "namespace inodes", ErrorCode::IsolationFailed))?;
            Ok(Launched {
                pid: leader.pid,
                pgid: leader.pid,
                starttime: leader.starttime,
                runtime_pid: pid,
                runtime_starttime,
                ns_inodes,
                session: Box::new(RuntimeSession {
                    sup,
                    shared,
                    login: None,
                    after_login: None,
                    sequence: 0,
                    dir: cleanup,
                    uid,
                    gid,
                    subuid,
                    subgid,
                    forbidden: cfg.allowed_uids.clone(),
                }),
            })
        })()
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
    shared: SharedCdp,
    /// v4 の login 区間（`auth_begin` で作り、`authenticate` で一度だけ使う）。
    login: Option<LoginSection>,
    /// v5: the login's outcome for the agent's verbs (ADR 2026-10-09 credential username /
    /// post-login D2-6). `None` until a login ran in this session.
    after_login: Option<AfterLogin>,
    sequence: u64,
    dir: SessionDir,
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

/// What the agent may do after a login in this session (ADR 2026-10-09 credential username /
/// post-login D2-6). The controller's CDP gate is the enforcement; this refuses the verbs earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AfterLogin {
    /// Observation stays stopped until the session ends (ADR-0080 H3).
    Held,
    /// The auth section closed on the post-login conditions; these reading/acting verbs may run.
    Resumed(Vec<Verb>),
}

impl AfterLogin {
    fn from_login(observation: LoginObservation, args: &AuthenticateArgs) -> Self {
        use task_core::browser_wait::PostLoginAction as A;
        match (observation, &args.post_login) {
            (LoginObservation::Resumed, Some(post)) => Self::Resumed(
                post.actions
                    .iter()
                    .map(|a| match a {
                        A::Snapshot => Verb::Snapshot,
                        A::Extract => Verb::Extract,
                        A::Screenshot => Verb::Screenshot,
                        A::Download => Verb::Download,
                        A::Click => Verb::Click,
                    })
                    .collect(),
            ),
            _ => Self::Held,
        }
    }

    /// Whether `verb` may run in this session after the login.
    pub(crate) fn permits(&self, verb: Verb) -> bool {
        let reads = matches!(
            verb,
            Verb::Snapshot | Verb::Extract | Verb::Screenshot | Verb::Download | Verb::Click
        );
        match self {
            _ if !reads => true,
            Self::Held => false,
            Self::Resumed(verbs) => verbs.contains(&verb),
        }
    }
}

/// launcher の login 区間（ADR 2026-10-09 付記「launcher の Authenticate 経路」4）。
/// controller の auth section（agent の観測停止）は `begin_login` で開き、session の終わりまで
/// 閉じない（ADR-0080 H3、daemon 経路と同じ）。
pub(crate) struct LoginSection {
    auth_section_id: String,
    target_id: String,
    used: bool,
}

impl LoginSection {
    /// `auth_begin` が作った login 用 target の id（CDP の不透明な id）。
    pub(crate) fn target_id(&self) -> &str {
        &self.target_id
    }
}

/// `auth_begin`: agent の CDP command・event・新規接続を止めてから login 用の target を作る。
pub(crate) fn begin_login(
    controller: &std::sync::Arc<std::sync::Mutex<CdpController>>,
    auth_section_id: &str,
) -> Result<LoginSection, ErrorCode> {
    let mut c = controller.lock().map_err(|_| ErrorCode::LaunchFailed)?;
    c.open_auth_section(auth_section_id.to_owned());
    let target_id = c
        .controller_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .map_err(|_| ErrorCode::LaunchFailed)?["result"]["targetId"]
        .as_str()
        .map(str::to_owned)
        .ok_or(ErrorCode::LaunchFailed)?;
    Ok(LoginSection {
        auth_section_id: auth_section_id.to_owned(),
        target_id,
        used: false,
    })
}

/// `authenticate`: daemon 経路の `browser.rs::inject_h3` / `complete_trusted_login` と同じ順で、
/// trusted login の `login_url` へ navigate し、`origin` の top document に password 欄がちょうど
/// 1 個現れるまで待ち、credentiald（`broker`）経由で password を注入し、`submit_selector` が
/// あれば送信し、元の document が消えるまで待つ。成否に関わらず注入値を消す。v5 で policy が
/// username selector を持てば username と password を 1 回の注入で入れる。`post_login` が無ければ
/// controller の auth section は閉じない（session の終わりまで観測停止）。あれば ADR 2026-10-09
/// credential username / post-login D2-2 の条件が 15 秒以内に揃ったときだけ閉じる。返すのは
/// 成否と観測の再開の有無（固定値）だけ。
pub(crate) fn run_login(
    controller: &std::sync::Arc<std::sync::Mutex<CdpController>>,
    section: &mut LoginSection,
    args: &AuthenticateArgs,
    broker: &mut dyn BrokerClient,
) -> Result<LoginObservation, ErrorCode> {
    if section.used || section.auth_section_id != args.auth_section_id {
        return Err(ErrorCode::Unauthorized);
    }
    section.used = true;
    task_core::browser_wait::validate_trusted_login_full(
        &args.login_url,
        &args.origin,
        &args.password_selector,
        args.submit_selector.as_deref(),
        args.username_selector.as_deref(),
        args.post_login.as_ref(),
    )
    .map_err(|_| ErrorCode::BadRequest)?;
    let lock = || controller.lock().map_err(|_| ErrorCode::LaunchFailed);
    let result = (|| -> Result<(String, String), ErrorCode> {
        let own = {
            let mut c = lock()?;
            let own = c
                .controller_command(
                    "Target.attachToTarget",
                    json!({"targetId":section.target_id,"flatten":true}),
                    None,
                )
                .map_err(|_| ErrorCode::LaunchFailed)?["result"]["sessionId"]
                .as_str()
                .map(str::to_owned)
                .ok_or(ErrorCode::LaunchFailed)?;
            c.controller_command("Network.enable", json!({}), Some(&own))
                .map_err(|_| ErrorCode::LaunchFailed)?;
            c.begin_login_navigation(&own, std::time::Duration::from_secs(15))
                .map_err(|_| ErrorCode::LaunchFailed)?;
            let nav = c
                .controller_command("Page.navigate", json!({"url":args.login_url}), Some(&own))
                .map_err(|_| ErrorCode::LaunchFailed)?;
            if !nav["error"].is_null() || nav["result"]["errorText"].is_string() {
                return Err(ErrorCode::LaunchFailed);
            }
            own
        };
        let deadline = lock()?.login_deadline().ok_or(ErrorCode::LaunchFailed)?;
        let (frame_id, loader_id, redirect_chain) = loop {
            let ready = {
                let mut c = lock()?;
                match c
                    .login_password_document(
                        &own,
                        &args.origin,
                        &args.password_selector,
                        args.username_selector.as_deref(),
                    )
                    .map_err(|_| ErrorCode::Unauthorized)?
                {
                    Some((frame, loader)) => Some((
                        frame,
                        loader,
                        c.login_redirect_chain(&args.origin)
                            .map_err(|_| ErrorCode::Unauthorized)?,
                    )),
                    None => None,
                }
            };
            if let Some(document) = ready {
                break document;
            }
            if std::time::Instant::now() >= deadline {
                return Err(ErrorCode::Timeout);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let request = InjectionRequest {
            request_id: format!("launcher-{}", args.auth_section_id),
            session_id: args.session_id.clone(),
            cdp_target_id: section.target_id.clone(),
            frame_id,
            loader_id: loader_id.clone(),
            exact_origin: args.origin.clone(),
            redirect_chain,
            selector: args.password_selector.clone(),
            field: "password".into(),
            auth_section_id: args.auth_section_id.clone(),
            lease_id: args.credential_lease_id.clone(),
            username_selector: args.username_selector.clone(),
        };
        lock()?
            .inject(&request, &own, broker)
            .map_err(|_| ErrorCode::Unauthorized)?;
        if let Some(selector) = &args.submit_selector {
            let expr = format!(
                "(()=>{{let e=document.querySelector({});if(!e)return 'missing';if(e.form)e.form.requestSubmit();else e.click();return 'ok'}})()",
                serde_json::to_string(selector).map_err(|_| ErrorCode::BadRequest)?
            );
            let submitted = lock()?
                .controller_command(
                    "Runtime.evaluate",
                    json!({"expression":expr,"returnByValue":true}),
                    Some(&own),
                )
                .map_err(|_| ErrorCode::LaunchFailed)?;
            if submitted["result"]["result"]["value"] != "ok" {
                return Err(ErrorCode::LaunchFailed);
            }
            // requestSubmit navigates asynchronously; wait until the injected document is gone.
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::time::Instant::now() < until {
                let tree = lock()?.controller_command("Page.getFrameTree", json!({}), Some(&own));
                if tree.is_ok_and(|t| t["result"]["frameTree"]["frame"]["loaderId"] != loader_id) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Ok((own, loader_id))
    })();
    // Clear injected values on the same document while observation stays stopped.
    let cleared = lock().and_then(|mut c| {
        c.clear_injected_values()
            .map_err(|_| ErrorCode::LaunchFailed)
    });
    let (own, loader) = result?;
    cleared?;
    let Some(post) = &args.post_login else {
        return Ok(LoginObservation::Held);
    };
    Ok(
        if crate::browser_cdp_sink::resume_after_login_blocking(
            controller,
            &own,
            &loader,
            &post.read_origins,
            std::time::Duration::from_secs(15),
        ) {
            LoginObservation::Resumed
        } else {
            LoginObservation::Held
        },
    )
}

/// One login per launcher session (the H3 interval lasts until the session ends): a second
/// `auth_begin` is refused.
pub(crate) fn begin_login_once(
    controller: &std::sync::Arc<std::sync::Mutex<CdpController>>,
    slot: &mut Option<LoginSection>,
    auth_section_id: &str,
) -> Result<String, ErrorCode> {
    if slot.is_some() {
        return Err(ErrorCode::BadRequest);
    }
    let section = begin_login(controller, auth_section_id)?;
    let target = section.target_id().to_owned();
    *slot = Some(section);
    Ok(target)
}

impl BackendSession for RuntimeSession {
    fn auth_begin(&mut self, auth_section_id: &str) -> Result<String, ErrorCode> {
        if !self.isolation_ok() {
            return Err(ErrorCode::IsolationFailed);
        }
        begin_login_once(&self.shared.controller(), &mut self.login, auth_section_id)
    }

    fn authenticate(
        &mut self,
        args: &AuthenticateArgs,
        broker: std::os::unix::net::UnixStream,
    ) -> Result<LoginObservation, ErrorCode> {
        if args.session_id != self.sup.session_id() {
            return Err(ErrorCode::Unauthorized);
        }
        if !self.isolation_ok() {
            return Err(ErrorCode::IsolationFailed);
        }
        let controller = self.shared.controller();
        let section = self.login.as_mut().ok_or(ErrorCode::Unauthorized)?;
        // A login attempt (successful or not) fixes what the agent may still do in this session.
        self.after_login = Some(AfterLogin::Held);
        let observation = run_login(
            &controller,
            section,
            args,
            &mut PassedInjectionStream::new(broker),
        )?;
        self.after_login = Some(AfterLogin::from_login(observation, args));
        Ok(observation)
    }

    fn action(&mut self, verb: Verb, args: &ActionArgs) -> Result<Observation, ErrorCode> {
        if !self.isolation_ok() {
            return Err(ErrorCode::IsolationFailed);
        }
        if self.after_login.as_ref().is_some_and(|a| !a.permits(verb)) {
            return Err(ErrorCode::Unauthorized);
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
        owners_match(pid, self.uid, self.subuid, &self.forbidden).is_ok()
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
            sup, shared, dir, ..
        } = *self;
        drop(shared);
        sup.stop();
        drop(dir);
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod test_loopback_tests;

#[cfg(test)]
mod after_login_tests {
    use super::{AfterLogin, AuthenticateArgs, LoginObservation, Verb};
    use task_core::browser_wait::{PostLogin, PostLoginAction};

    fn args(post: Option<PostLogin>) -> AuthenticateArgs {
        AuthenticateArgs {
            session_id: "s".into(),
            lease_id: "l".into(),
            auth_section_id: "a".into(),
            credential_lease_id: "c".into(),
            origin: "https://idp.test".into(),
            login_url: "https://idp.test/login".into(),
            password_selector: "#p".into(),
            submit_selector: None,
            username_selector: Some("#u".into()),
            post_login: post,
        }
    }

    /// ADR 2026-10-09 credential username / post-login D2-6: after a login the launcher refuses the
    /// reading/acting verbs unless observation resumed and the verb is in the post-login actions.
    #[test]
    fn launcher_post_login_verbs_follow_the_login_outcome() {
        let post = PostLogin {
            read_origins: vec!["https://lms.test".into()],
            actions: vec![PostLoginAction::Snapshot, PostLoginAction::Click],
        };
        let resumed = AfterLogin::from_login(LoginObservation::Resumed, &args(Some(post.clone())));
        assert!(resumed.permits(Verb::Snapshot) && resumed.permits(Verb::Click));
        assert!(!resumed.permits(Verb::Extract) && !resumed.permits(Verb::Download));
        assert!(resumed.permits(Verb::Open) && resumed.permits(Verb::Scroll));
        for held in [
            AfterLogin::from_login(LoginObservation::Held, &args(Some(post))),
            AfterLogin::from_login(LoginObservation::Resumed, &args(None)),
        ] {
            assert_eq!(held, AfterLogin::Held);
            for v in [
                Verb::Snapshot,
                Verb::Extract,
                Verb::Screenshot,
                Verb::Download,
                Verb::Click,
            ] {
                assert!(!held.permits(v), "{v:?}");
            }
            assert!(held.permits(Verb::Open) && held.permits(Verb::Close));
        }
    }
}

#[cfg(test)]
mod chain_tests {
    use super::{chain_ok, egress_policy};

    #[test]
    fn launcher_egress_uses_origin_scheme_and_port() {
        let domains = [
            "https://example.com",
            "http://localhost",
            "http://127.0.0.1:8123",
            "https://billing.example.com:8443",
            "legacy.example.com",
        ]
        .map(str::to_owned);
        let policy = egress_policy(&domains, "127.0.0.53".parse().unwrap(), &Default::default());
        assert_eq!(
            policy.allow,
            [
                "example.com:443",
                "localhost:80",
                "127.0.0.1:8123",
                "billing.example.com:8443",
                "legacy.example.com:443",
            ]
            .map(str::to_owned)
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn owner_chain_accepts_nested_bwrap_levels_and_rejects_daemon_uid() {
        // launcher B=995、subuid S=296608、daemon 1001。
        assert!(chain_ok(&[296608, 995], 995, 296608, &[1001]));
        assert!(chain_ok(&[296608, 296608, 995], 995, 296608, &[1001]));
        assert!(!chain_ok(&[296608, 1001, 995], 995, 296608, &[1001]));
        assert!(!chain_ok(&[296608, 296608, 1001], 1001, 296608, &[1001]));
        assert!(!chain_ok(&[296608, 296608], 995, 296608, &[1001]));
        assert!(!chain_ok(&[995], 995, 296608, &[1001]));
        assert!(!chain_ok(&[296608, 4242, 995], 995, 296608, &[1001]));
    }
}
