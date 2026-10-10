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
        Self::start_shared(
            Arc::new(Mutex::new(controller)),
            socket,
            token,
            allowed_domains,
            mode,
        )
    }

    /// Like [`Self::start_with_mode`] around a controller that is already shared.
    pub fn start_shared(
        controller: Arc<Mutex<CdpController>>,
        socket: &Path,
        token: String,
        allowed_domains: Vec<String>,
        mode: u32,
    ) -> std::io::Result<Self> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(std::io::Error::other("invalid relay token"));
        }
        let listener = bind_unix_listener(socket)?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(mode))?;
        listener.set_nonblocking(true)?;
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

/// `socket` に unix socket を bind する。path が `sun_path`（107 byte）に収まらないとき
/// （長い TMPDIR・深い workspace の下の session dir）は、親 dir を開いた fd の
/// `/proc/self/fd/<fd>/<name>` という短い別名で bind する。socket の実体は同じ dir に
/// できるので、sandbox からは従来どおり `/session/<name>` で見える。別名でも収まらなければ
/// path と長さを含む明示の誤りにする（ADR 2026-10-07-build-tmp-hygiene 付記）。
pub(crate) fn bind_unix_listener(socket: &Path) -> std::io::Result<UnixListener> {
    let max = crate::browser_action::SUN_PATH_MAX;
    if socket.as_os_str().len() <= max {
        return UnixListener::bind(socket);
    }
    let too_long = |len: usize| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "unix socket path is {len} bytes (limit {max}) and has no short alias: {}",
                socket.display()
            ),
        )
    };
    let (Some(parent), Some(name)) = (socket.parent(), socket.file_name()) else {
        return Err(too_long(socket.as_os_str().len()));
    };
    // fd は bind の間だけ開いておけばよい。socket の inode は dir に残る。
    let dir = std::fs::File::open(parent)?;
    let alias = short_alias(&dir, name);
    if alias.as_os_str().len() > max {
        return Err(too_long(socket.as_os_str().len()));
    }
    UnixListener::bind(&alias)
}

