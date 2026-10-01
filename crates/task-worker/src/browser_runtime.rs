//! ADR-0105（P4-A）: bubblewrap の isolated browser runtime を起動し、host 側から事実を採る。
//!
//! 同一 host UID の決定（p4a-uid）のもとで namespace・read-only root・書ける場所・CDP pipe・
//! orphan 回収を実装する。事実は既存の `verify_isolation` に渡し、弱めない。この host では
//! `SameUid` で attestation が出ないので、identity の復元は拒否のまま（ADR-0105 D2/D5）。

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::browser_relay::{self, CHANNEL_FD, CONNECT, GRANT, READY, REFUSE};

use nix::libc;
use task_core::browser_isolation::{
    CdpEndpoint, IsolationAttestation, IsolationViolation, Namespace, RuntimeFacts, SESSION_ROOT,
    StateRejected, verify_isolation,
};

/// sandbox の中の UID/GID（host から見た UID は変わらない。ADR-0105）。
pub const SANDBOX_UID: u32 = 1000;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("runtime io: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime did not report its child pid")]
    NoChildPid,
    #[error("runtime is not running")]
    NotRunning,
    #[error("runtime relay did not become ready")]
    RelayNotReady,
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

impl IsolatedRuntime {
    pub fn launch(spec: &RuntimeSpec) -> Result<Self, RuntimeError> {
        Self::launch_with(spec, true)
    }

    /// `arm_parent_death` が false なら bwrap に `PR_SET_PDEATHSIG` を掛けない。ADR-0108 D2 の
    /// 「発火を取りこぼした」場合（prctl 前の競合）を実プロセスで再現する試験のためだけに使う。
    pub fn launch_with(spec: &RuntimeSpec, arm_parent_death: bool) -> Result<Self, RuntimeError> {
        std::fs::create_dir_all(spec.session_dir.join("tmp"))?;
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
        rt.inner_pid = parse_child_pid(&info).ok_or(RuntimeError::NoChildPid)?;
        if let (Some(cfg), Some((ctrl, sandbox_end))) = (spec.egress.clone(), channel) {
            drop(sandbox_end);
            let stats = Arc::new(Mutex::new(EgressStats::default()));
            rt.egress_stats = Some(stats.clone());
            let (ready_tx, ready_rx) = mpsc::channel();
            // 長寿命の専用 thread（egress の PDEATHSIG は親 thread の終了で発火する。ADR-0108 D2）。
            std::thread::Builder::new()
                .name(format!("celeris-browser-rt-{}", spec.session_id))
                .spawn(move || relay_loop(ctrl, cfg, pgid, stats, ready_tx))?;
            // listener が立つまで browser は起動しない（sandboxd が READY の後に起動する）。
            if ready_rx.recv_timeout(Duration::from_secs(10)).is_err() {
                return Err(RuntimeError::RelayNotReady);
            }
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
    /// `verify_isolation` の attestation を要求する。
    #[default]
    Attested,
    /// 試験専用: 違反が `SameUid` だけの実 runtime を通す（別 UID の無い host で成功経路を実証する）。
    #[cfg(feature = "same-uid-harness")]
    SameUidHarness,
}

impl RestoreAdmission {
    pub fn admit(
        self,
        facts: &RuntimeFacts,
    ) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        match self {
            Self::Attested => verify_isolation(facts),
            #[cfg(feature = "same-uid-harness")]
            Self::SameUidHarness => match verify_isolation(facts) {
                Err(v) if v == [IsolationViolation::SameUid] => {
                    // 残りの検査は実の事実のまま。UID だけを別 UID として扱う。
                    let mut f = facts.clone();
                    f.runtime_uid = f.host_uid.wrapping_add(100_000).max(1);
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
                    spawn_egress(&cfg, pgid, &stats, &active)
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
) -> Option<std::os::unix::net::UnixStream> {
    use std::io::Write;
    let (proxy_end, sandbox_end) = std::os::unix::net::UnixStream::pair().ok()?;
    let fd = proxy_end.as_raw_fd();
    let mut cmd = Command::new(&cfg.proxy);
    cmd.env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: fork と exec の間は async-signal-safe な呼び出しだけ。fd は spawn まで親が保持する。
    unsafe {
        cmd.pre_exec(move || {
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
    let (stats, active) = (stats.clone(), active.clone());
    let waiter = std::thread::Builder::new()
        .name("celeris-browser-egress-wait".into())
        .spawn(move || {
            let _ = child.wait();
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
