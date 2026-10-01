//! ADR-0108 D2: browser・celeris-browser-sandboxd・接続ごとの celeris-browser-egress を 1 runtime として
//! `Supervisor` で起動し、controller process の SIGKILL・記録からの再起動時回収・正常停止のそれぞれの後に
//! 記録した process が `/proc` から消えることを実プロセスで確かめる。
//!
//! 中身は実 chrome-headless-shell（proxy 経由で local fixture の本文を取る）と、sandbox 内から
//! proxy 経由の CONNECT tunnel を張り続ける bash で、controller が死ぬ時点で egress が稼働している。
//! `/bin/sleep` の runtime は使わない（PID 再利用の試験の「別プロセス」役だけ sleep を使う）。
//! 試験は自分自身を `unshare --user --map-root-user --net` の中で再実行し、その試験用 netns の `lo`
//! に fixture を置く（外部ネットワークに出ない）。前提（unshare・ip・openssl・bwrap・browser）が
//! 無い環境では失敗する。明示的に `CELERIS_ISOLATION_TESTS=skip` を与えた時だけ飛ばす。
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use task_worker::browser_runtime::{
    DEFAULT_MAX_EGRESS, EgressRelay, RecordedProcess, RuntimeSpec, process_starttime, read_record,
    reap_recorded, same_process_alive, write_record,
};
use task_worker::browser_supervisor::{Supervisor, SupervisorOptions, reap_on_start};

const INNER: &str = "CELERIS_SUPERVISOR_TEST_INNER";
const MODE: &str = "CELERIS_SUPERVISOR_CTRL_MODE";
const DIRS: &str = "CELERIS_SUPERVISOR_CTRL_DIRS";
const FIXTURE_IP: &str = "93.184.216.34";
const MARKER: &str = "celeris-supervisor-fixture-body-7c2e";
const POLICY: &str = r#"{"allow":["fixture.example.com:443","fixture.example.com:8443"],"resolver":"127.0.0.1","allow_ipv6":false}"#;
const HOLD: &str = r#"#!/bin/bash
( exec 3<>/dev/tcp/127.0.0.1/3128
  printf 'CONNECT fixture.example.com:8443 HTTP/1.1\r\nHost: fixture.example.com:8443\r\n\r\n' >&3
  head -c 12 <&3 > /session/tunnel.txt
  exec cat <&3 >/dev/null ) &
exec "$@"
"#;

