//! 偽の Discord webhook: `POST /hook` に 204 を返し、受けた本文を残す。
//!
//! `#[path]` で読む。読む crate の根に `fake_http`（`tests/support/http_async.rs`）が要る。
//! 読み手: `cos_delivery_suite`（`cos_chat_triage_scenarios`）・`root_done_summary.rs`。

use std::sync::{Arc, Mutex};

use tokio::io::AsyncWriteExt;

use crate::fake_http;

pub struct Hook {
    pub url: String,
    /// 受けた request の本文（受けた順）。
    pub bodies: Arc<Mutex<Vec<String>>>,
    /// request を 1 件受けて応答するたびに 1 つ届く。
    pub seen: tokio::sync::mpsc::UnboundedReceiver<()>,
}

/// loopback の空き port で受け始め、その URL を持つ [`Hook`] を返す。
pub async fn hook() -> Hook {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/hook", listener.local_addr().expect("addr"));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let (tx, seen) = tokio::sync::mpsc::unbounded_channel();
    let kept = bodies.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let kept = kept.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let Some(raw) =
                    fake_http::read(&mut stream, |r| fake_http::matches(r, "POST", "/hook")).await
                else {
                    return;
                };
                let body = raw.split_once("\r\n\r\n").unwrap().1.to_owned();
                kept.lock().expect("bodies").push(body);
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = tx.send(());
            });
        }
    });
    Hook { url, bodies, seen }
}
