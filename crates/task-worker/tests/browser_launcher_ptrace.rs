//! ADR-0115: dedicated-user launcher の実 process 境界を daemon UID から測る。
//! host 未準備の CI では理由を表示して skip する。実証時は CELERIS_LAUNCHER_TESTS=require。

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nix::libc;
use task_worker::browser::verify_launcher_observation;
use task_worker::browser_launcher::{LauncherClient, Outcome, SessionPolicy, SessionState};

const DEFAULT_SOCKET: &str = "/run/celeris-browser/launcher.sock";

fn missing(reason: impl AsRef<str>) -> bool {
    if std::env::var("CELERIS_LAUNCHER_TESTS").as_deref() == Ok("require") {
        panic!("launcher test environment required: {}", reason.as_ref());
    }
    eprintln!("SKIPPED (not passed): {}", reason.as_ref());
    true
}

fn browser_uid() -> Option<u32> {
    let output = Command::new("getent")
        .args(["passwd", "celeris-browser"])
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

fn subid_start(path: &str) -> Option<u32> {
    fs::read_to_string(path).ok()?.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next() == Some("celeris-browser"))
            .then(|| fields.next()?.parse().ok())
            .flatten()
    })
}

fn has_subid(path: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|body| {
        body.lines().any(|line| {
            let mut fields = line.split(':');
            fields.next() == Some("celeris-browser")
                && fields.next().and_then(|s| s.parse::<u32>().ok()).is_some()
                && fields
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .is_some_and(|n| n > 0)
        })
    })
}

fn proc_pids() -> HashSet<i32> {
    fs::read_dir("/proc")
        .expect("read /proc")
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .collect()
}

fn is_browser(pid: i32, expected_map: &str) -> bool {
    let base = PathBuf::from(format!("/proc/{pid}"));
    let Ok(comm) = fs::read_to_string(base.join("comm")) else {
        return false;
    };
    if !comm.contains("chrome") && !comm.contains("chromium") {
        return false;
    }
    let Ok(cmdline) = fs::read(base.join("cmdline")) else {
        return false;
    };
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
    if !args.contains(&b"--remote-debugging-pipe".as_slice())
        || args.iter().any(|a| a.starts_with(b"--type="))
    {
        return false;
    }
    fs::read_to_string(base.join("uid_map")).is_ok_and(|map| map == expected_map)
}

fn chrome_pid(before: &HashSet<i32>, expected_map: &str) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let candidates: Vec<_> = proc_pids()
            .difference(before)
            .copied()
            .filter(|pid| is_browser(*pid, expected_map))
            .collect();
        if candidates.len() == 1 {
            return candidates[0];
        }
        assert!(
            candidates.len() <= 1,
            "ambiguous new Chrome PIDs: {candidates:?}"
        );
        assert!(
            Instant::now() < deadline,
            "Chrome PID not visible in /proc within 20s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn maps_host_id(map: &str, id: u32) -> bool {
    map.lines().any(|line| {
        let fields: Vec<u32> = line
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        fields.len() == 3
            && id >= fields[1]
            && u64::from(id) < u64::from(fields[1]) + u64::from(fields[2])
    })
}

