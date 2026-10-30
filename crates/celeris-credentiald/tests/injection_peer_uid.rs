//! A13（ADR-0109 表・ADR-0138「検証と本番反映」）: 別 host UID の**実 process** から broker の
//! `injection.sock` へ接続し、`peer_uid_mismatch` で拒否されることを、ptrace 拒否
//! （`crates/task-worker/tests/browser_launcher_ptrace.rs`）とは別の判定として記録する。
//! 設計は `agent-docs/adr/2026-10-11-browser-a13-broker-peer-uid-real-process.md`。
//!
//! 別 UID の process は launcher の UID（`celeris-browser`、設定 `[browser].launcher_uid`）で
//! `sudo -n -u` により起こす。`unshare -r` の子は host では同 UID なので別 UID の証拠にしない。
//! sudo が使えない・user が無い環境では理由を出して skip する（黙って pass しない）。
//! `CELERIS_LAUNCHER_TESTS=require` では skip せず失敗する。既定では ADR-0126 B の opt-in
//! （`CELERIS_USERNS_TESTS=1`）が無いと走らない（sudo の試行を CI の既定に入れないため）。
//!
//! 判定は 2 層に分けて出す（`PEER-UID` 行）:
//! - `dac`: 本番と同じ permission（runtime dir 0700・socket 0600）では、別 UID の process は
//!   `connect()` が `EACCES` で止まり、broker は接続を見ない（journal に残らない）。
//! - `broker`: 試験だけが socket と dir の permission を緩めて DAC 層を迂回させると、
//!   broker 自身の `SO_PEERCRED` 照合（`InjectionService::check_and_inject` step 1）が
//!   `peer_uid_mismatch` を返し、journal に `peer_uid=<別 UID>` の拒否が残る。
//!   設定上の launcher UID そのものが peer として来ても拒否される（launcher UID は session 証明の
//!   照合先であって、injection の peer としては許されない）。

use celeris_credentiald::{
    Broker, ManualProvider,
    injection_ipc::{self, Field, InjectionReply, InjectionRequest},
    ipc,
};
use std::{
    fs,
    io::ErrorKind,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const ORIGIN: &str = "https://login.example.test";
const TARGET: &str = "TARGET-A";
const PROBE_SOCKET_ENV: &str = "CELERIS_PEER_UID_PROBE_SOCKET";
const PROBE_USER_ENV: &str = "CELERIS_PEER_UID_USER";
const DEFAULT_PROBE_USER: &str = "celeris-browser";
const MARK: &str = "PEER-UID-PROBE";

fn require() -> bool {
    std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() == Ok("require")
}

/// 既定 skip と opt-in（ADR-0126 B2/B3 と同じ優先順位。task-worker の `userns_gate` と揃える）。
fn skip_by_default() -> bool {
    if std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("skip") {
        eprintln!("SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip");
        return true;
    }
    if std::env::var("CELERIS_USERNS_TESTS").as_deref() == Ok("1")
        || std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("require")
        || require()
    {
        return false;
    }
    eprintln!(
        "SKIPPED (peer-uid real-process test, not passed): set CELERIS_USERNS_TESTS=1 or CELERIS_LAUNCHER_TESTS=require to run (ADR-0126)"
    );
    true
}

fn missing(reason: impl AsRef<str>) -> bool {
    if require() {
        panic!(
            "peer-uid real-process test environment required: {}",
            reason.as_ref()
        );
    }
    eprintln!("SKIP: (not passed) {}", reason.as_ref());
    true
}

fn request() -> InjectionRequest {
    InjectionRequest {
        v: 1,
        request_id: "01J0000000000000000000REQ1".into(),
        session_id: "sess-peer-uid".into(),
        cdp_target_id: TARGET.into(),
        cdp_session_id: Some("CDPSESSION1".into()),
        frame_id: "FRAME-1".into(),
        loader_id: "LOADER-1".into(),
        frame_chain: vec![ORIGIN.into()],
        redirect_chain: vec![ORIGIN.into()],
        selector: "input[name=password]".into(),
        object_id: "-4611686018427387903.1.7".into(),
        field: Field::Password,
        input_type: "password".into(),
        auth_section_id: "auth-1".into(),
        lease_id: "lease-1".into(),
        cdp_command_id: 1,
        username: None,
    }
}

fn seqpacket_sink() -> (OwnedFd, OwnedFd) {
    let mut fds = [0; 2];
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    assert_eq!(rc, 0, "socketpair");
    unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}

/// 1 回の接続の結果。別 UID の子 process は stdout の `PEER-UID-PROBE` 行で返す。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    /// broker が応答した（`code` は拒否コード。`None` は receipt）。
    Reply { code: Option<String> },
    /// `connect()` が失敗した（errno）。broker は接続を見ていない。
    ConnectFailed { errno: Option<i32> },
    /// 接続はできたが送受信で失敗した。
    IoFailed,
}

