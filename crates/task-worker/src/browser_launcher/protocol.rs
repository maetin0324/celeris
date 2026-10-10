//! ADR-0116 D2: launcher の固定 protocol。
//!
//! 枠は 4 byte big-endian の長さ前置き + UTF-8 JSON 1 個。JSON は `deny_unknown_fields` の
//! `tag = "type"` enum で、未知の type・field は decode で拒否する。任意 argv・mount・環境変数・
//! UID/GID map・FD・path を表す型は無い（受け取れない）。cookie・credential・CDP payload も
//! 応答の型として表現できない。

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use task_core::browser_isolation::Namespace;

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

/// Fixed, secret-free login request (protocol v4, ADR 2026-10-09 付記「launcher の Authenticate
/// 経路」4). `lease_id` authorizes the launcher session; `credential_lease_id` is the credentiald
/// lease the daemon granted for this section. `login_url` and the selectors are the trusted login
/// pinned in the approved wait (administrator site policy); credentiald compares the password
/// selector with its own copy before it consumes the lease. No credential value is ever carried.
/// The request travels with exactly one `SCM_RIGHTS` FD: an `injection.sock` connection the daemon
/// opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticateArgs {
    pub session_id: String,
    pub lease_id: String,
    pub auth_section_id: String,
    pub credential_lease_id: String,
    pub origin: String,
    pub login_url: String,
    pub password_selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_selector: Option<String>,
    /// v5 (ADR 2026-10-09 credential username / post-login D1-5): the pinned username selector.
    /// The launcher fills username and password in one injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_selector: Option<String>,
    /// v5 (same D2): the effective post-login read. When present the launcher closes the auth
    /// section only on the post-login conditions and then lets the agent read `read_origins` with
    /// `actions`; otherwise observation stays stopped until the session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_login: Option<task_core::browser_wait::PostLogin>,
    /// v6: the daemon reads `held_reason` in the answer. A launcher answers with it only when asked,
    /// so a v5 daemon (which does not know the field) can talk to a v6 launcher.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub report_held_reason: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationStatus {
    Success,
    Rejected,
}

/// v5: whether agent observation resumed after a successful login (fixed; no page data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginObservation {
    /// ADR-0080 H3 holds until the session ends (no opt-in, or the post-login conditions did
    /// not hold within 15 s: `post_login_unconfirmed`).
    Held,
    /// The auth section closed on the post-login conditions.
    Resumed,
}

/// v6: what a successful login left (the fixed observation state and, when held after an opt-in,
/// the fixed reason — no URL, no page data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginResult {
    pub observation: LoginObservation,
    pub held_reason: Option<crate::browser_cdp_sink::PostLoginHeld>,
}

