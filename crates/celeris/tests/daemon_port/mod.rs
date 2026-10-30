//! 試験 daemon の API port を、その daemon 1 つだけのものにする。
//!
//! celeris は API を `SO_REUSEPORT` で bind する（ADR-0040 D4。standby が同じ port に相乗りするため）。同じ uid の
//! 2 つの試験 daemon が同じ port を選ぶと、両方の bind が成功し、kernel が接続ごとに振り分ける。`POST /tasks` が片方に、
//! `GET /tasks/<id>` がもう片方に届くと 404 `task_not_found` になる。`127.0.0.1:0` に bind して port を取り、すぐ閉じて
//! daemon に渡すと、閉じた後に別の試験が同じ port を取れる。
//!
//! [`PortReservation`] は `SO_REUSEPORT` だけを立てた socket を bind したまま（listen しない）持ち続ける。daemon の
//! bind（`SO_REUSEADDR` + `SO_REUSEPORT`、同じ uid）は通り、port 0 への bind（autobind）は使用中の port を選ばず、
//! `SO_REUSEPORT` の無い明示 bind は `EADDRINUSE` で落ちる。[`listening_pids`] は起動後にその port で listen している
//! process を `/proc` から数え、spawn した daemon 本人だけが応答していることを確かめる。
#![allow(dead_code)]

use std::{
    collections::BTreeSet,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

use socket2::{Domain, Socket, Type};

/// 試験の間だけ port を押さえる。drop で放す（daemon を止めた後に drop する）。
pub struct PortReservation {
    _socket: Socket,
    port: u16,
}

impl PortReservation {
    pub fn new() -> Self {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, None).expect("reserve socket");
        socket.set_reuse_port(true).expect("SO_REUSEPORT");
        socket
            .bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).into())
            .expect("reserve bind");
        let port = socket
            .local_addr()
            .expect("reserve addr")
            .as_socket()
            .expect("inet addr")
            .port();
        Self {
            _socket: socket,
            port,
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port))
    }
}

/// `127.0.0.1:<port>` で LISTEN している socket の持ち主の pid（読める /proc/<pid>/fd だけ。同じ uid の daemon は読める）。
pub fn listening_pids(port: u16) -> BTreeSet<u32> {
    let local = format!("0100007F:{port:04X}");
    let inodes: BTreeSet<String> = fs::read_to_string("/proc/net/tcp")
        .expect("/proc/net/tcp")
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // f[1] = local_address, f[3] = st（0A = LISTEN）, f[9] = inode
            (f.len() > 9 && f[1] == local && f[3] == "0A").then(|| f[9].to_string())
        })
        .collect();
    let mut pids = BTreeSet::new();
    if inodes.is_empty() {
        return pids;
    }
    for proc_entry in fs::read_dir("/proc").expect("/proc").flatten() {
        let Some(pid) = proc_entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = fs::read_dir(proc_entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = fs::read_link(fd.path()) else {
                continue;
            };
            let t = target.to_string_lossy();
            if let Some(ino) = t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']'))
                && inodes.contains(ino)
            {
                pids.insert(pid);
            }
        }
    }
    pids
}

/// 応答した daemon が `expected` 本人で、port で listen しているのも `expected` だけか。違えば理由を返す。
pub fn check_sole_owner(port: u16, expected: u32, identity_pid: u32) -> Result<(), String> {
    if identity_pid != expected {
        return Err(format!(
            "port {port}: identity.pid {identity_pid} answered, spawned daemon is {expected}"
        ));
    }
    let pids = listening_pids(port);
    if pids != BTreeSet::from([expected]) {
        return Err(format!(
            "port {port}: listeners {pids:?}, spawned daemon is {expected}"
        ));
    }
    Ok(())
}
