//! ADR-0116 D2/D5: daemon 側の同期 client。
//!
//! 1 接続 = 1 daemon 側の利用者。session は開いた接続に結びつくので、session を使い終わるまで
//! 同じ [`LauncherClient`] を持ち続ける（drop = 切断 = launcher が回収）。どの失敗も呼び出し側は
//! `isolated_runtime_unavailable` として fail closed に扱う。
//!
//! launcher の身元は `SO_PEERCRED` では確かめない（ADR-0116 付記 D-P）。socket 起動
//! （`celeris-browser-launcher.socket`）では listen socket を作ったのが systemd（root）なので、
//! `SO_PEERCRED` は uid 0 を返す。代わりに接続に `SO_PASSCRED` を立て、応答のたびに kernel が
//! 付ける `SCM_CREDENTIALS`（実際にその応答を書いた process の pid/uid/gid。送り手は偽れない）を
//! 読み、全応答で同じ送り手であることを確かめる。

use nix::libc;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::protocol::{
    ActionArgs, AuthenticateArgs, AuthenticationStatus, DEFAULT_MAX_FRAME, ErrorCode, FrameError,
    LoginObservation, LoginResult, Observation, Receipt, Request, Response, SessionFacts,
    SessionPolicy, SessionState, Verb, read_frame, write_message,
};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("launcher io: {0}")]
    Io(#[from] std::io::Error),
    #[error("launcher closed the connection")]
    Closed,
    /// 応答が読めない・要求に合わない。中身は診断用（serde の失敗理由と生 JSON の先頭、または
    /// 予期しない応答の種類）。launcher と daemon の protocol の版ずれもここに出る。
    #[error("launcher sent a malformed response: {0}")]
    Protocol(String),
    #[error("launcher refused: {0}")]
    Remote(ErrorCode),
}

impl From<FrameError> for ClientError {
    fn from(e: FrameError) -> Self {
        match e {
            FrameError::Closed => ClientError::Closed,
            FrameError::TooLarge(n) => ClientError::Protocol(format!("frame too large: {n} bytes")),
            FrameError::Io(e) => ClientError::Io(e),
        }
    }
}

/// launcher への 1 接続。
#[derive(Debug)]
pub struct LauncherClient {
    stream: UnixStream,
    max_frame: usize,
    responder: Responder,
}

/// 応答の送り手（`SCM_CREDENTIALS`）の記録。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Responder {
    /// まだ応答を受けていない。
    Unseen,
    /// 全応答が同じ送り手だった。
    Seen(SenderCred),
    /// 資格情報が採れない応答、または送り手の違う応答があった（以後ずっと `None` を返す）。
    Inconsistent,
}

/// kernel が付けた送り手の資格情報（受け手の pid/user namespace で見た値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderCred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// `start_session` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedSession {
    pub session_id: String,
    pub instance_id: String,
    pub receipt: Receipt,
}

