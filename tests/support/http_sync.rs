//! Blocking counterpart of http_async for CLI fixture threads.
#![allow(dead_code)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "http_request.rs"]
mod framing;
pub use framing::matches;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

pub fn read(stream: &mut TcpStream, expected: impl Fn(&str) -> bool) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = (|| {
        let mut bytes = Vec::new();
        let mut buf = [0; 8192];
        while Instant::now() < deadline {
            let n = stream.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            bytes.extend_from_slice(&buf[..n]);
            if framing::complete(&bytes).ok()?.is_some() {
                let raw = String::from_utf8(bytes).ok()?;
                return expected(&raw).then_some(raw);
            }
        }
        None
    })();
    if result.is_none() {
        let _ = stream.write_all(framing::NOT_FOUND);
    }
    result
}

pub fn accept(listener: &TcpListener, method: &str, path: &str) -> (TcpStream, String) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let (tx, rx) = std::sync::mpsc::channel();
    while Instant::now() < deadline {
        if let Ok(request) = rx.try_recv() {
            return request;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                let (method, path, tx) = (method.to_owned(), path.to_owned(), tx.clone());
                std::thread::spawn(move || {
                    if let Some(raw) = read(&mut stream, |r| matches(r, &method, &path)) {
                        let _ = tx.send((stream, raw));
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("fake server accept: {e}"),
        }
    }
    panic!("expected HTTP request within 60 seconds");
}
