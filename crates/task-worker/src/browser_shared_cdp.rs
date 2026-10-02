//! Controller-owned CDP pipe exposed to the isolated agent through a checked
//! WebSocket endpoint. The browser never opens a debugging TCP port.
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use url::Url;

use crate::browser_cdp_sink::{CdpController, InjectionError};

const MAX_MESSAGE: usize = 1 << 20;
pub const RELAY_PORT: u16 = 9223;
pub const AUTH_ERROR: &str = "auth_section_active";

pub struct SharedCdp {
    controller: Arc<Mutex<CdpController>>,
    socket: PathBuf,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SharedCdp {
    pub fn start(
        controller: CdpController,
        socket: &Path,
        token: String,
        allowed_domains: Vec<String>,
    ) -> std::io::Result<Self> {
        Self::start_with_mode(controller, socket, token, allowed_domains, 0o600)
    }

    /// Launcher sessions use a private parent directory and a token; the
    /// browser's mapped subordinate UID must be able to connect to this socket.
    pub fn start_with_mode(
        controller: CdpController,
        socket: &Path,
        token: String,
        allowed_domains: Vec<String>,
        mode: u32,
    ) -> std::io::Result<Self> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(std::io::Error::other("invalid relay token"));
        }
        let listener = UnixListener::bind(socket)?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(mode))?;
        listener.set_nonblocking(true)?;
        let controller = Arc::new(Mutex::new(controller));
        let shared = controller.clone();
        let path = socket.to_path_buf();
        let (stop, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("celeris-cdp-relay".into())
            .spawn(move || {
                while rx.try_recv().is_err() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let c = shared.clone();
                            let token = token.clone();
                            let domains = allowed_domains.clone();
                            let _ = std::thread::Builder::new()
                                .name("celeris-cdp-client".into())
                                .spawn(move || serve(stream, &c, &token, &domains));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            controller,
            socket: path,
            stop,
            thread: Some(thread),
        })
    }

    pub fn controller(&self) -> Arc<Mutex<CdpController>> {
        self.controller.clone()
    }

    pub fn endpoint(&self, token: &str) -> String {
        format!("ws://127.0.0.1:{RELAY_PORT}/{token}")
    }
}

