//! ADR-0105（P4-A）: 実 bubblewrap runtime を起動し、host 側で事実を採って検査する。
//! 同一 host UID の決定（p4a-uid）で実行する。外部ネットワークには出ない（netns に経路が無い）。
//! bwrap か browser が無い環境では失敗する（成功扱いにしない）。明示的に
//! `CELERIS_ISOLATION_TESTS=skip` を与えた時だけ飛ばし、その旨を stderr に出す。
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use task_core::browser_isolation::{IsolationViolation, LiveIsolation, Namespace};
use task_worker::browser_runtime::{
    IsolatedRuntime, LiveSession, RuntimeSpec, listening_tcp, process_starttime, reap_recorded,
    record, same_process_alive,
};

fn skip() -> bool {
    if std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("skip") {
        eprintln!("SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip");
        return true;
    }
    false
}

fn bwrap() -> PathBuf {
    let p = PathBuf::from(std::env::var("CELERIS_TEST_BWRAP").unwrap_or("/usr/bin/bwrap".into()));
    assert!(
        p.exists(),
        "bwrap not found at {p:?}; see docs/ops/browser-isolated-runtime-subuid.md"
    );
    p
}

fn browser() -> PathBuf {
    if let Ok(p) = std::env::var("CELERIS_TEST_BROWSER") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME");
    let base = Path::new(&home).join(".cache/ms-playwright");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .expect("playwright cache (agent-browser's browser) is required")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("chromium_headless_shell-"))
        })
        .map(|p| p.join("chrome-headless-shell-linux64/chrome-headless-shell"))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.pop().expect("chrome-headless-shell not installed")
}

fn spec(session: &Path, id: &str, ro: Vec<PathBuf>, argv: &[&str], cdp: bool) -> RuntimeSpec {
    RuntimeSpec {
        userns: task_worker::browser_runtime::UsernsMode::Unshare,
        bwrap: bwrap(),
        session_id: id.into(),
        session_dir: session.to_path_buf(),
        ro_dirs: ro,
        argv: argv.iter().map(OsString::from).collect(),
        cdp_pipe: cdp,
        egress: None,
    }
}

fn wait_file(p: &Path, t: Duration) -> String {
    let deadline = Instant::now() + t;
    while Instant::now() < deadline {
        if let Ok(s) = std::fs::read_to_string(p)
            && s.ends_with("END\n")
        {
            return s;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timeout waiting for {p:?}");
}

fn alive(pid: i32) -> bool {
    process_starttime(pid).is_some_and(|starttime| same_process_alive(pid, starttime))
}

fn process_diagnostics(pid: i32) -> String {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .unwrap_or_else(|e| format!("unavailable: {e}"));
    let ppid = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("PPid:"))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "PPid: unavailable".to_owned());
    format!("pid={pid} {ppid} stat={stat}")
}

#[test]
fn process_liveness_counts_running_but_not_unreaped_zombie() {
    let mut running = Command::new("/bin/sleep").arg("5").spawn().unwrap();
    let running_pid = running.id() as i32;
    let running_starttime = process_starttime(running_pid).unwrap();
    assert!(same_process_alive(running_pid, running_starttime));
    running.kill().unwrap();
    running.wait().unwrap();

    let mut zombie = Command::new("/bin/true").spawn().unwrap();
    let zombie_pid = zombie.id() as i32;
    let zombie_starttime = process_starttime(zombie_pid).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while same_process_alive(zombie_pid, zombie_starttime) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(format!("/proc/{zombie_pid}/stat"))
            .unwrap()
            .rsplit(')')
            .next()
            .unwrap()
            .split_whitespace()
            .next(),
        Some("Z"),
        "child must still be an unreaped zombie"
    );
    assert!(!same_process_alive(zombie_pid, zombie_starttime));
    zombie.wait().unwrap();
    assert!(!same_process_alive(zombie_pid, zombie_starttime));
}

