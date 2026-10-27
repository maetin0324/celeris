//! `celerisctl terminal-pty`: web 端末の PTY helper（ADR 2026-10-27-web-terminal D1）。
//!
//! web gateway が `systemd-run --user --scope` の中（試験では直接の子）で起こす。PTY を開き、子を新しい
//! session の先頭・制御端末つきで exec し、stdin の frame を PTY へ、PTY の出力をそのまま stdout へ流す。
//! LLM も DB も触らない。入出力は log に出さない（stderr に出すのは失敗の種類だけ）。
//!
//! stdin の frame: 1 byte の種類 + 4 byte（big endian）の長さ + 本文。
//! - `0`: 端末への入力（本文がそのまま PTY へ）
//! - `1`: リサイズ（本文 4 byte = cols u16 BE + rows u16 BE）
//!
//! 終わり方（D1 の終了の保証）: stdin の EOF・SIGTERM/SIGHUP/SIGINT・子の終了のどれでも、子の session
//! （sid = 子の pid）に属する全 process に SIGHUP、猶予の後 SIGKILL を送る。続けて session を抜けても slave（pts）を
//! 開いている自 uid の process に SIGKILL を送り、`celeris-terminal-*.scope` の中で自分以外の process が残って
//! いればその cgroup の `cgroup.kill` に書く。出力の読み切りは上限（`DRAIN_LIMIT`）つきで待つ。

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use clap::Args;

const FRAME_DATA: u8 = 0;
const FRAME_RESIZE: u8 = 1;
/// 1 frame の本文の上限（gateway は 64 KiB 以下に切って送る）。
const MAX_FRAME: usize = 1 << 20;
/// SIGHUP から SIGKILL までの猶予。
const HANGUP_GRACE: Duration = Duration::from_secs(2);
/// 終了後に PTY の出力を読み切る待ちの上限。超えたら読み切らずに終わる。
const DRAIN_LIMIT: Duration = Duration::from_secs(2);
/// 子の終了・停止の知らせを見る間隔（待ちの上限。CPU は使わない）。
const POLL: Duration = Duration::from_millis(50);

static STOP: AtomicBool = AtomicBool::new(false);

#[derive(Args, Debug)]
pub struct TerminalPtyArgs {
    /// 子の作業 dir。
    #[arg(long)]
    pub cwd: PathBuf,
    /// 最初の端末の幅。
    #[arg(long, default_value_t = 80)]
    pub cols: u16,
    /// 最初の端末の高さ。
    #[arg(long, default_value_t = 24)]
    pub rows: u16,
    /// 起こす argv（`--` の後ろ）。
    #[arg(last = true, required = true)]
    pub argv: Vec<String>,
}

enum Input {
    Data(Vec<u8>),
    Resize(u16, u16),
    Eof,
}

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn open_pty(cols: u16, rows: u16) -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let ws = winsize(cols, rows);
    // SAFETY: 出力引数は有効な int へのポインタ。name と termios は null（使わない）。
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &ws,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openpty が成功したので両方とも開いた fd で、ここだけが持つ。
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    // 子に master を継がせない。
    // SAFETY: 開いた fd への fcntl。
    unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok((master, slave))
}

fn read_frames(mut input: impl Read, tx: mpsc::Sender<Input>) {
    let mut head = [0_u8; 5];
    loop {
        if input.read_exact(&mut head).is_err() {
            break;
        }
        let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if len > MAX_FRAME {
            break;
        }
        let mut body = vec![0_u8; len];
        if input.read_exact(&mut body).is_err() {
            break;
        }
        let msg = match head[0] {
            FRAME_DATA => Input::Data(body),
            FRAME_RESIZE if len == 4 => Input::Resize(
                u16::from_be_bytes([body[0], body[1]]),
                u16::from_be_bytes([body[2], body[3]]),
            ),
            _ => continue,
        };
        if tx.send(msg).is_err() {
            return;
        }
    }
    let _ = tx.send(Input::Eof);
}

