//! Protocol v9: the owner's Live View inside the launcher (ADR 2026-10-10-browser-launcher-live-view-frames
//! D1/D2/D4、付記 2026-10-10b).
//!
//! Chrome's CDP pipe is owned by the launcher's [`CdpController`]. [`LiveTap::interpose`] puts a
//! demultiplexer between Chrome's read pipe and the controller: every message is forwarded to the
//! controller unchanged except the events of the launcher's own screencast CDP sessions. Those
//! (`Page.screencastFrame` and the attach/detach announcements of the screencast session) never reach
//! the controller, so they can never be queued for an agent connection, and frames keep flowing
//! during an auth section (D3: the owner sees the login; agent observation stays stopped).
//!
//! [`ScreencastFeed`] drives `Page.startScreencast` / `Page.screencastFrameAck` /
//! `Page.stopScreencast` through the controller (the only writer of the pipe) and hands out
//! [`LiveImage`]s. The image types implement neither `Debug` nor `Serialize` and zero their bytes on
//! drop; they are never written to disk, logged or turned into an event (D2). There is no input path
//! (D4): the feed only sends the four screencast commands plus target discovery/attach/detach.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine as _;
use nix::libc;
use serde_json::{Value, json};
use zeroize::Zeroize;

use super::protocol::{LiveEncoding, MAX_LIVE_BODY};
use crate::browser_cdp_sink::CdpController;

/// Chrome's single CDP message limit (the controller's own bound).
const MAX_CDP_MESSAGE: usize = 64 << 20;
/// Stop reading Chrome while this many bytes wait for the controller (backpressure, as before).
const MAX_PENDING: usize = 4 << 20;
/// How long the demux sleeps in `poll` when nothing happens (held attach announcements are
/// released at the latest after this long).
const DEMUX_TICK_MS: i32 = 100;
/// Screencast size bound (the encoder keeps frames well below [`MAX_LIVE_BODY`]).
const MAX_WIDTH: u32 = 1280;
const MAX_HEIGHT: u32 = 960;
/// A failed screencast start is retried at most this often.
const RETRY: Duration = Duration::from_secs(1);

/// One decoded frame for the owner's viewer. Volatile: no `Debug`, no `Serialize`, no `Clone`;
/// the bytes are zeroed on drop.
pub struct LiveImage {
    width: u32,
    height: u32,
    encoding: LiveEncoding,
    body: Vec<u8>,
}

