use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio::net::TcpListener;

async fn recorded_request(
    request: Vec<u8>,
    policy: EgressPolicy,
) -> (Result<(), EgressError>, Option<Denial>) {
    let (mut client, server) = UnixStream::pair().unwrap();
    let job = tokio::spawn(async move { serve_recorded(server, &policy).await });
    client.write_all(&request).await.unwrap();
    client.shutdown().await.unwrap();
    job.await.unwrap()
}

#[tokio::test]
async fn egress_denial_record_reasons_and_no_request_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let mut recorder =
        crate::browser_runtime::DenialRecorder::new(dir.path(), "session-1").unwrap();
    let (resolver, dns) = dns_fixture(&["10.1.2.3"]).await;
    let mut policy = policy();
    policy.resolver = resolver.ip();
    for (request, expected_kind, expected_host) in [
        (b"CONNECT evil.com:443 HTTP/1.1\r\nHost: evil.com:443\r\nUser-Agent: HEADER_SECRET\r\n\r\nBODY_SECRET".to_vec(), "not_allowed", "evil.com"),
        (connect("127.0.0.1:443"), "ip_literal", "127.0.0.1"),
    ] {
        let (result, denial) = recorded_request(request, policy.clone()).await;
        assert_eq!(result, Err(EgressError::Denied));
        let denial = denial.unwrap();
        assert_eq!(denial.kind, expected_kind);
        assert_eq!(denial.host.as_deref(), Some(expected_host));
        recorder.append(denial).unwrap();
    }
    // A listed domain can still be rejected after DNS returns a private address.
    let (mut client, server) = UnixStream::pair().unwrap();
    let job = tokio::spawn(async move {
        let mut denial = None;
        let mut server = server;
        let result = destination_recorded(&mut server, &policy, resolver, &mut denial).await;
        (result, denial)
    });
    client.write_all(&connect("example.com:443")).await.unwrap();
    let (result, denial) = job.await.unwrap();
    assert_eq!(result, Err(EgressError::Denied));
    let denial = denial.unwrap();
    assert_eq!(denial.kind, "private_address");
    recorder.append(denial).unwrap();
    dns.await.unwrap();

    let path = dir.path().join("egress-denied.jsonl");
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(!contents.contains("HEADER_SECRET"));
    assert!(!contents.contains("BODY_SECRET"));
    assert!(!contents.contains("10.1.2.3"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let lines: Vec<serde_json::Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    for (line, kind) in lines
        .iter()
        .zip(["not_allowed", "ip_literal", "private_address"])
    {
        assert_eq!(line["kind"], kind);
        assert_eq!(line["port"], 443);
        assert_eq!(line["session_id"], "session-1");
        time::OffsetDateTime::parse(
            line["at"].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
    }
}

#[tokio::test]
async fn egress_denial_record_allowed_connection_has_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let _recorder = crate::browser_runtime::DenialRecorder::new(dir.path(), "session-2").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut policy = policy();
    policy.allow.insert(format!("127.0.0.1:{port}"));
    policy
        .test_loopback_allow
        .insert(format!("127.0.0.1:{port}"));
    let (mut client, server) = UnixStream::pair().unwrap();
    let job = tokio::spawn(async move { serve_recorded(server, &policy).await });
    client
        .write_all(&connect(&format!("127.0.0.1:{port}")))
        .await
        .unwrap();
    let (upstream, _) = listener.accept().await.unwrap();
    let mut response = [0u8; 39];
    let _ = client.read(&mut response).await.unwrap();
    drop(client);
    drop(upstream);
    let (result, denial) = job.await.unwrap();
    assert!(result.is_ok());
    assert!(denial.is_none());
    assert!(
        std::fs::read(dir.path().join("egress-denied.jsonl"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn egress_denial_record_rejects_preexisting_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("egress-denied.jsonl");
    std::fs::write(&path, b"browser-owned").unwrap();
    assert!(crate::browser_runtime::DenialRecorder::new(dir.path(), "session-3").is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"browser-owned");
}

#[tokio::test]
async fn egress_denial_record_malformed_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let mut recorder =
        crate::browser_runtime::DenialRecorder::new(dir.path(), "session-4").unwrap();
    let (result, denial) = recorded_request(b"BAD SECRET_HEADER\r\n\r\n".to_vec(), policy()).await;
    assert_eq!(result, Err(EgressError::Denied));
    let denial = denial.unwrap();
    assert_eq!(denial.kind, "malformed");
    assert!(denial.host.is_none());
    for _ in 0..130 {
        recorder.append(denial.clone()).unwrap();
    }
    let contents = std::fs::read_to_string(dir.path().join("egress-denied.jsonl")).unwrap();
    let lines: Vec<_> = contents.lines().collect();
    assert_eq!(lines.len(), 129);
    assert!(lines.iter().all(|line| line.len() < 512));
    assert!(!contents.contains("SECRET_HEADER"));
    let last: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["kind"], "truncated");
}

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
        test_loopback_allow: Default::default(),
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

/// D2.0: the egress proxy fed from the prepared policy (task ∩ grant) refuses CONNECT to origins
/// outside it — scheme (port 80) and port differences, grant-only and task-only hosts — before
/// any DNS, and admits the intersection.
#[tokio::test]
async fn browser_allowed_domains_egress_denies_outside_task_and_grant() {
    let grant = task_core::BrowserCapability {
        allowed_domains: vec![
            "https://*.example.com".into(),
            "http://127.0.0.1:3000".into(),
        ],
        ..Default::default()
    };
    let stored = task_core::BrowserTaskPolicy {
        policy_id: "p".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: grant.allowed_domains.clone(),
        allowed_actions: vec![task_core::BrowserAction::Navigate],
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    };
    let mut task = crate::protocol::tests::sample_task();
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec![
            "https://billing.example.com".into(),
            "https://evil.test".into(),
        ],
    });
    let prepared =
        crate::browser_policy::prepare_for_task(&grant, &task, Some(&stored), "0.38.1").unwrap();
    let policy = EgressPolicy {
        allow: prepared.egress_allow(),
        resolver: "127.0.0.1".parse().unwrap(),
        allow_ipv6: false,
        test_loopback_allow: Default::default(),
    };
    for authority in [
        "billing.example.com:80",
        "billing.example.com:8443",
        "app.example.com:443",
        "evil.test:443",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (mut client, mut server) = UnixStream::pair().unwrap();
        client.write_all(&connect(authority)).await.unwrap();
        assert_eq!(
            destination(&mut server, &policy, listener.local_addr().unwrap()).await,
            Err(EgressError::Denied),
            "{authority}"
        );
    }
    let (address, job) = dns_fixture(&["93.184.216.34"]).await;
    let (mut client, mut server) = UnixStream::pair().unwrap();
    client
        .write_all(&connect("billing.example.com:443"))
        .await
        .unwrap();
    assert_eq!(
        destination(&mut server, &policy, address).await,
        Ok("93.184.216.34:443".parse().unwrap())
    );
    assert_eq!(job.await.unwrap(), vec![1, 28]);
}