impl Probe {
    fn line(&self, uid: u32, pid: u32) -> String {
        let result = match self {
            Probe::Reply { code } => format!("reply={}", code.as_deref().unwrap_or("receipt")),
            Probe::ConnectFailed { errno } => format!("connect_errno={}", errno.unwrap_or(-1)),
            Probe::IoFailed => "io_failed".to_owned(),
        };
        format!("{MARK} uid={uid} pid={pid} {result}")
    }

    fn parse(line: &str) -> Option<(u32, Self)> {
        let rest = line.strip_prefix(MARK)?.trim();
        let mut uid = None;
        let mut probe = None;
        for field in rest.split_whitespace() {
            let (k, v) = field.split_once('=').unwrap_or((field, ""));
            match k {
                "uid" => uid = v.parse().ok(),
                "pid" => {}
                "reply" => {
                    probe = Some(Probe::Reply {
                        code: (v != "receipt").then(|| v.to_owned()),
                    })
                }
                "connect_errno" => {
                    probe = Some(Probe::ConnectFailed {
                        errno: v.parse().ok().filter(|n| *n >= 0),
                    })
                }
                "io_failed" => probe = Some(Probe::IoFailed),
                _ => return None,
            }
        }
        Some((uid?, probe?))
    }
}

/// 本番の controller client（`injection_ipc::call`）と同じ経路で 1 要求を送る。
fn probe(socket: &Path) -> Probe {
    let (sink, _far) = seqpacket_sink();
    match injection_ipc::call(socket, &request(), sink.as_raw_fd_i32()) {
        Ok(InjectionReply { code, .. }) => Probe::Reply { code },
        Err(_) => match UnixStream::connect(socket) {
            Ok(_) => Probe::IoFailed,
            Err(e) => Probe::ConnectFailed {
                errno: e.raw_os_error(),
            },
        },
    }
}

trait RawFdI32 {
    fn as_raw_fd_i32(&self) -> i32;
}
impl RawFdI32 for OwnedFd {
    fn as_raw_fd_i32(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.as_raw_fd()
    }
}

/// `sudo -n -u <user>` で起こした別 UID の process の側。環境変数が無ければ何もしない
/// （通常の test 実行では no-op）。
#[test]
fn inner_peer_uid_probe() {
    let Some(socket) = std::env::var_os(PROBE_SOCKET_ENV) else {
        return;
    };
    let result = probe(Path::new(&socket));
    let uid = unsafe { libc::geteuid() };
    println!("{}", result.line(uid, std::process::id()));
}

struct Fixture {
    root: TempDir,
}

