use super::*;
use tokio::net::TcpListener;

fn policy() -> EgressPolicy {
    EgressPolicy {
        allow: [
            "example.com:443".into(),
            "example.com:53".into(),
            "example.com:853".into(),
        ]
        .into(),
        resolver: "127.0.0.1".parse().unwrap(),
        allow_ipv6: false,
    }
}

fn connect(authority: &str) -> Vec<u8> {
    format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").into_bytes()
}

fn response(query: &[u8], ips: &[IpAddr]) -> Vec<u8> {
    let kind = word(query, query.len() - 4).unwrap();
    let ips: Vec<_> = ips
        .iter()
        .filter(|ip| (kind == 1) == ip.is_ipv4())
        .collect();
    let mut bytes = query.to_vec();
    bytes[2..4].copy_from_slice(&0x8180u16.to_be_bytes());
    bytes[6..8].copy_from_slice(&(ips.len() as u16).to_be_bytes());
    for ip in ips {
        bytes.extend([0xc0, 0x0c]);
        bytes.extend(kind.to_be_bytes());
        bytes.extend(1u16.to_be_bytes());
        bytes.extend(30u32.to_be_bytes());
        let octets = match ip {
            IpAddr::V4(ip) => ip.octets().to_vec(),
            IpAddr::V6(ip) => ip.octets().to_vec(),
        };
        bytes.extend((octets.len() as u16).to_be_bytes());
        bytes.extend(octets);
    }
    bytes
}

async fn dns_fixture(ips: &[&str]) -> (SocketAddr, tokio::task::JoinHandle<Vec<u16>>) {
    let ips: Vec<IpAddr> = ips.iter().map(|ip| ip.parse().unwrap()).collect();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut kinds = vec![];
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let size = socket.read_u16().await.unwrap();
            let mut packet = vec![0; usize::from(size)];
            socket.read_exact(&mut packet).await.unwrap();
            kinds.push(word(&packet, packet.len() - 4).unwrap());
            let bytes = response(&packet, &ips);
            socket.write_u16(bytes.len() as u16).await.unwrap();
            socket.write_all(&bytes).await.unwrap();
        }
        kinds
    });
    (address, task)
}

#[tokio::test]
async fn real_unix_transport_rejects_proxy_dns_and_http_bypasses_before_resolution() {
    let mut requests: Vec<Vec<u8>> = [
        "127.0.0.1:443",
        "[::1]:443",
        "2130706433:443",
        "0x7f.1:443",
        "example.com:53",
        "example.com:853",
        "example.com:0443",
        "evil.com:443",
        "example.com.:443",
        "EXAMPLE.com:443",
        "example.com:8443",
        "user@example.com:443",
    ]
    .iter()
    .map(|s| connect(s))
    .collect();
    for header in [
        "Proxy-Authorization: sentinel",
        "Forwarded: host=evil.com",
        "Content-Length: 0",
        "Transfer-Encoding: chunked",
        "Host: evil.com:443",
        "Upgrade: websocket",
        " Host: example.com:443",
        "Host : example.com:443",
    ] {
        requests.push(
            format!(
                "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n{header}\r\n\r\n"
            )
            .into_bytes(),
        );
    }
    requests.extend([
        b"GET https://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".to_vec(),
        b"CONNECT example.com:443 HTTP/1.1\r\n\r\n".to_vec(),
        b"CONNECT example.com:443 HTTP/1.1\nHost: example.com:443\r\n\r\n".to_vec(),
    ]);
    for bytes in requests {
        let (mut client, server) = UnixStream::pair().unwrap();
        let job = tokio::spawn(async move { serve(server, &policy()).await });
        client.write_all(&bytes).await.unwrap();
        client.shutdown().await.unwrap();
        let mut output = vec![];
        timeout(Duration::from_secs(2), client.read_to_end(&mut output))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.await.unwrap(), Err(EgressError::Denied));
        assert_eq!(output, DENIED);
        assert!(!String::from_utf8_lossy(&output).contains("sentinel"));
    }
}

#[tokio::test]
async fn actual_dns_transport_checks_both_families_and_pins_the_result() {
    let (address, job) = dns_fixture(&["93.184.216.34"]).await;
    let (mut client, mut server) = UnixStream::pair().unwrap();
    client.write_all(&connect("example.com:443")).await.unwrap();
    let result = destination(&mut server, &policy(), address).await.unwrap();
    assert_eq!(result, "93.184.216.34:443".parse().unwrap());
    assert_eq!(job.await.unwrap(), vec![1, 28]);
    // No TCP connection to this public address is made in this test.
}

#[tokio::test]
async fn actual_dns_transport_refuses_private_ipv6_and_rebinding() {
    for ips in [
        vec!["127.0.0.1"],
        vec!["169.254.169.254"],
        vec!["10.0.0.1"],
        vec!["93.184.216.34", "192.168.1.1"],
        vec!["::1"],
        vec!["fd00::1"],
        vec!["::ffff:127.0.0.1"],
        vec!["2606:4700::1111"],
        vec!["93.184.216.34", "fd00::1"],
    ] {
        let (address, job) = dns_fixture(&ips).await;
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(&connect("example.com:443")).await.unwrap();
        assert_eq!(
            destination(&mut server, &policy(), address).await,
            Err(EgressError::Denied),
            "{ips:?}"
        );
        assert_eq!(job.await.unwrap(), vec![1, 28]);
    }
}