/// One CONNECT through `serve` over a Unix pair; returns the proxy's status line
/// and, when established, echoes a byte through the tunnel.
async fn loopback_connect(policy: EgressPolicy, authority: &str) -> (String, Option<u8>) {
    let (mut client, server) = UnixStream::pair().unwrap();
    let job = tokio::spawn(async move { serve(server, &policy).await });
    client.write_all(&connect(authority)).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        match client.read_u8().await {
            Ok(b) => head.push(b),
            Err(_) => break,
        }
    }
    let status = String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let mut echoed = None;
    if status.contains(" 200 ") {
        client.write_all(b"x").await.unwrap();
        echoed = Some(client.read_u8().await.unwrap());
        drop(client);
    }
    let _ = job.await.unwrap();
    (status, echoed)
}

async fn echo_listener() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut b = [0u8; 1];
            if socket.read_exact(&mut b).await.is_ok() {
                let _ = socket.write_all(&b).await;
            }
        }
    });
    (port, task)
}

#[tokio::test]
async fn egress_test_loopback_connect_allowed_only_when_listed() {
    let (port, listener) = echo_listener().await;
    let authority = format!("127.0.0.1:{port}");
    // Default (empty set): the loopback literal stays denied with 403.
    let (status, echoed) = loopback_connect(policy(), &authority).await;
    assert!(status.contains(" 403 "), "{status}");
    assert_eq!(echoed, None);
    // Listed: 200 and the tunnel reaches the loopback listener without DNS
    // (the policy resolver 127.0.0.1:53 is not listening).
    let mut allowed = policy();
    allowed.test_loopback_allow = [authority.clone()].into();
    let (status, echoed) = loopback_connect(allowed.clone(), &authority).await;
    assert!(status.contains(" 200 "), "{status}");
    assert_eq!(echoed, Some(b'x'));
    listener.await.unwrap();
    // Another port, a name resolving to loopback, ::1 and literal variants: 403.
    for other in [
        format!("127.0.0.1:{}", port.wrapping_add(1).max(1)),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
        format!("10.0.0.1:{port}"),
        format!("2130706433:{port}"),
        format!("0x7f.0.0.1:{port}"),
        format!("0177.0.0.1:{port}"),
    ] {
        let (status, _) = loopback_connect(allowed.clone(), &other).await;
        assert!(status.contains(" 403 "), "{other}: {status}");
    }
}

#[tokio::test]
async fn egress_test_loopback_destination_skips_dns_only_for_listed_literal() {
    let mut allowed = policy();
    allowed.test_loopback_allow = ["127.0.0.1:18080".to_string()].into();
    // Resolver points at a closed port: any DNS attempt would fail with Resolution.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let resolver = closed.local_addr().unwrap();
    drop(closed);
    let (mut client, mut server) = UnixStream::pair().unwrap();
    client.write_all(&connect("127.0.0.1:18080")).await.unwrap();
    assert_eq!(
        destination(&mut server, &allowed, resolver).await,
        Ok("127.0.0.1:18080".parse().unwrap())
    );
    // A name for the same port is unaffected: not in `allow`, denied before DNS.
    let (mut client, mut server) = UnixStream::pair().unwrap();
    client.write_all(&connect("localhost:18080")).await.unwrap();
    assert_eq!(
        destination(&mut server, &allowed, resolver).await,
        Err(EgressError::Denied)
    );
    // Allowed name that resolves to loopback via DNS stays PrivateAddress (Denied).
    allowed.allow.insert("example.com:18080".into());
    let (address, job) = dns_fixture(&["127.0.0.1"]).await;
    let (mut client, mut server) = UnixStream::pair().unwrap();
    client
        .write_all(&connect("example.com:18080"))
        .await
        .unwrap();
    assert_eq!(
        destination(&mut server, &allowed, address).await,
        Err(EgressError::Denied)
    );
    assert_eq!(job.await.unwrap(), vec![1, 28]);
}
