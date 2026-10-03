//! Exercise the actual independently launched proxy, without external networking.
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use nix::libc;

const POLICY: &[u8] = br#"{"allow":["example.com:443"],"resolver":"127.0.0.1","allow_ipv6":false}"#;

fn spawn() -> (Child, UnixStream) {
    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let fd = server.as_raw_fd();
    let mut cmd =
        Command::new(std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("proxy binary"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Only async-signal-safe operations between fork and exec; the parent retains
    // ownership of server until spawn returns. fd 3 is the documented protocol.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    (cmd.spawn().unwrap(), client)
}

/// helper の終了を待つ。主判定は終了そのもので、60 秒は止まったときの保険（ADR-0125）。
/// 終了までの時間の上限は、それを主張する試験が別に確かめる。
fn finish(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("egress helper did not terminate within its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn denied(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"browser egress denied\n");
}

#[test]
fn independent_proxy_refuses_worker_selected_private_or_proxy_destinations() {
    for request in [
        "CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n\r\n",
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: sentinel-secret\r\n\r\n",
        "CONNECT example.com:853 HTTP/1.1\r\nHost: example.com:853\r\n\r\n",
    ] {
        let (mut child, mut client) = spawn();
        child.stdin.take().unwrap().write_all(POLICY).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = vec![];
        client.read_to_end(&mut response).unwrap();
        assert_eq!(
            response,
            b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
        );
        denied(&finish(child));
    }
}

#[test]
fn malformed_or_oversized_policy_never_appears_in_process_output() {
    for bytes in [b"sentinel-secret".to_vec(), vec![b'X'; 65537]] {
        let (mut child, _client) = spawn();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        denied(&finish(child));
    }
}

#[test]
fn policy_eof_deadline_terminates_process_without_a_blocking_stdin_thread() {
    let (child, _client) = spawn();
    // Retain the policy writer and send no data. A blocking stdin implementation
    // would keep runtime shutdown waiting forever after the timeout.
    let start = Instant::now();
    denied(&finish(child));
    assert!(start.elapsed() < Duration::from_secs(8));
}

#[test]
fn peer_disconnect_terminates_the_proxy() {
    let (mut child, client) = spawn();
    child.stdin.take().unwrap().write_all(POLICY).unwrap();
    drop(client);
    denied(&finish(child));
}

#[test]
fn missing_inherited_socket_is_refused() {
    let mut cmd =
        Command::new(std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("proxy binary"));
    unsafe {
        cmd.pre_exec(|| {
            libc::close(3);
            Ok(())
        });
    }
    denied(&cmd.stdin(Stdio::null()).output().unwrap());
}

#[test]
fn controller_death_kills_and_reaps_the_actual_proxy_even_with_a_live_client() {
    // The isolated test wrapper is a subreaper; changing this property in the
    // cargo test process would interfere with unrelated child-process tests.
    let script = r#"
import ctypes, json, os, signal, socket, subprocess, sys, time
libc = ctypes.CDLL(None, use_errno=True)
assert libc.prctl(36, 1, 0, 0, 0) == 0
client, server = socket.socketpair()
read_fd, write_fd = os.pipe()
parent = os.fork()
if parent == 0:
    os.close(read_fd)
    os.dup2(server.fileno(), 3)
    child = subprocess.Popen([sys.argv[1]], pass_fds=(3,), stdin=subprocess.PIPE,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    child.stdin.write(b'{"allow":["example.com:443"],"resolver":"127.0.0.1","allow_ipv6":false}')
    child.stdin.close()
    os.write(write_fd, str(child.pid).encode() + b'\n')
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        with open('/proc/%d/status' % child.pid) as f:
            if 'NoNewPrivs:\t1' in f.read():
                try: os.readlink('/proc/%d/fd/0' % child.pid)
                except PermissionError: os._exit(0)
        time.sleep(.01)
    os._exit(1)
os.close(write_fd)
server.close()
pid = None
def expired(*_):
    raise RuntimeError('deadline')
signal.signal(signal.SIGALRM, expired)
signal.alarm(90)
try:
    with os.fdopen(read_fd) as f:
        pid = int(f.readline())
    _, parent_status = os.waitpid(parent, 0)
    parent = None
    assert os.waitstatus_to_exitcode(parent_status) == 0
    _, status = os.waitpid(pid, 0)
    pid = None
    assert os.waitstatus_to_exitcode(status) == -signal.SIGKILL, status
    print('parent-death SIGKILL; proxy reaped')
finally:
    signal.alarm(0)
    client.close()
    for target in [parent, pid]:
        if target:
            try: os.kill(target, signal.SIGKILL)
            except ProcessLookupError: pass
    while True:
        try: os.waitpid(-1, 0)
        except ChildProcessError: break
"#;
    let child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(std::env::var("CARGO_BIN_EXE_celeris-browser-egress").expect("proxy binary"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let output = finish(child);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"parent-death SIGKILL; proxy reaped\n");
}
