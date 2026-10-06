//! ADR-0105（P4-A）: bubblewrap の isolated browser runtime を起動し、host 側から事実を採る。
//!
//! 同一 host UID の決定（p4a-uid）のもとで namespace・read-only root・書ける場所・CDP pipe・
//! orphan 回収を実装する。事実は既存の `verify_isolation` に渡し、弱めない。この host では
//! `SameUid` で attestation が出ないので、identity の復元は拒否のまま（ADR-0105 D2/D5）。

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::browser_relay::{self, CHANNEL_FD, CONNECT, GRANT, READY, REFUSE};

use nix::libc;
use task_core::browser_isolation::{
    CdpEndpoint, IsolationAttestation, IsolationViolation, LauncherObservation,
    LauncherSessionProof, Namespace, RuntimeFacts, SESSION_ROOT, StateRejected, verify_isolation,
    verify_launcher_session,
};

/// sandbox の中の UID/GID（host から見た UID は変わらない。ADR-0105）。
pub const SANDBOX_UID: u32 = 1000;

/// The existing daemon path creates its own user namespace. A launcher may supply
/// a namespace created under its dedicated host UID instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UsernsMode {
    #[default]
    Unshare,
    Fd(RawFd),
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime io: {0}")]
    Io(#[from] std::io::Error),
    /// bwrap が `--info-fd` を書かずに終わった。中身は bwrap の stderr の先頭（診断用）。
    #[error("runtime did not report its child pid: {0}")]
    NoChildPid(String),
    #[error("runtime is not running")]
    NotRunning,
    #[error("runtime relay did not become ready: {0}")]
    RelayNotReady(String),
    #[error("bubblewrap pid namespace init did not become ready")]
    InitNotReady,
}

/// ADR-0108 D1: sandbox の proxy listener から来た接続ごとに起動する celeris-browser-egress。
#[derive(Debug, Clone)]
pub struct EgressRelay {
    /// host の celeris-browser-egress（sandbox には入れない）。
    pub proxy: PathBuf,
    /// proxy の stdin に渡す `EgressPolicy` の JSON（ADR-0104 D7）。
    pub policy: Vec<u8>,
    /// 同時 egress 数の上限（ADR-0104 D4 の呼出し側の制限）。超過分は fd を返さず閉じる。
    pub max_concurrent: usize,
}

/// 同時 egress 数の既定（ADR-0108 D1）。
pub const DEFAULT_MAX_EGRESS: usize = 32;

const DENIAL_RECORD_LIMIT: usize = 128;
const DENIAL_LINE_LIMIT: usize = 512;

pub(crate) struct DenialRecorder {
    file: File,
    session_id: String,
    count: usize,
}

impl DenialRecorder {
    pub(crate) fn new(session_dir: &Path, session_id: &str) -> std::io::Result<Self> {
        // The session directory is writable by the browser UID. Claim the name
        // before starting it, and retain the descriptor so it cannot redirect writes.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(session_dir.join("egress-denied.jsonl"))?;
        Ok(Self {
            file,
            session_id: session_id.into(),
            count: 0,
        })
    }

    pub(crate) fn append(&mut self, denial: crate::browser_egress::Denial) -> std::io::Result<()> {
        const KINDS: &[&str] = &[
            "malformed",
            "not_allowed",
            "ip_literal",
            "private_address",
            "ipv6_disabled",
            "unresolved",
            "dns_bypass",
            "proxy_chain",
            "invalid_host",
        ];
        if !KINDS.contains(&denial.kind.as_str()) || self.count > DENIAL_RECORD_LIMIT {
            return Ok(());
        }
        if let Some(host) = &denial.host {
            if host.len() > 253
                || !host.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')
                })
                || denial.port.is_none()
            {
                return Ok(());
            }
        } else if denial.port.is_some() {
            return Ok(());
        }
        let at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(std::io::Error::other)?;
        let mut line = if self.count == DENIAL_RECORD_LIMIT {
            serde_json::json!({"kind":"truncated", "at":at, "session_id":self.session_id})
        } else {
            serde_json::json!({"kind":denial.kind, "host":denial.host, "port":denial.port, "at":at, "session_id":self.session_id})
        };
        if denial.host.is_none()
            && self.count < DENIAL_RECORD_LIMIT
            && let Some(fields) = line.as_object_mut()
        {
            fields.remove("host");
            fields.remove("port");
        }
        let mut bytes = serde_json::to_vec(&line).map_err(std::io::Error::other)?;
        if bytes.len() + 1 > DENIAL_LINE_LIMIT {
            return Ok(());
        }
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.count += 1;
        Ok(())
    }
}

/// 起動した egress の観測（pid は起動順）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressStats {
    pub spawned: Vec<i32>,
    pub exited: Vec<i32>,
    pub refused: usize,
}

/// 起動の指定。`argv` は sandbox の中で実行する（browser 本体、または試験の probe）。
#[derive(Debug, Clone)]
pub struct RuntimeSpec {
    pub bwrap: PathBuf,
    pub userns: UsernsMode,
    pub session_id: String,
    /// host 側の session dir。sandbox の `/session` にだけ書ける形で bind する。
    pub session_dir: PathBuf,
    /// read-only で同じ path に bind する dir（browser の install dir など）。
    pub ro_dirs: Vec<PathBuf>,
    pub argv: Vec<OsString>,
    /// true なら fd 3/4 を CDP pipe として渡す（`--remote-debugging-pipe`）。
    pub cdp_pipe: bool,
    /// `Some` なら argv の先頭は celeris-browser-sandboxd で、FD 6 に request channel を渡す。
    pub egress: Option<EgressRelay>,
}

/// `/etc` は丸ごと bind しない（`/etc/celeris` を含むので broker path の検査に掛かる）。
const ETC_FILES: [&str; 10] = [
    "/etc/fonts",
    "/etc/ssl",
    "/etc/ca-certificates",
    "/etc/passwd",
    "/etc/group",
    "/etc/nsswitch.conf",
    "/etc/localtime",
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/alternatives",
];

/// spawn の子が私有 mount ns に session dir を bind し直すための C 文字列（fork 前に用意）。
struct SessionMount {
    source: std::ffi::CString,
    base: std::ffi::CString,
    target: std::ffi::CString,
}

