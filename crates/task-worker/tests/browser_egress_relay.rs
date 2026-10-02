//! ADR-0108 D1: 実 bwrap + 実 chrome-headless-shell + 実 sandboxd + 実 celeris-browser-egress で、
//! local fixture へは proxy 経由でだけ届くことを確かめる。
//!
//! 試験は自分自身を `unshare --user --map-root-user --net` の中で再実行し、その試験用 netns の
//! `lo` に公開扱いの `93.184.216.34` を付けて HTTPS fixture（`openssl s_server`）と TCP/53 の DNS
//! fixture を置く。外部ネットワークへの経路は無い。proxy の拒否境界は変えず、許可する名前と
//! resolver は試験の stdin policy にだけ書く。前提（unshare・ip・openssl・bwrap・browser）が
//! 無い環境では失敗する。既定では skip し、`CELERIS_USERNS_TESTS=1` を与えた時だけ走る（ADR-0126 B）。
mod userns_gate;

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use task_worker::browser_runtime::{DEFAULT_MAX_EGRESS, EgressRelay, IsolatedRuntime, RuntimeSpec};

const INNER: &str = "CELERIS_RELAY_TEST_INNER";
const FIXTURE_IP: &str = "93.184.216.34";
const MARKER: &str = "celeris-relay-fixture-body-4b1d";
const POLICY: &str = r#"{"allow":["fixture.example.com:443","private.example.com:443","v6.example.com:443"],"resolver":"127.0.0.1","allow_ipv6":false}"#;

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

fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| {
            s.rsplit(')')
                .next()
                .is_some_and(|r| !r.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

#[test]
fn fixture_reachable_only_through_per_connection_egress_proxy() {
    if userns_gate::skip_unless_userns_tests() {
        return;
    }
    run_fixture_reachable_only_through_per_connection_egress_proxy();
}

fn run_fixture_reachable_only_through_per_connection_egress_proxy() {
    for t in ["unshare", "ip", "openssl", "bwrap", "bash"] {
        tool(t);
    }
    browser();
    let out = Command::new(tool("unshare"))
        .args(["--user", "--map-root-user", "--net", "--"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inner_relay_in_test_netns",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    eprintln!("--- inner stdout ---\n{stdout}\n--- inner stderr ---\n{stderr}");
    assert!(
        out.status.success(),
        "inner relay test failed: {:?}",
        out.status
    );
    assert!(stdout.contains("1 passed"), "inner test did not run");
    assert!(
        stderr.contains("RELAY-EVIDENCE positive ok"),
        "no positive evidence"
    );
}

// ---- 以下は試験用 netns の中でだけ動く（INNER が無ければ何もしない補助） ----

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

const PROBE: &str = r#"#!/bin/bash
out=/session/probe.txt
: > "$out"
direct() {
  if timeout 3 bash -c "exec 3<>/dev/tcp/$2/$3" 2>/dev/null; then echo "direct $1 open" >>"$out"; else echo "direct $1 blocked" >>"$out"; fi
}
via_proxy() {
  resp=$(timeout 8 bash -c 'exec 3<>/dev/tcp/127.0.0.1/3128; printf "CONNECT %s HTTP/1.1\r\nHost: %s\r\n\r\n" "$1" "$1" >&3; head -c 12 <&3' _ "$2" 2>/dev/null)
  echo "proxy $1 ${resp:-none}" >>"$out"
}
direct fixture_ip 93.184.216.34 443
direct private_ip 10.0.0.5 443
direct ipv6_loopback ::1 443
direct ipv6_public 2606:2800:220:1::1 443
direct dns_tcp_host 127.0.0.1 53
direct dns_tcp_fixture 93.184.216.34 53
direct loopback_443 127.0.0.1 443
if timeout 3 getent hosts fixture.example.com >/dev/null 2>&1; then echo "direct dns_udp open" >>"$out"; else echo "direct dns_udp blocked" >>"$out"; fi
via_proxy ok fixture.example.com:443
via_proxy ip_literal 93.184.216.34:443
via_proxy private_name private.example.com:443
via_proxy private_literal 10.0.0.5:443
via_proxy ipv6_name v6.example.com:443
via_proxy ipv6_literal [::1]:443
via_proxy not_allowed other.example.com:443
via_proxy dns_port fixture.example.com:53
echo END >>"$out"
for _ in $(seq 600); do [ -e /session/go ] && exec "$@"; sleep 0.1; done
exit 3
"#;
const PROXY_PROBES: usize = 8;

struct Cdp {
    w: std::fs::File,
    rx: mpsc::Receiver<serde_json::Value>,
    next: u64,
}

impl Cdp {
    fn new(rt: &mut IsolatedRuntime) -> Self {
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
        self.call("Page.navigate", serde_json::json!({"url": url}), Some(s));
        let deadline = Instant::now() + wait;
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
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    }
}

fn launch(session: &Path, proxy: PathBuf, argv: Vec<OsString>) -> IsolatedRuntime {
    let sandboxd = PathBuf::from(
        std::env::var("CARGO_BIN_EXE_celeris-browser-sandboxd").expect("sandboxd binary"),
    );
    let exe = browser();
    let mut full = vec![sandboxd.clone().into_os_string()];
    full.extend(argv);
    IsolatedRuntime::launch(&RuntimeSpec {
        bwrap: tool("bwrap"),
        session_id: "relay-test".into(),
        session_dir: session.to_path_buf(),
        ro_dirs: vec![
            exe.parent().unwrap().to_path_buf(),
            sandboxd.parent().unwrap().to_path_buf(),
        ],
        argv: full,
        cdp_pipe: true,
        egress: Some(EgressRelay {
            proxy,
            policy: POLICY.as_bytes().to_vec(),
            max_concurrent: DEFAULT_MAX_EGRESS,
        }),
    })
    .unwrap()
}

fn browser_argv(with_proxy: bool) -> Vec<OsString> {
    let mut v: Vec<OsString> = [
        browser().to_string_lossy().as_ref(),
        "--headless",
        "--no-sandbox",
        "--no-zygote",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        "--remote-debugging-pipe",
        "--user-data-dir=/session/profile",
        "--disable-background-networking",
        "--disable-component-update",
        "--no-first-run",
        // 試験で作った自己署名の fixture 証明書だけのため（proxy の検査とは無関係）。
        "--ignore-certificate-errors",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if with_proxy {
        v.push("--proxy-server=http://127.0.0.1:3128".into());
    }
    v.push("about:blank".into());
    v
}

fn wait_exited(rt: &IsolatedRuntime, n: usize) -> task_worker::browser_runtime::EgressStats {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let s = rt.egress_stats().unwrap();
        if s.spawned.len() >= n && s.exited.len() == s.spawned.len() {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "egress processes did not all exit: {s:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn inner_relay_in_test_netns() {
    if std::env::var(INNER).is_err() {
        return;
    }
    let proxy =
        PathBuf::from(std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("proxy binary"));
    sh(&format!(
        "ip link set lo up && ip addr add {FIXTURE_IP}/32 dev lo"
    ));
    let dns = TcpListener::bind("127.0.0.1:53").unwrap();
    std::thread::spawn(move || dns_server(dns));
    let fixture_dir = tempfile::tempdir().unwrap();
    let mut fixture = start_fixture(fixture_dir.path());

    // (c)(d): 同じ runtime の中の probe（sandboxd の子 bash）→ 後で同じ runtime で browser を exec。
    let session = tempfile::tempdir().unwrap();
    std::fs::write(session.path().join("probe.sh"), PROBE).unwrap();
    let mut argv: Vec<OsString> = vec!["/usr/bin/bash".into(), "/session/probe.sh".into()];
    argv.extend(browser_argv(true));
    let mut rt = launch(session.path(), proxy.clone(), argv);
    let probe_file = session.path().join("probe.txt");
    let deadline = Instant::now() + Duration::from_secs(120);
    let probe = loop {
        let s = std::fs::read_to_string(&probe_file).unwrap_or_default();
        if s.ends_with("END\n") {
            break s;
        }
        assert!(Instant::now() < deadline, "probe did not finish: {s}");
        std::thread::sleep(Duration::from_millis(100));
    };
    eprintln!("RELAY-EVIDENCE probe\n{probe}");
    for line in probe.lines().filter(|l| l.starts_with("direct ")) {
        assert!(
            line.ends_with(" blocked"),
            "direct egress must fail: {line}"
        );
    }
    assert!(probe.contains("proxy ok HTTP/1.1 200"), "{probe}");
    for name in [
        "ip_literal",
        "private_name",
        "private_literal",
        "ipv6_name",
        "ipv6_literal",
        "not_allowed",
        "dns_port",
    ] {
        assert!(
            probe.contains(&format!("proxy {name} HTTP/1.1 403")),
            "{name} must be refused by the proxy: {probe}"
        );
    }
    // (d) proxy 経由の接続 1 本ごとに egress が 1 つ起動し、終了している。
    let stats = wait_exited(&rt, PROXY_PROBES);
    eprintln!("RELAY-EVIDENCE probe egress {stats:?}");
    assert_eq!(
        stats.spawned.len(),
        PROXY_PROBES,
        "one egress per connection"
    );
    let mut uniq = stats.spawned.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), PROXY_PROBES, "distinct process per connection");
    assert!(stats.spawned.iter().all(|p| !alive(*p)));

    // (a) 同じ runtime で実 browser が proxy 経由で fixture の本文を取る。
    std::fs::write(session.path().join("go"), "").unwrap();
    let mut cdp = Cdp::new(&mut rt);
    let s = cdp.page();
    let before = rt.egress_stats().unwrap().spawned.len();
    assert!(
        cdp.fetch_marker(
            &s,
            "https://fixture.example.com/index.html",
            Duration::from_secs(60)
        ),
        "fixture body must arrive through the proxy"
    );
    let after = rt.egress_stats().unwrap().spawned.len();
    assert!(after > before, "browser fetch must start an egress process");
    eprintln!("RELAY-EVIDENCE positive ok (egress spawned {before}->{after})");
    // (c) browser からの迂回・拒否対象（同じ runtime）。
    for url in [
        format!("https://{FIXTURE_IP}/index.html"),
        "https://private.example.com/index.html".into(),
        "https://v6.example.com/index.html".into(),
        "https://[::1]/index.html".into(),
        "https://localhost/index.html".into(),
        "https://127.0.0.1/index.html".into(),
        "https://other.example.com/index.html".into(),
    ] {
        assert!(
            !cdp.fetch_marker(&s, &url, Duration::from_secs(3)),
            "{url} must not reach the fixture"
        );
    }
    eprintln!("RELAY-EVIDENCE browser negatives ok");
    rt.kill();
    let stats = wait_exited(&rt, after);
    assert!(stats.spawned.iter().all(|p| !alive(*p)));
    eprintln!(
        "RELAY-EVIDENCE browser egress spawned={} exited={}",
        stats.spawned.len(),
        stats.exited.len()
    );

    // (b) proxy を外す: browser に proxy を指定しない → 届かず、egress も起動しない。
    let session2 = tempfile::tempdir().unwrap();
    let mut rt2 = launch(session2.path(), proxy, browser_argv(false));
    let mut cdp2 = Cdp::new(&mut rt2);
    let s2 = cdp2.page();
    assert!(!cdp2.fetch_marker(
        &s2,
        "https://fixture.example.com/index.html",
        Duration::from_secs(5)
    ));
    assert!(!cdp2.fetch_marker(
        &s2,
        &format!("https://{FIXTURE_IP}/index.html"),
        Duration::from_secs(3)
    ));
    assert_eq!(rt2.egress_stats().unwrap().spawned.len(), 0);
    rt2.kill();
    eprintln!("RELAY-EVIDENCE no-proxy negative ok");

    // (b) proxy を止める: controller が egress を起動できない → listener はあるが届かない。
    let session3 = tempfile::tempdir().unwrap();
    let mut rt3 = launch(
        session3.path(),
        PathBuf::from("/nonexistent/celeris-browser-egress"),
        browser_argv(true),
    );
    let mut cdp3 = Cdp::new(&mut rt3);
    let s3 = cdp3.page();
    assert!(!cdp3.fetch_marker(
        &s3,
        "https://fixture.example.com/index.html",
        Duration::from_secs(5)
    ));
    let st3 = rt3.egress_stats().unwrap();
    assert!(st3.refused >= 1 && st3.spawned.is_empty(), "{st3:?}");
    rt3.kill();
    eprintln!("RELAY-EVIDENCE stopped-proxy negative ok ({st3:?})");

    let _ = fixture.kill();
    let _ = fixture.wait();
}
