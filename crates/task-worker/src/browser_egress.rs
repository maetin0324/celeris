//! ADR-0104: bounded CONNECT transport for the trusted browser controller.
//!
//! The caller supplies an already authorised Unix stream; this module opens no host
//! listener. It is not an isolation attestation: namespace plumbing and controller
//! admission must be established before using it as a browser's only network exit.
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use task_core::browser_isolation::{EgressPolicy, EgressRequest, check_egress};
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
    // failure here. Re-run with the complete trusted DNS result before connecting.
    if check_egress(
        policy,
        &EgressRequest::Connect {
            host: host.into(),
            port,
            resolved: vec![],
        },
    ) != Err(task_core::browser_isolation::EgressDenied::Unresolved)
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
    // A CNAME without a terminal answer is not silently treated as NODATA. The
    // recursive resolver must complete it; no arbitrary follow-up resolver exists.
    if !aliases.is_empty() && addresses.is_empty() {
        return Err(EgressError::Resolution);
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

async fn destination(
    stream: &mut UnixStream,
    policy: &EgressPolicy,
    resolver: SocketAddr,
) -> Result<SocketAddr, EgressError> {
    let header = read_header(stream).await?;
    let (host, port) = request_authority(&header, policy)?;
    let resolved = resolve_at(resolver, &host).await?;
    check_egress(
        policy,
        &EgressRequest::Connect {
            host,
            port,
            resolved: resolved.clone(),
        },
    )
    .map_err(|_| EgressError::Denied)?;
    let ip = resolved
        .first()
        .ok_or(EgressError::Resolution)?
        .to_canonical();
    Ok(SocketAddr::new(ip, port))
}

/// Serve one bounded CONNECT connection. Only the configured resolver and a
/// checked, pinned destination IP are reachable from this transport.
pub async fn serve(mut stream: UnixStream, policy: &EgressPolicy) -> Result<(), EgressError> {
    let setup = timeout(SETUP_TIMEOUT, async {
        let address =
            destination(&mut stream, policy, SocketAddr::new(policy.resolver, 53)).await?;
        TcpStream::connect(address)
            .await
            .map_err(|_| EgressError::Transport)
    })
    .await
    .unwrap_or(Err(EgressError::Timeout));
    match setup {
        Ok(upstream) => tunnel(stream, upstream).await,
        Err(error) => {
            let _ = timeout(Duration::from_secs(1), stream.write_all(DENIED)).await;
            Err(error)
        }
    }
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

#[cfg(test)]
#[path = "browser_egress/tests.rs"]
mod tests;