fn tool(name: &str) -> PathBuf {
    let p = PathBuf::from("/usr/bin").join(name);
    assert!(
        p.exists(),
        "{name} not found at {p:?}; this real-runtime test cannot run without it"
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

fn sh(cmd: &str) {
    let st = Command::new("/bin/sh").args(["-c", cmd]).status().unwrap();
    assert!(st.success(), "{cmd} failed");
}

fn dns_server(listener: TcpListener) {
    for conn in listener.incoming() {
        let Ok(mut c) = conn else { continue };
        let mut len = [0u8; 2];
        if c.read_exact(&mut len).is_err() {
            continue;
        }
        let mut q = vec![0u8; usize::from(u16::from_be_bytes(len))];
        if c.read_exact(&mut q).is_err() || q.len() < 17 {
            continue;
        }
        let mut pos = 12;
        let mut labels = vec![];
        while q[pos] != 0 {
            let n = usize::from(q[pos]);
            labels.push(String::from_utf8_lossy(&q[pos + 1..pos + 1 + n]).into_owned());
            pos += n + 1;
        }
        let qname = labels.join(".");
        let qtype = u16::from_be_bytes([q[pos + 1], q[pos + 2]]);
        let answer: Option<Vec<u8>> = match (qname.as_str(), qtype) {
            ("fixture.example.com", 1) => Some(vec![93, 184, 216, 34]),
            ("private.example.com", 1) => Some(vec![10, 0, 0, 5]),
            ("v6.example.com", 28) => Some(
                "2606:2800:220:1::1"
                    .parse::<std::net::Ipv6Addr>()
                    .unwrap()
                    .octets()
                    .to_vec(),
            ),
            _ => None,
        };
        let mut r = q[..pos + 5].to_vec();
        r[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
        r[6..8].copy_from_slice(&u16::from(answer.is_some()).to_be_bytes());
        if let Some(a) = answer {
            r.extend([0xc0, 0x0c]);
            r.extend(qtype.to_be_bytes());
            r.extend([0, 1, 0, 0, 0, 30]);
            r.extend((a.len() as u16).to_be_bytes());
            r.extend(a);
        }
        let _ = c.write_all(&(r.len() as u16).to_be_bytes());
        let _ = c.write_all(&r);
    }
}

fn start_fixture(dir: &Path) -> Child {
    std::fs::write(
        dir.join("index.html"),
        format!("<html><body>{MARKER}</body></html>"),
    )
    .unwrap();
    sh(&format!(
        "cd {} && openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem \
         -days 1 -subj /CN=fixture.example.com -addext subjectAltName=DNS:fixture.example.com \
         >/dev/null 2>&1",
        dir.display()
    ));
    let child = Command::new(tool("openssl"))
        .args([
            "s_server", "-quiet", "-WWW", "-cert", "cert.pem", "-key", "key.pem", "-accept",
        ])
        .arg(format!("{FIXTURE_IP}:443"))
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect((FIXTURE_IP, 443)).is_err() {
        assert!(Instant::now() < deadline, "fixture did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

struct Cdp {
    w: std::fs::File,
    rx: mpsc::Receiver<serde_json::Value>,
    next: u64,
}

impl Cdp {
    fn new(rt: &mut Supervisor) -> Self {
        let w = rt.cdp_write.take().unwrap();
        let mut r = rt.cdp_read.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 65536];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                while let Some(i) = buf.iter().position(|b| *b == 0) {
                    let msg: Vec<u8> = buf.drain(..=i).collect();
                    if let Ok(v) = serde_json::from_slice(&msg[..i]) {
                        let _ = tx.send(v);
                    }
                }
            }
        });
        Self { w, rx, next: 1 }
    }

    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        session: Option<&str>,
    ) -> serde_json::Value {
        let id = self.next;
        self.next += 1;
        let mut m = serde_json::json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            m["sessionId"] = s.into();
        }
        let mut bytes = serde_json::to_vec(&m).unwrap();
        bytes.push(0);
        self.w.write_all(&bytes).unwrap();
        // 高負荷の評価器では browser の起動・navigation の失敗確定が 20 秒を超えることがある。
        // 待ちの上限は長く取り、成否は返答の中身（MARKER の有無）だけで判定する。
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let v = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|e| panic!("CDP reply to {method} (id {id}): {e:?}"));
            if v["id"] == id {
                return v;
            }
        }
    }

    fn page(&mut self) -> String {
        let t = self.call(
            "Target.createTarget",
            serde_json::json!({"url": "about:blank"}),
            None,
        );
        let target = t["result"]["targetId"].as_str().unwrap().to_owned();
        let a = self.call(
            "Target.attachToTarget",
            serde_json::json!({"targetId": target, "flatten": true}),
            None,
        );
        a["result"]["sessionId"].as_str().unwrap().to_owned()
    }

    /// `url` を開き、`wait` の間に本文へ MARKER が現れたかを返す。
    fn fetch_marker(&mut self, s: &str, url: &str, wait: Duration) -> bool {
        let nav = self.call("Page.navigate", serde_json::json!({"url": url}), Some(s));
        println!("CTRL NAV {nav}");
        let deadline = Instant::now() + wait;
        let mut last = serde_json::Value::Null;
        while Instant::now() < deadline {
            let v = self.call(
                "Runtime.evaluate",
                serde_json::json!({"expression": "document.documentElement.outerHTML", "returnByValue": true}),
                Some(s),
            );
            if v["result"]["result"]["value"]
                .as_str()
                .is_some_and(|h| h.contains(MARKER))
            {
                return true;
            }
            last = v;
            std::thread::sleep(Duration::from_millis(200));
        }
        println!("CTRL LAST {last}");
        false
    }
}

/// proxy 経由の tunnel の相手（開いた数と閉じた数を数える）。
fn hold_server(listener: TcpListener, open: Arc<AtomicUsize>, closed: Arc<AtomicUsize>) {
    for conn in listener.incoming() {
        let Ok(mut c) = conn else { continue };
        open.fetch_add(1, Ordering::SeqCst);
        let closed = closed.clone();
        std::thread::spawn(move || {
            let mut b = [0u8; 1024];
            while matches!(c.read(&mut b), Ok(n) if n > 0) {}
            closed.fetch_add(1, Ordering::SeqCst);
        });
    }
}

fn browser_argv() -> Vec<OsString> {
    [
        browser().to_string_lossy().as_ref(),
        "--headless",
        "--no-sandbox",
        "--no-zygote",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        "--user-data-dir=/session/profile",
        "--disable-background-networking",
        "--disable-component-update",
        "--no-first-run",
        // 試験で作った自己署名の fixture 証明書だけのため（proxy の検査とは無関係）。
        "--ignore-certificate-errors",
        "--proxy-server=http://127.0.0.1:3128",
    ]
    .iter()
    .map(OsString::from)
    .collect()
}

