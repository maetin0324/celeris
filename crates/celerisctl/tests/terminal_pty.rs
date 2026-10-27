//! `celerisctl terminal-pty`（ADR 2026-10-27-web-terminal D1）の試験。偽の shell（`/bin/sh` の固定スクリプト）
//! で、入出力・リサイズ・stdin の EOF と子の終了で session の process（別 process group の job、SIGHUP を
//! 無視する job を含む）と、`setsid` で session を抜けて tty を持ったままの job が残らないことを確かめる。
//! 待ちは出来事（出力の行・process の消滅）で、上限は保険。

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(30);

const SCRIPT: &str = r#"
set -m
(trap '' HUP; exec sleep 300) &
echo "bg:$!"
sleep 300 &
echo "bg:$!"
echo ready
while read -r line; do
  case "$line" in
    size) stty size ;;
    quit) exit 3 ;;
    *) echo "got:$line" ;;
  esac
done
"#;

struct Pty {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<Vec<u8>>,
    seen: String,
}

impl Pty {
    fn start(script: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
            .args(["terminal-pty", "--cwd"])
            .arg(dir.path())
            .args([
                "--cols", "80", "--rows", "24", "--", "/bin/sh", "-c", script,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn helper");
        let mut out = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        // tempdir は helper の cwd。試験の間だけ残ればよい。
        std::mem::forget(dir);
        Pty {
            child,
            stdin,
            rx,
            seen: String::new(),
        }
    }

    fn frame(&mut self, kind: u8, body: &[u8]) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        let mut msg = vec![kind];
        msg.extend_from_slice(&(body.len() as u32).to_be_bytes());
        msg.extend_from_slice(body);
        stdin.write_all(&msg).expect("write frame");
        stdin.flush().expect("flush");
    }

    fn send(&mut self, text: &str) {
        self.frame(0, text.as_bytes());
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let mut body = cols.to_be_bytes().to_vec();
        body.extend_from_slice(&rows.to_be_bytes());
        self.frame(1, &body);
    }

    fn wait_for(&mut self, needle: &str) {
        let deadline = Instant::now() + LIMIT;
        while !self.seen.contains(needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(chunk) => self.seen.push_str(&String::from_utf8_lossy(&chunk)),
                Err(_) => panic!("timed out waiting for {needle:?}; output: {:?}", self.seen),
            }
        }
    }

    fn background_pids(&mut self) -> Vec<i32> {
        self.wait_for("ready");
        self.seen
            .lines()
            .filter_map(|l| l.trim().strip_prefix("bg:"))
            .map(|p| p.trim().parse().expect("pid"))
            .collect()
    }

    fn wait_exit(&mut self) -> i32 {
        let deadline = Instant::now() + LIMIT;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status.code().unwrap_or(-1);
            }
            assert!(Instant::now() < deadline, "helper did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn alive(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rfind(')')
            .map(|i| !stat[i + 1..].trim_start().starts_with('Z'))
            .unwrap_or(false),
        Err(_) => false,
    }
}

fn assert_gone(pids: &[i32]) {
    let deadline = Instant::now() + LIMIT;
    while pids.iter().any(|p| alive(*p)) {
        assert!(Instant::now() < deadline, "processes left: {pids:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn input_output_resize_and_exit_code() {
    let mut pty = Pty::start(SCRIPT);
    let pids = pty.background_pids();
    assert_eq!(pids.len(), 2);
    pty.send("hello 日本語\n");
    pty.wait_for("got:hello 日本語");
    pty.resize(100, 40);
    pty.send("size\n");
    pty.wait_for("40 100");
    pty.send("quit\n");
    assert_eq!(pty.wait_exit(), 3);
    // 子が終わった後、残った job（SIGHUP を無視するものを含む）も消える。
    assert_gone(&pids);
}

#[test]
fn stdin_eof_ends_the_whole_session() {
    let mut pty = Pty::start(SCRIPT);
    let pids = pty.background_pids();
    let shell = pty.child.id();
    drop(pty.stdin.take());
    pty.wait_exit();
    assert_gone(&pids);
    assert!(!alive(shell as i32));
}

#[test]
fn sigterm_ends_the_whole_session() {
    let mut pty = Pty::start(SCRIPT);
    let pids = pty.background_pids();
    let status = Command::new("kill")
        .args(["-TERM", &pty.child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    pty.wait_exit();
    assert_gone(&pids);
}

/// `setsid` で session を抜け、stdout・stderr に slave（tty）を持ったままの job。session の SIGHUP・SIGKILL は
/// 届かず、master の読み取りも EIO にならない。
const DETACHED: &str = r#"
setsid sleep 300 &
echo "bg:$!"
echo ready
while read -r line; do echo "got:$line"; done
"#;

/// helper の終了の上限。session の猶予 2 秒 + SIGKILL 後の待ち + pts の回収 + 読み切りの上限 2 秒に余裕を見る。
const DETACHED_LIMIT: Duration = Duration::from_secs(15);

fn wait_exit_within(pty: &mut Pty, limit: Duration) {
    let deadline = Instant::now() + limit;
    while pty.child.try_wait().expect("try_wait").is_none() {
        assert!(
            Instant::now() < deadline,
            "helper did not exit within {limit:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn detached_pid(pty: &mut Pty) -> i32 {
    let pids = pty.background_pids();
    assert_eq!(pids.len(), 1);
    // 確かに別 session（helper の子の shell の session ではない）で生きている。
    assert!(alive(pids[0]));
    pids[0]
}

#[test]
fn stdin_eof_ends_a_job_that_left_the_session_holding_the_tty() {
    let mut pty = Pty::start(DETACHED);
    let pid = detached_pid(&mut pty);
    drop(pty.stdin.take());
    wait_exit_within(&mut pty, DETACHED_LIMIT);
    assert_gone(&[pid]);
}

#[test]
fn sigterm_ends_a_job_that_left_the_session_holding_the_tty() {
    let mut pty = Pty::start(DETACHED);
    let pid = detached_pid(&mut pty);
    let status = Command::new("kill")
        .args(["-TERM", &pty.child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    wait_exit_within(&mut pty, DETACHED_LIMIT);
    assert_gone(&[pid]);
}
