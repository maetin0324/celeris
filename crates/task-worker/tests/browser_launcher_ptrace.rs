//! ADR-0115: dedicated-user launcher の実 process 境界を daemon UID から測る。
//! host 未準備の CI では理由を表示して skip する。実証時は CELERIS_LAUNCHER_TESTS=require。

use std::collections::{HashMap, HashSet};
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

/// `/proc/<pid>/uid_map` の外側の ID は読む側の userns で訳される。celeris の check runner
/// （db_guard の `1001 1001 1`）からは subuid が写らず `4294967295` に見えるので、内側の ID と
/// 長さが一致し、外側が一致するか読む側で写らない（u32::MAX）ときを同じ map とみなす。
/// 読む側は daemon UID を恒等に写すので、Chrome の map に daemon UID があればそのまま見える。
fn same_map_from_reader(seen: &str, expected: &str) -> bool {
    let parse = |map: &str| -> Vec<Vec<u32>> {
        map.lines()
            .map(|line| {
                line.split_whitespace()
                    .filter_map(|field| field.parse().ok())
                    .collect()
            })
            .collect()
    };
    let (seen, expected) = (parse(seen), parse(expected));
    seen.len() == expected.len()
        && seen.iter().zip(&expected).all(|(s, e)| {
            s.len() == 3
                && e.len() == 3
                && s[0] == e[0]
                && s[2] == e[2]
                && (s[1] == e[1] || s[1] == u32::MAX)
        })
}

// 並走 session での Chrome 特定（phase-browser-4.md『並走 session での Chrome 特定』）。
//
// host の launcher は共有で、他の task の試験や browser run が同時に session を起こす。
// 試験前後の PID 差分で「新しい Chrome は 1 つ」と仮定すると、並走時に曖昧になって落ちる。
// launcher は daemon UID から読める /proc の上に自分の session を示す印を出さない:
// bwrap の引数は固定（session dir は私有 mount ns の /tmp/celeris-session に bind し直す）、
// sandboxd・Chrome の argv も session に依らず、session id 入りの thread 名は comm の 15 byte で
// 切れ、診断は journal にだけ出る。Started の receipt にも pid は無い。
// そこで launcher の直接の子（session ごとの bwrap = session root）ごとに候補を束ね、
// 新しく現れた全候補に同じ拒否検査（NS_GET_OWNER_UID・PTRACE_ATTACH・strace -p・
// /proc environ・mem）を当てて 1 件以上を要求する。自分の Chrome は必ず候補に含まれるので、
// 全候補が拒否されれば自分の Chrome も拒否されている。最後に自分の session を止め、
// 検査済みの session root のどれかが消えることで、自分の session が検査済みだったことを確かめる。

/// /proc の 1 行分（試験が読む欄だけ）。選択の論理は /proc を読む部分と分けて偽の表で試す。
#[derive(Debug, Clone)]
struct ProcEntry {
    pid: i32,
    ppid: i32,
    argv: Vec<String>,
    uid_map: Option<String>,
}

/// 検査する Chrome の候補と、それを起こした session root（launcher の直接の子 = bwrap）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChromeCandidate {
    pid: i32,
    session_root: i32,
}

fn is_launcher(entry: &ProcEntry) -> bool {
    entry
        .argv
        .first()
        .is_some_and(|arg0| arg0.ends_with("celeris-browser-launcher"))
}

fn is_browser_argv(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--remote-debugging-pipe")
        && !argv.iter().any(|a| a.starts_with("--type="))
}

/// `pid` から PPid を辿り、launcher の直接の子（session root）を返す。launcher に届かなければ None。
fn session_root(table: &HashMap<i32, &ProcEntry>, pid: i32) -> Option<i32> {
    let mut current = pid;
    for _ in 0..16 {
        let parent = table.get(&table.get(&current)?.ppid)?;
        if is_launcher(parent) {
            return Some(current);
        }
        if parent.pid <= 1 {
            return None;
        }
        current = parent.pid;
    }
    None
}