impl LauncherClient {
    /// socket に接続する。`timeout` は 1 要求の読み書きの期限（launcher 側の期限より長く取る）。
    pub fn connect(path: &Path, timeout: Duration) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        // launcher は要求を受けてから書くので、connect 後に立てても最初の応答から資格情報が付く。
        set_passcred(&stream)?;
        Ok(Self {
            stream,
            max_frame: DEFAULT_MAX_FRAME,
            responder: Responder::Unseen,
        })
    }

    /// 応答を書いた launcher process の資格情報（kernel が付けた `SCM_CREDENTIALS`）。
    /// 応答をまだ受けていない・資格情報の無い応答があった・送り手が応答ごとに違った場合は `None`。
    pub fn responder(&self) -> Option<SenderCred> {
        match self.responder {
            Responder::Seen(c) => Some(c),
            Responder::Unseen | Responder::Inconsistent => None,
        }
    }

    /// 応答を書いた launcher の UID（[`Self::responder`] の uid）。
    pub fn responder_uid(&self) -> Option<u32> {
        self.responder().map(|c| c.uid)
    }

    #[cfg(test)]
    pub(crate) fn stream_for_test(&self) -> &UnixStream {
        &self.stream
    }

    fn note_responder(&mut self, seen: Option<SenderCred>) {
        self.responder = match (self.responder, seen) {
            (Responder::Unseen, Some(c)) => Responder::Seen(c),
            (Responder::Seen(prev), Some(c)) if prev == c => Responder::Seen(c),
            _ => Responder::Inconsistent,
        };
    }

    /// 要求を 1 個送り、応答を 1 個受ける。`error` 応答は `Remote` にする。
    pub fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        write_message(&mut self.stream, req, self.max_frame)?;
        self.read_response()
    }

    /// 要求を 1 個、FD 1 本（`SCM_RIGHTS`）を付けて送り、応答を 1 個受ける（v4 の `authenticate`）。
    /// この process の FD の写しは送った直後に閉じる（応答を待つ間も持たない）。
    fn request_with_fd(
        &mut self,
        req: &Request,
        fd: std::os::fd::OwnedFd,
    ) -> Result<Response, ClientError> {
        use std::os::fd::AsFd;
        let body = serde_json::to_vec(req).map_err(|e| ClientError::Protocol(e.to_string()))?;
        let sent = send_frame_with_fd(&self.stream, &body, self.max_frame, fd.as_fd());
        drop(fd);
        sent?;
        self.read_response()
    }

    fn read_response(&mut self) -> Result<Response, ClientError> {
        // 応答の先頭を覗いて送り手を記録してから frame を読む（覗くだけなので frame は崩れない）。
        match peek_sender(&self.stream)? {
            Peeked::Closed => return Err(ClientError::Closed),
            Peeked::Data(seen) => self.note_responder(seen),
        }
        let body = read_frame(&mut self.stream, self.max_frame)?;
        let resp: Response = serde_json::from_slice(&body).map_err(|e| {
            ClientError::Protocol(format!("{e}; raw response: {}", raw_excerpt(&body)))
        })?;
        match resp {
            Response::Error { code } => Err(ClientError::Remote(code)),
            other => Ok(other),
        }
    }

    /// launcher の申告（protocol の版と試験専用 loopback 許可）を問う。session は作らない。
    pub fn hello(&mut self) -> Result<(u32, Vec<String>), ClientError> {
        match self.request(&Request::Hello {})? {
            Response::Hello {
                protocol_version,
                test_loopback_allow,
            } => Ok((protocol_version, test_loopback_allow)),
            other => Err(unexpected("hello", &other)),
        }
    }

    pub fn start_session(
        &mut self,
        task_id: &str,
        run_id: &str,
        lease_id: &str,
        policy: SessionPolicy,
    ) -> Result<StartedSession, ClientError> {
        match self.request(&Request::StartSession {
            task_id: task_id.into(),
            run_id: run_id.into(),
            lease_id: lease_id.into(),
            policy,
        })? {
            Response::Started {
                session_id,
                instance_id,
                receipt,
            } => Ok(StartedSession {
                session_id,
                instance_id,
                receipt,
            }),
            other => Err(unexpected("started", &other)),
        }
    }

    pub fn action(
        &mut self,
        session_id: &str,
        lease_id: &str,
        verb: Verb,
        args: ActionArgs,
    ) -> Result<(Receipt, Observation), ClientError> {
        match self.request(&Request::Action {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
            verb,
            args,
        })? {
            Response::ActionResult {
                receipt,
                observation,
            } => Ok((receipt, observation)),
            other => Err(unexpected("action_result", &other)),
        }
    }

    /// v8: one chunk of an artifact (`kind`, whole `size`, decoded bytes from `offset`). The answer
    /// must echo the name and offset; anything else is a protocol error (no body text is kept).
    pub fn fetch_artifact(
        &mut self,
        session_id: &str,
        lease_id: &str,
        name: &str,
        offset: u64,
    ) -> Result<super::server::ArtifactChunk, ClientError> {
        use base64::Engine as _;
        match self.request(&Request::FetchArtifact {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
            name: name.into(),
            offset,
        })? {
            Response::Artifact {
                name: got,
                kind,
                size,
                offset: at,
                data,
            } if got == name && at == offset => {
                let data = base64::engine::general_purpose::STANDARD
                    .decode(data.as_bytes())
                    .map_err(|_| ClientError::Protocol("artifact chunk is not base64".into()))?;
                Ok(super::server::ArtifactChunk { kind, size, data })
            }
            Response::Artifact { .. } => Err(ClientError::Protocol(
                "artifact chunk does not match the request".into(),
            )),
            other => Err(unexpected("artifact", &other)),
        }
    }

    pub fn observe(
        &mut self,
        session_id: &str,
        lease_id: &str,
    ) -> Result<(SessionState, SessionFacts), ClientError> {
        match self.request(&Request::Observe {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
        })? {
            Response::Observed { state, facts } => Ok((state, facts)),
            other => Err(unexpected("observed", &other)),
        }
    }

    pub fn stop(&mut self, session_id: &str, lease_id: &str) -> Result<Receipt, ClientError> {
        match self.request(&Request::Stop {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
        })? {
            Response::Stopped { receipt } => Ok(receipt),
            other => Err(unexpected("stopped", &other)),
        }
    }

    /// v4: stop agent observation in the launcher session and create the login target.
    pub fn auth_begin(
        &mut self,
        session_id: &str,
        lease_id: &str,
        auth_section_id: &str,
    ) -> Result<String, ClientError> {
        match self.request(&Request::AuthBegin {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
            auth_section_id: auth_section_id.into(),
        })? {
            Response::AuthBegun { cdp_target_id } => Ok(cdp_target_id),
            other => Err(unexpected("auth_begun", &other)),
        }
    }

    /// Ask the launcher to perform the credential login over `broker` (an `injection.sock`
    /// connection this daemon opened; passed with `SCM_RIGHTS`). The only result is a fixed status.
    pub fn authenticate(
        &mut self,
        args: AuthenticateArgs,
        broker: std::os::fd::OwnedFd,
    ) -> Result<(AuthenticationStatus, LoginResult), ClientError> {
        match self.request_with_fd(&Request::Authenticate { args }, broker)? {
            // A v4 launcher answers without `observation`: observation stays stopped.
            Response::AuthenticateResult {
                status,
                observation,
                held_reason,
                consent_pressed,
                consent_controls,
            } => Ok((
                status,
                LoginResult {
                    observation: observation.unwrap_or(LoginObservation::Held),
                    held_reason,
                    consent_pressed,
                    // The launcher already sanitized them; the daemon does it again.
                    consent_controls: crate::browser_cdp_sink::sanitize_consent_controls(
                        consent_controls,
                    ),
                },
            )),
            other => Err(unexpected("authenticate_result", &other)),
        }
    }
}

