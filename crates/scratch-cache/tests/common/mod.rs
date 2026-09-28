//! 統合テストの共通部: loopback の port 0 に cache server を立て、素の HTTP/1.1 で叩く（外部ネットワークに出ない）。

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use scratch_cache::TieredStore;
use scratch_cache::server::{ServerConfig, router, serve};

pub struct Running {
    pub port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `store` を loopback の port 0 で出す（専用のスレッドと runtime）。
pub fn start(store: TieredStore, token: Option<&str>) -> Running {
    start_on(store, token, 0)
}

pub fn start_on(store: TieredStore, token: Option<&str>, port: u16) -> Running {
    let (ptx, prx) = std::sync::mpsc::channel();
    let (stx, srx) = tokio::sync::oneshot::channel::<()>();
    let token = token.map(str::to_string);
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .unwrap();
            ptx.send(listener.local_addr().unwrap().port()).unwrap();
            let app = router(
                store,
                ServerConfig {
                    token,
                    ..Default::default()
                },
            );
            serve(listener, app, async move {
                let _ = srx.await;
            })
            .await
            .unwrap();
        });
    });
    let port = prx.recv_timeout(Duration::from_secs(10)).unwrap();
    Running {
        port,
        stop: Some(stx),
        thread: Some(thread),
    }
}

pub struct Resp {
    pub status: u16,
    pub headers: String,
    pub body: Vec<u8>,
}

/// 1 リクエスト（`Connection: close`）。
pub fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no header end: {:?}", String::from_utf8_lossy(&buf)));
    let headers = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut body = buf[split + 4..].to_vec();
    if let Some(n) = headers.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("content-length")
            .then(|| v.trim().parse::<usize>().ok())?
    }) && method != "HEAD"
    {
        body.truncate(n);
    }
    Resp {
        status,
        headers,
        body,
    }
}
