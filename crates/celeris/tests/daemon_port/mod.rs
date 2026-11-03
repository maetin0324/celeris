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

/// spawn した試験 daemon の API が答えるまで待つ（ADR-0125 の出来事待ち + 長い保険）。
///
/// - `daemon` が先に終われば、上限を待たずに run の結果（`Ok`・`Err` の文面か panic の文面）を入れたエラーを返す。
/// - 要求ごとに `request_timeout` を付ける。受け付けても答えない接続 1 本で残り時間を使い切らない。
/// - `overall` に達したら、経過秒と最後の接続エラーを入れたエラーを返す。
///
/// HTTP の応答が 1 つ返れば（status は問わない）ready とみなす。
pub async fn wait_api_ready<R: std::fmt::Debug>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    daemon: &mut tokio::task::JoinHandle<R>,
    request_timeout: std::time::Duration,
    overall: std::time::Duration,
) -> Result<(), String> {
    use std::time::Instant;
    let started = Instant::now();
    let deadline = started + overall;
    let mut attempts = 0u64;
    let mut last_error = String::from("no request finished yet");
    loop {
        if Instant::now() >= deadline {
            return Err(format!(
                "daemon API at {url} not ready after {:.1}s (limit {}s, {attempts} attempts); last error: {last_error}",
                started.elapsed().as_secs_f64(),
                overall.as_secs()
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let request = client
            .get(url)
            .bearer_auth(token)
            .timeout(request_timeout)
            .send();
        attempts += 1;
        tokio::select! {
            outcome = &mut *daemon => {
                return Err(format!(
                    "daemon exited before its API at {url} was ready ({:.1}s, {attempts} attempts; last error: {last_error}): {}",
                    started.elapsed().as_secs_f64(),
                    describe_join(outcome)
                ));
            }
            response = tokio::time::timeout(remaining, request) => match response {
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(e)) => last_error = error_chain(&e),
                Err(_) => last_error = format!("request still pending at the overall limit ({request_timeout:?} per request)"),
            },
        }
        tokio::select! {
            outcome = &mut *daemon => {
                return Err(format!(
                    "daemon exited before its API at {url} was ready ({:.1}s, {attempts} attempts; last error: {last_error}): {}",
                    started.elapsed().as_secs_f64(),
                    describe_join(outcome)
                ));
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
        }
    }
}

fn describe_join<R: std::fmt::Debug>(outcome: Result<R, tokio::task::JoinError>) -> String {
    match outcome {
        Ok(result) => format!("run returned {result:?}"),
        Err(e) if e.is_panic() => {
            let payload = e.into_panic();
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            format!("run panicked: {msg}")
        }
        Err(e) => format!("run task ended: {e}"),
    }
}

fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}