impl Fixture {
    /// 本番と同じ `serve_attested`（設定上の launcher UID 付き）で実 broker を起こす。
    fn serve(launcher_uid: u32) -> Self {
        let root = short_tempdir().expect("tempdir");
        for d in ["config", "data", "run"] {
            let p = root.path().join(d);
            fs::create_dir(&p).expect("mkdir");
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).expect("chmod");
        }
        let manual = ManualProvider::open(
            root.path().join("config/keys"),
            root.path().join("data/vault"),
        )
        .expect("open provider");
        manual.initialize_key().expect("init key");
        let broker = Arc::new(Broker::new(manual, root.path().join("data/audit")).expect("broker"));
        let run = root.path().join("run");
        thread::spawn(move || {
            ipc::serve_attested(broker, &run, vec![std::process::id()], Some(launcher_uid))
        });
        let fx = Self { root };
        let deadline = Instant::now() + Duration::from_secs(60);
        while !fx.socket().exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for injection.sock"
            );
            thread::sleep(Duration::from_millis(10));
        }
        fx
    }

    fn socket_dir(&self) -> PathBuf {
        self.root.path().join("run/celeris-credentiald")
    }

    fn socket(&self) -> PathBuf {
        self.socket_dir().join("injection.sock")
    }

    fn journal(&self) -> String {
        fs::read_to_string(self.root.path().join("data/audit/journal.jsonl")).unwrap_or_default()
    }

    /// journal のうち、`peer_uid` が `uid` で `decision_code` が `code` の行数。
    fn journal_count(&self, uid: u32, code: &str) -> usize {
        self.journal()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| {
                v.get("action").and_then(|a| a.as_str()) == Some("inject")
                    && v.get("peer_uid").and_then(|u| u.as_u64()) == Some(u64::from(uid))
                    && v.get("decision_code").and_then(|c| c.as_str()) == Some(code)
            })
            .count()
    }

    fn journal_peer_entries(&self, uid: u32) -> usize {
        self.journal()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v.get("peer_uid").and_then(|u| u.as_u64()) == Some(u64::from(uid)))
            .count()
    }

    /// 試験だけが DAC 層を迂回させる: 他 UID が socket まで辿れて connect できる permission にする。
    /// broker は起動時にだけ 0700/0600 を検査するので、起動後の緩和は broker 側の照合だけを残す。
    fn loosen_dac(&self) {
        for dir in [
            self.root.path().to_path_buf(),
            self.root.path().join("run"),
            self.socket_dir(),
        ] {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o711)).expect("chmod dir");
        }
        fs::set_permissions(self.socket(), fs::Permissions::from_mode(0o666)).expect("chmod sock");
    }

    /// fixture の外側（tempdir の親から `/` まで）が他 UID から辿れるか。辿れない dir を返す。
    fn untraversable_ancestor(&self) -> Option<PathBuf> {
        let mut dir = self.root.path().parent()?.to_path_buf();
        loop {
            let mode = fs::metadata(&dir).ok()?.permissions().mode();
            if mode & 0o001 == 0 {
                return Some(dir);
            }
            let parent = dir.parent()?;
            if parent == dir {
                return None;
            }
            dir = parent.to_path_buf();
        }
    }
}

fn uid_of(user: &str) -> Option<u32> {
    let output = Command::new("getent")
        .args(["passwd", user])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .split(':')
        .nth(2)?
        .parse()
        .ok()
}

