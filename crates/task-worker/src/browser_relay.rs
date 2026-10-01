//! ADR-0108 D1: netns 内 listener と sandbox 外 controller の間の request channel。
//!
//! channel は controller が作る無名の `SOCK_SEQPACKET` socketpair で、sandbox 側は継承 FD 6。
//! sandboxd → controller は 1 byte（[`READY`] / [`CONNECT`]）、controller → sandboxd は
//! [`GRANT`]（`SCM_RIGHTS` で unix stream を 1 本）か [`REFUSE`]（fd 無し）だけを送る。
//! HTTP は解釈しない（検査は celeris-browser-egress だけが行う）。

use std::os::fd::{FromRawFd, OwnedFd, RawFd};

use nix::libc;

/// sandbox 内で channel を受け取る FD 番号（CDP の 3/4、`--info-fd` 5 と重ねない）。
pub const CHANNEL_FD: RawFd = 6;
/// sandbox の netns の loopback で browser の proxy として listen する port。
pub const LISTEN_PORT: u16 = 3128;
pub const READY: u8 = b'R';
pub const CONNECT: u8 = b'C';
pub const GRANT: u8 = b'G';
pub const REFUSE: u8 = b'N';

/// `SOCK_SEQPACKET` の socketpair（両端 CLOEXEC）。
pub fn seqpacket_pair() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: fds は 2 要素。成功時の fd は新規で、ここで所有権を取る。
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: 直前の socketpair が返した fd。
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// 1 byte の message を送る。`fd` があれば `SCM_RIGHTS` で添える。
pub fn send(channel: RawFd, byte: u8, fd: Option<RawFd>) -> std::io::Result<()> {
    let mut data = [byte];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: 1,
    };
    // u64 で整列した control buffer（CMSG_SPACE(4) は 24 byte 以下）。
    let mut control = [0u64; 4];
    // SAFETY: msghdr は zero 初期化で有効。
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        // SAFETY: control は CMSG_SPACE(sizeof(int)) 以上で整列している。
        unsafe {
            let space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = space as _;
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
        }
    }
    loop {
        // SAFETY: msg は上で組んだ有効な msghdr。
        let n = unsafe { libc::sendmsg(channel, &msg, libc::MSG_NOSIGNAL) };
        if n == 1 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if n < 0 && e.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(e);
    }
}

/// 1 message を受ける。EOF は `Ok(None)`。添えられた fd は所有権を持って返す
/// （2 本以上や想定外の control message は閉じて誤りにする）。
pub fn recv(channel: RawFd) -> std::io::Result<Option<(u8, Option<OwnedFd>)>> {
    let mut data = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0u64; 8];
    // SAFETY: msghdr は zero 初期化で有効。
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&control) as _;
    let n = loop {
        // SAFETY: msg は有効な buffer を指す。受けた fd は CLOEXEC で開く。
        let n = unsafe { libc::recvmsg(channel, &mut msg, libc::MSG_CMSG_CLOEXEC) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        break n;
    };
    let mut fds = Vec::new();
    // SAFETY: recvmsg が埋めた control buffer を CMSG_* で辿る。
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let body = (*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for i in 0..body / std::mem::size_of::<RawFd>() {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    let truncated = msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0;
    if n == 0 && fds.is_empty() {
        return Ok(None);
    }
    if n != 1 || truncated || fds.len() > 1 {
        return Err(std::io::Error::other("unexpected relay message"));
    }
    Ok(Some((data[0], fds.pop())))
}

/// 継承 fd を検査する: `SOCK_SEQPACKET` の AF_UNIX でなければ誤り。
pub fn check_channel(fd: RawFd) -> std::io::Result<()> {
    let mut ty: libc::c_int = 0;
    let mut size = std::mem::size_of_val(&ty) as libc::socklen_t;
    // SAFETY: ty は c_int、size はその大きさ。
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut ty).cast(),
            &mut size,
        )
    } != 0
        || ty != libc::SOCK_SEQPACKET
    {
        return Err(std::io::Error::other(
            "relay channel is not a seqpacket socket",
        ));
    }
    Ok(())
}
