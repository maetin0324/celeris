//! F1 diagnosis only (not for commit): real Chrome on --remote-debugging-pipe, wrapped by the
//! production CdpController + SharedCdp relay, bridged to TCP like sandboxd does.
//! Usage: f1_relay_repro <chrome> <workdir> <tcp_port> <allowed_origin>
use std::fs::File;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{FromRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use task_worker::browser_cdp_sink::CdpController;
use task_worker::browser_shared_cdp::SharedCdp;

const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn pipe() -> (File, File) {
    let mut fds = [0; 2];
    // SAFETY: two-element buffer.
    assert_eq!(unsafe { nix::libc::pipe2(fds.as_mut_ptr(), nix::libc::O_CLOEXEC) }, 0);
    // SAFETY: fresh fds.
    unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
}

fn copy(mut a: impl Read, mut b: impl Write) {
    let mut buf = [0u8; 65536];
    while let Ok(n) = a.read(&mut buf) {
        if n == 0 || b.write_all(&buf[..n]).is_err() {
            break;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let chrome = &args[1];
    let work = PathBuf::from(&args[2]);
    let port: u16 = args[3].parse().expect("port");
    let origin = args[4].clone();
    let (cmd_r, cmd_w) = pipe(); // chrome reads fd 3
    let (ev_r, ev_w) = pipe(); // chrome writes fd 4
    let r3 = cmd_r.into_raw_fd();
    let w4 = ev_w.into_raw_fd();
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
        &format!("--user-data-dir={}", work.join("profile").display()),
        "--remote-debugging-pipe",
        // No external network: everything but loopback goes to a dead proxy.
        "--proxy-server=http://127.0.0.1:9",
        "--proxy-bypass-list=127.0.0.1:17740",
        "about:blank",
    ]);
    // SAFETY: dup2 is async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            if nix::libc::dup2(r3, 3) < 0 || nix::libc::dup2(w4, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            for fd in [3, 4] {
                nix::libc::fcntl(fd, nix::libc::F_SETFD, 0);
            }
            Ok(())
        });
    }
    let mut child = command.spawn().expect("chrome spawn");
    // SAFETY: close parent copies of the child's ends.
    unsafe {
        nix::libc::close(r3);
        nix::libc::close(w4);
    }
    eprintln!("chrome pid {}", child.id());
    let controller = CdpController::new(cmd_w, ev_r);
    let socket = work.join("cdp-relay.sock");
    let _relay = SharedCdp::start(controller, &socket, TOKEN.into(), vec![origin]).expect("relay");
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    eprintln!("READY ws://127.0.0.1:{port}/{TOKEN}");
    std::thread::spawn(move || {
        for tcp in listener.incoming().flatten() {
            let socket = socket.clone();
            std::thread::spawn(move || {
                let Ok(unix) = UnixStream::connect(&socket) else { return };
                let (t2, u2) = (tcp.try_clone().expect("tcp"), unix.try_clone().expect("unix"));
                let h = std::thread::spawn(move || {
                    copy(&t2, &u2);
                    let _ = u2.shutdown(Shutdown::Write);
                });
                copy(&unix, &tcp);
                let _ = tcp.shutdown(Shutdown::Both);
                let _ = h.join();
            });
        }
    });
    let _ = child.wait();
    let _ = TcpStream::connect(("127.0.0.1", port));
}