/// 別 UID で 1 要求を送る process を `sudo -n -u <user>` で起こし、その `PEER-UID-PROBE` 行を返す。
/// 自分の test binary を `--exact inner_peer_uid_probe` で再実行する（client は本番と同じ `injection_ipc::call`）。
fn probe_as(user: &str, socket: &Path) -> Result<(u32, Probe), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let output = Command::new("sudo")
        .args(["-n", "-u", user, "/usr/bin/env"])
        .arg(format!("{PROBE_SOCKET_ENV}={}", socket.display()))
        .arg(&exe)
        .args([
            "--exact",
            "inner_peer_uid_probe",
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .map_err(|e| format!("spawn sudo: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match stdout.lines().find_map(Probe::parse) {
        Some(found) => Ok(found),
        None => Err(format!(
            "no {MARK} line from `sudo -n -u {user} {}` (exit={:?}); stdout={:?} stderr={:?}",
            exe.display(),
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        )),
    }
}

/// 別 UID の実 process から broker へ接続し、DAC 層と broker 層の判定を別々に出す。
#[test]
fn real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch() {
    if skip_by_default() {
        return;
    }
    let user = std::env::var(PROBE_USER_ENV).unwrap_or_else(|_| DEFAULT_PROBE_USER.to_owned());
    let Some(other_uid) = uid_of(&user) else {
        missing(format!(
            "user {user} is absent (set {PROBE_USER_ENV} to another local user)"
        ));
        return;
    };
    let my_uid = unsafe { libc::geteuid() };
    assert_ne!(other_uid, my_uid, "{user} must differ from the test UID");
    // sudo が使えるか（NOPASSWD か `sudo -v` 済み）。無ければ理由を出して skip。
    match Command::new("sudo")
        .args(["-n", "-u", &user, "/usr/bin/true"])
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            missing(format!(
                "cannot run a process as {user}: `sudo -n -u {user} true` exit={:?}: {}",
                o.status.code(),
                String::from_utf8_lossy(&o.stderr).trim()
            ));
            return;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            missing("sudo is not installed");
            return;
        }
        Err(e) => {
            missing(format!("sudo: {e}"));
            return;
        }
    }

    let fx = Fixture::serve(other_uid);
    let socket = fx.socket();
    eprintln!(
        "broker uid={my_uid} launcher_uid(configured)={other_uid} socket={}",
        socket.display()
    );

    // 正の対照: 同じ要求を broker の UID から送ると peer 照合は通り、session 未登録の拒否になる。
    // これで要求の形（step 0）が通ることを確かめ、別 UID の拒否を UID の差だけに帰す。
    let control = probe(&socket);
    eprintln!("PEER-UID control uid={my_uid} {control:?}");
    assert_eq!(
        control,
        Probe::Reply {
            code: Some("injection_worker_not_allowed".into())
        }
    );
    assert_eq!(fx.journal_count(my_uid, "injection_worker_not_allowed"), 1);

    // 層 1（dac）: 本番と同じ permission。別 UID は connect で EACCES、broker は接続を見ない。
    let (uid, dac) = match probe_as(&user, &socket) {
        Ok(found) => found,
        Err(reason) => {
            missing(reason);
            return;
        }
    };
    eprintln!("PEER-UID dac uid={uid} {dac:?}");
    assert_eq!(uid, other_uid, "probe process did not run as {user}");
    assert_eq!(
        dac,
        Probe::ConnectFailed {
            errno: Some(libc::EACCES)
        },
        "production permissions must stop another UID at connect()"
    );
    assert_eq!(
        fx.journal_peer_entries(other_uid),
        0,
        "broker must not have seen the other UID: {}",
        fx.journal()
    );

    // 層 2（broker）: DAC を試験だけが緩め、broker 自身の SO_PEERCRED 照合に届かせる。
    if let Some(dir) = fx.untraversable_ancestor() {
        missing(format!(
            "broker layer: fixture ancestor {} is not traversable by {user} (run with TMPDIR=/tmp)",
            dir.display()
        ));
        return;
    }
    fx.loosen_dac();
    let (uid, broker) = match probe_as(&user, &socket) {
        Ok(found) => found,
        Err(reason) => {
            missing(reason);
            return;
        }
    };
    eprintln!("PEER-UID broker uid={uid} {broker:?}");
    assert_eq!(uid, other_uid);
    assert_eq!(
        broker,
        Probe::Reply {
            code: Some("peer_uid_mismatch".into())
        },
        "broker must reject a connected peer whose UID differs from its own"
    );
    assert_eq!(
        fx.journal_count(other_uid, "peer_uid_mismatch"),
        1,
        "journal: {}",
        fx.journal()
    );
    assert_eq!(fx.journal_peer_entries(other_uid), 1);
    eprintln!(
        "PEER-UID result dac=connect_errno={} broker=peer_uid_mismatch (peer uid {other_uid}, broker uid {my_uid})",
        libc::EACCES
    );
}

/// `PEER-UID-PROBE` 行の往復（別 UID の子の出力を親が読む部分）。
#[test]
fn probe_line_round_trips() {
    for p in [
        Probe::Reply {
            code: Some("peer_uid_mismatch".into()),
        },
        Probe::Reply { code: None },
        Probe::ConnectFailed {
            errno: Some(libc::EACCES),
        },
        Probe::ConnectFailed { errno: None },
        Probe::IoFailed,
    ] {
        let line = p.line(995, 4242);
        assert_eq!(Probe::parse(&line), Some((995, p.clone())), "{line}");
    }
    assert_eq!(Probe::parse("running 1 test"), None);
    assert_eq!(Probe::parse("PEER-UID-PROBE uid=995 bogus=1"), None);
}

/// 同じ UID からでも、要求の形が通った上で session 未登録の拒否になる（正の対照の単体）。
#[test]
fn same_uid_control_is_worker_not_allowed() {
    let fx = Fixture::serve(995);
    assert_eq!(
        probe(&fx.socket()),
        Probe::Reply {
            code: Some("injection_worker_not_allowed".into())
        }
    );
    let my_uid = unsafe { libc::geteuid() };
    assert_eq!(fx.journal_count(my_uid, "injection_worker_not_allowed"), 1);
}

/// Unix socket を置く試験用の一時 dir。socket の path は `sun_path`（108 byte）に収まる必要があり、
/// run の `TMPDIR` のような深い path の下では bind できないので、そのときは `/tmp` に作る。
fn short_tempdir() -> std::io::Result<tempfile::TempDir> {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > 40 {
        std::path::PathBuf::from("/tmp")
    } else {
        base
    };
    tempfile::Builder::new().prefix("cz").tempdir_in(base)
}