/// 開いた dir の中の `name` を指す短い path（`/proc/self/fd/<fd>/<name>`）。
pub(crate) fn short_alias(dir: &std::fs::File, name: &std::ffi::OsStr) -> PathBuf {
    use std::os::fd::AsRawFd;
    Path::new("/proc/self/fd")
        .join(dir.as_raw_fd().to_string())
        .join(name)
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
    loop {
        // F1: while the agent sends nothing, keep delivering the browser's events
        // (agent-browser waits for `Page.loadEventFired` after `Page.navigate` replies).
        if !agent_readable(&stream, IDLE_PUMP) {
            let sent = match controller.lock() {
                Ok(mut c) if !c.observation_stopped() => match c.pump_events() {
                    Ok(()) => {
                        let events = c.take_agent_events_for(&sessions);
                        forward_events(&mut stream, events, &sessions).is_ok()
                    }
                    Err(_) => break,
                },
                _ => true,
            };
            if !sent {
                break;
            }
            continue;
        }
        let Some(frame) = read_ws(&mut stream) else {
            break;
        };
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
            || denied_method(method);
        let (response, events) = match controller.lock() {
            Ok(mut c) if !c.auth_section_active() && !blocked => {
                let result = c.agent_command(method, params, session);
                let mut events = c.take_agent_events_for(&sessions);
                let reply = match result {
                    Ok(mut reply) => {
                        if method == "Target.attachToTarget"
                            && let Some(s) = reply["result"]["sessionId"].as_str()
                        {
                            sessions.insert(s.to_owned());
                            // Events of the new session read during this call.
                            events.extend(c.take_agent_events_for(&sessions));
                        }
                        reply["id"] = id.clone();
                        reply
                    }
                    Err(
                        e @ (InjectionError::RedisplayDetected
                        | InjectionError::ObservationOriginDenied
                        | InjectionError::PasswordFieldPresent),
                    ) => error(id, e.code()),
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
        let sent = match controller.lock() {
            Ok(c) if !c.observation_stopped() => {
                forward_events(&mut stream, events, &sessions).is_ok()
            }
            _ => true,
        };
        if !sent {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

const IDLE_PUMP: Duration = Duration::from_millis(20);

/// Whether the agent has sent bytes (or closed) within `wait`.
fn agent_readable(stream: &UnixStream, wait: Duration) -> bool {
    use std::os::fd::AsRawFd;
    let mut pollfd = nix::libc::pollfd {
        fd: stream.as_raw_fd(),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to one valid descriptor and structure.
    unsafe { nix::libc::poll(&mut pollfd, 1, wait.as_millis() as i32) != 0 }
}

/// CDP methods no agent connection may call: request/response bodies, cookie and storage reads
/// (ADR 2026-10-09 credential username / post-login D2-3), tracing and screencast.
pub(crate) fn denied_method(method: &str) -> bool {
    matches!(
        method,
        "Network.getRequestPostData"
            | "Network.getResponseBody"
            | "Network.getResponseBodyForInterception"
            | "Network.takeResponseBodyForInterceptionAsStream"
            | "Network.getCookies"
            | "Network.getAllCookies"
            | "Network.replayXHR"
            | "Fetch.getResponseBody"
            | "Fetch.takeResponseBodyAsStream"
            | "Storage.getCookies"
            | "Storage.getSharedStorageEntries"
            | "Page.getResourceContent"
            | "Page.searchInResource"
            | "Page.startScreencast"
            | "IO.read"
    ) || method.starts_with("Tracing.")
        || method.starts_with("DOMStorage.")
        || method.starts_with("IndexedDB.")
        || method.starts_with("CacheStorage.")
}

/// Header names never forwarded to an agent connection (case-insensitive).
const STRIPPED_HEADERS: [&str; 4] = [
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
];

/// Remove credential-bearing headers and cookie lists from a CDP `Network.*` / `Fetch.*` event
/// (ADR 2026-10-09 credential username / post-login D2-3). Raw header text is dropped entirely.
pub(crate) fn strip_credential_headers(event: &mut Value) {
    let Some(params) = event.get_mut("params").and_then(Value::as_object_mut) else {
        return;
    };
    for key in [
        "headersText",
        "associatedCookies",
        "blockedCookies",
        "exemptedCookies",
        "cookiePartitionKey",
    ] {
        params.remove(key);
    }
    let strip = |headers: &mut Value| match headers {
        Value::Object(map) => {
            map.retain(|name, _| !STRIPPED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
        }
        // Fetch domain: [{name, value}].
        Value::Array(entries) => entries.retain(|h| {
            h["name"]
                .as_str()
                .is_none_or(|n| !STRIPPED_HEADERS.contains(&n.to_ascii_lowercase().as_str()))
        }),
        _ => {}
    };
    for key in ["headers", "responseHeaders"] {
        if let Some(h) = params.get_mut(key) {
            strip(h);
        }
    }
    for holder in ["request", "response", "redirectResponse"] {
        if let Some(obj) = params.get_mut(holder).and_then(Value::as_object_mut) {
            obj.remove("headersText");
            obj.remove("requestHeadersText");
            for key in ["headers", "requestHeaders"] {
                if let Some(h) = obj.get_mut(key) {
                    strip(h);
                }
            }
        }
    }
}

/// What every event crossing to an agent connection loses: request bodies (ADR-0110 D1) and
/// credential headers / cookie lists (ADR 2026-10-09 credential username / post-login D2-3).
pub(crate) fn sanitize_agent_event(event: &mut Value) {
    if let Some(request) = event
        .pointer_mut("/params/request")
        .and_then(Value::as_object_mut)
    {
        request.remove("postData");
        request.remove("postDataEntries");
        request.remove("hasPostData");
    }
    strip_credential_headers(event);
}

/// Only events of sessions this connection attached cross; request bodies and credential headers
/// never do.
fn forward_events(
    stream: &mut UnixStream,
    events: Vec<Value>,
    sessions: &HashSet<String>,
) -> std::io::Result<()> {
    for mut event in events {
        let allowed = event["sessionId"]
            .as_str()
            .is_some_and(|s| sessions.contains(s));
        if !allowed {
            continue;
        }
        sanitize_agent_event(&mut event);
        send_ws(stream, &event)?;
    }
    Ok(())
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
    crate::browser_policy::url_origin_allowed(url, domains)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// sun_path（107 byte）を確実に超える深さの dir を作る。
    fn long_dir(base: &Path) -> PathBuf {
        let dir = base
            .join("abcdefghijklmnopqrstuvwxyz0123")
            .join("runs")
            .join("01M4BH3KF5GHH13SM1RZHDJNYW")
            .join("browser-fallback-1");
        std::fs::create_dir_all(&dir).expect("long dir");
        dir
    }

    #[test]
    fn tmpdir_long_path_bind_unix_listener_binds_in_place() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = long_dir(tmp.path());
        let socket = dir.join("cdp-relay.sock");
        assert!(socket.as_os_str().len() > crate::browser_action::SUN_PATH_MAX);
        assert!(
            UnixListener::bind(&socket).is_err(),
            "direct bind must overflow"
        );
        let listener = bind_unix_listener(&socket).expect("bind via short alias");
        use std::os::unix::fs::FileTypeExt;
        let meta = std::fs::symlink_metadata(&socket).expect("socket in session dir");
        assert!(meta.file_type().is_socket());
        let handle = std::fs::File::open(&dir).expect("dir");
        let mut client =
            UnixStream::connect(short_alias(&handle, "cdp-relay.sock".as_ref())).expect("connect");
        let (mut server, _) = listener.accept().expect("accept");
        client.write_all(b"ping").expect("write");
        let mut buf = [0; 4];
        server.read_exact(&mut buf).expect("read");
        assert_eq!(&buf, b"ping");
    }

    #[test]
    fn tmpdir_long_path_bind_unix_listener_short_path_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let socket = tmp.path().join("s.sock");
        let listener = bind_unix_listener(&socket).expect("bind");
        let addr = listener.local_addr().expect("addr");
        assert_eq!(addr.as_pathname(), Some(socket.as_path()));
    }

    #[test]
    fn tmpdir_long_path_bind_unix_listener_rejects_overlong_name_explicitly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let socket = tmp.path().join("n".repeat(120));
        let err = bind_unix_listener(&socket).expect_err("name alone exceeds sun_path");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("limit 107"), "{err}");
    }

    #[test]
    fn tmpdir_long_path_shared_cdp_starts_and_cleans_up() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = long_dir(tmp.path());
        let socket = dir.join("cdp-relay.sock");
        let devnull = || {
            std::fs::File::options()
                .read(true)
                .write(true)
                .open("/dev/null")
        };
        let controller = CdpController::new(devnull().expect("null"), devnull().expect("null"));
        let shared = SharedCdp::start(controller, &socket, "a".repeat(64), vec![])
            .expect("relay starts under a long session dir");
        let mode = std::fs::metadata(&socket)
            .expect("socket")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        drop(shared);
        assert!(!socket.exists(), "relay socket removed on drop");
    }

    /// ADR 2026-10-09 credential username / post-login D2-3: cookie / storage / body reads are refused
    /// and credential headers never reach the agent.
    #[test]
    fn post_login_relay_denies_cookie_storage_body_and_strips_credential_headers() {
        for m in [
            "Network.getCookies",
            "Network.getAllCookies",
            "Storage.getCookies",
            "DOMStorage.getDOMStorageItems",
            "IndexedDB.requestData",
            "CacheStorage.requestEntries",
            "Network.getResponseBody",
            "Fetch.getResponseBody",
            "Network.getRequestPostData",
            "Page.getResourceContent",
            "Tracing.start",
            "Page.startScreencast",
        ] {
            assert!(denied_method(m), "{m}");
        }
        for m in [
            "Page.navigate",
            "Runtime.evaluate",
            "Page.captureScreenshot",
            "Network.enable",
        ] {
            assert!(!denied_method(m), "{m}");
        }
        let mut sent = json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{
            "request":{"url":"https://lms.test/","headers":{"Cookie":"sid=1","Accept":"x","AUTHORIZATION":"Bearer t"}},
            "redirectResponse":{"headers":{"set-cookie":"sid=2","x":"y"},"requestHeadersText":"Cookie: sid=1"}}});
        strip_credential_headers(&mut sent);
        assert_eq!(sent["params"]["request"]["headers"], json!({"Accept":"x"}));
        assert_eq!(
            sent["params"]["redirectResponse"]["headers"],
            json!({"x":"y"})
        );
        assert!(sent["params"]["redirectResponse"]["requestHeadersText"].is_null());
        let mut extra = json!({"method":"Network.responseReceivedExtraInfo","params":{
            "headers":{"Set-Cookie":"sid=3","Content-Type":"text/html"},"headersText":"Set-Cookie: sid=3",
            "blockedCookies":[{"cookie":{"value":"v"}}]}});
        strip_credential_headers(&mut extra);
        assert_eq!(
            extra["params"]["headers"],
            json!({"Content-Type":"text/html"})
        );
        assert!(
            extra["params"]["headersText"].is_null() && extra["params"]["blockedCookies"].is_null()
        );
        let mut paused = json!({"method":"Fetch.requestPaused","params":{
            "request":{"headers":{"cookie":"a"}},
            "responseHeaders":[{"name":"Set-Cookie","value":"b"},{"name":"X","value":"c"}]}});
        strip_credential_headers(&mut paused);
        assert_eq!(paused["params"]["request"]["headers"], json!({}));
        assert_eq!(
            paused["params"]["responseHeaders"],
            json!([{"name":"X","value":"c"}])
        );
        let text = serde_json::to_string(&[sent, extra, paused]).unwrap();
        assert!(!text.contains("sid=") && !text.contains("Bearer"));
    }
}
