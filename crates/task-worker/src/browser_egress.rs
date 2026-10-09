//! ADR-0104: bounded CONNECT transport for the trusted browser controller.
//! ADR 2026-10-06-egress-http-forward-get: also one absolute-URI `http://` GET/HEAD per
//! connection, under the same admission, forwarded in origin-form with `Connection: close`.
//!
//! The caller supplies an already authorised Unix stream; this module opens no host
//! listener. It is not an isolation attestation: namespace plumbing and controller
//! admission must be established before using it as a browser's only network exit.
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use task_core::browser_isolation::{
    EgressDenied, EgressPolicy, EgressRequest, check_egress, egress_destination,
    test_loopback_target,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};
use tokio::time::timeout;

const HEADER_LIMIT: usize = 8192;
const DNS_LIMIT: usize = 16384;
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(300);
const TRANSFER_LIMIT: u64 = 64 * 1024 * 1024;
const DENIED: &[u8] = b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
const CONNECTED: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";

/// Fixed errors only; never attach a request, DNS packet or upstream error to a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressError {
    #[error("egress request denied")]
    Denied,
    #[error("egress resolution failed")]
    Resolution,
    #[error("egress transport failed")]
    Transport,
    #[error("egress time limit")]
    Timeout,
}

/// The only data the untrusted request can contribute to a denial record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Denial {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl Denial {
    fn malformed() -> Self {
        Self {
            kind: "malformed".into(),
            host: None,
            port: None,
        }
    }

    fn policy(reason: EgressDenied, host: &str, port: u16) -> Self {
        let kind = match reason {
            EgressDenied::NotAllowed => "not_allowed",
            EgressDenied::IpLiteral => "ip_literal",
            EgressDenied::PrivateAddress => "private_address",
            EgressDenied::Ipv6Disabled => "ipv6_disabled",
            EgressDenied::Unresolved => "unresolved",
            EgressDenied::DnsBypass => "dns_bypass",
            EgressDenied::ProxyChain => "proxy_chain",
            EgressDenied::InvalidHost => "invalid_host",
        };
        Self {
            kind: kind.into(),
            host: Some(host.into()),
            port: Some(port),
        }
    }

    fn forward(kind: &str, target: Option<(&str, u16)>) -> Self {
        Self {
            kind: kind.into(),
            host: target.map(|(host, _)| host.into()),
            port: target.map(|(_, port)| port),
        }
    }
}

/// How an admitted request is carried to its pinned destination.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    /// CONNECT: answer 200 and splice bytes both ways.
    Tunnel,
    /// Absolute-URI GET/HEAD: send this origin-form request upstream, stream the reply, close.
    Forward(Vec<u8>),
}

/// Hop-by-hop headers never forwarded (RFC 9110 §7.6.1) plus proxy credentials.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "proxy-connection",
    "proxy-authorization",
    "keep-alive",
    "te",
    "trailer",
    "upgrade",
];

fn recordable_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
}

/// `host[:port]` with the HTTP default port; strict decimal port like CONNECT.
fn split_authority(authority: &str) -> Option<(&str, u16)> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port_text)) => {
            let port: u16 = port_text.parse().ok()?;
            if port.to_string() != port_text {
                return None;
            }
            (host, port)
        }
        None => (authority, 80),
    };
    if host.is_empty() || matches!(port, 0 | 53 | 853) {
        return None;
    }
    Some((host, port))
}

fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Parse and admit (before DNS) one absolute-URI `http://` GET/HEAD. Every refusal
/// carries its record; only a syntax-checked URI host may appear in it.
fn forward_request(header: &[u8], policy: &EgressPolicy) -> Result<(String, u16, Mode), Denial> {
    let malformed = Denial::malformed;
    let text = std::str::from_utf8(header).map_err(|_| malformed())?;
    if !text.ends_with("\r\n\r\n")
        || !text.is_ascii()
        || text
            .split("\r\n")
            .any(|line| line.chars().any(char::is_control))
    {
        return Err(malformed());
    }
    let mut lines = text[..text.len() - 4].split("\r\n");
    let line = lines.next().ok_or_else(malformed)?;
    let [method @ ("GET" | "HEAD"), uri, "HTTP/1.1"] = line.split(' ').collect::<Vec<_>>()[..]
    else {
        return Err(malformed());
    };
    let (scheme, rest) = uri.split_once("://").ok_or_else(malformed)?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let target = split_authority(authority).filter(|(host, _)| recordable_host(host));
    if !scheme.eq_ignore_ascii_case("http") {
        let kind = if scheme.eq_ignore_ascii_case("https") {
            "scheme_not_allowed"
        } else {
            "malformed"
        };
        return Err(Denial::forward(kind, target));
    }
    if authority.contains('@') || path.contains('#') {
        return Err(malformed());
    }
    let (host, port) = target.ok_or_else(malformed)?;
    // Same pre-DNS admission as CONNECT: Unresolved is the only acceptable failure,
    // except a listed test-only loopback literal which is admitted outright.
    let verdict = check_egress(
        policy,
        &EgressRequest::Connect {
            host: host.into(),
            port,
            resolved: vec![],
        },
    );
    let loopback = test_loopback_target(policy, host, port).is_some();
    match verdict {
        Ok(()) if loopback => {}
        Err(EgressDenied::Unresolved) => {}
        Ok(()) => return Err(malformed()),
        Err(reason) => return Err(Denial::policy(reason, host, port)),
    }
    let mut seen_host = false;
    let mut dropped: BTreeSet<String> = HOP_BY_HOP.iter().map(|s| s.to_string()).collect();
    let mut kept = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(malformed)?;
        if !is_token(name) {
            return Err(malformed());
        }
        let lower = name.to_ascii_lowercase();
        let value = value.trim_matches([' ', '\t']);
        match lower.as_str() {
            "host" => {
                if seen_host || split_authority(value) != Some((host, port)) {
                    return Err(Denial::forward("host_mismatch", Some((host, port))));
                }
                seen_host = true;
            }
            "transfer-encoding" => {
                return Err(Denial::forward("request_body", Some((host, port))));
            }
            "content-length" => {
                if value != "0" {
                    return Err(Denial::forward("request_body", Some((host, port))));
                }
            }
            "connection" => {
                dropped.extend(
                    value
                        .split(',')
                        .map(|token| token.trim().to_ascii_lowercase())
                        .filter(|token| !token.is_empty()),
                );
            }
            _ => kept.push((lower, name, value)),
        }
    }
    if !seen_host {
        return Err(Denial::forward("host_mismatch", Some((host, port))));
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\n");
    for (lower, name, value) in kept {
        if !dropped.contains(&lower) {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    request.push_str("Connection: close\r\n\r\n");
    Ok((host.into(), port, Mode::Forward(request.into_bytes())))
}

/// Dispatch on the request method: GET/HEAD are forwarded, everything else takes
/// the CONNECT path (which records any other method as malformed).
fn parse_request(
    header: &[u8],
    policy: &EgressPolicy,
    denial: &mut Option<Denial>,
) -> Result<(String, u16, Mode), EgressError> {
    if header.starts_with(b"GET ") || header.starts_with(b"HEAD ") {
        return forward_request(header, policy).map_err(|record| {
            *denial = Some(record);
            EgressError::Denied
        });
    }
    let (host, port) = request_authority_recorded(header, policy, denial)?;
    Ok((host, port, Mode::Tunnel))
}

fn request_authority_recorded(
    header: &[u8],
    policy: &EgressPolicy,
    denial: &mut Option<Denial>,
) -> Result<(String, u16), EgressError> {
    let result = request_authority(header, policy);
    if result.is_err() {
        // Only a successfully parsed CONNECT authority may supply a host. Never
        // copy arbitrary header bytes into a record.
        *denial = Some(Denial::malformed());
        if let Ok(text) = std::str::from_utf8(header)
            && let Some(line) = text.split("\r\n").next()
            && let ["CONNECT", authority, "HTTP/1.1"] =
                line.split(' ').collect::<Vec<_>>().as_slice()
            && let Some((host, port_text)) = authority.rsplit_once(':')
            && let Ok(port) = port_text.parse::<u16>()
            && port != 0
            && port != 53
            && port != 853
            && port.to_string() == port_text
            && recordable_host(host)
        {
            let reason = check_egress(
                policy,
                &EgressRequest::Connect {
                    host: host.into(),
                    port,
                    resolved: vec![],
                },
            );
            if let Err(reason) = reason
                && reason != EgressDenied::Unresolved
            {
                *denial = Some(Denial::policy(reason, host, port));
            }
        }
    }
    result
}

fn request_authority(header: &[u8], policy: &EgressPolicy) -> Result<(String, u16), EgressError> {
    let text = std::str::from_utf8(header).map_err(|_| EgressError::Denied)?;
    if !text.ends_with("\r\n\r\n")
        || text
            .chars()
            .any(|c| c.is_control() && c != '\r' && c != '\n')
    {
        return Err(EgressError::Denied);
    }
    let mut lines = text[..text.len() - 4].split("\r\n");
    let line = lines.next().ok_or(EgressError::Denied)?;
    let parts: Vec<_> = line.split(' ').collect();
    let ["CONNECT", authority, "HTTP/1.1"] = parts.as_slice() else {
        return Err(EgressError::Denied);
    };
    let (host, port_text) = authority.rsplit_once(':').ok_or(EgressError::Denied)?;
    let port: u16 = port_text.parse().map_err(|_| EgressError::Denied)?;
    if port == 0 || port == 53 || port == 853 || port.to_string() != port_text {
        return Err(EgressError::Denied);
    }
    // Validate syntax and admission *before DNS*. Unresolved is the sole permitted
    // failure here (an allowed test-only loopback literal passes without DNS). Re-run
    // with the complete trusted DNS result before connecting.
    let verdict = check_egress(
        policy,
        &EgressRequest::Connect {
            host: host.into(),
            port,
            resolved: vec![],
        },
    );
    let loopback = test_loopback_target(policy, host, port).is_some();
    if !(verdict == Err(task_core::browser_isolation::EgressDenied::Unresolved)
        || (loopback && verdict == Ok(())))
    {
        return Err(EgressError::Denied);
    }
    let mut seen = BTreeSet::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(EgressError::Denied)?;
        let name = name.to_ascii_lowercase();
        let value = value.trim_matches(' ');
        if !seen.insert(name.clone()) || value.contains(['\r', '\n']) {
            return Err(EgressError::Denied);
        }
        match name.as_str() {
            "host" if value == *authority => {}
            "user-agent" => {}
            "proxy-connection" if value.eq_ignore_ascii_case("keep-alive") => {}
            // No proxy auth, Forwarded, Upgrade, body, or ambiguous framing.
            _ => return Err(EgressError::Denied),
        }
    }
    if !seen.contains("host") {
        return Err(EgressError::Denied);
    }
    Ok((host.into(), port))
}

async fn read_header(stream: &mut UnixStream) -> Result<Vec<u8>, EgressError> {
    let mut bytes = Vec::with_capacity(512);
    while bytes.len() < HEADER_LIMIT {
        // Do not read ahead and lose TLS bytes coalesced with CONNECT.
        bytes.push(stream.read_u8().await.map_err(|_| EgressError::Denied)?);
        if bytes.ends_with(b"\r\n\r\n") {
            return Ok(bytes);
        }
    }
    Err(EgressError::Denied)
}

fn word(packet: &[u8], at: usize) -> Result<u16, EgressError> {
    let bytes = packet.get(at..at + 2).ok_or(EgressError::Resolution)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// A deliberately bounded DNS name decoder (including compressed names). Pointers
/// must go backwards; loops, extended labels and embedded separators are rejected.
fn name(packet: &[u8], cursor: &mut usize) -> Result<String, EgressError> {
    let mut pos = *cursor;
    let mut consumed = None;
    let mut labels = Vec::new();
    let mut length = 0;
    for _ in 0..128 {
        let size = *packet.get(pos).ok_or(EgressError::Resolution)?;
        if size & 0xc0 == 0xc0 {
            let pointer = usize::from(word(packet, pos)? & 0x3fff);
            if pointer >= pos {
                return Err(EgressError::Resolution);
            }
            consumed.get_or_insert(pos + 2);
            pos = pointer;
        } else if size == 0 {
            *cursor = consumed.unwrap_or(pos + 1);
            return Ok(labels.join("."));
        } else {
            if size > 63 {
                return Err(EgressError::Resolution);
            }
            pos += 1;
            let label = packet
                .get(pos..pos + usize::from(size))
                .ok_or(EgressError::Resolution)?;
            if !label
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
            {
                return Err(EgressError::Resolution);
            }
            length += label.len() + 1;
            if length > 254 {
                return Err(EgressError::Resolution);
            }
            labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
            pos += usize::from(size);
        }
    }
    Err(EgressError::Resolution)
}

fn query(host: &str, kind: u16, id: u16) -> Vec<u8> {
    let mut bytes = vec![];
    for word in [id, 0x0100, 1, 0, 0, 0] {
        bytes.extend(word.to_be_bytes());
    }
    for label in host.split('.') {
        bytes.push(label.len() as u8);
        bytes.extend(label.as_bytes());
    }
    bytes.push(0);
    bytes.extend(kind.to_be_bytes());
    bytes.extend(1u16.to_be_bytes());
    bytes
}

fn answer(packet: &[u8], host: &str, kind: u16, id: u16) -> Result<Vec<IpAddr>, EgressError> {
    let flags = word(packet, 2)?;
    if word(packet, 0)? != id
        || flags & 0xf80f != 0x8000
        || flags & 0x0200 != 0
        || word(packet, 4)? != 1
    {
        return Err(EgressError::Resolution);
    }
    let mut pos = 12;
    if name(packet, &mut pos)? != host || word(packet, pos)? != kind || word(packet, pos + 2)? != 1
    {
        return Err(EgressError::Resolution);
    }
    pos += 4;
    let mut aliases = BTreeMap::new();
    let mut addresses = Vec::new();
    let mut soa_owners = Vec::new();
    let counts = [word(packet, 6)?, word(packet, 8)?, word(packet, 10)?];
    for (section, count) in counts.into_iter().enumerate() {
        for _ in 0..count {
            let owner = name(packet, &mut pos)?;
            let record_kind = word(packet, pos)?;
            let class = word(packet, pos + 2)?;
            let size = usize::from(word(packet, pos + 8)?);
            pos += 10;
            let data = packet.get(pos..pos + size).ok_or(EgressError::Resolution)?;
            if section == 0 {
                if class != 1 {
                    return Err(EgressError::Resolution);
                }
                match record_kind {
                    5 => {
                        let mut end = pos;
                        let alias = name(packet, &mut end)?;
                        if end != pos + size || aliases.insert(owner, alias).is_some() {
                            return Err(EgressError::Resolution);
                        }
                    }
                    1 if kind == 1 && size == 4 => {
                        addresses.push((
                            owner,
                            IpAddr::V4(Ipv4Addr::new(data[0], data[1], data[2], data[3])),
                        ));
                    }
                    28 if kind == 28 && size == 16 => {
                        let octets: [u8; 16] =
                            data.try_into().map_err(|_| EgressError::Resolution)?;
                        addresses.push((owner, IpAddr::V6(Ipv6Addr::from(octets))));
                    }
                    1 | 28 => return Err(EgressError::Resolution),
                    _ => return Err(EgressError::Resolution),
                }
            } else if section == 1 && record_kind == 6 && class == 1 {
                soa_owners.push(owner);
            }
            pos += size;
        }
    }
    if pos != packet.len() {
        return Err(EgressError::Resolution);
    }
    let mut terminal = host.to_string();
    let mut seen = BTreeSet::new();
    while let Some(next) = aliases.get(&terminal) {
        if !seen.insert(terminal.clone()) || seen.len() > 16 {
            return Err(EgressError::Resolution);
        }
        terminal = next.clone();
    }
    if addresses.iter().any(|(owner, _)| owner != &terminal) {
        return Err(EgressError::Resolution);
    }
    // A CNAME without a terminal answer is NODATA only in the RFC 2308 form: the
    // authority section carries the SOA of the terminal name's zone, i.e. the
    // recursive resolver completed the chain and the terminal has no record of this
    // type (CloudFront names without AAAA). Otherwise the chain is incomplete; no
    // arbitrary follow-up resolver exists.
    if !aliases.is_empty() && addresses.is_empty() {
        let completed = soa_owners.iter().any(|zone| {
            terminal == *zone
                || terminal
                    .strip_suffix(zone.as_str())
                    .is_some_and(|sub| sub.ends_with('.'))
        });
        return if completed {
            Ok(vec![])
        } else {
            Err(EgressError::Resolution)
        };
    }
    Ok(addresses.into_iter().map(|(_, ip)| ip).collect())
}

async fn resolve_at(server: SocketAddr, host: &str) -> Result<Vec<IpAddr>, EgressError> {
    static NEXT_ID: AtomicU16 = AtomicU16::new(1);
    let mut addresses = BTreeSet::new();
    for kind in [1, 28] {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let packet = query(host, kind, id);
        // An IP socket, not a hostname: never consult OS DNS or proxy env vars.
        let mut dns = TcpStream::connect(server)
            .await
            .map_err(|_| EgressError::Resolution)?;
        dns.write_u16(packet.len() as u16)
            .await
            .map_err(|_| EgressError::Resolution)?;
        dns.write_all(&packet)
            .await
            .map_err(|_| EgressError::Resolution)?;
        let length = usize::from(dns.read_u16().await.map_err(|_| EgressError::Resolution)?);
        if !(12..=DNS_LIMIT).contains(&length) {
            return Err(EgressError::Resolution);
        }
        let mut response = vec![0; length];
        dns.read_exact(&mut response)
            .await
            .map_err(|_| EgressError::Resolution)?;
        addresses.extend(answer(&response, host, kind, id)?);
    }
    Ok(addresses.into_iter().collect())
}

#[cfg(test)]
async fn destination(
    stream: &mut UnixStream,
    policy: &EgressPolicy,
    resolver: SocketAddr,
) -> Result<SocketAddr, EgressError> {
    destination_recorded(stream, policy, resolver, &mut None)
        .await
        .map(|(address, _)| address)
}

async fn destination_recorded(
    stream: &mut UnixStream,
    policy: &EgressPolicy,
    resolver: SocketAddr,
    denial: &mut Option<Denial>,
) -> Result<(SocketAddr, Mode), EgressError> {
    let header = read_header(stream).await?;
    let (host, port, mode) = parse_request(&header, policy, denial)?;
    // Test-only loopback literal (ADR addendum E1): connect directly, never via DNS.
    if let Some(address) = test_loopback_target(policy, &host, port) {
        return Ok((address, mode));
    }
    let resolved = resolve_at(resolver, &host).await?;
    check_egress(
        policy,
        &EgressRequest::Connect {
            host: host.clone(),
            port,
            resolved: resolved.clone(),
        },
    )
    .map_err(|reason| {
        *denial = Some(Denial::policy(reason, &host, port));
        EgressError::Denied
    })?;
    let ip = egress_destination(policy, &resolved).ok_or(EgressError::Resolution)?;
    Ok((SocketAddr::new(ip, port), mode))
}

/// Serve one bounded CONNECT (or forwarded GET/HEAD) connection. Only the configured resolver and a
/// checked, pinned destination IP are reachable from this transport.
pub async fn serve(stream: UnixStream, policy: &EgressPolicy) -> Result<(), EgressError> {
    serve_recorded(stream, policy).await.0
}

pub async fn serve_recorded(
    mut stream: UnixStream,
    policy: &EgressPolicy,
) -> (Result<(), EgressError>, Option<Denial>) {
    let mut denial = None;
    let setup = timeout(SETUP_TIMEOUT, async {
        let (address, mode) = destination_recorded(
            &mut stream,
            policy,
            SocketAddr::new(policy.resolver, 53),
            &mut denial,
        )
        .await?;
        let upstream = TcpStream::connect(address)
            .await
            .map_err(|_| EgressError::Transport)?;
        Ok((upstream, mode))
    })
    .await
    .unwrap_or(Err(EgressError::Timeout));
    let result = match setup {
        Ok((upstream, Mode::Tunnel)) => tunnel(stream, upstream).await,
        Ok((upstream, Mode::Forward(request))) => forward(stream, upstream, request).await,
        Err(error) => {
            let _ = timeout(Duration::from_secs(1), stream.write_all(DENIED)).await;
            discard_and_close(stream).await;
            Err(error)
        }
    };
    if result == Err(EgressError::Denied) && denial.is_none() {
        denial = Some(Denial::malformed());
    }
    (result, denial)
}

async fn tunnel(mut client: UnixStream, mut upstream: TcpStream) -> Result<(), EgressError> {
    timeout(TUNNEL_TIMEOUT, async {
        client
            .write_all(CONNECTED)
            .await
            .map_err(|_| EgressError::Transport)?;
        let (cr, mut cw) = client.split();
        let (ur, mut uw) = upstream.split();
        let up = async {
            tokio::io::copy(&mut cr.take(TRANSFER_LIMIT), &mut uw).await?;
            uw.shutdown().await
        };
        let down = async {
            tokio::io::copy(&mut ur.take(TRANSFER_LIMIT), &mut cw).await?;
            cw.shutdown().await
        };
        tokio::try_join!(up, down).map_err(|_| EgressError::Transport)?;
        Ok(())
    })
    .await
    .unwrap_or(Err(EgressError::Timeout))
}

/// One request, one response: write the origin-form request, stream the reply until
/// the upstream closes (it was sent `Connection: close`), then close the client. Any
/// further client bytes (a pipelined request) are never read.
async fn forward(
    mut client: UnixStream,
    mut upstream: TcpStream,
    request: Vec<u8>,
) -> Result<(), EgressError> {
    timeout(TUNNEL_TIMEOUT, async {
        upstream
            .write_all(&request)
            .await
            .map_err(|_| EgressError::Transport)?;
        tokio::io::copy(&mut (&mut upstream).take(TRANSFER_LIMIT), &mut client)
            .await
            .map_err(|_| EgressError::Transport)?;
        Ok(())
    })
    .await
    .unwrap_or(Err(EgressError::Timeout))?;
    discard_and_close(client).await;
    Ok(())
}

/// Half-close, then throw away (never parse) whatever the client still sends, bounded
/// in size and time. Closing a Unix stream with unread input resets the peer, which
/// could cost it the response it was just sent.
async fn discard_and_close(mut client: UnixStream) {
    if client.shutdown().await.is_err() {
        return;
    }
    let _ = timeout(
        Duration::from_secs(1),
        tokio::io::copy(&mut (&mut client).take(64 * 1024), &mut tokio::io::sink()),
    )
    .await;
}

#[cfg(test)]
#[path = "browser_egress/tests.rs"]
mod tests;