/// 試験の前に無かった PID のうち、Chrome の browser process（pipe CDP・`--type=` なし）で
/// uid_map が launcher の観測と同じで、launcher の子孫であるものを全部返す（pid 順）。
fn chrome_pick_candidates(
    table: &[ProcEntry],
    before: &HashSet<i32>,
    expected_map: &str,
) -> Vec<ChromeCandidate> {
    let by_pid: HashMap<i32, &ProcEntry> = table.iter().map(|e| (e.pid, e)).collect();
    let mut picked: Vec<_> = table
        .iter()
        .filter(|e| !before.contains(&e.pid) && is_browser_argv(&e.argv))
        .filter(|e| {
            e.uid_map
                .as_deref()
                .is_some_and(|map| same_map_from_reader(map, expected_map))
        })
        .filter_map(|e| {
            Some(ChromeCandidate {
                pid: e.pid,
                session_root: session_root(&by_pid, e.pid)?,
            })
        })
        .collect();
    picked.sort_by_key(|c| c.pid);
    picked
}

/// 自分の session を止めた後、検査済みの session root のうち消えたもの。
/// 空でなければ、止めた自分の session は検査済みの中にあった。
fn chrome_pick_stopped_roots(checked: &[ChromeCandidate], alive: &HashSet<i32>) -> Vec<i32> {
    let mut roots: Vec<_> = checked
        .iter()
        .map(|c| c.session_root)
        .filter(|root| !alive.contains(root))
        .collect();
    roots.sort_unstable();
    roots.dedup();
    roots
}

fn read_proc_table() -> Vec<ProcEntry> {
    proc_pids()
        .into_iter()
        .filter_map(|pid| {
            let base = PathBuf::from(format!("/proc/{pid}"));
            let ppid = fs::read_to_string(base.join("status"))
                .ok()?
                .lines()
                .find_map(|line| line.strip_prefix("PPid:"))?
                .trim()
                .parse()
                .ok()?;
            let argv = fs::read(base.join("cmdline"))
                .ok()?
                .split(|b| *b == 0)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect();
            Some(ProcEntry {
                pid,
                ppid,
                argv,
                uid_map: fs::read_to_string(base.join("uid_map")).ok(),
            })
        })
        .collect()
}

