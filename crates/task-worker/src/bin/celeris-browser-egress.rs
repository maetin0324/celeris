//! One inherited connection, one proxy process (ADR-0086).
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use nix::libc;
use task_core::browser_isolation::EgressPolicy;

fn inherited_stream() -> Result<UnixStream, ()> {
    let mut socket_type: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&socket_type) as libc::socklen_t;
    // Validate the inherited descriptor before taking ownership. No user-provided
    // descriptor number, path, endpoint or command is accepted.
    let rc = unsafe {
        libc::getsockopt(
            3,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut socket_type).cast(),
            &mut size,
        )
    };
    if rc != 0 || socket_type != libc::SOCK_STREAM {
        return Err(());
    }
    // SAFETY: getsockopt established that fd 3 is open. This process owns it and
    // takes ownership exactly once, before spawning threads that could close it.
    let owned = unsafe { OwnedFd::from_raw_fd(3) };
    let stream = UnixStream::from(owned);
    // A TCP fd must not be accepted as the trusted inherited Unix transport.
    let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of_val(&addr) as libc::socklen_t;
    if unsafe { libc::getsockname(3, (&raw mut addr).cast(), &mut len) } != 0
        || i32::from(addr.ss_family) != libc::AF_UNIX
        || stream.peer_addr().is_err()
    {
        return Err(());
    }
    stream.set_nonblocking(true).map_err(|_| ())?;
    Ok(stream)
}

fn main() -> ExitCode {
    // This helper has no persistent state and must not outlive its controller.
    let parent = unsafe { libc::getppid() };
    if parent <= 1
        || unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0
        || unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) } != 0
        || unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0
        || unsafe { libc::getppid() } != parent
        || std::env::args_os().len() != 1
    {
        return fail();
    }
    let Ok(stream) = inherited_stream() else {
        return fail();
    };
    let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return fail();
    }
    // SAFETY: fcntl verified fd 0, owned by this process. No other stdin reader is used.
    let input = unsafe { std::fs::File::from_raw_fd(0) };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return fail();
    };
    let result = runtime.block_on(async {
        let mut bytes = vec![];
        // A nonblocking policy pipe avoids spawning a blocking read
        // thread which would keep runtime shutdown alive after the timeout.
        let stdin = tokio::io::unix::AsyncFd::new(input).map_err(|_| ())?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let mut ready = stdin.readable().await.map_err(|_| ())?;
                let result = ready.try_io(|fd| {
                    use std::io::Read;
                    let mut buffer = [0u8; 4096];
                    let read = fd.get_ref().read(&mut buffer)?;
                    Ok((buffer, read))
                });
                if let Ok(result) = result {
                    let (buffer, read) = result.map_err(|_| ())?;
                    if read == 0 {
                        break;
                    }
                    if bytes.len() + read > 65536 {
                        return Err(());
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| ())??;
        let policy: EgressPolicy = serde_json::from_slice(&bytes).map_err(|_| ())?;
        let stream = tokio::net::UnixStream::from_std(stream).map_err(|_| ())?;
        task_worker::browser_egress::serve(stream, &policy)
            .await
            .map_err(|_| ())
    });
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        fail()
    }
}

fn fail() -> ExitCode {
    eprintln!("browser egress denied");
    ExitCode::from(2)
}