fn roles(p: &[RecordedProcess]) -> Vec<String> {
    let mut r: Vec<String> = p.iter().map(|p| p.role.clone()).collect();
    r.sort();
    r.dedup();
    r
}

/// controller process の本体（MODE がある時だけ動く）。
#[test]
fn controller_main() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let dirs = std::env::var(DIRS).unwrap();
    let (record_dir, session_dir) = dirs.split_once(':').unwrap();
    let record_dir = PathBuf::from(record_dir);
    if mode == "reap" {
        let killed = reap_on_start(&record_dir, Duration::from_secs(30)).unwrap();
        println!("CTRL REAPED {killed:?}");
        return;
    }
    let session_dir = PathBuf::from(session_dir);
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join("hold.sh"), HOLD).unwrap();
    let sandboxd = PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-browser-sandboxd").unwrap());
    let proxy = PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-browser-egress").unwrap());
    let exe = browser();
    let cdp_pipe = mode != "b";
    let mut argv: Vec<OsString> = vec![
        sandboxd.clone().into_os_string(),
        "/usr/bin/bash".into(),
        "/session/hold.sh".into(),
    ];
    argv.extend(browser_argv());
    if cdp_pipe {
        argv.push("--remote-debugging-pipe".into());
    }
    argv.push("about:blank".into());
    let spec = RuntimeSpec {
        bwrap: tool("bwrap"),
        session_id: format!("sup-{mode}"),
        session_dir: session_dir.clone(),
        ro_dirs: vec![
            exe.parent().unwrap().to_path_buf(),
            sandboxd.parent().unwrap().to_path_buf(),
        ],
        argv,
        cdp_pipe,
        egress: Some(EgressRelay {
            proxy,
            policy: POLICY.as_bytes().to_vec(),
            max_concurrent: DEFAULT_MAX_EGRESS,
        }),
    };
    let mut opts = SupervisorOptions::new(&record_dir);
    // (b) は PDEATHSIG の取りこぼしを再現する（起動時回収が拾うことを示すため）。
    opts.arm_parent_death = mode != "b";
    let mut sup = Supervisor::start(spec, opts).unwrap();
    if cdp_pipe {
        let mut cdp = Cdp::new(&mut sup);
        let s = cdp.page();
        assert!(
            cdp.fetch_marker(
                &s,
                "https://fixture.example.com/index.html",
                Duration::from_secs(60)
            ),
            "fixture body must arrive through the proxy"
        );
        println!("CTRL POSITIVE ok");
        std::mem::forget(cdp);
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let tunnel = std::fs::read_to_string(session_dir.join("tunnel.txt")).unwrap_or_default();
        let r = roles(&sup.processes());
        if tunnel.starts_with("HTTP/1.1 200")
            && ["browser", "bwrap", "egress", "sandboxd"]
                .iter()
                .all(|x| r.iter().any(|y| y == x))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "runtime not up: tunnel={tunnel:?} roles={r:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("CTRL READY {}", sup.record_path().display());
    std::io::stdout().flush().unwrap();
    if mode == "c" {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        let before = sup.processes();
        sup.stop();
        let left: Vec<_> = before
            .iter()
            .filter(|p| same_process_alive(p.pid, p.starttime))
            .collect();
        println!("CTRL STOPPED left={left:?}");
        return;
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

struct Ctrl {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

fn controller(mode: &str, record: &Path, session: &Path) -> Ctrl {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "controller_main",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(MODE, mode)
        .env(DIRS, format!("{}:{}", record.display(), session.display()))
        .env_remove(INNER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let out = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            eprintln!("[ctrl] {l}");
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let stdin = child.stdin.take();
    Ctrl {
        child,
        stdin,
        lines,
    }
}

fn expect_line(c: &Ctrl, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let l = c
            .lines
            .recv_timeout(left)
            .unwrap_or_else(|e| panic!("controller never printed {prefix}: {e:?}"));
        // libtest の `test controller_main ... ` が同じ行の前に付くことがある。
        if let Some(i) = l.find(prefix) {
            return l[i + prefix.len()..].trim().to_owned();
        }
    }
}

fn sigkill(c: &mut Ctrl) {
    // SAFETY: 自分が起こしてまだ wait していない子。
    unsafe { nix::libc::kill(c.child.id() as i32, nix::libc::SIGKILL) };
    let _ = c.child.wait();
}

fn wait_gone(procs: &[RecordedProcess], within: Duration) -> Vec<RecordedProcess> {
    let deadline = Instant::now() + within;
    loop {
        let left: Vec<_> = procs
            .iter()
            .filter(|p| same_process_alive(p.pid, p.starttime))
            .cloned()
            .collect();
        if left.is_empty() || Instant::now() >= deadline {
            return left;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 記録が「4 役が揃い、載っている process が全て生きている」状態になるまで読み直す。
/// 記録は supervisor が 100ms ごとに採り直すので、browser の fixture 取得に使った接続ごとの
/// egress が READY の直後に終わると、書き直し前の記録に死んだ egress が一時的に残る。
/// 時計ではなくこの条件を待ち、上限を過ぎたら最後に読んだ記録で `assert_full_runtime` を判定する。
fn read_full_runtime(path: &Path) -> Vec<RecordedProcess> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let procs = read_record(path);
        let r = roles(&procs);
        let full = ["browser", "bwrap", "egress", "sandboxd"]
            .iter()
            .all(|w| r.iter().any(|x| x == w));
        if full && procs.iter().all(|p| same_process_alive(p.pid, p.starttime)) {
            return procs;
        }
        if Instant::now() >= deadline {
            assert_full_runtime(&procs);
            return procs;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assert_full_runtime(procs: &[RecordedProcess]) {
    let r = roles(procs);
    for want in ["browser", "bwrap", "egress", "sandboxd"] {
        assert!(
            r.iter().any(|x| x == want),
            "{want} not recorded: {procs:?}"
        );
    }
    for p in procs {
        assert!(same_process_alive(p.pid, p.starttime), "not alive: {p:?}");
    }
}

#[test]
fn runtime_processes_do_not_survive_controller_kill_restart_or_stop() {
    if std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("skip") {
        eprintln!("SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip");
        return;
    }
    for t in ["unshare", "ip", "openssl", "bwrap", "bash"] {
        tool(t);
    }
    browser();
    let out = Command::new(tool("unshare"))
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inner_supervisor_in_test_netns",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("--- inner stdout ---\n{stdout}\n--- inner stderr ---\n{stderr}");
    assert!(out.status.success(), "inner test failed: {:?}", out.status);
    assert!(stdout.contains("1 passed"), "inner test did not run");
    for tag in ["(a) ok", "(b) ok", "(c) ok"] {
        assert!(
            stderr.contains(&format!("SUPERVISOR-EVIDENCE {tag}")),
            "missing {tag}"
        );
    }
}

#[test]
fn inner_supervisor_in_test_netns() {
    if std::env::var(INNER).is_err() {
        return;
    }
    sh(&format!(
        "ip link set lo up && ip addr add {FIXTURE_IP}/32 dev lo"
    ));
    let dns = TcpListener::bind("127.0.0.1:53").unwrap();
    std::thread::spawn(move || dns_server(dns));
    let (open, closed) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let hold = TcpListener::bind((FIXTURE_IP, 8443)).unwrap();
    let (o, c) = (open.clone(), closed.clone());
    std::thread::spawn(move || hold_server(hold, o, c));
    let fixture_dir = tempfile::tempdir().unwrap();
    let mut fixture = start_fixture(fixture_dir.path());
    let base = tempfile::tempdir().unwrap();

    // (a) controller を SIGKILL → PDEATHSIG と pid namespace で browser・sandboxd・proxy が消える。
    let rec_a = base.path().join("rec-a");
    let mut ca = controller("a", &rec_a, &base.path().join("sess-a"));
    expect_line(&ca, "CTRL POSITIVE");
    let path = PathBuf::from(expect_line(&ca, "CTRL READY"));
    let procs = read_full_runtime(&path);
    eprintln!("SUPERVISOR-EVIDENCE (a) recorded {procs:?}");
    let held = open.load(Ordering::SeqCst);
    assert!(held >= 1 && closed.load(Ordering::SeqCst) < held);
    sigkill(&mut ca);
    let left = wait_gone(&procs, Duration::from_secs(30));
    assert!(left.is_empty(), "(a) survived controller SIGKILL: {left:?}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while closed.load(Ordering::SeqCst) < open.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "(a) proxy tunnel still open");
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!(
        "SUPERVISOR-EVIDENCE (a) ok: 0 of {} remain, tunnel closed",
        procs.len()
    );

    // (b) PDEATHSIG を取りこぼした runtime: controller の SIGKILL 後も残る → 新しい controller の
    //     起動時回収（記録の pid+starttime 照合）で残存 0。
    let rec_b = base.path().join("rec-b");
    let mut cb = controller("b", &rec_b, &base.path().join("sess-b"));
    let path = PathBuf::from(expect_line(&cb, "CTRL READY"));
    let procs = read_full_runtime(&path);
    eprintln!("SUPERVISOR-EVIDENCE (b) recorded {procs:?}");
    sigkill(&mut cb);
    std::thread::sleep(Duration::from_secs(1));
    let orphans: Vec<_> = procs
        .iter()
        .filter(|p| same_process_alive(p.pid, p.starttime))
        .cloned()
        .collect();
    eprintln!("SUPERVISOR-EVIDENCE (b) orphans after controller SIGKILL {orphans:?}");
    for want in ["bwrap", "sandboxd", "browser"] {
        assert!(
            orphans.iter().any(|p| p.role == want),
            "(b) {want} should be orphaned before reap: {orphans:?}"
        );
    }
    assert!(path.exists(), "record must survive the controller");
    let mut cr = controller("reap", &rec_b, &base.path().join("unused"));
    let reaped = expect_line(&cr, "CTRL REAPED");
    assert!(cr.child.wait().unwrap().success());
    let left = wait_gone(&procs, Duration::from_secs(5));
    assert!(left.is_empty(), "(b) survived restart reap: {left:?}");
    assert!(!path.exists(), "record must be removed by reap");
    eprintln!(
        "SUPERVISOR-EVIDENCE (b) ok: reaped {reaped}, 0 of {} remain",
        procs.len()
    );

    // (c) 正常停止（TERM → 猶予 → KILL → wait）で残存 0、記録も消える。
    let rec_c = base.path().join("rec-c");
    let mut cc = controller("c", &rec_c, &base.path().join("sess-c"));
    expect_line(&cc, "CTRL POSITIVE");
    let path = PathBuf::from(expect_line(&cc, "CTRL READY"));
    let procs = read_full_runtime(&path);
    eprintln!("SUPERVISOR-EVIDENCE (c) recorded {procs:?}");
    writeln!(cc.stdin.as_mut().unwrap(), "stop").unwrap();
    let stopped = expect_line(&cc, "CTRL STOPPED");
    assert_eq!(stopped, "left=[]");
    assert!(cc.child.wait().unwrap().success());
    let left = wait_gone(&procs, Duration::from_secs(1));
    assert!(left.is_empty(), "(c) survived stop: {left:?}");
    assert!(!path.exists(), "record must be removed on stop");
    eprintln!("SUPERVISOR-EVIDENCE (c) ok: 0 of {} remain", procs.len());

    let _ = fixture.kill();
    let _ = fixture.wait();
}

/// PID 再利用: 記録の pid が生きていても starttime が違えば signal を送らない（別プロセスを守る）。
/// ここの `sleep` は「無関係な別プロセス」役で、runtime の証拠ではない。
#[test]
fn reap_does_not_signal_a_reused_pid() {
    let dir = tempfile::tempdir().unwrap();
    let mut other = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let pid = other.id() as i32;
    let st = process_starttime(pid).unwrap();
    for role in ["bwrap", "browser"] {
        write_record(
            dir.path(),
            "reused",
            &[RecordedProcess {
                pid,
                starttime: st + 1,
                role: role.into(),
            }],
        )
        .unwrap();
        assert!(reap_recorded(dir.path()).unwrap().is_empty());
        assert!(!dir.path().join("reused.pid").exists());
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            other.try_wait().unwrap().is_none(),
            "reused pid was signalled"
        );
    }
    // 回収済みの pid（zombie も含めて本人が居ない）にも送らない。
    // 対照: starttime が一致すれば本人として回収する。
    write_record(
        dir.path(),
        "same",
        &[RecordedProcess {
            pid,
            starttime: st,
            role: "browser".into(),
        }],
    )
    .unwrap();
    assert_eq!(reap_recorded(dir.path()).unwrap(), vec![pid]);
    let status = other.wait().unwrap();
    assert!(!status.success());
    // wait で回収した後の pid は記録に残っていても本人ではない → 何も送らない。
    write_record(
        dir.path(),
        "gone",
        &[RecordedProcess {
            pid,
            starttime: st,
            role: "bwrap".into(),
        }],
    )
    .unwrap();
    assert!(reap_recorded(dir.path()).unwrap().is_empty());
}
