//! Shared fake-server reader. Reject probes before recording or consuming scripted replies.
#![allow(dead_code)]
#[path = "http_request.rs"]
mod framing;
pub use framing::matches;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub async fn read(stream: &mut TcpStream, expected: impl Fn(&str) -> bool) -> Option<String> {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        let mut buf = [0; 8192];
        loop {
            let n = stream.read(&mut buf).await.ok()?;
            if n == 0 {
                return None;
            }
            bytes.extend_from_slice(&buf[..n]);
            if framing::complete(&bytes).ok()?.is_some() {
                let raw = String::from_utf8(bytes).ok()?;
                return expected(&raw).then_some(raw);
            }
        }
    })
    .await
    .ok()
    .flatten();
    if result.is_none() {
        let _ = stream.write_all(framing::NOT_FOUND).await;
    }
    result
}

pub async fn accept(listener: &TcpListener, method: &str, path: &str) -> (TcpStream, String) {
    // Silent connections must not hold up a real client or consume a one-shot listener.
    let mut pending = tokio::task::JoinSet::new();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            tokio::select! {
                connection = listener.accept() => {
                    let (mut stream, _) = connection.expect("fake server accept");
                    let (method, path) = (method.to_owned(), path.to_owned());
                    pending.spawn(async move {
                        let raw = read(&mut stream, |r| matches(r, &method, &path)).await?;
                        Some((stream, raw))
                    });
                }
                Some(result) = pending.join_next(), if !pending.is_empty() => {
                    if let Some(request) = result.expect("fake server reader") { return request; }
                }
            }
        }
    })
    .await
    .expect("expected HTTP request within 60 seconds")
}

pub async fn inject_noise(url: &str) {
    let authority = url
        .strip_prefix("http://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    for raw in [
        "",
        "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "POST /decisions HTTP/1.1\r\nContent-Length: 8\r\n\r\nx",
        "POST /decisions HTTP/1.1\r\nContent-Len",
    ] {
        let mut stream = TcpStream::connect(authority).await.unwrap();
        stream.write_all(raw.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 404"), "{reply:?}");
    }
}