impl SessionMount {
    fn new(session_dir: &Path) -> std::io::Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let c = |b: &[u8]| {
            std::ffi::CString::new(b).map_err(|_| std::io::Error::other("path contains NUL"))
        };
        Ok(Self {
            source: c(session_dir.as_os_str().as_bytes())?,
            base: c(SESSION_MOUNT_BASE.as_bytes())?,
            target: c(SESSION_MOUNT.as_bytes())?,
        })
    }

    /// fork と exec の間で呼ぶ（syscall だけ）。新しい mount ns は launcher の userns の持ち物で、
    /// 親の mount は slave になる。念のため private にしてから `/tmp` に tmpfs（0755）を張り、
    /// その下の dir に session dir を bind する。launcher 本体の mount ns は変わらない。
    fn apply(&self) -> std::io::Result<()> {
        let err = || Err(std::io::Error::last_os_error());
        // SAFETY: 引数はすべて NUL 終端の C 文字列か null。
        unsafe {
            if libc::unshare(libc::CLONE_NEWNS) < 0 {
                return err();
            }
            if libc::mount(
                std::ptr::null(),
                c"/".as_ptr(),
                std::ptr::null(),
                libc::MS_REC | libc::MS_PRIVATE,
                std::ptr::null(),
            ) < 0
            {
                return err();
            }
            if libc::mount(
                c"tmpfs".as_ptr(),
                self.base.as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV,
                c"mode=0755,size=64k".as_ptr().cast(),
            ) < 0
            {
                return err();
            }
            if libc::mkdir(self.target.as_ptr(), 0o755) < 0 {
                return err();
            }
            if libc::mount(
                self.source.as_ptr(),
                self.target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND | libc::MS_REC,
                std::ptr::null(),
            ) < 0
            {
                return err();
            }
        }
        Ok(())
    }
}

/// Only diagnostics emitted by bwrap and sandboxd are eligible for the launcher journal.
/// A descendant can still write to the same pipe, so keep output bounded and reject other lines.
fn safe_startup_diagnostics(stderr: &str) -> String {
    let lines: Vec<_> = stderr
        .lines()
        .filter(|line| line.starts_with("bwrap: ") || line.starts_with("sandboxd: "))
        .map(|line| {
            line.chars()
                .filter(|c| !c.is_control())
                .take(240)
                .collect::<String>()
        })
        .collect();
    lines
        .iter()
        .rev()
        .take(4)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A running launcher reports only fixed Chrome lifecycle fields. Never forward
/// arbitrary descendant stderr, which could contain browser data.
fn chrome_lifecycle_diagnostic(line: &str) -> Option<String> {
    if let Some(pid) = line.strip_prefix("sandboxd: Chrome started pid=")
        && let Some(pid) = pid.strip_suffix(" flags=remote-debugging-pipe")
        && pid.parse::<u32>().is_ok()
    {
        return Some(format!(
            "Chrome started pid={pid} flags=remote-debugging-pipe"
        ));
    }
    if let Some(rest) = line.strip_prefix("sandboxd: Chrome exited code=")
        && let Some((code, signal)) = rest.split_once(" signal=")
        && [code, signal].iter().all(|value| {
            value == &"None"
                || value
                    .strip_prefix("Some(")
                    .and_then(|v| v.strip_suffix(')'))
                    .is_some_and(|v| v.parse::<i32>().is_ok())
        })
    {
        return Some(format!("Chrome exited code={code} signal={signal}"));
    }
    if let Some(category) = line.strip_prefix("sandboxd: Chrome stderr category=")
        && ["profile-lock", "permission-denied", "other-startup-error"].contains(&category)
    {
        return Some(format!("Chrome stderr category={category}"));
    }
    None
}

#[cfg(test)]
mod startup_diagnostics_tests {
    use super::{chrome_lifecycle_diagnostic, safe_startup_diagnostics};

    #[test]
    fn journal_diagnostics_exclude_browser_output_and_are_bounded() {
        let stderr = format!(
            "Chrome: secret URL\nsandboxd: proxy listen: EPERM\nbwrap: {}\n",
            "x".repeat(500)
        );
        let safe = safe_startup_diagnostics(&stderr);
        assert!(!safe.contains("secret URL"));
        assert!(safe.contains("sandboxd: proxy listen: EPERM"));
        assert!(safe.len() <= 280);
    }

    #[test]
    fn lifecycle_journal_excludes_unstructured_browser_output() {
        assert_eq!(
            chrome_lifecycle_diagnostic(
                "sandboxd: Chrome started pid=123 flags=remote-debugging-pipe"
            ),
            Some("Chrome started pid=123 flags=remote-debugging-pipe".into())
        );
        assert_eq!(
            chrome_lifecycle_diagnostic("sandboxd: Chrome exited code=Some(70) signal=None"),
            Some("Chrome exited code=Some(70) signal=None".into())
        );
        assert_eq!(
            chrome_lifecycle_diagnostic("sandboxd: Chrome stderr category=profile-lock"),
            Some("Chrome stderr category=profile-lock".into())
        );
        assert!(chrome_lifecycle_diagnostic("sandboxd: Chrome stderr category=/secret").is_none());
        assert!(chrome_lifecycle_diagnostic("sandboxd: Chrome started pid=1 secret=abc").is_none());
    }
}

const ROOT_LINKS: [&str; 5] = ["bin", "lib", "lib32", "lib64", "sbin"];

/// `UsernsMode::Fd` のとき spawn の子が私有の mount ns で session dir を bind する先。
/// bwrap は bind の source を realpath で辿る（`--bind-fd` の `/proc/self/fd/N` も実 path に
/// 展開して各段を lstat する）ため、内側 1000 が辿れない 0700 の session_root の下は使えない。
/// tmpfs（0755）の下に置き直し、bwrap にはこの path を渡す（ADR-0116 付記）。
const SESSION_MOUNT_BASE: &str = "/tmp";
const SESSION_MOUNT: &str = "/tmp/celeris-session";
/// sandbox の中の UID / GID（`--uid` / `--gid`、launcher の 2 map の内側 ID）。
const INNER_ID: u32 = 1000;

/// bwrap の引数（`--info-fd` を除く）。I/O は `/` 直下の symlink 判定だけ。
///
/// `UsernsMode::Fd` でも bwrap には `--userns` を渡さない。bwrap 0.11 は `--userns` のとき
/// setuid → setgid の順に切り替え、setuid で capability を失って setgid が EPERM になる。
/// 代わりに spawn の子が launcher の userns へ入って GID → UID の順に内側 1000 へ切り替え、
/// bwrap はそこから通常の `--unshare-user` で入れ子の userns を作る（ADR-0116 付記）。
/// 内側 1000 は session_root の私有 dir を辿れないので、session dir は子の私有 mount ns で
/// `SESSION_MOUNT` に bind し直してから渡す。
pub fn bwrap_args(spec: &RuntimeSpec) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["--unshare-user".into()];
    a.extend(
        [
            "--uid",
            "1000",
            "--gid",
            "1000",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup-try",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--hostname",
            "celeris-browser",
            "--tmpfs",
            "/",
            "--ro-bind",
            "/usr",
            "/usr",
        ]
        .iter()
        .map(OsString::from),
    );
    for l in ROOT_LINKS {
        let host = Path::new("/").join(l);
        if let Ok(t) = std::fs::read_link(&host) {
            a.extend([
                OsString::from("--symlink"),
                t.into_os_string(),
                host.into_os_string(),
            ]);
        } else if host.is_dir() {
            a.extend([
                OsString::from("--ro-bind"),
                host.clone().into_os_string(),
                host.into_os_string(),
            ]);
        }
    }
    for e in ETC_FILES {
        a.extend(["--ro-bind-try", e, e].map(OsString::from));
    }
    for d in &spec.ro_dirs {
        a.extend([
            OsString::from("--ro-bind"),
            d.clone().into_os_string(),
            d.clone().into_os_string(),
        ]);
    }
    a.extend(["--proc", "/proc", "--dev", "/dev"].map(OsString::from));
    match spec.userns {
        UsernsMode::Unshare => {
            a.push("--bind".into());
            a.push(spec.session_dir.clone().into_os_string());
        }
        UsernsMode::Fd(_) => {
            a.push("--bind".into());
            a.push(SESSION_MOUNT.into());
        }
    }
    a.push(SESSION_ROOT.into());
    for (k, v) in [
        ("HOME", SESSION_ROOT.to_owned()),
        ("TMPDIR", format!("{SESSION_ROOT}/tmp")),
        ("PATH", "/usr/bin:/bin".to_owned()),
    ] {
        a.extend([OsString::from("--setenv"), k.into(), v.into()]);
    }
    a.extend(["--remount-ro", "/", "--"].map(OsString::from));
    a.extend(spec.argv.iter().cloned());
    a
}