/// sandbox の中から host の broker/control socket・/run/user・host の loopback fixture・
/// private IP・IPv6・DNS 直叩きに届かないこと、root が書けないことを実測する。
#[test]
fn probe_inside_runtime_cannot_reach_host_sockets_or_network() {
    if skip() {
        return;
    }
    let host = tempfile::tempdir().unwrap();
    let broker = host.path().join("credentiald.sock");
    let control = host.path().join("agent-browser-control.sock");
    let _l1 = UnixListener::bind(&broker).unwrap();
    let _l2 = UnixListener::bind(&control).unwrap();
    // host の loopback fixture（proxy 迂回で届いてはいけない）
    let fixture = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = fixture.local_addr().unwrap().port();
    let session = tempfile::tempdir().unwrap();
    let script = format!(
        r#"exec > /session/probe.txt 2>&1
chk() {{ if eval "$2" >/dev/null 2>&1; then echo "$1=yes"; else echo "$1=no"; fi; }}
chk broker "test -e {b}"
chk control "test -e {c}"
chk run_user "test -e /run/user"
chk tmp_host "test -e {h}"
chk root_write "touch /usr/celeris-probe"
chk etc_write "touch /etc/celeris-probe"
chk session_write "touch /session/ok"
chk loopback_fixture "timeout 2 bash -c 'echo > /dev/tcp/127.0.0.1/{port}'"
chk private_ip "timeout 2 bash -c 'echo > /dev/tcp/10.0.0.1/80'"
chk ipv6 "timeout 2 bash -c 'echo > /dev/tcp/::1/{port}'"
chk dns_direct "timeout 2 bash -c 'echo > /dev/tcp/192.0.2.53/53'"
echo "uid=$(id -u) gid=$(id -g) host=$(hostname)"
echo END
"#,
        b = broker.display(),
        c = control.display(),
        h = host.path().display(),
    );
    let mut rt = IsolatedRuntime::launch(&spec(
        session.path(),
        "probe-1",
        vec![],
        &["/bin/bash", "-c", &script],
        false,
    ))
    .unwrap();
    let out = wait_file(&session.path().join("probe.txt"), Duration::from_secs(20));
    eprintln!("{out}");
    for (k, v) in [
        ("broker", "no"),
        ("control", "no"),
        ("run_user", "no"),
        ("tmp_host", "no"),
        ("root_write", "no"),
        ("etc_write", "no"),
        ("session_write", "yes"),
        ("loopback_fixture", "no"),
        ("private_ip", "no"),
        ("ipv6", "no"),
        ("dns_direct", "no"),
    ] {
        assert!(
            out.contains(&format!("{k}={v}\n")),
            "{k} expected {v}: {out}"
        );
    }
    assert!(
        out.contains("uid=1000 gid=1000 host=celeris-browser"),
        "{out}"
    );
    rt.kill();
}

/// 実 browser（agent-browser が使う chrome-headless-shell）を隔離 runtime で起動し、
/// CDP pipe で応答を得て、host 側で namespace・UID・mount・TCP listen を採取する。
#[test]
fn real_browser_in_runtime_facts_and_restore_refused_on_same_uid() {
    if skip() {
        return;
    }
    let exe = browser();
    let install = exe.parent().unwrap().to_path_buf();
    let session = tempfile::tempdir().unwrap();
    let exe_s = exe.to_string_lossy().into_owned();
    let rt = IsolatedRuntime::launch(&spec(
        session.path(),
        "browser-1",
        vec![install],
        &[
            &exe_s,
            "--headless",
            "--no-sandbox",
            "--no-zygote",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--remote-debugging-pipe",
            "--user-data-dir=/session/profile",
            "--proxy-server=http://127.0.0.1:3128",
            "about:blank",
        ],
        true,
    ))
    .unwrap();
    let mut rt = rt;
    // CDP は pipe だけ（NUL 区切り JSON）
    let mut w = rt.cdp_write.take().unwrap();
    let mut r = rt.cdp_read.take().unwrap();
    w.write_all(b"{\"id\":1,\"method\":\"Browser.getVersion\"}\0")
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut b = [0u8; 4096];
        while let Ok(n) = r.read(&mut b) {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&b[..n]);
            if buf.contains(&0) {
                break;
            }
        }
        let _ = tx.send(buf);
    });
    let resp = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("CDP reply over pipe");
    let resp = String::from_utf8_lossy(&resp);
    assert!(
        resp.contains("\"id\":1") && resp.contains("HeadlessChrome"),
        "{resp}"
    );

    let facts = rt.facts().unwrap();
    eprintln!("{facts:#?}");
    for ns in [
        Namespace::User,
        Namespace::Pid,
        Namespace::Net,
        Namespace::Mount,
        Namespace::Ipc,
        Namespace::Uts,
    ] {
        assert!(facts.namespaces.contains(&ns), "{ns:?} not unshared");
    }
    assert!(facts.root_readonly);
    assert!(facts.no_new_privs && facts.capabilities_dropped);
    assert_eq!(facts.writable_mounts, vec!["/session".to_owned()]);
    assert!(!facts.visible_paths.iter().any(|p| p.starts_with("/run")));
    // 決定 p4a-uid: host UID は daemon と同じ。sandbox 内は 1000 に写像される。
    assert_eq!(facts.runtime_uid, facts.host_uid);
    let map = std::fs::read_to_string(format!("/proc/{}/uid_map", rt.inner_pid())).unwrap();
    assert_eq!(
        map.split_whitespace().collect::<Vec<_>>(),
        vec!["1000", &facts.host_uid.to_string(), "1"]
    );
    assert_eq!(
        listening_tcp(rt.inner_pid()).unwrap(),
        0,
        "CDP must not listen on TCP"
    );
    // 検査は弱めない: 同一 UID と daemon 所有 userns に attestation は出ない。
    assert_eq!(
        rt.attest().unwrap_err(),
        vec![
            IsolationViolation::SameUid,
            IsolationViolation::UsernsOwnedByDaemon,
        ]
    );
    let live = LiveSession(std::sync::Mutex::new(rt));
    assert!(live.current_attestation().is_err());
    // 停止後も出ない
    live.0.lock().unwrap().kill();
    assert_eq!(
        live.current_attestation().unwrap_err(),
        vec![IsolationViolation::NoProcessGroup]
    );
}

