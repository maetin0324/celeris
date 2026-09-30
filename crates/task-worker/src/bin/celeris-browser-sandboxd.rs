//! ADR-0088 D1: sandbox の最初の process。netns の `127.0.0.1:3128` を listen してから
//! argv の browser（または probe）を子として起動し、accept した TCP を controller が
//! 返す unix stream（celeris-browser-egress の FD 3 の相手）へ byte 単位で中継する。
//! HTTP は解釈しない。listen できなければ子を起動せずに固定コードで終わる。
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use nix::libc;
use task_worker::browser_relay::{
    CHANNEL_FD, CONNECT, GRANT, LISTEN_PORT, READY, check_channel, recv, send,
};
use task_worker::browser_shared_cdp::RELAY_PORT;

/// listen 失敗・channel 不正（browser を起動しない）。
const EXIT_SETUP: u8 = 70;

fn pump(mut from: impl Read, mut to: impl Write) {
    let mut buf = [0u8; 16384];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

/// 両方向を写し、片側の EOF は相手側の書き込み終了（half-close）として伝える。
fn relay(tcp: TcpStream, unix: UnixStream) {
    let (Ok(tcp_r), Ok(unix_w)) = (tcp.try_clone(), unix.try_clone()) else {
        return;
    };
    let up = std::thread::spawn(move || {
        pump(&tcp_r, &unix_w);
        let _ = unix_w.shutdown(Shutdown::Write);
    });
    pump(&unix, &tcp);
    let _ = tcp.shutdown(Shutdown::Write);
    let _ = up.join();
}

fn main() -> ExitCode {
    // channel を検査し、子に継承させない。
    // SAFETY: fd 6 に対する fcntl だけ。
    if check_channel(CHANNEL_FD).is_err()
        || unsafe { libc::fcntl(CHANNEL_FD, libc::F_SETFD, libc::FD_CLOEXEC) } < 0
    {
        eprintln!("sandboxd: relay channel missing");
        return ExitCode::from(EXIT_SETUP);
    }
    let argv: Vec<_> = std::env::args_os().skip(1).collect();
    let (chrome, action) = if argv.first().is_some_and(|a| a == "--shared-cdp") {
        if argv.len() < 3 {
            return ExitCode::from(EXIT_SETUP);
        }
        (Some(argv[1].clone()), &argv[2..])
    } else {
        (None, argv.as_slice())
    };
    let Some((program, args)) = action.split_first() else {
        return ExitCode::from(EXIT_SETUP);
    };
    let Ok(listener) = TcpListener::bind(("127.0.0.1", LISTEN_PORT)) else {
        eprintln!("sandboxd: listen failed");
        return ExitCode::from(EXIT_SETUP);
    };
    if send(CHANNEL_FD, READY, None).is_err() {
        return ExitCode::from(EXIT_SETUP);
    }
    if let Some(chrome) = chrome {
        let Ok(cdp_listener) = TcpListener::bind(("127.0.0.1", RELAY_PORT)) else {
            return ExitCode::from(EXIT_SETUP);
        };
        std::thread::spawn(move || {
            for connection in cdp_listener.incoming() {
                let Ok(tcp) = connection else { continue };
                std::thread::spawn(move || {
                    if let Ok(unix) = UnixStream::connect("/session/cdp-relay.sock") {
                        relay(tcp, unix);
                    }
                });
            }
        });
        let mut command = Command::new(chrome);
        command.args([
            "--headless",
            "--no-sandbox",
            "--no-zygote",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--disable-background-networking",
            "--disable-component-update",
            "--no-first-run",
            "--user-data-dir=/session/profile",
            "--remote-debugging-pipe",
            "--proxy-server=http://127.0.0.1:3128",
            "--proxy-bypass-list=<-loopback>",
        ]);
        #[cfg(feature = "h3-e2e-insecure-cert")]
        command.arg("--ignore-certificate-errors");
        command.arg("about:blank");
        // SAFETY: fcntl is async-signal-safe and only changes inherited CDP fds.
        unsafe {
            command.pre_exec(|| {
                for fd in [3, 4] {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let Ok(mut browser) = command.spawn() else {
            return ExitCode::from(EXIT_SETUP);
        };
        std::thread::spawn(move || {
            let _ = browser.wait();
            std::process::exit(i32::from(EXIT_SETUP));
        });
        // The action process must never inherit Chromium's pipe endpoints.
        for fd in [3, 4] {
            // SAFETY: these are the inherited CDP fds, held by sandboxd.
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return ExitCode::from(EXIT_SETUP);
            }
        }
    }
    let mut child = match Command::new(program).args(args).spawn() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("sandboxd: child spawn failed");
            return ExitCode::from(EXIT_SETUP);
        }
    };
    // 子が終われば runtime も終わる（sandboxd は pid namespace の 1 番なので残りは消える）。
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
        std::process::exit(code);
    });
    for tcp in listener.incoming() {
        let Ok(tcp) = tcp else { continue };
        // 要求と応答は 1 本ずつ順に対応させる（channel は accept loop だけが使う）。
        if send(CHANNEL_FD, CONNECT, None).is_err() {
            break;
        }
        match recv(CHANNEL_FD) {
            Ok(Some((GRANT, Some(fd)))) => {
                let unix = UnixStream::from(fd);
                std::thread::spawn(move || relay(tcp, unix));
            }
            Ok(Some(_)) => drop(tcp),
            Ok(None) | Err(_) => break,
        }
    }
    // channel が閉じた = controller が居ない。出口の無い browser を残さない。
    ExitCode::from(EXIT_SETUP)
}