#[cfg(test)]
mod userns_args_tests {
    use super::*;

    #[test]
    fn launcher_tmp_is_writable_by_chrome_subuid() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let spec = RuntimeSpec {
            bwrap: "/usr/bin/bwrap".into(),
            userns: UsernsMode::Fd(11),
            session_id: "test".into(),
            session_dir: dir.path().into(),
            ro_dirs: Vec::new(),
            argv: vec!["/usr/bin/true".into()],
            cdp_pipe: false,
            egress: None,
        };
        prepare_session_tmp(&spec).unwrap();
        let mode = std::fs::metadata(dir.path().join("tmp"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o1777);
    }

    #[test]
    fn external_userns_binds_session_from_private_mount() {
        let mut spec = RuntimeSpec {
            bwrap: "/usr/bin/bwrap".into(),
            userns: UsernsMode::Unshare,
            session_id: "test".into(),
            session_dir: "/tmp/test-session".into(),
            ro_dirs: Vec::new(),
            argv: vec!["/usr/bin/true".into()],
            cdp_pipe: false,
            egress: None,
        };
        let old = bwrap_args(&spec);
        assert_eq!(old.first().and_then(|s| s.to_str()), Some("--unshare-user"));
        spec.userns = UsernsMode::Fd(11);
        let new = bwrap_args(&spec);
        assert!(!new.contains(&OsString::from("--userns")));
        let bind = |args: &[OsString], flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .map(|i| args[i + 1].clone())
        };
        assert_eq!(bind(&old, "--bind"), Some("/tmp/test-session".into()));
        assert_eq!(bind(&new, "--bind"), Some(SESSION_MOUNT.into()));
        assert!(!new.contains(&OsString::from("--bind-fd")));
        let strip = |args: &[OsString]| -> Vec<OsString> {
            args.iter()
                .filter(|a| {
                    !["/tmp/test-session", SESSION_MOUNT].contains(&a.to_str().unwrap_or(""))
                })
                .cloned()
                .collect()
        };
        assert_eq!(strip(&old), strip(&new));
    }
}

/// 稼働中の runtime。drop で process group ごと SIGKILL する。
pub struct IsolatedRuntime {
    child: Child,
    session_id: String,
    /// sandbox の中の最初の process（host の pid）。
    inner_pid: i32,
    /// controller が持つ CDP pipe（書き側 = browser の fd 3、読み側 = fd 4）。
    pub cdp_write: Option<File>,
    pub cdp_read: Option<File>,
    egress_stats: Option<Arc<Mutex<EgressStats>>>,
}

fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: fds は 2 要素。成功時の fd は新規で、ここで所有権を取る。
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: 直前の pipe2 が返した fd。
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn prepare_session_tmp(spec: &RuntimeSpec) -> std::io::Result<()> {
    let tmp = spec.session_dir.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    if matches!(spec.userns, UsernsMode::Fd(_)) {
        // The launcher (host UID B) creates this directory, while Chrome
        // runs as host subuid S. Chromium's ProcessSingleton creates its
        // socket under TMPDIR; a default 0755 directory makes that fail
        // with PROFILE_IN_USE (exit 21). The session mount is private.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o1777))?;
    }
    Ok(())
}

impl IsolatedRuntime {
    pub fn launch(spec: &RuntimeSpec) -> Result<Self, RuntimeError> {
        Self::launch_with(spec, true)
    }

