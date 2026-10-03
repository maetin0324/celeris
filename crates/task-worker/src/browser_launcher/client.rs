//! ADR-0116 D2/D5: daemon 側の同期 client。
//!
//! 1 接続 = 1 daemon 側の利用者。session は開いた接続に結びつくので、session を使い終わるまで
//! 同じ [`LauncherClient`] を持ち続ける（drop = 切断 = launcher が回収）。どの失敗も呼び出し側は
//! `isolated_runtime_unavailable` として fail closed に扱う。

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use super::protocol::{
    ActionArgs, DEFAULT_MAX_FRAME, ErrorCode, FrameError, Observation, Receipt, Request, Response,
    SessionFacts, SessionPolicy, SessionState, Verb, write_message,
};
use nix::libc;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("launcher io: {0}")]
    Io(#[from] std::io::Error),
    #[error("launcher closed the connection")]
    Closed,
    #[error("launcher sent a malformed response")]
    Protocol,
    #[error("launcher refused: {0}")]
    Remote(ErrorCode),
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn forked_response(with_credentials: bool) {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("launcher.sock");
        // Socket activation: the listener is created by the parent, then accepted by a child.
        let listener = UnixListener::bind(&socket).expect("bind");
        // SAFETY: the child only accepts, writes one response and exits without returning to
        // the test harness. The parent reaps it before the test finishes.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let code = match listener.accept() {
                Ok((mut stream, _)) => {
                    if super::super::protocol::read_frame(&mut stream, DEFAULT_MAX_FRAME).is_err() {
                        unsafe { libc::_exit(1) };
                    }
                    let response = Response::Observed {
                        state: SessionState::Running,
                        facts: SessionFacts::default(),
                    };
                    let result = if with_credentials {
                        super::super::server::write_credentialed_response(
                            &mut stream,
                            &response,
                            DEFAULT_MAX_FRAME,
                        )
                    } else {
                        write_message(&mut stream, &response, DEFAULT_MAX_FRAME)
                    };
                    if result.is_ok() { 0 } else { 1 }
                }
                Err(_) => 1,
            };
            unsafe { libc::_exit(code) };
        }
        let mut client = LauncherClient::connect(&socket, Duration::from_secs(5)).expect("connect");
        let socket_peer = super::super::server::peer_cred(&client.stream).expect("SO_PEERCRED");
        assert_eq!(socket_peer.0, unsafe { libc::getpid() });
        if !with_credentials {
            // Linux can synthesize SCM_CREDENTIALS for plain writes when SO_PASSCRED is on.
            // Disable it here to exercise the missing-control-message fail-closed path.
            let disabled: libc::c_int = 0;
            let rc = unsafe {
                libc::setsockopt(
                    client.stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PASSCRED,
                    (&disabled as *const libc::c_int).cast(),
                    std::mem::size_of_val(&disabled) as libc::socklen_t,
                )
            };
            assert_eq!(rc, 0);
        }
        assert_eq!(client.peer_uid(), None);
        let response = client.request(&Request::Observe {
            session_id: "session".into(),
            lease_id: "lease".into(),
        });
        assert!(
            matches!(response, Ok(Response::Observed { .. })),
            "{response:?}"
        );
        if with_credentials {
            assert_eq!(client.response_cred.map(|c| c.pid), Some(pid));
            assert_eq!(client.peer_uid(), Some(unsafe { libc::getuid() }));
        } else {
            assert_eq!(client.peer_uid(), None);
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(status, 0);
    }

    #[test]
    fn response_credentials_identify_accepting_child() {
        forked_response(true);
    }

    #[test]
    fn response_without_credentials_has_no_launcher_uid() {
        forked_response(false);
    }
}

impl From<FrameError> for ClientError {
    fn from(e: FrameError) -> Self {
        match e {
            FrameError::Closed => ClientError::Closed,
            FrameError::TooLarge(_) => ClientError::Protocol,
            FrameError::Io(e) => ClientError::Io(e),
        }
    }
}

/// launcher への 1 接続。
#[derive(Debug)]
pub struct LauncherClient {
    stream: UnixStream,
    max_frame: usize,
    response_cred: Option<libc::ucred>,
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
        let enabled: libc::c_int = 1;
        // SAFETY: enabled points to a valid c_int with the stated length.
        if unsafe {
            libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                (&enabled as *const libc::c_int).cast(),
                std::mem::size_of_val(&enabled) as libc::socklen_t,
            )
        } != 0
        {
            return Err(ClientError::Io(std::io::Error::last_os_error()));
        }
        Ok(Self {
            stream,
            max_frame: DEFAULT_MAX_FRAME,
            response_cred: None,
        })
    }

    /// 最後の応答に付いた `SCM_CREDENTIALS` の UID。応答前・資格情報なしなら `None`。
    pub fn peer_uid(&self) -> Option<u32> {
        self.response_cred.map(|cred| cred.uid)
    }

    /// 要求を 1 個送り、応答を 1 個受ける。`error` 応答は `Remote` にする。
    pub fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        self.response_cred = None;
        write_message(&mut self.stream, req, self.max_frame)?;
        let body = self
            .read_response()
            .inspect_err(|_| self.response_cred = None)?;
        let resp: Response = serde_json::from_slice(&body).map_err(|_| {
            self.response_cred = None;
            ClientError::Protocol
        })?;
        match resp {
            Response::Error { code } => {
                self.response_cred = None;
                Err(ClientError::Remote(code))
            }
            other => Ok(other),
        }
    }

    fn read_response(&mut self) -> Result<Vec<u8>, ClientError> {
        let mut header = [0u8; 4];
        let mut iov = libc::iovec {
            iov_base: header.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<libc::ucred>() as u32) } as usize;
        let mut control = vec![0usize; space.div_ceil(std::mem::size_of::<usize>())];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space;
        // SAFETY: iov and control point to live writable buffers for this call.
        let n = unsafe { libc::recvmsg(self.stream.as_raw_fd(), &mut msg, 0) };
        if n < 0 {
            return Err(ClientError::Io(std::io::Error::last_os_error()));
        }
        if n == 0 {
            return Err(ClientError::Closed);
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(ClientError::Protocol);
        }
        // SAFETY: recvmsg initialized msg and the control buffer; the bounds are checked.
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET
                    && (*cmsg).cmsg_type == libc::SCM_CREDENTIALS
                    && (*cmsg).cmsg_len
                        >= libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as u32) as usize
                {
                    self.response_cred = Some(std::ptr::read_unaligned(
                        libc::CMSG_DATA(cmsg).cast::<libc::ucred>(),
                    ));
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
        self.stream.read_exact(&mut header[1..])?;
        let len = u32::from_be_bytes(header) as usize;
        if len > self.max_frame {
            return Err(ClientError::Protocol);
        }
        let mut body = vec![0u8; len];
        self.stream.read_exact(&mut body)?;
        Ok(body)
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
            _ => Err(ClientError::Protocol),
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
            _ => Err(ClientError::Protocol),
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
            _ => Err(ClientError::Protocol),
        }
    }

    pub fn stop(&mut self, session_id: &str, lease_id: &str) -> Result<Receipt, ClientError> {
        match self.request(&Request::Stop {
            session_id: session_id.into(),
            lease_id: lease_id.into(),
        })? {
            Response::Stopped { receipt } => Ok(receipt),
            _ => Err(ClientError::Protocol),
        }
    }
}
