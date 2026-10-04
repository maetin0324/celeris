//! e2e: fake ワーカーだけで動く end-to-end テスト（DESIGN §3, ADR-0005 D8）。テスト本体は `tests/`。

use std::net::{Ipv4Addr, SocketAddr};

/// celeris の API に渡すポートを、テストが終わるまで予約しておく。
///
/// celeris は API を `SO_REUSEPORT` で bind する（ADR-0040 D4、`daemon::api::bind_reuseport`）。
/// 空きポートを選んで閉じるだけだと、並走する別のテストが同じポートを選び、2 つの celeris が
/// 同じポートに bind できてしまう。カーネルは接続ごとに振り分けるので、1 つのテストの要求が
/// 別の daemon（別の DB）に届く（例: `agent/begin` だけが他所へ行き、続く pause が `paused` を返す）。
///
/// 予約は `SO_REUSEPORT` だけを立て（`SO_REUSEADDR` は立てない）、listen しないソケットで持つ。
/// - listen しないので接続は受けない（celeris の listener だけが受ける）。
/// - 同じ uid の `SO_REUSEPORT` の bind（このテストの celeris）とは両立する。
/// - `SO_REUSEADDR` 付き・無しのどちらの `bind(0)` からも使用中に見えるので、他のテスト
///   （別プロセスを含む）の空きポート選びがこのポートを選ばない。
pub struct PortReservation {
    _socket: socket2::Socket,
    port: u16,
}

impl PortReservation {
    pub fn new() -> std::io::Result<Self> {
        use socket2::{Domain, Protocol, Socket, Type};
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_port(true)?;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        socket.bind(&addr.into())?;
        let port = socket
            .local_addr()?
            .as_socket()
            .map(|a| a.port())
            .ok_or_else(|| std::io::Error::other("reserved socket has no inet address"))?;
        Ok(Self {
            _socket: socket,
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn reserved_port_is_not_handed_out_and_accepts_no_connection() {
        let r = PortReservation::new().unwrap();
        // 他の空きポート選び（std は SO_REUSEADDR を立てる）は予約中のポートを選べない。
        for _ in 0..200 {
            let other = TcpListener::bind("127.0.0.1:0").unwrap();
            assert_ne!(other.local_addr().unwrap().port(), r.port());
        }
        assert!(TcpListener::bind(("127.0.0.1", r.port())).is_err());
        // listen していないので接続は拒否される（予約が要求を飲み込まない）。
        assert!(std::net::TcpStream::connect(("127.0.0.1", r.port())).is_err());
    }

    #[test]
    fn reserved_port_admits_a_reuseport_listener_of_the_same_user() {
        use socket2::{Domain, Protocol, Socket, Type};
        let r = PortReservation::new().unwrap();
        // celeris の bind_reuseport と同じ立て方。
        let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        s.set_reuse_address(true).unwrap();
        s.set_reuse_port(true).unwrap();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, r.port()));
        s.bind(&addr.into()).unwrap();
        s.listen(16).unwrap();
        assert!(std::net::TcpStream::connect(("127.0.0.1", r.port())).is_ok());
    }
}
