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
    ActionArgs, DEFAULT_MAX_FRAME, ErrorCode, FrameError, Observation, Receipt, Request, Response,
    SessionFacts, SessionPolicy, SessionState, Verb, read_frame, write_message,
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
}

/// 生の応答の先頭（診断用）。秘密は応答に載らない（session id・receipt・観測値のみ）。
const RAW_EXCERPT_MAX: usize = 512;

fn raw_excerpt(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    match text.char_indices().nth(RAW_EXCERPT_MAX) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.into_owned(),
    }
}

fn unexpected(want: &str, got: &Response) -> ClientError {
    let got = match got {
        Response::Started { .. } => "started",
        Response::ActionResult { .. } => "action_result",
        Response::Observed { .. } => "observed",
        Response::Stopped { .. } => "stopped",
        Response::Error { .. } => "error",
    };
    ClientError::Protocol(format!("expected a {want} response, got {got}"))
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
