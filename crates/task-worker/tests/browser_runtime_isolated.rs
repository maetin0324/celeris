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

use nix::libc;
use task_core::browser_isolation::{IsolationViolation, LiveIsolation, Namespace};
use task_worker::browser_runtime::{
    IsolatedRuntime, LiveSession, RuntimeSpec, listening_tcp, process_starttime, reap_recorded,
    record, same_process_alive, test_hook,
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
    let deadline = Instant::now() + Duration::from_secs(60);
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
    let out = wait_file(&session.path().join("probe.txt"), Duration::from_secs(60));
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
        .recv_timeout(Duration::from_secs(60))
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
            IsolationViolation::LauncherProofMissing,
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

/// 壊れたときに止まるための保険（ADR-0125: 成立条件ではなく異常時の上限）。
const GUARD: Duration = Duration::from_secs(60);

fn signal(pid: i32, sig: libc::c_int) {
    // SAFETY: 試験が起こした process（helper・runtime）にだけ送る。
    unsafe { libc::kill(pid, sig) };
}

fn proc_state(pid: i32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().next().map(str::to_owned)
}

fn proc_ppid(pid: i32) -> Option<i32> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("PPid:")?.trim().parse().ok())
}

fn restore_sigchld_reaping() {
    // The invoking shell may have ignored or blocked SIGCHLD. In that case Linux
    // auto-reaps children, so neither waitpid nor the stopped-reaper proof works.
    // SAFETY: SIGCHLD disposition and mask belong to this test/helper process.
    unsafe {
        assert_ne!(libc::signal(libc::SIGCHLD, libc::SIG_DFL), libc::SIG_ERR);
        let mut set: libc::sigset_t = std::mem::zeroed();
        assert_eq!(libc::sigemptyset(&raw mut set), 0);
        assert_eq!(libc::sigaddset(&raw mut set, libc::SIGCHLD), 0);
        assert_eq!(
            libc::sigprocmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut()),
            0
        );
    }
}

#[test]
fn ignored_sigchld_still_keeps_unreaped_child_after_reset() {
    let status = Command::new("/bin/sh")
        .args([
            "-c",
            "trap '' CHLD; exec \"$1\" --exact helper_sigchld_probe --ignored --nocapture",
            "sh",
        ])
        .arg(std::env::current_exe().unwrap())
        .env("CELERIS_SIGCHLD_PROBE", "1")
        .status()
        .unwrap();
    assert!(status.success(), "SIGCHLD reset probe: {status}");
}