fn chrome_candidates(before: &HashSet<i32>, expected_map: &str) -> Vec<ChromeCandidate> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let candidates = chrome_pick_candidates(&read_proc_table(), before, expected_map);
        if !candidates.is_empty() {
            return candidates;
        }
        if Instant::now() >= deadline {
            let matching_map: Vec<_> = read_proc_table()
                .into_iter()
                .filter(|e| !before.contains(&e.pid))
                .filter(|e| {
                    e.uid_map
                        .as_deref()
                        .is_some_and(|map| same_map_from_reader(map, expected_map))
                })
                .map(|e| (e.pid, e.argv.first().cloned().unwrap_or_default()))
                .collect();
            panic!(
                "Chrome PID not visible in /proc within 20s; new processes with expected uid_map: {matching_map:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn starttime(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
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

/// 読めたら panic。読めなければ errno を返す（拒否か消えたかは呼ぶ側が決める）。
fn denied_read(path: &Path) -> Option<i32> {
    let mut buf = [0u8; 1];
    let error = File::open(path)
        .and_then(|mut file| file.read(&mut buf).map(|_| ()))
        .expect_err(&format!("{} unexpectedly readable", path.display()));
    eprintln!("{}: errno={:?}", path.display(), error.raw_os_error());
    error.raw_os_error()
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
    // なった bwrap が Chrome の userns を作る（--dev の devpts のため bwrap 内で 2 段）。Chrome から
    // launcher の ns までの owner の鎖は [S, S, B] で、launcher は鎖に daemon UID が無いことを検査する。
    let subuid = subid_start("/etc/subuid").expect("celeris-browser subuid");
    assert_eq!(observed.host_uid, browser_uid);
    assert_eq!(observed.ns_owner_uid, Some(subuid));
    assert_ne!(observed.ns_owner_uid, Some(daemon_uid));
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

    // 並走 session があっても曖昧にしない: 新しい Chrome 候補の全部に同じ拒否検査を当てる
    // （冒頭の『並走 session での Chrome 特定』）。
    let candidates = chrome_candidates(&before, &observed.uid_map);
    eprintln!("Chrome candidates (pid, session root): {candidates:?}");
    let checked: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|c| check_candidate(c.pid, daemon_uid, &observed))
        .collect();
    assert!(
        !checked.is_empty(),
        "no Chrome candidate stayed alive through the denial checks: {candidates:?}"
    );

    // 自分の session を止め、検査済みの session root のどれかが消えることを確かめる。
    drop(session);
    let deadline = Instant::now() + Duration::from_secs(15);
    let stopped = loop {
        let stopped = chrome_pick_stopped_roots(&checked, &proc_pids());
        if !stopped.is_empty() || Instant::now() >= deadline {
            break stopped;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    eprintln!("stopped own session: checked session roots gone={stopped:?}");
    assert!(
        !stopped.is_empty(),
        "own session was not among the checked Chrome candidates: {checked:?}"
    );
}

/// Chrome 候補 1 件に拒否検査を当てる。検査の途中で process が消えた（他の session の停止）
/// ときだけ false を返し、生きているのに拒否されない・期待外の errno は panic。
fn check_candidate(
    pid: i32,
    daemon_uid: u32,
    observed: &task_worker::browser_launcher::SessionFacts,
) -> bool {
    let Some(st) = starttime(pid) else {
        return false;
    };
    let gone = || starttime(pid) != Some(st);
    let denied = |errno: Option<i32>| matches!(errno, Some(libc::EACCES) | Some(libc::EPERM));
    let (Ok(uid_map), Ok(gid_map)) = (
        fs::read_to_string(format!("/proc/{pid}/uid_map")),
        fs::read_to_string(format!("/proc/{pid}/gid_map")),
    ) else {
        assert!(gone(), "Chrome {pid} maps unreadable while alive");
        return false;
    };
    eprintln!(
        "Chrome pid={pid} NS_GET_OWNER_UID(launcher)={:?} uid_map={uid_map:?} gid_map={gid_map:?}",
        observed.ns_owner_uid
    );
    assert!(
        same_map_from_reader(&uid_map, &observed.uid_map),
        "uid_map {uid_map:?} != launcher {:?}",
        observed.uid_map
    );
    assert!(
        same_map_from_reader(&gid_map, &observed.gid_map),
        "gid_map {gid_map:?} != launcher {:?}",
        observed.gid_map
    );
    for map in [&uid_map, &gid_map] {
        assert!(
            !maps_host_id(map, daemon_uid),
            "daemon UID/GID must not be mapped: {map}"
        );
    }
    // daemon UID からは Chrome の namespace link も開けない（ptrace read の検査）。
    let ns_error = owner_uid(pid).expect_err("daemon UID opened Chrome /proc/<pid>/ns/user");
    eprintln!(
        "NS_GET_OWNER_UID from daemon UID: errno={:?}",
        ns_error.raw_os_error()
    );
    if !denied(ns_error.raw_os_error()) {
        assert!(gone(), "NS_GET_OWNER_UID on {pid}: {ns_error}");
        return false;
    }
    let error = ptrace_attach(pid).expect_err("daemon UID attached to Chrome");
    eprintln!(
        "PTRACE_ATTACH Chrome pid={pid}: errno={:?}",
        error.raw_os_error()
    );
    if error.raw_os_error() != Some(libc::EPERM) {
        assert!(gone(), "PTRACE_ATTACH on {pid}: {error}");
        return false;
    }
    strace_denied(pid);
    for file in ["environ", "mem"] {
        let errno = denied_read(&PathBuf::from(format!("/proc/{pid}/{file}")));
        if !denied(errno) {
            assert!(
                gone(),
                "/proc/{pid}/{file}: expected EACCES/EPERM, got {errno:?}"
            );
            return false;
        }
    }
    true
}

const LAUNCHER_MAP: &str = "      1000     296608          1\n";

fn proc(pid: i32, ppid: i32, argv: &[&str], uid_map: Option<&str>) -> ProcEntry {
    ProcEntry {
        pid,
        ppid,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        uid_map: uid_map.map(str::to_owned),
    }
}

/// launcher（pid 100）の下の 1 session: bwrap（root）→ bwrap の init → sandboxd → Chrome と renderer。
fn session_tree(root: i32) -> Vec<ProcEntry> {
    let map = Some(LAUNCHER_MAP);
    vec![
        proc(root, 100, &["/usr/bin/bwrap", "--info-fd", "5"], None),
        proc(root + 1, root, &["/usr/bin/bwrap", "--info-fd", "5"], map),
        proc(
            root + 2,
            root + 1,
            &["celeris-browser-sandboxd", "--shared-cdp"],
            map,
        ),
        proc(
            root + 3,
            root + 2,
            &["/opt/celeris-browser/chrome", "--remote-debugging-pipe"],
            map,
        ),
        proc(
            root + 4,
            root + 3,
            &[
                "/opt/celeris-browser/chrome",
                "--type=renderer",
                "--remote-debugging-pipe",
            ],
            map,
        ),
    ]
}

fn base_table() -> Vec<ProcEntry> {
    vec![
        proc(1, 0, &["/sbin/init"], Some("0 0 4294967295\n")),
        proc(
            100,
            1,
            &[
                "/usr/local/libexec/celeris/celeris-browser-launcher",
                "--config",
                "x",
            ],
            None,
        ),
    ]
}

#[test]
fn chrome_pick_parallel_sessions_returns_every_candidate_with_its_session_root() {
    let mut table = base_table();
    table.extend(session_tree(2000));
    table.extend(session_tree(3000));
    let before: HashSet<i32> = [1, 100].into();
    let picked = chrome_pick_candidates(&table, &before, LAUNCHER_MAP);
    assert_eq!(
        picked,
        vec![
            ChromeCandidate {
                pid: 2003,
                session_root: 2000
            },
            ChromeCandidate {
                pid: 3003,
                session_root: 3000
            },
        ]
    );
    // 自分の session（root 3000）を止めると、検査済みの中から 3000 だけが消える。
    let alive: HashSet<i32> = table
        .iter()
        .map(|e| e.pid)
        .filter(|pid| !(3000..3005).contains(pid))
        .collect();
    assert_eq!(chrome_pick_stopped_roots(&picked, &alive), vec![3000]);
}

#[test]
fn chrome_pick_own_session_only_returns_its_browser_process() {
    let mut table = base_table();
    table.extend(session_tree(2000));
    let before: HashSet<i32> = [1, 100].into();
    let picked = chrome_pick_candidates(&table, &before, LAUNCHER_MAP);
    assert_eq!(
        picked,
        vec![ChromeCandidate {
            pid: 2003,
            session_root: 2000
        }]
    );
    let alive: HashSet<i32> = [1, 100].into();
    assert_eq!(chrome_pick_stopped_roots(&picked, &alive), vec![2000]);
}

#[test]
fn chrome_pick_excludes_old_foreign_and_unmapped_chrome() {
    let mut table = base_table();
    // 試験より前からある session（PID が before にある）。
    table.extend(session_tree(2000));
    // launcher の子孫でない Chrome（同 UID の別 runtime）。
    table.push(proc(
        4000,
        1,
        &["/opt/celeris-browser/chrome", "--remote-debugging-pipe"],
        Some(LAUNCHER_MAP),
    ));
    // launcher の子孫だが map が違う。
    let mut odd = session_tree(5000);
    for e in &mut odd {
        e.uid_map = e.uid_map.as_ref().map(|_| "1000 1001 1\n".to_owned());
    }
    table.extend(odd);
    let before: HashSet<i32> = [1, 100, 2000, 2001, 2002, 2003, 2004].into();
    assert!(chrome_pick_candidates(&table, &before, LAUNCHER_MAP).is_empty());
    // 何も検査していなければ、止めても検査済みの root は無い。
    assert!(chrome_pick_stopped_roots(&[], &HashSet::new()).is_empty());
}

#[test]
fn chrome_pick_reader_without_subuid_mapping_still_matches() {
    let mut table = base_table();
    let mut tree = session_tree(2000);
    for e in &mut tree {
        e.uid_map = e.uid_map.as_ref().map(|_| "1000 4294967295 1\n".to_owned());
    }
    table.extend(tree);
    let picked = chrome_pick_candidates(&table, &[1, 100].into(), LAUNCHER_MAP);
    assert_eq!(picked.len(), 1);
    // 他の session が止まっても、自分の root が生きていれば消えたことにしない。
    let alive: HashSet<i32> = [2000].into();
    assert!(chrome_pick_stopped_roots(&picked, &alive).is_empty());
}
