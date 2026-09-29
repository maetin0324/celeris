//! ADR-0087（P4-A）: bubblewrap の isolated browser runtime を起動し、host 側から事実を採る。
//!
//! 同一 host UID の決定（p4a-uid）のもとで namespace・read-only root・書ける場所・CDP pipe・
//! orphan 回収を実装する。事実は既存の `verify_isolation` に渡し、弱めない。この host では
//! `SameUid` で attestation が出ないので、identity の復元は拒否のまま（ADR-0087 D2/D5）。

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nix::libc;
use task_core::browser_isolation::{
    CdpEndpoint, IsolationAttestation, IsolationViolation, Namespace, RuntimeFacts, SESSION_ROOT,
    verify_isolation,
};

/// sandbox の中の UID/GID（host から見た UID は変わらない。ADR-0087）。
pub const SANDBOX_UID: u32 = 1000;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime io: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime did not report its child pid")]
    NoChildPid,
    #[error("runtime is not running")]
    NotRunning,
}

/// 起動の指定。`argv` は sandbox の中で実行する（browser 本体、または試験の probe）。
#[derive(Debug, Clone)]
pub struct RuntimeSpec {
    pub bwrap: PathBuf,
    pub session_id: String,
    /// host 側の session dir。sandbox の `/session` にだけ書ける形で bind する。
    pub session_dir: PathBuf,
    /// read-only で同じ path に bind する dir（browser の install dir など）。
    pub ro_dirs: Vec<PathBuf>,
    pub argv: Vec<OsString>,
    /// true なら fd 3/4 を CDP pipe として渡す（`--remote-debugging-pipe`）。
    pub cdp_pipe: bool,
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

const ROOT_LINKS: [&str; 5] = ["bin", "lib", "lib32", "lib64", "sbin"];

/// bwrap の引数（`--info-fd` を除く）。I/O は `/` 直下の symlink 判定だけ。
pub fn bwrap_args(spec: &RuntimeSpec) -> Vec<OsString> {
    let mut a: Vec<OsString> = [
        "--unshare-user",
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
    .map(OsString::from)
    .collect();
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
    a.extend(["--proc", "/proc", "--dev", "/dev", "--bind"].map(OsString::from));
    a.push(spec.session_dir.clone().into_os_string());
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

/// 稼働中の runtime。drop で process group ごと SIGKILL する。
pub struct IsolatedRuntime {
    child: Child,
    session_id: String,
    /// sandbox の中の最初の process（host の pid）。
    inner_pid: i32,
    /// controller が持つ CDP pipe（書き側 = browser の fd 3、読み側 = fd 4）。
    pub cdp_write: Option<File>,
    pub cdp_read: Option<File>,
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

impl IsolatedRuntime {
    pub fn launch(spec: &RuntimeSpec) -> Result<Self, RuntimeError> {
        std::fs::create_dir_all(spec.session_dir.join("tmp"))?;
        let (info_r, info_w) = pipe()?;
        let (to_browser_r, to_browser_w) = pipe()?;
        let (from_browser_r, from_browser_w) = pipe()?;
        let info_fd = info_w.as_raw_fd();
        let (cdp_in, cdp_out) = (to_browser_r.as_raw_fd(), from_browser_w.as_raw_fd());
        let pipe_on = spec.cdp_pipe;
        let mut cmd = Command::new(&spec.bwrap);
        cmd.arg("--info-fd").arg("5").args(bwrap_args(spec));
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0);
        // SAFETY: fork と exec の間は async-signal-safe な呼び出しだけ。fd は親が spawn まで保持する。
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0
                    || libc::dup2(info_fd, 5) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if pipe_on && (libc::dup2(cdp_in, 3) < 0 || libc::dup2(cdp_out, 4) < 0) {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop((info_w, to_browser_r, from_browser_w));
        let mut info = String::new();
        File::from(info_r).take(4096).read_to_string(&mut info)?;
        let inner_pid = parse_child_pid(&info).ok_or(RuntimeError::NoChildPid)?;
        Ok(Self {
            child,
            session_id: spec.session_id.clone(),
            inner_pid,
            cdp_write: spec.cdp_pipe.then(|| File::from(to_browser_w)),
            cdp_read: spec.cdp_pipe.then(|| File::from(from_browser_r)),
        })
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

    /// 事実を採り直して検査する（ADR-0087 D5）。
    pub fn attest(&mut self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        match self.facts() {
            Ok(f) => verify_isolation(&f),
            Err(_) => Err(vec![IsolationViolation::NoProcessGroup]),
        }
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

/// identity 復元に渡す稼働中 session（ADR-0087 D5）。
pub struct LiveSession(pub std::sync::Mutex<IsolatedRuntime>);

impl task_core::browser_isolation::LiveIsolation for LiveSession {
    fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        match self.0.lock() {
            Ok(mut rt) => rt.attest(),
            Err(_) => Err(vec![IsolationViolation::NoProcessGroup]),
        }
    }
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
    // Uid: real effective saved fs。host から見た値（ADR-0087 D2）。
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

// ---- daemon 再起動後の回収（ADR-0087 D4）----

fn starttime(pid: i32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm は括弧つきで空白を含みうる。閉じ括弧の後の 20 番目が starttime（field 22）。
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// runtime を `dir/<session>.pid` に記録する。
pub fn record(dir: &Path, rt: &IsolatedRuntime) -> std::io::Result<()> {
    let pid = rt.bwrap_pid();
    let st = starttime(pid).ok_or_else(|| std::io::Error::other("no starttime"))?;
    std::fs::write(
        dir.join(format!("{}.pid", rt.session_id)),
        format!("{pid} {st}\n"),
    )
}

/// 記録された runtime のうち、pid と starttime が一致する process group を SIGKILL する。
/// 戻り値は殺した pgid。記録は全部消す。
pub fn reap_recorded(dir: &Path) -> std::io::Result<Vec<i32>> {
    let mut killed = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let path = e?.path();
        if path.extension().and_then(|x| x.to_str()) != Some("pid") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut it = text.split_whitespace();
        let pid: Option<i32> = it.next().and_then(|p| p.parse().ok());
        let st: Option<u64> = it.next().and_then(|p| p.parse().ok());
        if let (Some(pid), Some(st)) = (pid, st)
            && pid > 1
            && starttime(pid) == Some(st)
        {
            // SAFETY: 記録と starttime が一致した runtime の process group だけ。
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            killed.push(pid);
        }
        std::fs::remove_file(&path)?;
    }
    Ok(killed)
}