#[tokio::test]
async fn denied_origin_never_reaches_even_the_configured_dns_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (mut client, mut server) = UnixStream::pair().unwrap();
    client.write_all(&connect("evil.com:443")).await.unwrap();
    assert_eq!(
        destination(&mut server, &policy(), listener.local_addr().unwrap()).await,
        Err(EgressError::Denied)
    );
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn tunnel_preserves_prefetched_bytes_and_half_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut echo, _) = listener.accept().await.unwrap();
    let echo_job = tokio::spawn(async move {
        let mut bytes = vec![];
        echo.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"tls bytes");
        echo.write_all(b"server response").await.unwrap();
    });
    let (mut client, mut server) = UnixStream::pair().unwrap();
    let mut bytes = connect("example.com:443");
    bytes.extend(b"tls bytes");
    client.write_all(&bytes).await.unwrap();
    client.shutdown().await.unwrap();
    assert_eq!(
        request_authority(&read_header(&mut server).await.unwrap(), &policy()).unwrap(),
        ("example.com".into(), 443)
    );
    let job = tokio::spawn(tunnel(server, upstream));
    let mut output = vec![];
    timeout(Duration::from_secs(2), client.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    let mut expected = CONNECTED.to_vec();
    expected.extend(b"server response");
    assert_eq!(output, expected);
    assert_eq!(job.await.unwrap(), Ok(()));
    echo_job.await.unwrap();
}

#[tokio::test]
async fn oversized_header_is_bounded_and_denied() {
    let (mut client, server) = UnixStream::pair().unwrap();
    let job = tokio::spawn(async move { serve(server, &policy()).await });
    client.write_all(&vec![b'a'; HEADER_LIMIT]).await.unwrap();
    let mut output = vec![];
    timeout(Duration::from_secs(2), client.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.await.unwrap(), Err(EgressError::Denied));
    assert_eq!(output, DENIED);
}

#[test]
fn malformed_dns_cannot_inject_an_address() {
    let query = query("example.com", 1, 7);
    let good = response(&query, &["93.184.216.34".parse().unwrap()]);
    assert_eq!(
        answer(&good, "example.com", 1, 7).unwrap(),
        vec!["93.184.216.34".parse::<IpAddr>().unwrap()]
    );
    for offset in [
        0,
        2,
        4,
        12,
        query.len(),
        query.len() + 2,
        query.len() + 4,
        query.len() + 10,
    ] {
        let mut packet = good.clone();
        packet[offset] = 0xff;
        assert_eq!(
            answer(&packet, "example.com", 1, 7),
            Err(EgressError::Resolution),
            "offset {offset}"
        );
    }
    for length in 0..good.len() {
        assert!(answer(&good[..length], "example.com", 1, 7).is_err());
    }
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(answer(&trailing, "example.com", 1, 7).is_err());
    let mut cycle = good;
    let pointer = (0xc000u16 | query.len() as u16).to_be_bytes();
    cycle[query.len()..query.len() + 2].copy_from_slice(&pointer);
    assert!(answer(&cycle, "example.com", 1, 7).is_err());
}

#[test]
fn dns_cname_requires_terminal_owner_and_refuses_cycles() {
    let q = query("example.com", 1, 7);
    let mut packet = response(&q, &[]);
    packet[6..8].copy_from_slice(&2u16.to_be_bytes());
    // example.com CNAME canonical.example.com
    packet.extend([0xc0, 0x0c, 0, 5, 0, 1, 0, 0, 0, 30]);
    let alias = query("canonical.example.com", 1, 0);
    let encoded = &alias[12..alias.len() - 4];
    packet.extend((encoded.len() as u16).to_be_bytes());
    let alias_offset = packet.len();
    packet.extend(encoded);
    // canonical.example.com A 93.184.216.34
    let owner_offset = packet.len();
    packet.extend((0xc000u16 | alias_offset as u16).to_be_bytes());
    packet.extend([0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 93, 184, 216, 34]);
    assert_eq!(
        answer(&packet, "example.com", 1, 7).unwrap(),
        vec!["93.184.216.34".parse::<IpAddr>().unwrap()]
    );
    let mut wrong_owner = packet.clone();
    wrong_owner[owner_offset..owner_offset + 2].copy_from_slice(&[0xc0, 0x0c]);
    assert_eq!(
        answer(&wrong_owner, "example.com", 1, 7),
        Err(EgressError::Resolution)
    );
    let mut incomplete = packet[..owner_offset].to_vec();
    incomplete[6..8].copy_from_slice(&1u16.to_be_bytes());
    assert_eq!(
        answer(&incomplete, "example.com", 1, 7),
        Err(EgressError::Resolution)
    );
    // Replace the terminal A record with CNAME example.com, closing a cycle.
    packet.truncate(owner_offset + 2);
    packet.extend([0, 5, 0, 1, 0, 0, 0, 30, 0, 2, 0xc0, 0x0c]);
    assert_eq!(
        answer(&packet, "example.com", 1, 7),
        Err(EgressError::Resolution)
    );
}

#[tokio::test]
async fn malformed_dns_length_or_transaction_is_rejected_over_tcp() {
    for invalid_length in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let job = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let size = usize::from(stream.read_u16().await.unwrap());
            let mut q = vec![0; size];
            stream.read_exact(&mut q).await.unwrap();
            if invalid_length {
                stream.write_u16(u16::MAX).await.unwrap();
            } else {
                let mut bytes = response(&q, &["93.184.216.34".parse().unwrap()]);
                bytes[0] ^= 0xff;
                stream.write_u16(bytes.len() as u16).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
            }
        });
        assert_eq!(
            resolve_at(address, "example.com").await,
            Err(EgressError::Resolution)
        );
        job.await.unwrap();
    }
}