/// `sid` の session に属する process（自分を除く）の pid。
fn session_members(sid: i32) -> Vec<i32> {
    let me = std::process::id() as i32;
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| *pid != me)
        .filter(|pid| {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            // `pid (comm) state ppid pgrp session ...`。comm は空白・括弧を含みうるので最後の `)` から読む。
            let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
                return false;
            };
            let fields: Vec<&str> = rest.split_whitespace().collect();
            // fields[0]=state, [1]=ppid, [2]=pgrp, [3]=session。zombie は数えない（親が回収する）。
            fields.first() != Some(&"Z")
                && fields.get(3).and_then(|s| s.parse::<i32>().ok()) == Some(sid)
        })
        .collect()
}

fn signal_session(sid: i32, sig: libc::c_int) -> usize {
    let members = session_members(sid);
    for pid in &members {
        // SAFETY: kill は pid と signal 番号だけを受ける。
        unsafe { libc::kill(*pid, sig) };
    }
    members.len()
}

/// 自分が `celeris-terminal-*.scope` の中にいて、自分以外の process（zombie を除く）が残っていれば、その cgroup の
/// `cgroup.kill` に書いて全 process（自分を含む）を消す。何も残っていなければ書かない（子の終了 code を返せる）。
fn kill_own_scope() {
    let Ok(text) = std::fs::read_to_string("/proc/self/cgroup") else {
        return;
    };
    let Some(path) = text.lines().find_map(|l| l.strip_prefix("0::")) else {
        return;
    };
    let leaf = path.rsplit('/').next().unwrap_or_default();
    if !(leaf.starts_with("celeris-terminal-") && leaf.ends_with(".scope")) {
        return;
    }
    let me = std::process::id() as i32;
    let procs =
        std::fs::read_to_string(format!("/sys/fs/cgroup{path}/cgroup.procs")).unwrap_or_default();
    let others = procs
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .any(|pid| pid != me && alive(pid));
    if others {
        let _ = std::fs::write(format!("/sys/fs/cgroup{path}/cgroup.kill"), "1");
    }
}

/// zombie でない process か。
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rfind(')')
                .map(|i| stat[i + 1..].trim_start().to_string())
        })
        .is_some_and(|rest| !rest.starts_with('Z'))
}

/// `tty`（`/dev/pts/N`）を fd に開いている自 uid の process（自分を除く、zombie を除く）の pid。他の uid の
/// `/proc/<pid>/fd` は読めないので数えない。
fn pts_holders(tty: &std::path::Path) -> Vec<i32> {
    use std::os::unix::fs::MetadataExt;
    let me = std::process::id() as i32;
    // SAFETY: getuid は引数を取らず失敗しない。
    let uid = unsafe { libc::getuid() };
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| *pid != me)
        .filter(|pid| {
            std::fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == uid) && alive(*pid)
        })
        .filter(|pid| {
            let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
                return false;
            };
            fds.filter_map(|fd| fd.ok())
                .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|target| target == tty))
        })
        .collect()
}