/// 生の応答の先頭（診断用）。秘密は応答に載らない（session id・receipt・観測値のみ）。
const RAW_EXCERPT_MAX: usize = 512;

fn raw_excerpt(body: &[u8]) -> String {
    // v8: an artifact chunk carries file bytes; never copy them into a diagnostic.
    if body.windows(17).any(|w| w == br#""type":"artifact""#) {
        return "(artifact chunk withheld)".into();
    }
    let text = String::from_utf8_lossy(body);
    match text.char_indices().nth(RAW_EXCERPT_MAX) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.into_owned(),
    }
}

fn unexpected(want: &str, got: &Response) -> ClientError {
    let got = match got {
        Response::Hello { .. } => "hello",
        Response::Started { .. } => "started",
        Response::ActionResult { .. } => "action_result",
        Response::Observed { .. } => "observed",
        Response::Stopped { .. } => "stopped",
        Response::AuthBegun { .. } => "auth_begun",
        Response::Artifact { .. } => "artifact",
        Response::AuthenticateResult { .. } => "authenticate_result",
        Response::Error { .. } => "error",
    };
    ClientError::Protocol(format!("expected a {want} response, got {got}"))
}

/// 長さ前置きの frame を、先頭に `SCM_RIGHTS` の FD 1 本を付けて書く。
pub(super) fn send_frame_with_fd(
    stream: &UnixStream,
    body: &[u8],
    max: usize,
    fd: std::os::fd::BorrowedFd<'_>,
) -> Result<(), FrameError> {
    if body.len() > max {
        return Err(FrameError::TooLarge(body.len()));
    }
    let len = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge(body.len()))?;
    let mut frame = len.to_be_bytes().to_vec();
    frame.extend_from_slice(body);
    let mut iov = libc::iovec {
        iov_base: frame.as_mut_ptr().cast(),
        iov_len: frame.len(),
    };
    let mut control = [0u64; 4];
    // SAFETY: msghdr・iov・整列した control buffer は SCM_RIGHTS の FD 1 本に足りる。
    let sent = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<libc::c_int>() as u32) as _;
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<libc::c_int>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::c_int>(), fd.as_raw_fd());
        libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL)
    };
    if sent < 0 {
        return Err(FrameError::Io(std::io::Error::last_os_error()));
    }
    // FD は最初の送信に付いた。残りがあれば普通に書く。
    let rest = frame.get(sent as usize..).unwrap_or_default();
    if !rest.is_empty() {
        use std::io::Write;
        let mut w = stream;
        w.write_all(rest)?;
    }
    Ok(())
}

