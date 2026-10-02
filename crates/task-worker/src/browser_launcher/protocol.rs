//! ADR-0116 D2: launcher の固定 protocol。
//!
//! 枠は 4 byte big-endian の長さ前置き + UTF-8 JSON 1 個。JSON は `deny_unknown_fields` の
//! `tag = "type"` enum で、未知の type・field は decode で拒否する。任意 argv・mount・環境変数・
//! UID/GID map・FD・path を表す型は無い（受け取れない）。cookie・credential・CDP payload も
//! 応答の型として表現できない。

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

/// frame の上限の既定値（64 KiB）。
pub const DEFAULT_MAX_FRAME: usize = 64 * 1024;
/// policy の許可 domain の数の上限。
pub const MAX_DOMAINS: usize = 64;
/// 1 個の文字列（id・domain・selector・URL）の長さの上限。
pub const MAX_STR: usize = 2048;
/// id（task / run / lease / session）の長さの上限。
pub const MAX_ID: usize = 128;
/// lease の長さの上限（秒）。
pub const MAX_LEASE_SECONDS: u64 = 24 * 60 * 60;

/// action の固定 verb（`browser_action` と同じ集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    Open,
    Click,
    Snapshot,
    Extract,
    Screenshot,
    Download,
    Scroll,
    Close,
}

/// browser policy の非機密部分（許可 domain・許可 action・lease の長さ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPolicy {
    pub allowed_domains: Vec<String>,
    pub allowed_actions: Vec<Verb>,
    pub lease_seconds: u64,
}

/// verb ごとの固定の引数。使わない欄は無い（`None`）こと。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
}

/// daemon → launcher。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    StartSession {
        task_id: String,
        run_id: String,
        lease_id: String,
        policy: SessionPolicy,
    },
    Action {
        session_id: String,
        lease_id: String,
        verb: Verb,
        #[serde(default)]
        args: ActionArgs,
    },
    Observe {
        session_id: String,
        lease_id: String,
    },
    Stop {
        session_id: String,
        lease_id: String,
    },
}

/// session の状態（固定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// 固定の error code。自由文の診断は返さない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    #[error("bad_request")]
    BadRequest,
    #[error("unauthorized")]
    Unauthorized,
    #[error("lease_mismatch")]
    LeaseMismatch,
    #[error("limit")]
    Limit,
    #[error("timeout")]
    Timeout,
    #[error("launch_failed")]
    LaunchFailed,
    #[error("isolation_failed")]
    IsolationFailed,
}

/// receipt の結果種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Started,
    Ok,
    Failed,
    Stopped,
}

/// 固定の receipt（session・instance・verb・結果・時刻・verify_isolation の結果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub session_id: String,
    pub instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verb: Option<Verb>,
    pub outcome: Outcome,
    pub at_unix_ms: u64,
    pub isolation_ok: bool,
}

/// `RuntimeFacts` の非機密の項目だけ。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionFacts {
    pub host_uid: u32,
    pub host_gid: u32,
    pub uid_map: String,
    pub gid_map: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ns_owner_uid: Option<u32>,
    pub cap_eff: String,
    pub no_new_privs: bool,
    pub listen_count: u32,
}

/// action の非機密の出力（snapshot / extract の text、screenshot の artifact 参照）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

/// launcher → daemon。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Started {
        session_id: String,
        instance_id: String,
        receipt: Receipt,
    },
    ActionResult {
        receipt: Receipt,
        observation: Observation,
    },
    Observed {
        state: SessionState,
        facts: SessionFacts,
    },
    Stopped {
        receipt: Receipt,
    },
    Error {
        code: ErrorCode,
    },
}

/// frame の読み書きの失敗。
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame too large: {0} bytes")]
    TooLarge(usize),
    #[error("peer closed")]
    Closed,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// 長さ前置きの frame を 1 個読む。長さが `max` を超えたら本体を読まずに `TooLarge`。
pub fn read_frame(r: &mut impl Read, max: usize) -> Result<Vec<u8>, FrameError> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > max {
        return Err(FrameError::TooLarge(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// 長さ前置きの frame を 1 個書く。
pub fn write_frame(w: &mut impl Write, body: &[u8], max: usize) -> Result<(), FrameError> {
    if body.len() > max {
        return Err(FrameError::TooLarge(body.len()));
    }
    let len = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge(body.len()))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(body)?;
    w.flush()?;
    Ok(())
}

/// 値を JSON にして frame で書く。
pub fn write_message<T: Serialize>(
    w: &mut impl Write,
    msg: &T,
    max: usize,
) -> Result<(), FrameError> {
    let body = serde_json::to_vec(msg).map_err(|e| FrameError::Io(std::io::Error::other(e)))?;
    write_frame(w, &body, max)
}

/// 要求を decode し、値の範囲（長さ・個数・lease）を検査する。どの失敗も `bad_request` か `limit`。
pub fn decode_request(body: &[u8]) -> Result<Request, ErrorCode> {
    let req: Request = serde_json::from_slice(body).map_err(|_| ErrorCode::BadRequest)?;
    req.validate()?;
    Ok(req)
}

fn check_id(s: &str) -> Result<(), ErrorCode> {
    let ok = !s.is_empty()
        && s.len() <= MAX_ID
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ErrorCode::BadRequest)
    }
}

fn check_str(s: &str) -> Result<(), ErrorCode> {
    if s.len() > MAX_STR {
        return Err(ErrorCode::Limit);
    }
    if s.chars().any(char::is_control) {
        return Err(ErrorCode::BadRequest);
    }
    Ok(())
}

impl SessionPolicy {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        if self.allowed_domains.len() > MAX_DOMAINS {
            return Err(ErrorCode::Limit);
        }
        for d in &self.allowed_domains {
            check_str(d)?;
        }
        if self.lease_seconds == 0 {
            return Err(ErrorCode::BadRequest);
        }
        if self.lease_seconds > MAX_LEASE_SECONDS {
            return Err(ErrorCode::Limit);
        }
        Ok(())
    }
}

impl ActionArgs {
    fn validate(&self) -> Result<(), ErrorCode> {
        for s in [&self.url, &self.selector].into_iter().flatten() {
            check_str(s)?;
        }
        Ok(())
    }
}

impl Request {
    /// 要求が触る session id（`start_session` は無し）。
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Request::StartSession { .. } => None,
            Request::Action { session_id, .. }
            | Request::Observe { session_id, .. }
            | Request::Stop { session_id, .. } => Some(session_id),
        }
    }

    pub fn validate(&self) -> Result<(), ErrorCode> {
        match self {
            Request::StartSession {
                task_id,
                run_id,
                lease_id,
                policy,
            } => {
                check_id(task_id)?;
                check_id(run_id)?;
                check_id(lease_id)?;
                policy.validate()
            }
            Request::Action {
                session_id,
                lease_id,
                args,
                ..
            } => {
                check_id(session_id)?;
                check_id(lease_id)?;
                args.validate()
            }
            Request::Observe {
                session_id,
                lease_id,
            }
            | Request::Stop {
                session_id,
                lease_id,
            } => {
                check_id(session_id)?;
                check_id(lease_id)
            }
        }
    }
}