impl LoginResult {
    pub const HELD: Self = Self {
        observation: LoginObservation::Held,
        held_reason: None,
    };
    pub const RESUMED: Self = Self {
        observation: LoginObservation::Resumed,
        held_reason: None,
    };
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
    /// launcher の申告を問う（session は作らない）。本番の daemon は `start_session` の前に送り、
    /// 試験専用 loopback 許可が有効な launcher を拒否する（ADR 2026-10-05-browser-department-web-live-view
    /// 付記 E2）。
    Hello {},
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
    /// v4: stop agent observation on the launcher's controller and create the login target.
    AuthBegin {
        session_id: String,
        lease_id: String,
        auth_section_id: String,
    },
    Authenticate {
        args: AuthenticateArgs,
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

/// launcher protocol の版。v2 で `start_session` の receipt に [`SessionBinding`] を足した
/// （ADR-0138 D-L）。v3 で束縛の pid を launcher が隔離を検査した runtime process にし、その
/// namespace の inode（`ns_inodes`）を足した（daemon UID は別 UID の runtime の
/// `/proc/<pid>/ns/*` を開けないため）。v1 の receipt（`binding` 無し）と v2 の束縛（`ns_inodes`
/// 無し）は decode できるが、daemon は証明なしとして扱う。v4 で credential login の
/// `auth_begin` と `authenticate`（login_url・selector・credentiald lease と、`SCM_RIGHTS` で渡す
/// injection 接続）を足した（ADR 2026-10-09 付記「launcher の Authenticate 経路」）。daemon は
/// v4 未満の launcher に credential login を頼まない。v5 で `authenticate` に username selector と
/// post_login、応答に `observation` を足した（ADR 2026-10-09 credential username / post-login D1-5）。
/// daemon は policy がそのどちらかを使うなら v5 未満の launcher に頼まない（承認は消費しない）。
///
/// v6 で `authenticate_result` に固定の `held_reason`（観測を再開しなかった理由）を足し、ログイン後の
/// 待ちを IdP の中継頁を通して最長 60 秒にした（ADR 2026-10-09 credential username / post-login 付記
/// 2026-10-10）。post_login を使う login は v6 を要求する。
pub const PROTOCOL_VERSION: u32 = 6;

/// credential login（`auth_begin` / `authenticate`）を受ける最小の protocol 版。
pub const CREDENTIAL_LOGIN_PROTOCOL: u32 = 4;

/// username 欄の注入・ログイン後の読み取りを受ける最小の protocol 版。
pub const POST_LOGIN_PROTOCOL: u32 = 6;

/// username 欄の一括注入だけを受ける最小の protocol 版（v5）。
pub const USERNAME_PROTOCOL: u32 = 5;

/// launcher が `start_session` で返す session の束縛（launcher が `verify_isolation` を掛けた
/// runtime process の pid・starttime、launcher が採った userns の owner UID と 6 つの namespace の
/// inode）。daemon はこれを自分の観測と照合してから `LauncherSessionProof` を組む。値を持って
/// いるだけでは何も許さない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBinding {
    pub pid: i32,
    pub starttime: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ns_owner_uid: Option<u32>,
    /// v3。v2 以前の launcher は出さない（空 = 証明なし）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ns_inodes: BTreeMap<Namespace, u64>,
}

/// 固定の receipt（session・instance・verb・結果・時刻・verify_isolation の結果）。
/// `binding` は `start_session` の receipt にだけ入る（v2 以降）。
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<SessionBinding>,
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
    /// `hello` への応答。`test_loopback_allow` は launcher の root 所有 config で有効にした試験専用
    /// loopback 許可（`127.0.0.1:<port>`）。既定（空）は出さない。
    Hello {
        protocol_version: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        test_loopback_allow: Vec<String>,
    },
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
    /// v4: the login target the launcher created (an opaque CDP target id, no page data).
    AuthBegun {
        cdp_target_id: String,
    },
    AuthenticateResult {
        status: AuthenticationStatus,
        /// v5. v4 の launcher は出さない（= 観測停止のまま）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<LoginObservation>,
        /// v6: why observation stayed stopped after a post-login opt-in (fixed code).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held_reason: Option<crate::browser_cdp_sink::PostLoginHeld>,
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
            Request::Hello {} | Request::StartSession { .. } => None,
            Request::Action { session_id, .. }
            | Request::Observe { session_id, .. }
            | Request::Stop { session_id, .. } => Some(session_id),
            Request::AuthBegin { session_id, .. } => Some(session_id),
            Request::Authenticate { args } => Some(&args.session_id),
        }
    }

    pub fn validate(&self) -> Result<(), ErrorCode> {
        match self {
            Request::Hello {} => Ok(()),
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
            Request::AuthBegin {
                session_id,
                lease_id,
                auth_section_id,
            } => {
                check_id(session_id)?;
                check_id(lease_id)?;
                check_id(auth_section_id)
            }
            Request::Authenticate { args } => {
                check_id(&args.session_id)?;
                check_id(&args.lease_id)?;
                check_id(&args.auth_section_id)?;
                check_id(&args.credential_lease_id)?;
                for s in [&args.origin, &args.login_url, &args.password_selector]
                    .into_iter()
                    .chain(args.submit_selector.as_ref())
                    .chain(args.username_selector.as_ref())
                {
                    check_str(s)?;
                    if s.is_empty() {
                        return Err(ErrorCode::BadRequest);
                    }
                }
                if let Some(post) = &args.post_login {
                    for s in &post.read_origins {
                        check_str(s)?;
                    }
                }
                // ADR-0110 D2 の形式検証（https・login_url の origin・selector の文法）と、username
                // selector・post_login の形式（ADR 2026-10-09 credential username / post-login）。
                task_core::browser_wait::validate_trusted_login_full(
                    &args.login_url,
                    &args.origin,
                    &args.password_selector,
                    args.submit_selector.as_deref(),
                    args.username_selector.as_deref(),
                    args.post_login.as_ref(),
                )
                .map_err(|_| ErrorCode::BadRequest)
            }
        }
    }
}