fn set_passcred(stream: &UnixStream) -> std::io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: on は c_int、長さはその大きさ。
    let rc = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&raw const on).cast(),
            std::mem::size_of_val(&on) as libc::socklen_t,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

enum Peeked {
    Closed,
    /// 受信待ちの先頭の skb の送り手（付いていなければ `None`）。
    Data(Option<SenderCred>),
}

/// `MSG_PEEK` で 1 byte と `SCM_CREDENTIALS` を覗く。AF_UNIX stream は送り手の資格情報が違う
/// skb を 1 回の読みにまとめないので、先頭 skb の資格情報はその応答を書いた process のもの。
fn peek_sender(stream: &UnixStream) -> std::io::Result<Peeked> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: byte.len(),
    };
    // CMSG_SPACE(sizeof(ucred)) = 32。cmsghdr の整列のため u64 で取る。
    let mut control = [0u64; 8];
    // SAFETY: msghdr は全 field 0 で有効。
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = std::mem::size_of_val(&control) as _;
    let n = loop {
        // SAFETY: msg は有効な buffer を指す。MSG_PEEK なので受信 queue は変わらない。
        let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, libc::MSG_PEEK) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        break n;
    };
    if n == 0 {
        return Ok(Peeked::Closed);
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Ok(Peeked::Data(None));
    }
    let mut cred = None;
    // SAFETY: recvmsg が埋めた control buffer を CMSG_* で辿る。
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET
                && (*cmsg).cmsg_type == libc::SCM_CREDENTIALS
                && (*cmsg).cmsg_len as usize
                    >= libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as u32) as usize
            {
                let u = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::ucred>());
                cred = Some(SenderCred {
                    pid: u.pid,
                    uid: u.uid,
                    gid: u.gid,
                });
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
    }
    Ok(Peeked::Data(cred))
}

#[cfg(test)]
mod excerpt_tests {
    #[test]
    fn artifact_chunks_are_withheld_from_diagnostics() {
        let body =
            br#"{"type":"artifact","name":"download-x","data":"U0VDUkVULUJZVEVT","bogus":1}"#;
        let text = super::raw_excerpt(body);
        assert!(!text.contains("U0VDUkVU"), "{text}");
        assert!(super::raw_excerpt(br#"{"type":"hello"}"#).contains("hello"));
    }
}