impl LiveImage {
    pub fn new(width: u32, height: u32, encoding: LiveEncoding, body: Vec<u8>) -> Self {
        Self {
            width,
            height,
            encoding,
            body,
        }
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn encoding(&self) -> LiveEncoding {
        self.encoding
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    /// Moves the bytes out (the daemon hands them to its volatile frame slot).
    pub fn into_body(mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }
}

impl Drop for LiveImage {
    fn drop(&mut self) {
        self.body.zeroize();
    }
}

/// What [`LiveFeed::next_frame`] saw within its wait.
pub enum LiveNext {
    Frame(LiveImage),
    /// No new frame yet.
    Idle,
    /// The session's browser is gone; the stream ends.
    Ended,
}

/// A session's frame source. Dropping it stops the screencast.
pub trait LiveFeed: Send {
    fn next_frame(&mut self, wait: Duration) -> LiveNext;
}

/// The latest screencast frame Chrome sent (capacity 1; a newer one replaces it).
struct RawFrame {
    cdp_session: String,
    ack: i64,
    width: u32,
    height: u32,
    data: String,
}

impl Drop for RawFrame {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

#[derive(Default)]
struct TapState {
    /// The current screencast CDP session.
    live: Option<String>,
    /// Every screencast CDP session this tap attached: their events never reach the controller.
    known: HashSet<String>,
    /// A target the feed is attaching to: its `Target.attachedToTarget` waits in `held` until the
    /// attach reply names the session.
    pending: Option<String>,
    held: Vec<Vec<u8>>,
    /// The attach finished: `held` goes to the controller (minus the screencast session's).
    release: bool,
    frame: Option<RawFrame>,
    /// Target the owner most likely looks at (the login target during an auth section).
    preferred: Option<String>,
    ended: bool,
    /// Frames Chrome sent for the current session (test diagnostics; no content).
    frames_seen: u64,
}

/// The demultiplexer between Chrome's read pipe and the controller (see the module doc).
pub struct LiveTap {
    state: Mutex<TapState>,
    cv: Condvar,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn cloexec_pipe() -> std::io::Result<(File, File)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: fds has room for the two descriptors pipe2 writes.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: pipe2 succeeded; both descriptors are new and owned only here.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    Ok((File::from(r), File::from(w)))
}

fn set_nonblocking(f: &File) -> std::io::Result<()> {
    // SAFETY: fcntl on a valid descriptor.
    let flags = unsafe { libc::fcntl(f.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        // SAFETY: as above.
        || unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

impl LiveTap {
    /// Puts the tap between `chrome_read` (Chrome's CDP output) and the controller. Returns the
    /// read end to give to [`CdpController::new`].
    pub fn interpose(chrome_read: File) -> std::io::Result<(File, Arc<LiveTap>)> {
        let (ctrl_read, ctrl_write) = cloexec_pipe()?;
        set_nonblocking(&ctrl_write)?;
        let tap = Arc::new(LiveTap {
            state: Mutex::new(TapState::default()),
            cv: Condvar::new(),
        });
        let t2 = tap.clone();
        std::thread::Builder::new()
            .name("celeris-launcher-live-tap".into())
            .spawn(move || {
                t2.demux(chrome_read, ctrl_write);
                lock(&t2.state).ended = true;
                t2.cv.notify_all();
            })?;
        Ok((ctrl_read, tap))
    }

    /// The target the feed should show (the auth section's login target, v4 `auth_begin`).
    pub fn set_preferred_target(&self, target: Option<String>) {
        lock(&self.state).preferred = target;
        self.cv.notify_all();
    }

    pub fn ended(&self) -> bool {
        lock(&self.state).ended
    }

    #[cfg(test)]
    pub(crate) fn frames_seen(&self) -> u64 {
        lock(&self.state).frames_seen
    }

    fn begin_attach(&self, target: &str) {
        let mut st = lock(&self.state);
        st.pending = Some(target.to_owned());
        st.release = false;
    }

    fn finish_attach(&self, session: Option<&str>) {
        let mut st = lock(&self.state);
        if let Some(s) = session {
            st.known.insert(s.to_owned());
            st.live = Some(s.to_owned());
            st.frame = None;
        }
        st.pending = None;
        st.release = true;
    }

    fn clear_live(&self) {
        let mut st = lock(&self.state);
        st.live = None;
        st.frame = None;
    }

    /// Classifies one Chrome message. `true` = forward to the controller.
    fn route(&self, st: &mut TapState, msg: &[u8]) -> Route {
        let Ok(v) = serde_json::from_slice::<Value>(msg) else {
            return Route::Forward;
        };
        // Replies (to the controller's commands, including the feed's) always go to the controller.
        let Some(method) = v["method"].as_str() else {
            return Route::Forward;
        };
        let session = v["sessionId"].as_str();
        let inner_session = v["params"]["sessionId"].as_str();
        if let Some(s) = session
            && st.known.contains(s)
        {
            if method == "Page.screencastFrame" && st.live.as_deref() == Some(s) {
                let p = &v["params"];
                if let (Some(data), Some(ack)) = (p["data"].as_str(), p["sessionId"].as_i64()) {
                    st.frames_seen += 1;
                    st.frame = Some(RawFrame {
                        cdp_session: s.to_owned(),
                        ack,
                        width: dim(&p["metadata"]["deviceWidth"]),
                        height: dim(&p["metadata"]["deviceHeight"]),
                        data: data.to_owned(),
                    });
                    self.cv.notify_all();
                }
            }
            return Route::Drop;
        }
        if let Some(s) = inner_session
            && st.known.contains(s)
        {
            if method == "Target.detachedFromTarget" && st.live.as_deref() == Some(s) {
                st.live = None;
                st.frame = None;
                self.cv.notify_all();
            }
            return Route::Drop;
        }
        if method == "Target.attachedToTarget"
            && st.pending.is_some()
            && v["params"]["targetInfo"]["targetId"].as_str() == st.pending.as_deref()
        {
            return Route::Hold;
        }
        Route::Forward
    }

    fn demux(&self, mut chrome: File, mut ctrl: File) {
        let mut buffered: Vec<u8> = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut written = 0usize;
        let mut chunk = [0u8; 16 * 1024];
        loop {
            // Held attach announcements go out once the attach finished.
            {
                let mut st = lock(&self.state);
                if st.release {
                    st.release = false;
                    for mut m in std::mem::take(&mut st.held) {
                        let drop_it = serde_json::from_slice::<Value>(&m[..m.len() - 1])
                            .ok()
                            .and_then(|v| v["params"]["sessionId"].as_str().map(str::to_owned))
                            .is_some_and(|s| st.known.contains(&s));
                        if !drop_it {
                            pending.extend_from_slice(&m);
                        }
                        m.zeroize();
                    }
                }
            }
            let want_read = pending.len() - written < MAX_PENDING;
            let mut fds = [
                libc::pollfd {
                    fd: chrome.as_raw_fd(),
                    events: if want_read { libc::POLLIN } else { 0 },
                    revents: 0,
                },
                libc::pollfd {
                    fd: ctrl.as_raw_fd(),
                    events: if written < pending.len() {
                        libc::POLLOUT
                    } else {
                        0
                    },
                    revents: 0,
                },
            ];
            // SAFETY: fds points to two valid pollfd structures.
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, DEMUX_TICK_MS) };
            if rc < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if fds[1].revents & (libc::POLLERR | libc::POLLHUP) != 0 {
                // The controller is gone (session stop).
                break;
            }
            if written < pending.len() && fds[1].revents & libc::POLLOUT != 0 {
                match ctrl.write(&pending[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
                if written == pending.len() {
                    pending.zeroize();
                    pending.clear();
                    written = 0;
                }
            }
            if want_read && fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                let n = match chrome.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buffered.extend_from_slice(&chunk[..n]);
                chunk[..n].zeroize();
                while let Some(end) = buffered.iter().position(|b| *b == 0) {
                    let mut msg: Vec<u8> = buffered.drain(..=end).collect();
                    let route = {
                        let mut st = lock(&self.state);
                        self.route(&mut st, &msg[..end])
                    };
                    match route {
                        Route::Forward => pending.extend_from_slice(&msg),
                        Route::Hold => {
                            lock(&self.state).held.push(std::mem::take(&mut msg));
                            continue;
                        }
                        Route::Drop => {}
                    }
                    msg.zeroize();
                }
                if buffered.len() >= MAX_CDP_MESSAGE {
                    break;
                }
            }
        }
        // Flush what the controller may still read (best effort), then close its pipe.
        if written < pending.len() {
            let _ = ctrl.write(&pending[written..]);
        }
        pending.zeroize();
        buffered.zeroize();
        let mut st = lock(&self.state);
        for m in &mut st.held {
            m.zeroize();
        }
        st.held.clear();
    }
}

enum Route {
    Forward,
    Hold,
    Drop,
}

fn dim(v: &Value) -> u32 {
    v.as_f64()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map_or(0, |f| f.round().min(f64::from(u32::MAX)) as u32)
}

/// The launcher's screencast of one session (the only [`LiveFeed`] of the runtime backend).
pub struct ScreencastFeed {
    tap: Arc<LiveTap>,
    controller: Arc<Mutex<CdpController>>,
    /// (target, CDP session) being screencast.
    current: Option<(String, String)>,
    last_try: Option<Instant>,
}

impl ScreencastFeed {
    pub fn new(tap: Arc<LiveTap>, controller: Arc<Mutex<CdpController>>) -> Self {
        Self {
            tap,
            controller,
            current: None,
            last_try: None,
        }
    }

    fn command(&self, method: &str, params: Value, session: Option<&str>) -> Option<Value> {
        let mut c = self.controller.lock().ok()?;
        c.controller_command(method, params, session).ok()
    }

    /// The page to show: the preferred (login) target when it exists, else the last page target.
    fn pick_target(&self) -> Option<String> {
        let targets = self.command("Target.getTargets", json!({}), None)?;
        let pages: Vec<&str> = targets["result"]["targetInfos"]
            .as_array()?
            .iter()
            .filter(|t| t["type"] == "page")
            .filter_map(|t| t["targetId"].as_str())
            .collect();
        let preferred = lock(&self.tap.state).preferred.clone();
        match preferred {
            Some(p) if pages.contains(&p.as_str()) => Some(p),
            _ => pages.last().map(|s| (*s).to_owned()),
        }
    }

    fn start(&mut self) {
        if self.last_try.is_some_and(|t| t.elapsed() < RETRY) {
            return;
        }
        self.last_try = Some(Instant::now());
        let Some(target) = self.pick_target() else {
            return;
        };
        self.tap.begin_attach(&target);
        let session = self
            .command(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )
            .and_then(|r| r["result"]["sessionId"].as_str().map(str::to_owned));
        self.tap.finish_attach(session.as_deref());
        let Some(session) = session else {
            return;
        };
        let started = self.command(
            "Page.startScreencast",
            json!({"format":"jpeg","quality":60,"maxWidth":MAX_WIDTH,"maxHeight":MAX_HEIGHT,"everyNthFrame":1}),
            Some(&session),
        );
        if started.is_none() {
            self.detach(&session);
            return;
        }
        self.current = Some((target, session));
    }

    fn detach(&self, session: &str) {
        self.tap.clear_live();
        let _ = self.command("Page.stopScreencast", json!({}), Some(session));
        let _ = self.command(
            "Target.detachFromTarget",
            json!({"sessionId":session}),
            None,
        );
    }

    fn stop(&mut self) {
        if let Some((_, session)) = self.current.take() {
            self.detach(&session);
        }
    }
}

impl LiveFeed for ScreencastFeed {
    fn next_frame(&mut self, wait: Duration) -> LiveNext {
        if self.tap.ended() {
            return LiveNext::Ended;
        }
        // Follow the auth section's login target, and re-attach after the target went away.
        let (live, preferred) = {
            let st = lock(&self.tap.state);
            (st.live.clone(), st.preferred.clone())
        };
        let stale = match &self.current {
            None => true,
            Some((target, session)) => {
                live.as_deref() != Some(session.as_str())
                    || preferred.as_ref().is_some_and(|p| p != target)
            }
        };
        if stale {
            if self.current.is_some() {
                self.stop();
                self.last_try = None;
            }
            self.start();
        }
        let Some((_, session)) = self.current.clone() else {
            std::thread::sleep(wait.min(RETRY));
            return LiveNext::Idle;
        };
        let raw = {
            let st = lock(&self.tap.state);
            let (mut st, _) = self
                .tap
                .cv
                .wait_timeout_while(st, wait, |s| s.frame.is_none() && !s.ended)
                .unwrap_or_else(|p| p.into_inner());
            if st.ended {
                return LiveNext::Ended;
            }
            st.frame.take()
        };
        let Some(raw) = raw else {
            return LiveNext::Idle;
        };
        if raw.cdp_session != session {
            return LiveNext::Idle;
        }
        // Chrome sends the next frame only after this ack (the launcher keeps the latest only).
        let _ = self.command(
            "Page.screencastFrameAck",
            json!({"sessionId":raw.ack}),
            Some(&session),
        );
        if raw.data.len() / 4 * 3 > MAX_LIVE_BODY + 3 {
            return LiveNext::Idle;
        }
        match base64::engine::general_purpose::STANDARD.decode(raw.data.as_bytes()) {
            Ok(body) if !body.is_empty() && body.len() <= MAX_LIVE_BODY => LiveNext::Frame(
                LiveImage::new(raw.width, raw.height, LiveEncoding::Jpeg, body),
            ),
            Ok(mut body) => {
                body.zeroize();
                LiveNext::Idle
            }
            Err(_) => LiveNext::Idle,
        }
    }
}

impl Drop for ScreencastFeed {
    fn drop(&mut self) {
        self.stop();
    }
}
