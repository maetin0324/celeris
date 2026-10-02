//! ADR-0116 D2/D5: daemon 側の同期 client。
//!
//! 1 接続 = 1 daemon 側の利用者。session は開いた接続に結びつくので、session を使い終わるまで
//! 同じ [`LauncherClient`] を持ち続ける（drop = 切断 = launcher が回収）。どの失敗も呼び出し側は
//! `isolated_runtime_unavailable` として fail closed に扱う。

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
    #[error("launcher sent a malformed response")]
    Protocol,
    #[error("launcher refused: {0}")]
    Remote(ErrorCode),
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
        Ok(Self {
            stream,
            max_frame: DEFAULT_MAX_FRAME,
        })
    }

    /// 要求を 1 個送り、応答を 1 個受ける。`error` 応答は `Remote` にする。
    pub fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        write_message(&mut self.stream, req, self.max_frame)?;
        let body = read_frame(&mut self.stream, self.max_frame)?;
        let resp: Response = serde_json::from_slice(&body).map_err(|_| ClientError::Protocol)?;
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