fn owner_uid(pid: i32) -> std::io::Result<u32> {
    let ns = File::open(format!("/proc/{pid}/ns/user"))?;
    let mut uid = 0u32;
    // Linux nsfs.h: NS_GET_OWNER_UID = _IO(0xb7, 0x4).
    if unsafe { libc::ioctl(ns.as_raw_fd(), 0xb704, &mut uid) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(uid)
}

fn ptrace_attach(pid: i32) -> std::io::Result<()> {
    if unsafe { libc::ptrace(libc::PTRACE_ATTACH, pid, 0, 0) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn positive_control() {
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("start positive-control child");
    let pid = child.id() as i32;
    ptrace_attach(pid).expect("PTRACE_ATTACH must work on own child");
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(
        libc::WIFSTOPPED(status),
        "positive control wait status={status}"
    );
    assert_eq!(unsafe { libc::ptrace(libc::PTRACE_DETACH, pid, 0, 0) }, 0);
    eprintln!("positive control: child pid={pid} PTRACE_ATTACH=0 PTRACE_DETACH=0");
    child.kill().expect("kill positive-control child");
    child.wait().expect("reap positive-control child");
}

fn denied_read(path: &Path) {
    let mut buf = [0u8; 1];
    let error = File::open(path)
        .and_then(|mut file| file.read(&mut buf).map(|_| ()))
        .expect_err(&format!("{} unexpectedly readable", path.display()));
    eprintln!("{}: errno={:?}", path.display(), error.raw_os_error());
    assert!(
        matches!(error.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)),
        "{}: expected EACCES/EPERM, got {error}",
        path.display()
    );
}

fn strace_denied(pid: i32) {
    let mut child = match Command::new("strace")
        .args(["-p", &pid.to_string(), "-o", "/dev/null"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("strace -p: unavailable (optional)");
            return;
        }
        Err(e) => panic!("strace spawn failed: {e}"),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().expect("poll strace").is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("strace -p {pid} remained attached or hung for 5s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().expect("read strace result");
    eprintln!(
        "strace -p {pid}: exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    assert!(!output.status.success(), "strace unexpectedly attached");
}

struct Session<'a> {
    client: &'a mut LauncherClient,
    id: String,
    lease: String,
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        let _ = self.client.stop(&self.id, &self.lease);
    }
}

#[test]
fn launcher_chrome_denies_daemon_uid_ptrace() {
    let Some(browser_uid) = browser_uid() else {
        missing("celeris-browser user is absent");
        return;
    };
    let socket = PathBuf::from(
        std::env::var_os("CELERIS_LAUNCHER_SOCKET").unwrap_or_else(|| DEFAULT_SOCKET.into()),
    );
    if !fs::metadata(&socket).is_ok_and(|m| m.file_type().is_socket()) {
        missing(format!("launcher socket is absent: {}", socket.display()));
        return;
    }
    for path in ["/etc/subuid", "/etc/subgid"] {
        if !has_subid(path) {
            missing(format!("celeris-browser has no allocation in {path}"));
            return;
        }
    }
    let daemon_uid = unsafe { libc::geteuid() };
    assert_eq!(daemon_uid, 1001, "run this test as daemon UID 1001");
    assert_ne!(
        browser_uid, daemon_uid,
        "launcher user must differ from daemon"
    );

    positive_control();
    let before = proc_pids();
    let mut client = LauncherClient::connect(&socket, Duration::from_secs(150))
        .expect("connect launcher socket");
    let lease = task_worker::browser_launcher::random_id().expect("random lease");
    let started = client
        .start_session(
            "launcher-ptrace-test",
            "launcher-ptrace-run",
            &lease,
            SessionPolicy {
                allowed_domains: vec![],
                allowed_actions: vec![],
                lease_seconds: 180,
            },
        )
        .expect("start real launcher session");
    assert_eq!(started.receipt.outcome, Outcome::Started);
    assert!(
        started.receipt.isolation_ok,
        "launcher isolation attestation"
    );
    let session = Session {
        client: &mut client,
        id: started.session_id,
        lease,
    };
    let (state, observed) = session
        .client
        .observe(&session.id, &session.lease)
        .expect("observe real session");
    assert_eq!(state, SessionState::Running);
    eprintln!(
        "observe: owner={:?} host_uid={} uid_map={:?} gid_map={:?}",
        observed.ns_owner_uid, observed.host_uid, observed.uid_map, observed.gid_map
    );
    // ADR-0116 付記: launcher（B）が 2 map の userns を作り、その中で内側 1000（= subuid S）に
    // なった bwrap が Chrome の userns を作る。Chrome の userns の owner は S、親の owner は B。
    let subuid = subid_start("/etc/subuid").expect("celeris-browser subuid");
    assert_eq!(observed.host_uid, browser_uid);
    assert_eq!(observed.ns_owner_uid, Some(subuid));
    assert_ne!(observed.ns_owner_uid, Some(daemon_uid));
    let pid = chrome_pid(&before, &observed.uid_map);
    let uid_map = fs::read_to_string(format!("/proc/{pid}/uid_map")).expect("Chrome uid_map");
    let gid_map = fs::read_to_string(format!("/proc/{pid}/gid_map")).expect("Chrome gid_map");
    eprintln!(
        "Chrome pid={pid} NS_GET_OWNER_UID(launcher)={:?} uid_map={uid_map:?} gid_map={gid_map:?}",
        observed.ns_owner_uid
    );
    // daemon UID からは Chrome の namespace link も開けない（ptrace read の検査）。
    let ns_error = owner_uid(pid).expect_err("daemon UID opened Chrome /proc/<pid>/ns/user");
    eprintln!(
        "NS_GET_OWNER_UID from daemon UID: errno={:?}",
        ns_error.raw_os_error()
    );
    assert!(matches!(
        ns_error.raw_os_error(),
        Some(libc::EACCES) | Some(libc::EPERM)
    ));
    assert_eq!(uid_map, observed.uid_map);
    assert_eq!(gid_map, observed.gid_map);
    for map in [&uid_map, &gid_map] {
        assert!(
            !maps_host_id(map, daemon_uid),
            "daemon UID/GID must not be mapped: {map}"
        );
    }

    // launcher は自分の verify_isolation（mount・namespace を含む実観測）が Ok のときだけ
    // isolation_ok を返す。daemon 側は観測値を同じ判定に掛ける。
    let attestation =
        verify_launcher_observation(&session.id, &observed, started.receipt.isolation_ok)
            .expect("launcher observation rejected (fail closed)")
            .expect("verify_isolation on launcher observation");
    assert_eq!(attestation.session_id(), session.id);
    eprintln!(
        "verify_isolation=Ok (launcher isolation_ok={}, CapEff={} NoNewPrivs={})",
        started.receipt.isolation_ok, observed.cap_eff, observed.no_new_privs
    );

    let error = ptrace_attach(pid).expect_err("daemon UID attached to Chrome");
    eprintln!(
        "PTRACE_ATTACH Chrome pid={pid}: errno={:?}",
        error.raw_os_error()
    );
    assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    strace_denied(pid);
    denied_read(&PathBuf::from(format!("/proc/{pid}/environ")));
    denied_read(&PathBuf::from(format!("/proc/{pid}/mem")));
}