#[test]
#[ignore = "helper process for ignored_sigchld_still_keeps_unreaped_child_after_reset"]
fn helper_sigchld_probe() {
    if std::env::var_os("CELERIS_SIGCHLD_PROBE").is_none() {
        return;
    }
    restore_sigchld_reaping();
    let mut child = Command::new("/bin/true").spawn().unwrap();
    let pid = child.id() as i32;
    let deadline = Instant::now() + GUARD;
    while proc_state(pid).as_deref() != Some("Z") {
        assert!(
            Instant::now() < deadline,
            "child was auto-reaped: {}",
            process_diagnostics(pid)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(proc_ppid(pid), Some(std::process::id() as i32));
    assert!(child.wait().unwrap().success());
}

/// `p` が `END` で閉じるまで待つ。helper（`watch`）が先に終われば、その旨で失敗する。
fn wait_marker(p: &Path, watch: &mut std::process::Child) -> String {
    let deadline = Instant::now() + GUARD;
    loop {
        if let Ok(s) = std::fs::read_to_string(p)
            && s.ends_with("END\n")
        {
            return s;
        }
        if let Some(status) = watch.try_wait().unwrap() {
            panic!("helper exited ({status}) before {p:?} appeared");
        }
        assert!(Instant::now() < deadline, "timeout waiting for {p:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn first_pid(s: &str) -> i32 {
    s.lines().next().and_then(|l| l.parse().ok()).expect("pid")
}

/// 失敗した時も止めた process を再開し、残った試験の process を回収する。
struct Cleanup {
    reaper: std::process::Child,
    identities: Vec<(i32, u64)>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for &(pid, starttime) in &self.identities {
            if same_process_alive(pid, starttime) {
                signal(pid, libc::SIGCONT);
                signal(pid, libc::SIGKILL);
            }
        }
        signal(self.reaper.id() as i32, libc::SIGCONT);
        let _ = self.reaper.kill();
        let _ = self.reaper.wait();
    }
}

/// controller が SIGKILL されたら bwrap と sandbox 内 process が残らない（実 process）。
///
/// ADR-0125 §5: 負荷に依らず競合点を固定する。(1) launch の試験フックが bwrap の
/// `--info-fd` 報告直後に init を SIGSTOP する（init が親死亡シグナルを設定する前）。
/// launch がその init の ready を待たずに戻れば、controller を殺してから init を再開し、
/// 実行中の init が残ることを検出する。(2) controller の親は subreaper（helper_reaper）で、
/// controller を殺す間それを SIGSTOP する。孤児は reap されない zombie になるので、
/// zombie を残存と数えないこと（実行中の本人は数えること）を毎回通る。
/// 待ちは controller の終了・runtime の終了・reaper の終了という出来事で判定し、
/// 60 秒は壊れたときの保険。
#[test]
fn controller_kill_leaves_no_runtime_processes() {
    if skip() {
        return;
    }
    restore_sigchld_reaping();
    let dir = tempfile::tempdir().unwrap();
    let reaper = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "helper_reaper", "--ignored", "--nocapture"])
        .env("CELERIS_RT_HELPER_DIR", dir.path())
        .env(test_hook::LAUNCH_HOOK_DIR_ENV, dir.path())
        .spawn()
        .unwrap();
    let mut cleanup = Cleanup {
        reaper,
        identities: Vec::new(),
    };
    let reaper_pid = cleanup.reaper.id() as i32;

    // (1) launch の競合点: init は info-fd 報告の直後に止まっている。
    let init = first_pid(&wait_marker(
        &dir.path().join("launch-info"),
        &mut cleanup.reaper,
    ));
    let init_starttime = process_starttime(init).expect("init starttime");
    cleanup.identities.push((init, init_starttime));
    let ctl = first_pid(&wait_marker(&dir.path().join("ctl"), &mut cleanup.reaper));
    let ctl_starttime = process_starttime(ctl).expect("controller starttime");
    cleanup.identities.push((ctl, ctl_starttime));
    // launch が ready 待ちに入る（launch-waiting）か、待たずに戻って PID を出す（pids）か。
    let deadline = Instant::now() + GUARD;
    let launch_waited = loop {
        if std::fs::read_to_string(dir.path().join("pids")).is_ok_and(|s| s.ends_with("END\n")) {
            break false;
        }
        if std::fs::read_to_string(dir.path().join("launch-waiting"))
            .is_ok_and(|s| s.ends_with("END\n"))
        {
            break true;
        }
        if let Some(status) = cleanup.reaper.try_wait().unwrap() {
            panic!("helper exited ({status}) during launch");
        }
        assert!(
            Instant::now() < deadline,
            "launch neither waited nor returned"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    if launch_waited {
        signal(init, libc::SIGCONT);
    }
    let pids = wait_marker(&dir.path().join("pids"), &mut cleanup.reaper);
    let pids: Vec<i32> = pids.lines().filter_map(|l| l.parse().ok()).collect();
    assert_eq!(pids.len(), 2);
    assert_eq!(pids[1], init, "launch reports the init it stopped");
    let identities: Vec<(i32, u64)> = pids
        .iter()
        .map(|&pid| (pid, process_starttime(pid).expect("runtime starttime")))
        .collect();
    cleanup.identities.extend(identities.iter().copied());

    // (2) subreaper を止めた状態で controller を殺す。
    signal(reaper_pid, libc::SIGSTOP);
    let deadline = Instant::now() + GUARD;
    while !matches!(proc_state(reaper_pid).as_deref(), Some("T" | "t")) {
        assert!(
            Instant::now() < deadline,
            "subreaper did not stop: {}",
            process_diagnostics(reaper_pid)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    signal(ctl, libc::SIGKILL);
    let deadline = Instant::now() + GUARD;
    while same_process_alive(ctl, ctl_starttime) {
        assert!(Instant::now() < deadline, "controller did not die");
        std::thread::sleep(Duration::from_millis(20));
    }
    // launch が待たずに戻っていた場合、ここで初めて init が親死亡シグナルの準備を再開する。
    signal(init, libc::SIGCONT);
    // PID 再利用や unreaped zombie は残存扱いしない。実行中の本人は引き続き失敗にする。
    let deadline = Instant::now() + GUARD;
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
        "running runtime survived controller kill (launch waited for init: {launch_waited}): {survivors:#?}"
    );
    // stutter が効いている証拠: 止めた subreaper の下で bwrap は reap されない zombie のまま。
    assert_eq!(
        proc_state(pids[0]).as_deref(),
        Some("Z"),
        "bwrap must be an unreaped zombie under the stopped subreaper: {}",
        process_diagnostics(pids[0])
    );
    assert_eq!(
        proc_ppid(pids[0]),
        Some(reaper_pid),
        "unreaped bwrap must belong to the stopped subreaper: {}",
        process_diagnostics(pids[0])
    );

    // subreaper を再開すると孤児を全て reap して終わる（出来事: reaper の終了）。
    signal(reaper_pid, libc::SIGCONT);
    let deadline = Instant::now() + GUARD;
    let status = loop {
        if let Some(status) = cleanup.reaper.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "subreaper did not finish reaping"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "helper_reaper: {status}");
    for &(pid, starttime) in &identities {
        assert!(
            process_starttime(pid) != Some(starttime),
            "pid {pid} not reaped"
        );
    }
}

/// controller の親になる subreaper。孤児（controller・bwrap・init）を全て reap したら終わる。
#[test]
#[ignore = "helper process for controller_kill_leaves_no_runtime_processes"]
fn helper_reaper() {
    let Ok(dir) = std::env::var("CELERIS_RT_HELPER_DIR") else {
        return;
    };
    restore_sigchld_reaping();
    // SAFETY: 自 process の属性を変えるだけ。
    assert_eq!(
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) },
        0,
        "PR_SET_CHILD_SUBREAPER"
    );
    let ctl = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "helper_controller", "--ignored", "--nocapture"])
        .env("CELERIS_RT_HELPER_DIR", &dir)
        .spawn()
        .unwrap();
    let dir = PathBuf::from(dir);
    std::fs::write(dir.join(".ctl.tmp"), format!("{}\nEND\n", ctl.id())).unwrap();
    std::fs::rename(dir.join(".ctl.tmp"), dir.join("ctl")).unwrap();
    std::mem::forget(ctl);
    loop {
        match nix::sys::wait::waitpid(None, None) {
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => {}
            Err(nix::errno::Errno::ECHILD) => break,
            Err(e) => panic!("waitpid: {e}"),
        }
    }
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
        dir.join(".pids.tmp"),
        format!("{}\n{}\nEND\n", rt.bwrap_pid(), rt.inner_pid()),
    )
    .unwrap();
    std::fs::rename(dir.join(".pids.tmp"), dir.join("pids")).unwrap();
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
    let deadline = Instant::now() + Duration::from_secs(60);
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
