//! ADR-0108 D1: sandbox の最初の process。netns の `127.0.0.1:3128` を listen してから
//! argv の browser（または probe）を子として起動し、accept した TCP を controller が
//! 返す unix stream（celeris-browser-egress の FD 3 の相手）へ byte 単位で中継する。
//! HTTP は解釈しない。listen できなければ子を起動せずに固定コードで終わる。
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};
use std::sync::mpsc;
use std::time::Duration;

use nix::libc;
use task_worker::browser_relay::{
    CHANNEL_FD, CONNECT, GRANT, LISTEN_PORT, READY, check_channel, recv, send,
};
use task_worker::browser_shared_cdp::RELAY_PORT;

/// listen 失敗・channel 不正（browser を起動しない）。
const EXIT_SETUP: u8 = 70;

// Chromium diagnostics may contain URLs, profile contents or credentials.
// Only fixed categories cross the launcher journal boundary.
fn chrome_stderr_category(line: &str) -> Option<&'static str> {
    if line.contains("ProcessSingleton") || line.contains("profile is in use") {
        Some("profile-lock")
    } else if line.contains("Permission denied") || line.contains("Operation not permitted") {
        Some("permission-denied")
    } else if line.contains("ERROR:") || line.contains("FATAL:") {
        Some("other-startup-error")
    } else {
        None
    }
}

fn report_chrome_stderr(mut stderr: impl Read) {
    let mut chunk = [0u8; 2048];
    let mut line = Vec::new();
    let mut reported = 0;
    while let Ok(n) = stderr.read(&mut chunk) {
        if n == 0 {
            break;
        }
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                if reported < 4
                    && let Some(category) = chrome_stderr_category(&String::from_utf8_lossy(&line))
                {
                    eprintln!("sandboxd: Chrome stderr category={category}");
                    reported += 1;
                }
                line.clear();
            } else if line.len() < 2048 {
                line.push(byte);
            }
        }
    }
}

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

/// The profile's initial preferences. Chrome (Chrome for Testing in the launcher) opens a PDF
/// served inline in its built-in viewer instead of downloading it, so a `download` of a PDF link
/// waited for a download that never began, and the tab was left on the viewer. With
/// `always_open_pdf_externally` every PDF becomes an ordinary download (chrome-headless-shell has
/// no viewer and already does this). An existing profile is left as it is.
const PROFILE_PREFERENCES: &[u8] = br#"{"plugins":{"always_open_pdf_externally":true}}"#;

fn prepare_profile(profile: &std::path::Path) -> std::io::Result<()> {
    let default = profile.join("Default");
    std::fs::create_dir_all(&default)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(default.join("Preferences"))
    {
        Ok(mut file) => file.write_all(PROFILE_PREFERENCES),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

fn main() -> ExitCode {
    // channel を検査し、子に継承させない。
    // SAFETY: fd 6 に対する fcntl だけ。
    if let Err(e) = check_channel(CHANNEL_FD) {
        eprintln!("sandboxd: relay channel invalid: {e}");
        return ExitCode::from(EXIT_SETUP);
    }
    if unsafe { libc::fcntl(CHANNEL_FD, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        eprintln!(
            "sandboxd: relay channel cloexec: {}",
            std::io::Error::last_os_error()
        );
        return ExitCode::from(EXIT_SETUP);
    }
    let argv: Vec<_> = std::env::args_os().skip(1).collect();
    let (chrome, action) = if argv.first().is_some_and(|a| a == "--shared-cdp") {
        if argv.len() < 3 {
            eprintln!("sandboxd: shared CDP arguments missing");
            return ExitCode::from(EXIT_SETUP);
        }
        (Some(argv[1].clone()), &argv[2..])
    } else {
        (None, argv.as_slice())
    };
    let Some((program, args)) = action.split_first() else {
        eprintln!("sandboxd: action arguments missing");
        return ExitCode::from(EXIT_SETUP);
    };
    let listener = match TcpListener::bind(("127.0.0.1", LISTEN_PORT)) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("sandboxd: proxy listen: {e}");
            return ExitCode::from(EXIT_SETUP);
        }
    };
    if let Err(e) = send(CHANNEL_FD, READY, None) {
        eprintln!("sandboxd: send ready: {e}");
        return ExitCode::from(EXIT_SETUP);
    }
    if let Some(chrome) = chrome {
        let cdp_listener = match TcpListener::bind(("127.0.0.1", RELAY_PORT)) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("sandboxd: CDP listen: {e}");
                return ExitCode::from(EXIT_SETUP);
            }
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
        if let Err(e) = prepare_profile(std::path::Path::new("/session/profile")) {
            // Only the error kind crosses (no path or content).
            eprintln!("sandboxd: profile preferences: {:?}", e.kind());
        }
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
        command.stderr(std::process::Stdio::piped());
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
        let mut browser = match command.spawn() {
            Ok(browser) => browser,
            Err(e) => {
                eprintln!("sandboxd: Chrome spawn: {e}");
                return ExitCode::from(EXIT_SETUP);
            }
        };
        // Only a PID and the fixed CDP transport reach the launcher journal.
        eprintln!(
            "sandboxd: Chrome started pid={} flags=remote-debugging-pipe",
            browser.id()
        );
        let (diagnostics_done, diagnostics_rx) = mpsc::channel();
        if let Some(stderr) = browser.stderr.take() {
            std::thread::spawn(move || {
                report_chrome_stderr(stderr);
                let _ = diagnostics_done.send(());
            });
        }
        std::thread::spawn(move || {
            match browser.wait() {
                Ok(status) => {
                    // Give the bounded diagnostic reader time to drain an
                    // immediately exiting browser before sandboxd exits.
                    let _ = diagnostics_rx.recv_timeout(Duration::from_millis(200));
                    eprintln!(
                        "sandboxd: Chrome exited code={:?} signal={:?}",
                        status.code(),
                        std::os::unix::process::ExitStatusExt::signal(&status)
                    );
                }
                Err(e) => eprintln!("sandboxd: Chrome wait failed errno={:?}", e.raw_os_error()),
            }
            std::process::exit(i32::from(EXIT_SETUP));
        });
        // The action process must never inherit Chromium's pipe endpoints.
        for fd in [3, 4] {
            // SAFETY: these are the inherited CDP fds, held by sandboxd.
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                eprintln!(
                    "sandboxd: CDP pipe cloexec: {}",
                    std::io::Error::last_os_error()
                );
                return ExitCode::from(EXIT_SETUP);
            }
        }
    }
    let mut child = match Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("sandboxd: action spawn: {e}");
            return ExitCode::from(EXIT_SETUP);
        }
    };
    // 子が終われば runtime も終わる（sandboxd は pid namespace の 1 番なので残りは消える）。
    std::thread::spawn(move || {
        let status = child.wait();
        eprintln!("sandboxd: action exited: {status:?}");
        let code = status.ok().and_then(|s| s.code()).unwrap_or(1);
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

#[cfg(test)]
mod tests {
    use super::chrome_stderr_category;

    #[test]
    fn chrome_diagnostics_expose_only_fixed_categories() {
        assert_eq!(
            chrome_stderr_category("Failed to create a ProcessSingleton for /secret/profile"),
            Some("profile-lock")
        );
        assert_eq!(
            chrome_stderr_category("/secret/path: Permission denied"),
            Some("permission-denied")
        );
        assert_eq!(chrome_stderr_category("https://secret.example"), None);
    }
}