    /// `arm_parent_death` が false なら bwrap に `PR_SET_PDEATHSIG` を掛けない。ADR-0108 D2 の
    /// 「発火を取りこぼした」場合（prctl 前の競合）を実プロセスで再現する試験のためだけに使う。
    pub fn launch_with(spec: &RuntimeSpec, arm_parent_death: bool) -> Result<Self, RuntimeError> {
        prepare_session_tmp(spec)?;
        let denial_recorder = spec
            .egress
            .as_ref()
            .map(|_| DenialRecorder::new(&spec.session_dir, &spec.session_id))
            .transpose()?;
        let (info_r, info_w) = pipe()?;
        let (to_browser_r, to_browser_w) = pipe()?;
        let (from_browser_r, from_browser_w) = pipe()?;
        let info_fd = info_w.as_raw_fd();
        let (cdp_in, cdp_out) = (to_browser_r.as_raw_fd(), from_browser_w.as_raw_fd());
        let pipe_on = spec.cdp_pipe;
        let channel = spec
            .egress
            .as_ref()
            .map(|_| browser_relay::seqpacket_pair())
            .transpose()?;
        let channel_fd = channel.as_ref().map(|(_, s)| s.as_raw_fd());
        let userns_fd = match spec.userns {
            UsernsMode::Unshare => None,
            UsernsMode::Fd(fd) => Some(fd),
        };
        // 内側 1000 からは session_root（launcher の 0700）を辿れない。子が私有の mount ns で
        // tmpfs の下に bind し直す。path は fork の前に C 文字列にしておく。
        let session_mount = match spec.userns {
            UsernsMode::Unshare => None,
            UsernsMode::Fd(_) => Some(SessionMount::new(&spec.session_dir)?),
        };
        // SAFETY: getpid は常に成功する。
        let parent = unsafe { libc::getpid() };
        let mut cmd = Command::new(&spec.bwrap);
        let mut args = bwrap_args(spec);
        if !arm_parent_death {
            // 取りこぼしの再現では bwrap 自身の PDEATHSIG（--die-with-parent）も掛けない。
            args.retain(|a| a != "--die-with-parent");
        }
        cmd.arg("--info-fd").arg("5").args(args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: fork と exec の間は async-signal-safe な呼び出しだけ。fd は親が spawn まで保持する。
        unsafe {
            cmd.pre_exec(move || {
                // The launcher blocks termination signals for sigwait; sandbox
                // children must receive normal SIGTERM during graceful teardown.
                let mut empty: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut empty);
                if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // launcher の userns へ入り、GID → UID の順に内側 1000 へ（UID が先だと
                // capability を失い setgid が EPERM）。資格の変更は PDEATHSIG を消すので prctl より前。
                if let Some(fd) = userns_fd {
                    if libc::setns(fd, libc::CLONE_NEWUSER) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    // まだ launcher の userns の 0（host の launcher UID）で capability がある間に。
                    if let Some(m) = &session_mount {
                        m.apply()?;
                    }
                    if libc::setresgid(INNER_ID, INNER_ID, INNER_ID) < 0
                        || libc::setresuid(INNER_ID, INNER_ID, INNER_ID) < 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if arm_parent_death && libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // prctl の前に親が死んでいれば PDEATHSIG は発火しない。ここで気付いて終わる（ADR-0108 D2）。
                if libc::getppid() != parent {
                    libc::_exit(127);
                }
                // 元の fd 番号が別の行き先（3〜6）と重なると、先の dup2 が後の元を潰す。
                // 先に全部を行き先の範囲より上へ退避してから並べる（退避側は CLOEXEC で残らない）。
                let lift = |fd: RawFd| -> std::io::Result<RawFd> {
                    let hi = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
                    if hi < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(hi)
                    }
                };
                let info = lift(info_fd)?;
                let cdp = if pipe_on {
                    Some((lift(cdp_in)?, lift(cdp_out)?))
                } else {
                    None
                };
                let chan = match channel_fd {
                    Some(fd) => Some(lift(fd)?),
                    None => None,
                };
                if libc::dup2(info, 5) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some((i, o)) = cdp
                    && (libc::dup2(i, 3) < 0 || libc::dup2(o, 4) < 0)
                {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(fd) = chan
                    && libc::dup2(fd, CHANNEL_FD) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop((info_w, to_browser_r, from_browser_w));
        let mut info = String::new();
        File::from(info_r).take(4096).read_to_string(&mut info)?;
        let pgid = child.id() as i32;
        let mut rt = Self {
            child,
            session_id: spec.session_id.clone(),
            inner_pid: 0,
            cdp_write: spec.cdp_pipe.then(|| File::from(to_browser_w)),
            cdp_read: spec.cdp_pipe.then(|| File::from(from_browser_r)),
            egress_stats: None,
        };
        rt.inner_pid = match parse_child_pid(&info) {
            Some(pid) => pid,
            None => return Err(RuntimeError::NoChildPid(rt.failed_stderr())),
        };
        #[cfg(feature = "attack-test-hooks")]
        test_hook::stop_init_after_info(rt.inner_pid);
        // --info-fd is written before bwrap releases its child from child_wait_fd. Killing the
        // controller immediately after that report can kill the outer monitor before pid 1
        // calls PR_SET_PDEATHSIG, leaving a live sandbox behind. bwrap's init enters do_wait
        // only after arming the signal; do not expose the runtime until then.
        // UsernsMode::Fd (launcher, ADR-0115/0116) also works: the launcher process is bwrap's
        // parent in the host pid ns, so --info-fd reports a host pid, and the launcher (euid
        // celeris-browser) owns the userns that is an ancestor of the init's userns, so it may
        // read /proc/<pid>/wchan and stat and may SIGKILL the init.
        let init_pid = rt.inner_pid;
        let init_starttime = process_starttime(init_pid);
        let init_ready = || {
            init_starttime.is_some_and(|st| same_process_alive(init_pid, st))
                && std::fs::read_to_string(format!("/proc/{init_pid}/wchan"))
                    .is_ok_and(|wchan| wchan.trim() == "do_wait")
        };
        #[cfg(feature = "attack-test-hooks")]
        test_hook::wait_until_init_resumed(init_pid);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && rt.is_running() {
            if init_ready() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !rt.is_running() || !init_ready() {
            // The pid namespace init may not have armed PDEATHSIG. Kill it explicitly while
            // its identity is still known; killing only the outer monitor can strand it.
            if init_starttime.is_some_and(|st| same_process_alive(init_pid, st)) {
                unsafe { libc::kill(init_pid, libc::SIGKILL) };
            }
            if rt.is_running() {
                rt.kill();
            }
            return Err(RuntimeError::InitNotReady);
        }
        if let (Some(cfg), Some((ctrl, sandbox_end))) = (spec.egress.clone(), channel) {
            drop(sandbox_end);
            let stats = Arc::new(Mutex::new(EgressStats::default()));
            rt.egress_stats = Some(stats.clone());
            let (ready_tx, ready_rx) = mpsc::channel();
            // 長寿命の専用 thread（egress の PDEATHSIG は親 thread の終了で発火する。ADR-0108 D2）。
            let recorder = Arc::new(Mutex::new(denial_recorder.expect("egress recorder")));
            std::thread::Builder::new()
                .name(format!("celeris-browser-rt-{}", spec.session_id))
                .spawn(move || relay_loop(ctrl, cfg, pgid, stats, recorder, ready_tx))?;
            // listener が立つまで browser は起動しない（sandboxd が READY の後に起動する）。
            if ready_rx.recv_timeout(Duration::from_secs(10)).is_err() {
                return Err(RuntimeError::RelayNotReady(rt.failed_stderr()));
            }
        }
        if matches!(spec.userns, UsernsMode::Fd(_))
            && let Some(stderr) = rt.child.stderr.take()
        {
            let session = spec.session_id.clone();
            std::thread::Builder::new()
                .name(format!("celeris-browser-diag-{session}"))
                .spawn(move || {
                    for line in BufReader::new(stderr.take(8192)).lines() {
                        let Ok(line) = line else { break };
                        if let Some(safe) = chrome_lifecycle_diagnostic(&line) {
                            eprintln!("celeris-browser-launcher: session {session}: {safe}");
                        }
                    }
                })?;
        }
        Ok(rt)
    }

    pub fn bwrap_pid(&self) -> i32 {
        self.child.id() as i32
    }
    pub fn inner_pid(&self) -> i32 {
        self.inner_pid
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// egress の起動・終了の観測（relay の無い runtime では `None`）。
    pub fn egress_stats(&self) -> Option<EgressStats> {
        self.egress_stats
            .as_ref()
            .and_then(|s| s.lock().ok().map(|s| s.clone()))
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 稼働中の process から事実を採る。
    pub fn facts(&mut self) -> Result<RuntimeFacts, RuntimeError> {
        if !self.is_running() {
            return Err(RuntimeError::NotRunning);
        }
        collect_facts(&self.session_id, self.inner_pid, self.bwrap_pid())
    }

    /// 事実を採り直して検査する（ADR-0105 D5）。
    pub fn attest(&mut self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        self.attest_with(RestoreAdmission::Attested)
    }

    /// 事実を採り直し、`admission` で検査する（ADR-0114 D2）。
    pub fn attest_with(
        &mut self,
        admission: RestoreAdmission,
    ) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        match self.facts() {
            Ok(f) => admission.admit(&f),
            Err(_) => Err(vec![IsolationViolation::NoProcessGroup]),
        }
    }

    /// 起動に失敗した bwrap の終了状態と、安全な診断行だけを返す。
    /// Chrome / action の stderr は sandboxd が破棄するため、機密を journal に流さない。
    fn failed_stderr(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && self.is_running() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.is_running() {
            // SAFETY: 未回収の子の process group にだけ送る。
            unsafe { libc::killpg(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.wait();
        }
        let status = self.child.try_wait().ok().flatten();
        let mut s = String::new();
        if let Some(e) = self.child.stderr.as_mut() {
            // A descendant may still hold stderr after bwrap exits. Never wait for EOF here.
            unsafe { libc::fcntl(e.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
            let _ = e.take(4096).read_to_string(&mut s);
        }
        format!(
            "bwrap status={status:?}; stderr={}",
            safe_startup_diagnostics(&s)
        )
    }

    /// stderr の先頭（診断用。起動失敗の試験で使う）。
    pub fn wait_stderr(mut self, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && self.is_running() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut s = String::new();
        if let Some(e) = self.child.stderr.as_mut() {
            let _ = e.take(8192).read_to_string(&mut s);
        }
        s
    }

    /// bwrap が終わったか（zombie を回収しない。回収前は pgid が再利用されないので signal を送れる）。
    pub(crate) fn exited_nowait(&self) -> bool {
        // SAFETY: siginfo は零初期化した出力領域。WNOWAIT なので子は zombie のまま残る。
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            let r = libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            r < 0 || info.si_pid() != 0
        }
    }

    /// 未回収の bwrap の process group に signal を送る（呼出し側は回収前であることを保証する）。
    pub(crate) fn signal_group(&self, sig: i32) {
        // SAFETY: bwrap を wait する前（zombie でも pgid は保持される）だけ呼ばれる。
        unsafe { libc::kill(-self.bwrap_pid(), sig) };
    }

    /// bwrap を回収する。以後この runtime の pid・pgid に signal を送らない。
    pub(crate) fn reap(&mut self) {
        let _ = self.child.wait();
    }

    pub fn kill(&mut self) {
        // SAFETY: 自分が起こした process group。wait 前なので pgid は再利用されていない。
        unsafe { libc::kill(-self.bwrap_pid(), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}

impl Drop for IsolatedRuntime {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.kill();
        }
    }
}

/// identity 復元に渡す稼働中 session（ADR-0105 D5）。
pub struct LiveSession(pub std::sync::Mutex<IsolatedRuntime>);

impl task_core::browser_isolation::LiveIsolation for LiveSession {
    fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        match self.0.lock() {
            Ok(mut rt) => rt.attest(),
            Err(_) => Err(vec![IsolationViolation::NoProcessGroup]),
        }
    }
}

/// 直に起動した runtime も隔離 session として登録できる（ADR-0108 D5）。CDP pipe を runtime が
/// 持っている間だけ、開封済み state を controller としてその pipe に投入する（ADR-0114 D1）。
impl task_core::browser_isolation::LiveSessionEntry for LiveSession {
    fn kind(&self) -> task_core::browser_isolation::RuntimeKind {
        task_core::browser_isolation::RuntimeKind::Isolated
    }

    fn accepts_state(&self) -> bool {
        self.0
            .lock()
            .map(|rt| rt.cdp_write.is_some() && rt.cdp_read.is_some())
            .unwrap_or(false)
    }

    fn deliver_state(&self, state: &[u8]) -> Result<(), StateRejected> {
        let rt = self.0.lock().map_err(|_| StateRejected)?;
        let (Some(w), Some(r)) = (rt.cdp_write.as_ref(), rt.cdp_read.as_ref()) else {
            return Err(StateRejected);
        };
        // lock を持つ間はこの pipe の読み手は自分だけ。応答を読み切ってから返す。
        let mut controller = crate::browser_cdp_sink::CdpController::new(
            w.try_clone().map_err(|_| StateRejected)?,
            r.try_clone().map_err(|_| StateRejected)?,
        );
        deliver_state_via(state, |method, params| {
            controller
                .controller_command(method, params, None)
                .map_err(|_| StateRejected)
        })
    }
}

/// identity 復元の隔離 admission（ADR-0114 D2）。production は [`RestoreAdmission::Attested`] だけ。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RestoreAdmission {
    /// `verify_isolation` の全条件に加え、launcher の session 証明（ADR-0138 D-L）を
    /// `verify_launcher_session` で検証できたときだけ attestation を出す。証明が無い runtime
    /// （daemon が直に起動した bwrap を含む）は owner 検査に通っても拒否する（fail closed）。
    #[default]
    Attested,
    /// 試験専用: 同一 UID の実 runtime を通す（別 UID の無い host で成功経路を実証する）。
    #[cfg(feature = "same-uid-harness")]
    SameUidHarness,
}

impl RestoreAdmission {
    /// launcher 証明なしで判定する。`Attested` は常に `LauncherProofMissing` を含めて拒否する。
    pub fn admit(
        self,
        facts: &RuntimeFacts,
    ) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        self.admit_launched(facts, None)
    }

    /// launcher の session 証明と daemon 自身の照合値を添えて判定する（ADR-0138 D-L）。
    /// `Attested` は `verify_launcher_session` を通ったときだけ attestation を返す。
    pub fn admit_launched(
        self,
        facts: &RuntimeFacts,
        launcher: Option<(&LauncherSessionProof, &LauncherObservation)>,
    ) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        match self {
            Self::Attested => match launcher {
                Some((proof, seen)) => verify_launcher_session(facts, Some(proof), seen)
                    .map(|att| att.isolation_attestation().clone()),
                None => {
                    let mut v = verify_isolation(facts).err().unwrap_or_default();
                    v.push(IsolationViolation::LauncherProofMissing);
                    Err(v)
                }
            },
            #[cfg(feature = "same-uid-harness")]
            Self::SameUidHarness => match verify_isolation(facts) {
                Err(v)
                    if v == [IsolationViolation::SameUid]
                        || v == [
                            IsolationViolation::SameUid,
                            IsolationViolation::UsernsOwnedByDaemon,
                        ] =>
                {
                    // 他の検査は実の事実のまま。同一 UID とその userns owner だけを
                    // 試験用の別 UID として扱い、本番の Attested は変えない。
                    let mut f = facts.clone();
                    f.runtime_uid = f.host_uid.wrapping_add(100_000).max(1);
                    if f.userns_owner_uid == Some(f.host_uid) {
                        f.userns_owner_uid = Some(f.runtime_uid);
                    }
                    verify_isolation(&f)
                }
                other => other,
            },
        }
    }
}

/// 開封済み state の 1 項目（`IdentityStatePlain` の JSON と同じ形）。値は drop で消す。
#[derive(serde::Deserialize)]
struct PlainEntry {
    origin: String,
    kind: String,
    name: String,
    value: String,
}
impl Drop for PlainEntry {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.name.zeroize();
        self.value.zeroize();
    }
}
#[derive(serde::Deserialize)]
struct PlainState {
    entries: Vec<PlainEntry>,
}

/// 開封済み state を controller の CDP（`Storage.setCookies`）へ投入する（ADR-0114 D1）。
/// `call` は controller の CDP 呼出しだけ。state は log・応答・argv・ファイルに出さない。
/// 知らない種別・https でない origin があれば 1 件も投入しない。
pub(crate) fn deliver_state_via(
    state: &[u8],
    mut call: impl FnMut(&str, serde_json::Value) -> Result<serde_json::Value, StateRejected>,
) -> Result<(), StateRejected> {
    let plain: PlainState = serde_json::from_slice(state).map_err(|_| StateRejected)?;
    if plain.entries.is_empty() {
        return Err(StateRejected);
    }
    let mut cookies = Vec::with_capacity(plain.entries.len());
    for e in &plain.entries {
        let url = url::Url::parse(&e.origin).map_err(|_| StateRejected)?;
        if e.kind != "cookie" || url.scheme() != "https" || url.host_str().is_none() {
            return Err(StateRejected);
        }
        cookies.push(serde_json::json!({
            "name": e.name,
            "value": e.value,
            "url": e.origin,
            "secure": true,
        }));
    }
    call(
        "Storage.setCookies",
        serde_json::json!({ "cookies": cookies }),
    )
    .map(|_| ())
}

/// sandboxd の要求ごとに egress を 1 本起動し、相手側の unix stream を返す（ADR-0108 D1）。
/// channel の EOF で終わる。
fn relay_loop(
    ctrl: OwnedFd,
    cfg: EgressRelay,
    pgid: i32,
    stats: Arc<Mutex<EgressStats>>,
    recorder: Arc<Mutex<DenialRecorder>>,
    ready: mpsc::Sender<()>,
) {
    let active = Arc::new(Mutex::new(0usize));
    while let Ok(Some((msg, fd))) = browser_relay::recv(ctrl.as_raw_fd()) {
        drop(fd);
        match msg {
            READY => {
                let _ = ready.send(());
            }
            CONNECT => {
                let busy = active
                    .lock()
                    .map(|a| *a >= cfg.max_concurrent)
                    .unwrap_or(true);
                let granted = if busy {
                    None
                } else {
                    spawn_egress(&cfg, pgid, &stats, &active, &recorder)
                };
                let sent = match &granted {
                    Some(end) => {
                        browser_relay::send(ctrl.as_raw_fd(), GRANT, Some(end.as_raw_fd()))
                    }
                    None => {
                        if let Ok(mut s) = stats.lock() {
                            s.refused += 1;
                        }
                        browser_relay::send(ctrl.as_raw_fd(), REFUSE, None)
                    }
                };
                if sent.is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
}

fn spawn_egress(
    cfg: &EgressRelay,
    pgid: i32,
    stats: &Arc<Mutex<EgressStats>>,
    active: &Arc<Mutex<usize>>,
    recorder: &Arc<Mutex<DenialRecorder>>,
) -> Option<std::os::unix::net::UnixStream> {
    use std::io::Write;
    let (proxy_end, sandbox_end) = std::os::unix::net::UnixStream::pair().ok()?;
    let fd = proxy_end.as_raw_fd();
    let mut cmd = Command::new(&cfg.proxy);
    cmd.env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: fork と exec の間は async-signal-safe な呼び出しだけ。fd は spawn まで親が保持する。
    unsafe {
        cmd.pre_exec(move || {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            if libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // runtime の process group に入れて一緒に回収する（ADR-0108 D2）。
            // 元がすでに fd 3 なら dup2 は何もせず CLOEXEC が残る（exec で閉じる）。その時は外す。
            let placed = if fd == 3 {
                libc::fcntl(3, libc::F_SETFD, 0)
            } else {
                libc::dup2(fd, 3)
            };
            if placed < 0 || libc::setpgid(0, pgid) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().ok()?;
    drop(proxy_end);
    let pid = child.id() as i32;
    let wrote = child
        .stdin
        .take()
        .map(|mut i| i.write_all(&cfg.policy).is_ok())
        .unwrap_or(false);
    if let Ok(mut s) = stats.lock() {
        s.spawned.push(pid);
    }
    if let Ok(mut a) = active.lock() {
        *a += 1;
    }
    let (stats, active, recorder) = (stats.clone(), active.clone(), recorder.clone());
    let stderr = child.stderr.take();
    let waiter = std::thread::Builder::new()
        .name("celeris-browser-egress-wait".into())
        .spawn(move || {
            let _ = child.wait();
            if let Some(stderr) = stderr {
                let mut line = String::new();
                let _ = BufReader::new(stderr.take(DENIAL_LINE_LIMIT as u64)).read_line(&mut line);
                if let Ok(denial) = serde_json::from_str::<crate::browser_egress::Denial>(&line)
                    && let Ok(mut recorder) = recorder.lock()
                {
                    let _ = recorder.append(denial);
                }
            }
            if let Ok(mut s) = stats.lock() {
                s.exited.push(pid);
            }
            if let Ok(mut a) = active.lock() {
                *a = a.saturating_sub(1);
            }
        });
    (wrote && waiter.is_ok()).then_some(sandbox_end)
}

fn parse_child_pid(info: &str) -> Option<i32> {
    let v: serde_json::Value = serde_json::from_str(info.trim()).ok()?;
    let pid = v.get("child-pid")?.as_i64()?;
    i32::try_from(pid).ok().filter(|p| *p > 1)
}

const NS: [(Namespace, &str); 6] = [
    (Namespace::User, "user"),
    (Namespace::Pid, "pid"),
    (Namespace::Net, "net"),
    (Namespace::Mount, "mnt"),
    (Namespace::Ipc, "ipc"),
    (Namespace::Uts, "uts"),
];

fn status_field(status: &str, key: &str) -> Option<String> {
    status.lines().find_map(|l| {
        l.strip_prefix(key)?
            .strip_prefix(':')
            .map(|v| v.trim().to_owned())
    })
}

/// `/proc/<pid>` から事実を採る。pgid は bwrap（runtime の process group）。
pub fn collect_facts(session_id: &str, pid: i32, pgid: i32) -> Result<RuntimeFacts, RuntimeError> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let mut namespaces = BTreeSet::new();
    for (ns, name) in NS {
        let mine = std::fs::read_link(format!("/proc/self/ns/{name}"))?;
        let theirs = std::fs::read_link(proc_dir.join("ns").join(name))?;
        if mine != theirs {
            namespaces.insert(ns);
        }
    }
    let status = std::fs::read_to_string(proc_dir.join("status"))?;
    // Uid: real effective saved fs。host から見た値（ADR-0105 D2）。
    let runtime_uid = status_field(&status, "Uid")
        .and_then(|v| v.split_whitespace().next().and_then(|u| u.parse().ok()))
        .unwrap_or(0);
    let no_new_privs = status_field(&status, "NoNewPrivs").as_deref() == Some("1");
    let zero = |k: &str| status_field(&status, k).is_some_and(|v| v.chars().all(|c| c == '0'));
    let capabilities_dropped = zero("CapEff") && zero("CapPrm");
    let mut root_readonly = false;
    let mut writable_mounts = Vec::new();
    let mut visible_paths = Vec::new();
    for line in BufReader::new(File::open(proc_dir.join("mountinfo"))?).lines() {
        let line = line?;
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let f: Vec<&str> = pre.split_whitespace().collect();
        let fstype = post.split_whitespace().next().unwrap_or("");
        let (Some(mp), Some(opts)) = (f.get(4), f.get(5)) else {
            continue;
        };
        let ro = opts.split(',').any(|o| o == "ro");
        visible_paths.push((*mp).to_owned());
        if *mp == "/" {
            root_readonly = ro;
        }
        let pseudo = fstype == "proc" || *mp == "/dev" || mp.starts_with("/dev/");
        if !ro && !pseudo {
            writable_mounts.push((*mp).to_owned());
        }
    }
    Ok(RuntimeFacts {
        session_id: session_id.to_owned(),
        host_uid: nix::unistd::getuid().as_raw(),
        runtime_uid,
        userns_owner_uid: task_core::browser_isolation::collect_userns_owner_uid(pid),
        namespaces,
        root_readonly,
        writable_mounts,
        visible_paths,
        cdp: CdpEndpoint::Pipe,
        no_new_privs,
        capabilities_dropped,
        pgid,
    })
}

/// netns の中で LISTEN している TCP socket の数（`/proc/<pid>/net/tcp{,6}`）。
pub fn listening_tcp(pid: i32) -> std::io::Result<usize> {
    let mut n = 0;
    for f in ["tcp", "tcp6"] {
        let s = std::fs::read_to_string(format!("/proc/{pid}/net/{f}"))?;
        n += s
            .lines()
            .skip(1)
            .filter(|l| l.split_whitespace().nth(3) == Some("0A"))
            .count();
    }
    Ok(n)
}

// ---- daemon 再起動後の回収（ADR-0105 D4）----

fn starttime(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm は括弧つきで空白を含みうる。閉じ括弧の後の 20 番目が starttime（field 22）。
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// 記録の 1 行（ADR-0108 D2）。`role` は診断用（bwrap / sandboxd / browser / egress / child）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedProcess {
    pub pid: i32,
    pub starttime: u64,
    pub role: String,
}

/// Test-only (ADR-0125 §5): launch の競合点（bwrap が `--info-fd` で init の PID を報告した直後、
/// init が親死亡シグナルを設定する前）を SIGSTOP で固定する stutter フック。
/// `attack-test-hooks` を入れた試験 binary でだけコンパイルされ、env はそのフックの引数に過ぎない。
/// 本番の ready 判定・PDEATHSIG・kill / reap の意味は変えない。
#[cfg(feature = "attack-test-hooks")]
pub mod test_hook {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use nix::libc;

    /// 段階の印（`launch-info` / `launch-waiting`）を書く dir。
    pub const LAUNCH_HOOK_DIR_ENV: &str = "CELERIS_TEST_RT_LAUNCH_HOOK_DIR";

    fn dir() -> Option<PathBuf> {
        std::env::var_os(LAUNCH_HOOK_DIR_ENV).map(PathBuf::from)
    }

    fn mark(dir: &Path, stage: &str, pid: i32) {
        let path = dir.join(format!("launch-{stage}"));
        let tmp = dir.join(format!(".launch-{stage}.tmp"));
        if std::fs::write(&tmp, format!("{pid}\nEND\n")).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }

    fn stopped(pid: i32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rfind(')')
                .and_then(|i| stat[i + 1..].split_whitespace().next())
                .is_some_and(|state| state == "T")
        })
    }

    /// info-fd の報告を読んだ直後に init を止め、`launch-info` に PID を書く。
    pub(super) fn stop_init_after_info(init_pid: i32) {
        let Some(dir) = dir() else { return };
        // SAFETY: 直前に bwrap が報告した自分の runtime の init だけに送る。
        unsafe { libc::kill(init_pid, libc::SIGSTOP) };
        mark(&dir, "info", init_pid);
    }

    /// launch が ready 待ちに入ったことを `launch-waiting` で知らせ、試験が init を SIGCONT
    /// するまで待つ（ready 待ちの上限は再開後から数える）。60 秒は壊れたときの保険。
    pub(super) fn wait_until_init_resumed(init_pid: i32) {
        let Some(dir) = dir() else { return };
        mark(&dir, "waiting", init_pid);
        let deadline = Instant::now() + Duration::from_secs(60);
        while stopped(init_pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// `pid` の process が記録した本人（starttime 一致）で、まだ終わっていない（zombie でない）か。
pub fn same_process_alive(pid: i32, starttime_expected: u64) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
        return false;
    };
    let mut f = rest.split_whitespace();
    let state = f.next();
    let st: Option<u64> = f.nth(18).and_then(|v| v.parse().ok());
    st == Some(starttime_expected) && !matches!(state, Some("Z") | Some("X"))
}

/// `pid` の starttime（`/proc/<pid>/stat` の field 22）。
pub fn process_starttime(pid: i32) -> Option<u64> {
    starttime(pid)
}

/// runtime の process を `dir/<session>.pid` に tmp+rename で書く（1 行目が bwrap = pgid）。
pub fn write_record(
    dir: &Path,
    session_id: &str,
    procs: &[RecordedProcess],
) -> std::io::Result<()> {
    let body: String = procs
        .iter()
        .map(|p| format!("{} {} {}\n", p.pid, p.starttime, p.role))
        .collect();
    let path = dir.join(format!("{session_id}.pid"));
    let tmp = dir.join(format!(".{session_id}.pid.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, path)
}

/// 記録を読む。壊れた行は捨てる。
pub fn read_record(path: &Path) -> Vec<RecordedProcess> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let starttime = it.next()?.parse().ok()?;
            let role = it
                .next()
                .map(str::to_owned)
                .unwrap_or_else(|| if i == 0 { "bwrap" } else { "child" }.to_owned());
            Some(RecordedProcess {
                pid,
                starttime,
                role,
            })
        })
        .collect()
}

/// runtime を `dir/<session>.pid` に記録する（bwrap だけ）。
pub fn record(dir: &Path, rt: &IsolatedRuntime) -> std::io::Result<()> {
    let pid = rt.bwrap_pid();
    let st = starttime(pid).ok_or_else(|| std::io::Error::other("no starttime"))?;
    write_record(
        dir,
        &rt.session_id,
        &[RecordedProcess {
            pid,
            starttime: st,
            role: "bwrap".into(),
        }],
    )
}

/// 記録された process のうち pid と starttime が一致するものだけを SIGKILL する（bwrap は
/// process group ごと）。starttime が違う pid（再利用）・終わった pid には何も送らない。
/// 戻り値は signal を送った pid。記録は全部消す。
pub fn reap_recorded(dir: &Path) -> std::io::Result<Vec<i32>> {
    let mut killed = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let path = e?.path();
        if path.extension().and_then(|x| x.to_str()) != Some("pid") {
            continue;
        }
        for p in read_record(&path) {
            if p.pid > 1 && same_process_alive(p.pid, p.starttime) {
                let target = if p.role == "bwrap" { -p.pid } else { p.pid };
                // SAFETY: 直前に starttime が記録と一致した本人（bwrap は自分の pgid の leader）。
                unsafe { libc::kill(target, libc::SIGKILL) };
                killed.push(p.pid);
            }
        }
        std::fs::remove_file(&path)?;
    }
    Ok(killed)
}