impl Drop for SharedCdp {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn serve(
    mut stream: UnixStream,
    controller: &Arc<Mutex<CdpController>>,
    token: &str,
    domains: &[String],
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(50)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(50)));
    let Some(headers) = read_headers(&mut stream) else {
        return;
    };
    let mut lines = headers.split("\r\n");
    if lines.next() != Some(format!("GET /{token} HTTP/1.1").as_str())
        || lines
            .clone()
            .any(|line| line.to_ascii_lowercase().starts_with("origin:"))
    {
        return;
    }
    let key = lines.find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "));
    let Some(key) = key else { return };
    if controller
        .lock()
        .map(|c| c.auth_section_active())
        .unwrap_or(true)
    {
        let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let mut seed = key.as_bytes().to_vec();
    seed.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(sha1(&seed));
    if write!(stream, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").is_err() {
        return;
    }
    let mut sessions = HashSet::new();
    while let Some(frame) = read_ws(&mut stream) {
        let Ok(command) = serde_json::from_slice::<Value>(&frame) else {
            break;
        };
        let (Some(id), Some(method)) = (command.get("id"), command["method"].as_str()) else {
            break;
        };
        let params = command.get("params").cloned().unwrap_or_else(|| json!({}));
        let session = command["sessionId"].as_str();
        let blocked = session.is_some_and(|s| !sessions.contains(s))
            || !navigation_allowed(method, &params, domains)
            || matches!(
                method,
                "Network.getRequestPostData"
                    | "Network.getResponseBody"
                    | "Fetch.getResponseBody"
                    | "Network.replayXHR"
            )
            || method.starts_with("Tracing.")
            || method == "Page.startScreencast";
        let (response, events) = match controller.lock() {
            Ok(mut c) if !c.auth_section_active() && !blocked => {
                let result = c.agent_command(method, params, session);
                let events = c.take_agent_events();
                let reply = match result {
                    Ok(mut reply) => {
                        if method == "Target.attachToTarget"
                            && let Some(s) = reply["result"]["sessionId"].as_str()
                        {
                            sessions.insert(s.to_owned());
                        }
                        reply["id"] = id.clone();
                        reply
                    }
                    Err(e @ InjectionError::RedisplayDetected) => error(id, e.code()),
                    Err(_) => error(id, "cdp_command_failed"),
                };
                (reply, events)
            }
            Ok(c) => (
                error(
                    id,
                    if c.auth_section_active() {
                        AUTH_ERROR
                    } else {
                        "cdp_command_denied"
                    },
                ),
                Vec::new(),
            ),
            Err(_) => (error(id, AUTH_ERROR), Vec::new()),
        };
        if send_ws(&mut stream, &response).is_err() {
            break;
        }
        for mut event in events {
            let allowed = event["sessionId"]
                .as_str()
                .is_some_and(|s| sessions.contains(s));
            if !allowed {
                continue;
            }
            if let Some(request) = event
                .pointer_mut("/params/request")
                .and_then(Value::as_object_mut)
            {
                request.remove("postData");
                request.remove("postDataEntries");
                request.remove("hasPostData");
            }
            if send_ws(&mut stream, &event).is_err() {
                break;
            }
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn navigation_allowed(method: &str, params: &Value, domains: &[String]) -> bool {
    let url = match method {
        "Page.navigate" | "Target.createTarget" => params["url"].as_str(),
        "Page.navigateToHistoryEntry" => return false,
        _ => return true,
    };
    let Some(url) = url else { return false };
    if url == "about:blank" {
        return true;
    }
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    domains.iter().any(|d| {
        d.strip_prefix("*.")
            .map_or(host == d, |base| host.ends_with(&format!(".{base}")))
    })
}

fn error(id: &Value, code: &str) -> Value {
    json!({"id":id,"error":{"code":-32000,"message":code}})
}

fn read_headers(stream: &mut UnixStream) -> Option<String> {
    let mut out = Vec::new();
    let mut byte = [0];
    while out.len() < 8192 {
        stream.read_exact(&mut byte).ok()?;
        out.push(byte[0]);
        if out.ends_with(b"\r\n\r\n") {
            return String::from_utf8(out).ok();
        }
    }
    None
}

fn read_ws(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut h = [0; 2];
    stream.read_exact(&mut h).ok()?;
    if h[0] != 0x81 || h[1] & 0x80 == 0 {
        return None;
    }
    let mut n = (h[1] & 0x7f) as usize;
    if n == 126 {
        let mut ext = [0; 2];
        stream.read_exact(&mut ext).ok()?;
        n = u16::from_be_bytes(ext) as usize;
    } else if n == 127 {
        let mut ext = [0; 8];
        stream.read_exact(&mut ext).ok()?;
        n = usize::try_from(u64::from_be_bytes(ext)).ok()?;
    }
    if n > MAX_MESSAGE {
        return None;
    }
    let mut mask = [0; 4];
    stream.read_exact(&mut mask).ok()?;
    let mut payload = vec![0; n];
    stream.read_exact(&mut payload).ok()?;
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }
    Some(payload)
}

fn send_ws(stream: &mut UnixStream, message: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    let mut header = vec![0x81];
    if bytes.len() < 126 {
        header.push(bytes.len() as u8);
    } else if bytes.len() <= u16::MAX as usize {
        header.push(126);
        header.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(&bytes)
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut data = input.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0)
    }
    data.extend_from_slice(&bits.to_be_bytes());
    let mut h = [
        0x67452301u32,
        0xefcdab89,
        0x98badcfe,
        0x10325476,
        0xc3d2e1f0,
    ];
    for block in data.as_chunks::<64>().0 {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]));
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}