/// session を抜けても slave を開いたままの process に SIGKILL を送り、消えるまで待つ（上限つき）。
fn kill_pts_holders(tty: &std::path::Path) {
    let deadline = Instant::now() + HANGUP_GRACE;
    loop {
        let holders = pts_holders(tty);
        if holders.is_empty() || Instant::now() >= deadline {
            return;
        }
        for pid in holders {
            // SAFETY: kill は pid と signal 番号だけを受ける。
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        std::thread::sleep(POLL);
    }
}

/// session の全 process を止める。SIGHUP → 猶予 → SIGKILL。子の回収は呼び出し側。
fn end_session(sid: i32, child: &mut std::process::Child) {
    signal_session(sid, libc::SIGHUP);
    let deadline = Instant::now() + HANGUP_GRACE;
    while Instant::now() < deadline {
        let _ = child.try_wait();
        if session_members(sid).is_empty() {
            return;
        }
        std::thread::sleep(POLL);
    }
    signal_session(sid, libc::SIGKILL);
    let _ = child.try_wait();
    // SIGKILL 後に消えるまで待つ（上限つき）。
    let deadline = Instant::now() + HANGUP_GRACE;
    while Instant::now() < deadline && !session_members(sid).is_empty() {
        let _ = child.try_wait();
        std::thread::sleep(POLL);
    }
}

pub fn run(args: TerminalPtyArgs) -> ExitCode {
    for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        // SAFETY: handler は atomic に書くだけ（async-signal-safe）。
        unsafe { libc::signal(sig, on_signal as *const () as libc::sighandler_t) };
    }
    let (master, slave) = match open_pty(args.cols, args.rows) {
        Ok(pair) => pair,
        Err(_) => {
            eprintln!("terminal-pty: openpty failed");
            return ExitCode::from(70);
        }
    };
    let (program, rest) = match args.argv.split_first() {
        Some(split) => split,
        None => return ExitCode::from(64),
    };
    // slave の名前（`/dev/pts/N`）。終了時に、session を抜けてもこれを開いている process を探す。
    let tty = std::fs::read_link(format!("/proc/self/fd/{}", slave.as_raw_fd())).ok();
    let stdio = |fd: &OwnedFd| fd.try_clone().map(Stdio::from);
    let (Ok(sin), Ok(sout), Ok(serr)) = (stdio(&slave), stdio(&slave), stdio(&slave)) else {
        eprintln!("terminal-pty: dup failed");
        return ExitCode::from(70);
    };
    let mut command = Command::new(program);
    command
        .args(rest)
        .current_dir(&args.cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(sin)
        .stdout(sout)
        .stderr(serr);
    // SAFETY: fork 後・exec 前に async-signal-safe な syscall（setsid・ioctl）だけを呼ぶ。
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            for sig in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            eprintln!("terminal-pty: spawn failed");
            return ExitCode::from(71);
        }
    };
    // Command は子に渡した slave の写しを持ったままなので、slave と一緒に閉じる（閉じないと master の読み取りが
    // EIO で終わらない）。
    drop(command);
    drop(slave);
    let sid = child.id() as i32;

    let Ok(reader) = master.try_clone() else {
        return ExitCode::from(70);
    };
    let (done_tx, done_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut reader = File::from(reader);
        let mut stdout = std::io::stdout().lock();
        let mut buf = [0_u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).is_err() || stdout.flush().is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                // slave 側が全て閉じると EIO。
                Err(_) => break,
            }
        }
        let _ = done_tx.send(());
    });
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || read_frames(std::io::stdin().lock(), tx));
    let mut writer = File::from(master);

    let mut status = None;
    loop {
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        if let Ok(Some(s)) = child.try_wait() {
            status = Some(s);
            break;
        }
        match rx.recv_timeout(POLL) {
            Ok(Input::Data(bytes)) => {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
            Ok(Input::Resize(cols, rows)) => {
                let ws = winsize(cols, rows);
                // SAFETY: 開いた master fd への TIOCSWINSZ（前面の process group に SIGWINCH が届く）。
                unsafe { libc::ioctl(writer.as_raw_fd(), libc::TIOCSWINSZ as _, &ws) };
            }
            Ok(Input::Eof) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    end_session(sid, &mut child);
    let status = status.or_else(|| child.wait().ok());
    if let Some(tty) = &tty {
        kill_pts_holders(tty);
    }
    kill_own_scope();
    drop(writer);
    // 出力の残りを読み切る（slave を持つ process はもう居ないので EIO で終わる）。他の uid が slave を持つなど
    // で終わらなくても、上限で切り上げて終了する（読み取りの thread は process の終了で消える）。
    let _ = done_rx.recv_timeout(DRAIN_LIMIT);
    match status {
        Some(s) => match s.code() {
            Some(code) => ExitCode::from(code.clamp(0, 255) as u8),
            None => ExitCode::from(128 + s.signal().unwrap_or(0).clamp(0, 127) as u8),
        },
        None => ExitCode::from(1),
    }
}