/// controller が SIGKILL されたら bwrap と sandbox 内 process が残らない（実 process）。
#[test]
fn controller_kill_leaves_no_runtime_processes() {
    if skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut ctl = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "helper_controller", "--ignored", "--nocapture"])
        .env("CELERIS_RT_HELPER_DIR", dir.path())
        .spawn()
        .unwrap();
    let pids = wait_file(&dir.path().join("pids"), Duration::from_secs(20));
    let pids: Vec<i32> = pids.lines().filter_map(|l| l.parse().ok()).collect();
    assert_eq!(pids.len(), 2);
    assert!(pids.iter().all(|p| alive(*p)));
    let identities: Vec<(i32, u64)> = pids
        .iter()
        .map(|&pid| (pid, process_starttime(pid).expect("runtime starttime")))
        .collect();
    ctl.kill().unwrap();
    ctl.wait().unwrap();
    // PID 再利用や unreaped zombie は残存扱いしない。実行中の本人は引き続き失敗にする。
    let deadline = Instant::now() + Duration::from_secs(10);
    while identities
        .iter()
        .any(|&(pid, starttime)| same_process_alive(pid, starttime))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let survivors: Vec<_> = identities
        .iter()
        .filter_map(|&(pid, starttime)| {
            if !same_process_alive(pid, starttime) {
                return None;
            }
            let diagnosis = process_diagnostics(pid);
            same_process_alive(pid, starttime).then_some(diagnosis)
        })
        .collect();
    assert!(
        survivors.is_empty(),
        "running runtime survived controller kill: {survivors:#?}"
    );
}

#[test]
#[ignore = "helper process for controller_kill_leaves_no_runtime_processes"]
fn helper_controller() {
    let Ok(dir) = std::env::var("CELERIS_RT_HELPER_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let session = dir.join("session");
    let rt = IsolatedRuntime::launch(&spec(
        &session,
        "orphan-1",
        vec![],
        &["/bin/sleep", "600"],
        false,
    ))
    .unwrap();
    std::fs::write(
        dir.join("pids"),
        format!("{}\n{}\nEND\n", rt.bwrap_pid(), rt.inner_pid()),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(600));
    drop(rt);
}

/// daemon が状態を失って再起動した場合: 記録から starttime 一致の runtime だけを回収する。
#[test]
fn restart_reaps_recorded_runtime_and_ignores_stale_records() {
    if skip() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let session = tempfile::tempdir().unwrap();
    let rt = IsolatedRuntime::launch(&spec(
        session.path(),
        "restart-1",
        vec![],
        &["/bin/sleep", "600"],
        false,
    ))
    .unwrap();
    record(dir.path(), &rt).unwrap();
    let (outer, inner) = (rt.bwrap_pid(), rt.inner_pid());
    // PID 再利用の模擬: 自分の pid に誤った starttime の記録。殺してはいけない。
    std::fs::write(
        dir.path().join("stale.pid"),
        format!("{} 1\n", std::process::id()),
    )
    .unwrap();
    // 状態を失った daemon（handle を持たない）
    std::mem::forget(rt);
    let killed = reap_recorded(dir.path()).unwrap();
    assert_eq!(killed, vec![outer]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while (alive(outer) || alive(inner)) && Instant::now() < deadline {
        // bwrap は自分の子（zombie）なので刈り取る
        let _ = nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(outer),
            Some(nix::sys::wait::WaitPidFlag::WNOHANG),
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!alive(outer) && !alive(inner));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
