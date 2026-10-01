//! ADR-0109 D1〜D3: injection-only IPC（`injection.sock`）。
//!
//! 役割は `SO_PEERCRED` の peer から broker が決める（自己申告は読まない）。照合は D3 の順に行い、
//! 最初の失敗で止まる。lease 消費（step 5）より前の拒否は lease を消費せず provider を呼ばない。
//! provider が返した秘密は [`CdpSink`]（controller の CDP mux へ繋がる sink FD）にだけ 1 frame で書き、
//! 要求者への応答は receipt か固定の拒否コードだけにする。

use crate::{Broker, CredentialProvider, canonical_origin, valid_id};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use task_core::browser_isolation::{
    IsolationAttestation, IsolationViolation, RuntimeFacts, verify_isolation,
};
use zeroize::Zeroize;

pub use crate::injection::PeerRole;
use crate::injection::{RedisplayGuard, RedisplayGuardWire};

/// 要求 frame の上限（ADR-0109 D1）。
pub const MAX_REQUEST: usize = 16 * 1024;
/// 読み取り・sink 応答の期限。
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// 拒否コード（D3）。応答には `code()` の固定語彙だけを出す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectCode {
    InvalidRequest,
    UnsupportedVersion,
    PeerUidMismatch,
    WorkerNotAllowed,
    SessionNotLive,
    IsolationRequired,
    TargetMismatch,
    EmptyFrameChain,
    CrossOriginFrame,
    Redirected,
    RedisplayField,
    AuthSectionRequired,
    AuthSectionMismatch,
    SelectorMismatch,
    TrustedSelectorMissing,
    LeaseExpired,
    LeaseUsed,
    OtherSession,
    LeaseInvalid,
    AuditUnavailable,
    ProviderFailed,
    TargetChanged,
    SinkFailed,
}

impl InjectCode {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnsupportedVersion => "unsupported_version",
            Self::PeerUidMismatch => "peer_uid_mismatch",
            Self::WorkerNotAllowed => "injection_worker_not_allowed",
            Self::SessionNotLive => "session_not_live",
            Self::IsolationRequired => "isolation_required",
            Self::TargetMismatch => "target_mismatch",
            Self::EmptyFrameChain => "empty_frame_chain",
            Self::CrossOriginFrame => "cross_origin_frame",
            Self::Redirected => "redirected",
            Self::RedisplayField => "redisplay_field",
            Self::AuthSectionRequired => "auth_section_required",
            Self::AuthSectionMismatch => "auth_section_mismatch",
            Self::SelectorMismatch => "selector_mismatch",
            Self::TrustedSelectorMissing => "trusted_selector_missing",
            Self::LeaseExpired => "lease_expired",
            Self::LeaseUsed => "lease_used",
            Self::OtherSession => "other_session",
            Self::LeaseInvalid => "lease_invalid",
            Self::AuditUnavailable => "audit_unavailable",
            Self::ProviderFailed => "provider_failed",
            Self::TargetChanged => "target_changed",
            Self::SinkFailed => "sink_failed",
        }
    }
    fn from_lease(d: crate::LeaseDenied) -> Self {
        match d {
            crate::LeaseDenied::Expired => Self::LeaseExpired,
            crate::LeaseDenied::Used => Self::LeaseUsed,
            crate::LeaseDenied::OtherSession => Self::OtherSession,
            crate::LeaseDenied::Invalid => Self::LeaseInvalid,
            crate::LeaseDenied::Audit => Self::AuditUnavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Username,
    Password,
}

/// controller の injection client → broker（D1）。秘密・value・長さ・hash は持たない。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectionRequest {
    pub v: u32,
    pub request_id: String,
    pub session_id: String,
    pub cdp_target_id: String,
    /// flatten 済み CDP session（`Target.attachToTarget`）。mux が frame を解釈せずに
    /// 正しい target へ流せるよう、broker は frame の `sessionId` にそのまま入れる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdp_session_id: Option<String>,
    pub frame_id: String,
    pub loader_id: String,
    pub frame_chain: Vec<String>,
    pub redirect_chain: Vec<String>,
    pub selector: String,
    pub object_id: String,
    pub field: Field,
    pub input_type: String,
    pub auth_section_id: String,
    pub lease_id: String,
    pub cdp_command_id: u64,
}

/// 成功時の receipt。値・長さ・hash・selector・object id・DOM 観測を入れない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectionReceipt {
    pub lease_id: String,
    pub auth_section_id: String,
    pub session_id: String,
    pub cdp_target_id: String,
    pub frame_id: String,
    pub loader_id: String,
    pub field: Field,
    pub injected_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectionReply {
    pub v: u32,
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<InjectionReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// ADR-0111: 注入した値の再表示 guard（salt・digest・長さだけ）。receipt の外に置き、
    /// controller は保持するだけで agent・worker へは渡さない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redisplay_guard: Option<RedisplayGuardWire>,
}

impl InjectionReply {
    fn denied(request_id: &str, code: InjectCode) -> Self {
        Self {
            v: 1,
            request_id: request_id.into(),
            ok: false,
            receipt: None,
            code: Some(code.code().into()),
            redisplay_guard: None,
        }
    }
}

// ---- 稼働中 session の登録（D2。memory だけ・永続化しない）----

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSessionRegistration {
    pub session_id: String,
    pub controller_pid: u32,
    pub controller_start: u64,
    pub runtime_pid: u32,
    pub runtime_start: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSectionRegistration {
    pub session_id: String,
    pub auth_section_id: String,
    pub lease_id: String,
    pub exact_origin: String,
    pub cdp_target_id: String,
}

struct LiveEntry {
    session: LiveSessionRegistration,
    section: Option<AuthSectionRegistration>,
}

#[derive(Default)]
pub struct LiveRegistry {
    sessions: Mutex<HashMap<String, LiveEntry>>,
}

impl LiveRegistry {
    pub fn register(&self, s: LiveSessionRegistration) -> Result<(), InjectCode> {
        if !valid_id(&s.session_id) || s.controller_pid <= 1 || s.runtime_pid <= 1 {
            return Err(InjectCode::InvalidRequest);
        }
        let mut m = self
            .sessions
            .lock()
            .map_err(|_| InjectCode::InvalidRequest)?;
        if m.contains_key(&s.session_id) {
            return Err(InjectCode::InvalidRequest);
        }
        m.insert(
            s.session_id.clone(),
            LiveEntry {
                session: s,
                section: None,
            },
        );
        Ok(())
    }
    pub fn unregister(&self, session_id: &str) -> Result<(), InjectCode> {
        let mut m = self
            .sessions
            .lock()
            .map_err(|_| InjectCode::InvalidRequest)?;
        m.remove(session_id)
            .map(|_| ())
            .ok_or(InjectCode::SessionNotLive)
    }
    pub fn open_section(&self, a: AuthSectionRegistration) -> Result<(), InjectCode> {
        if !valid_id(&a.auth_section_id)
            || !valid_id(&a.lease_id)
            || !valid_id(&a.cdp_target_id)
            || canonical_origin(&a.exact_origin).ok().as_deref() != Some(a.exact_origin.as_str())
        {
            return Err(InjectCode::InvalidRequest);
        }
        let mut m = self
            .sessions
            .lock()
            .map_err(|_| InjectCode::InvalidRequest)?;
        let entry = m.get_mut(&a.session_id).ok_or(InjectCode::SessionNotLive)?;
        if entry.section.is_some() {
            return Err(InjectCode::InvalidRequest);
        }
        entry.section = Some(a);
        Ok(())
    }
    pub fn close_section(&self, session_id: &str, auth_section_id: &str) -> Result<(), InjectCode> {
        let mut m = self
            .sessions
            .lock()
            .map_err(|_| InjectCode::InvalidRequest)?;
        let entry = m.get_mut(session_id).ok_or(InjectCode::SessionNotLive)?;
        match &entry.section {
            Some(s) if s.auth_section_id == auth_section_id => {
                entry.section = None;
                Ok(())
            }
            _ => Err(InjectCode::AuthSectionRequired),
        }
    }
    fn snapshot(
        &self,
        session_id: &str,
    ) -> Option<(LiveSessionRegistration, Option<AuthSectionRegistration>)> {
        let m = self.sessions.lock().ok()?;
        m.get(session_id)
            .map(|e| (e.session.clone(), e.section.clone()))
    }
    fn controllers_and_runtimes(&self) -> Vec<(u32, u64, u32)> {
        self.sessions
            .lock()
            .map(|m| {
                m.values()
                    .map(|e| {
                        (
                            e.session.controller_pid,
                            e.session.controller_start,
                            e.session.runtime_pid,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ---- 隔離の admission（D2 / D6）----

/// 稼働中 session の隔離を broker 自身が確かめる方法。production は [`Admission::Attested`] だけ。
#[derive(Clone, Copy)]
pub enum Admission {
    /// `/proc/<runtime_pid>` から事実を採り直し、`verify_isolation` の attestation を要求する。
    Attested,
    /// 試験専用（ADR-0109 D6）: 同一 UID の fixture runtime を通す。attestation は作らない。
    #[cfg(feature = "same-uid-harness")]
    SameUidHarness,
    /// 試験専用: `SameUidHarness` の判定に、試験が与えた事実を使う（fake の事実は D5 の証拠にしない）。
    #[cfg(feature = "same-uid-harness")]
    SameUidHarnessFacts(fn(&str, i32) -> RuntimeFacts),
}

impl Admission {
    pub fn name(self) -> &'static str {
        match self {
            Self::Attested => "attested",
            #[cfg(feature = "same-uid-harness")]
            Self::SameUidHarness | Self::SameUidHarnessFacts(_) => "same_uid_harness",
        }
    }
    /// Broker が稼働中 runtime を再検査する。`Attested` は試験用の例外を通らない。
    pub fn admit(self, session_id: &str, runtime_pid: i32) -> Result<(), InjectCode> {
        let facts = || -> Result<RuntimeFacts, InjectCode> {
            if runtime_pid <= 1 {
                return Err(InjectCode::SessionNotLive);
            }
            let pgid = unsafe { libc::getpgid(runtime_pid) };
            if pgid <= 0 {
                return Err(InjectCode::SessionNotLive);
            }
            task_core::browser_isolation::collect_runtime_facts(session_id, runtime_pid, pgid)
                .map_err(|_| InjectCode::IsolationRequired)
        };
        match self {
            Self::Attested => admit_attested(&facts()?).map(|_| ()),
            #[cfg(feature = "same-uid-harness")]
            Self::SameUidHarness => same_uid_only(&facts()?),
            #[cfg(feature = "same-uid-harness")]
            Self::SameUidHarnessFacts(f) => same_uid_only(&f(session_id, runtime_pid)),
        }
    }
}

/// 本番 admission の純粋な判定。owner を含むすべての隔離条件が満たされたときだけ証明を返す。
pub fn admit_attested(facts: &RuntimeFacts) -> Result<IsolationAttestation, InjectCode> {
    verify_isolation(facts).map_err(|_| InjectCode::IsolationRequired)
}

#[cfg(feature = "same-uid-harness")]
fn same_uid_only(facts: &RuntimeFacts) -> Result<(), InjectCode> {
    match verify_isolation(facts) {
        Ok(_) => Ok(()),
        Err(v)
            if v == [IsolationViolation::SameUid]
                || v == [
                    IsolationViolation::SameUid,
                    IsolationViolation::UsernsOwnedByDaemon,
                ] =>
        {
            Ok(())
        }
        // Some CI workers run the entire nested user namespace as host root.
        // This exception exists only in the test feature; Attested is unchanged.
        Err(v)
            if unsafe { libc::geteuid() } == 0
                && (v == [IsolationViolation::RootUid, IsolationViolation::SameUid]
                    || v == [
                        IsolationViolation::RootUid,
                        IsolationViolation::SameUid,
                        IsolationViolation::UsernsOwnedByDaemon,
                    ]) =>
        {
            Ok(())
        }
        Err(_) => Err(InjectCode::IsolationRequired),
    }
}
#[cfg(not(feature = "same-uid-harness"))]
const _: Option<IsolationViolation> = None;

// ---- sink（D4）----

/// sink の失敗。理由は持たない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkFailed;

/// controller の CDP mux へ 1 frame を渡し、予約した command id の応答を受ける口。
/// broker は frame を書いた後で自分の buffer を zeroize する。
pub trait CdpSink {
    fn exchange(&mut self, frame: &[u8]) -> Result<Vec<u8>, SinkFailed>;
}

/// `SOCK_SEQPACKET` の sink FD（要求に `SCM_RIGHTS` で付く）。
pub struct SeqpacketSink {
    fd: OwnedFd,
}

impl SeqpacketSink {
    /// FD の種別が `SOCK_SEQPACKET` の unix socket でなければ拒否する。
    pub fn new(fd: OwnedFd) -> Result<Self, InjectCode> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd.as_raw_fd(), st.as_mut_ptr()) } != 0 {
            return Err(InjectCode::InvalidRequest);
        }
        let st = unsafe { st.assume_init() };
        let mut ty: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_TYPE,
                (&raw mut ty).cast(),
                &mut len,
            )
        };
        if st.st_mode & libc::S_IFMT != libc::S_IFSOCK || rc != 0 || ty != libc::SOCK_SEQPACKET {
            return Err(InjectCode::InvalidRequest);
        }
        let tv = libc::timeval {
            tv_sec: IO_TIMEOUT.as_secs() as libc::time_t,
            tv_usec: 0,
        };
        for opt in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            let rc = unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    opt,
                    (&raw const tv).cast(),
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                )
            };
            if rc != 0 {
                return Err(InjectCode::InvalidRequest);
            }
        }
        Ok(Self { fd })
    }
}

impl CdpSink for SeqpacketSink {
    fn exchange(&mut self, frame: &[u8]) -> Result<Vec<u8>, SinkFailed> {
        let n = unsafe {
            libc::send(
                self.fd.as_raw_fd(),
                frame.as_ptr().cast(),
                frame.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 || n as usize != frame.len() {
            return Err(SinkFailed);
        }
        let mut buf = vec![0u8; 64 * 1024];
        let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n <= 0 {
            return Err(SinkFailed);
        }
        buf.truncate(n as usize);
        Ok(buf)
    }
}

/// 検査と代入を 1 回の同期実行で行う固定の関数（D4-2）。戻り値は固定語彙だけ。
pub const INJECT_FUNCTION: &str = "function(expected,depth,field,value){try{\
if(!this.isConnected)return 'target_changed';\
var w=this.ownerDocument&&this.ownerDocument.defaultView;\
if(!w||!(this instanceof w.HTMLInputElement)||w.location.origin!==expected)return 'target_changed';\
var d=0,f=w;while(f!==f.top){f=f.parent;if(f.location.origin!==expected)return 'target_changed';d++;}\
if(d!==depth)return 'target_changed';\
var t=String(this.type).toLowerCase();\
if(field==='password'?t!=='password':(t!=='text'&&t!=='email'))return 'target_changed';\
Object.getOwnPropertyDescriptor(w.HTMLInputElement.prototype,'value').set.call(this,value);\
this.dispatchEvent(new w.Event('input',{bubbles:true}));\
this.dispatchEvent(new w.Event('change',{bubbles:true}));\
return 'ok';}catch(e){return 'target_changed';}}";

fn cdp_frame(req: &InjectionRequest, origin: &str, value: &str) -> Result<Vec<u8>, InjectCode> {
    let field = match req.field {
        Field::Username => "username",
        Field::Password => "password",
    };
    let mut frame = serde_json::json!({
        "id": req.cdp_command_id,
        "method": "Runtime.callFunctionOn",
        "params": {
            "objectId": req.object_id,
            "functionDeclaration": INJECT_FUNCTION,
            "arguments": [
                {"value": origin},
                {"value": req.frame_chain.len().saturating_sub(1)},
                {"value": field},
                {"value": value},
            ],
            "returnByValue": true,
            "silent": true,
        },
    });
    if let (Some(sid), Some(o)) = (&req.cdp_session_id, frame.as_object_mut()) {
        o.insert("sessionId".into(), sid.clone().into());
    }
    let bytes = serde_json::to_vec(&frame)
        .map(|mut bytes| {
            // Chrome's remote-debugging-pipe uses NUL-delimited JSON messages.
            bytes.push(0);
            bytes
        })
        .map_err(|_| InjectCode::SinkFailed);
    // serde_json::Value の中の秘密の写しを消す。
    if let Some(args) = frame
        .pointer_mut("/params/arguments/3/value")
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            _ => None,
        })
    {
        args.zeroize();
    }
    bytes
}

fn sink_verdict(command_id: u64, reply: &[u8]) -> Result<(), InjectCode> {
    let v: serde_json::Value = serde_json::from_slice(reply).map_err(|_| InjectCode::SinkFailed)?;
    if v.get("id").and_then(|i| i.as_u64()) != Some(command_id) {
        return Err(InjectCode::SinkFailed);
    }
    if v.get("error").is_some() {
        return Err(InjectCode::TargetChanged);
    }
    match v.pointer("/result/result/value").and_then(|x| x.as_str()) {
        Some("ok") => Ok(()),
        Some(_) => Err(InjectCode::TargetChanged),
        None => Err(InjectCode::SinkFailed),
    }
}

// ---- 照合（D3）----

/// `SO_PEERCRED` で得た peer。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCred {
    pub uid: u32,
    pub pid: u32,
}

pub fn process_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(") ")?.1.split_whitespace();
    fields.nth(19)?.parse().ok()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn short_id(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_graphic())
}

pub struct InjectionService {
    broker: Arc<Broker>,
    registry: Arc<LiveRegistry>,
    admission: Admission,
    broker_uid: u32,
}

impl InjectionService {
    pub fn new(broker: Arc<Broker>, registry: Arc<LiveRegistry>, admission: Admission) -> Self {
        Self {
            broker,
            registry,
            admission,
            broker_uid: unsafe { libc::geteuid() },
        }
    }

    /// peer の役割（D2）。`session` はその要求の session の登録。
    pub fn role(&self, peer: PeerCred, session: Option<&LiveSessionRegistration>) -> PeerRole {
        let start = process_start(peer.pid);
        if let Some(s) = session
            && s.controller_pid == peer.pid
            && start == Some(s.controller_start)
        {
            return PeerRole::Injector;
        }
        let pgid = unsafe { libc::getpgid(peer.pid as i32) };
        let in_runtime = pgid > 1
            && self
                .registry
                .controllers_and_runtimes()
                .iter()
                .any(|(_, _, rt)| unsafe { libc::getpgid(*rt as i32) } == pgid);
        if in_runtime {
            PeerRole::Agent
        } else {
            PeerRole::Worker
        }
    }

    fn is_any_controller(&self, peer: PeerCred) -> bool {
        let start = process_start(peer.pid);
        self.registry
            .controllers_and_runtimes()
            .iter()
            .any(|(pid, st, _)| *pid == peer.pid && start == Some(*st))
    }

    /// 1 要求を照合して応答を作る。`sink` は step 6 でだけ使う。
    pub fn handle(&self, peer: PeerCred, body: &[u8], sink: &mut dyn CdpSink) -> InjectionReply {
        let req = match serde_json::from_slice::<InjectionRequest>(body) {
            Ok(r) => r,
            Err(_) => {
                self.audit(peer, None, InjectCode::InvalidRequest.code(), None);
                return InjectionReply::denied("", InjectCode::InvalidRequest);
            }
        };
        let result = self.check_and_inject(peer, &req, sink);
        let code = match &result {
            Ok(_) => "injected",
            Err(c) => c.code(),
        };
        let role = self
            .registry
            .snapshot(&req.session_id)
            .map(|(s, _)| self.role(peer, Some(&s)))
            .unwrap_or_else(|| self.role(peer, None));
        self.audit(peer, Some(&req), code, Some(role));
        match result {
            Ok((receipt, guard)) => InjectionReply {
                v: 1,
                request_id: req.request_id,
                ok: true,
                receipt: Some(receipt),
                code: None,
                redisplay_guard: Some(guard),
            },
            Err(c) => InjectionReply::denied(&req.request_id, c),
        }
    }

    fn check_and_inject(
        &self,
        peer: PeerCred,
        req: &InjectionRequest,
        sink: &mut dyn CdpSink,
    ) -> Result<(InjectionReceipt, RedisplayGuardWire), InjectCode> {
        // 0. 形
        if req.v != 1 {
            return Err(InjectCode::UnsupportedVersion);
        }
        if !valid_id(&req.request_id)
            || !valid_id(&req.session_id)
            || !short_id(&req.cdp_target_id, 128)
            || !short_id(&req.frame_id, 128)
            || !short_id(&req.loader_id, 128)
            || !short_id(&req.object_id, 256)
            || req
                .cdp_session_id
                .as_deref()
                .is_some_and(|s| !short_id(s, 128))
            || req.selector.is_empty()
            || req.selector.len() > 512
            || !valid_id(&req.auth_section_id)
            || !valid_id(&req.lease_id)
            || req.input_type.len() > 32
            || req.frame_chain.len() > 16
            || req.redirect_chain.len() > 32
        {
            return Err(InjectCode::InvalidRequest);
        }
        // 1. 役割
        if peer.uid != self.broker_uid {
            return Err(InjectCode::PeerUidMismatch);
        }
        let Some((session, section)) = self.registry.snapshot(&req.session_id) else {
            // その session の controller は存在しない。別 session の controller なら session 側の
            // 失敗として、それ以外（worker・agent）は役割の失敗として返す。
            return Err(if self.is_any_controller(peer) {
                InjectCode::SessionNotLive
            } else {
                InjectCode::WorkerNotAllowed
            });
        };
        if self.role(peer, Some(&session)) != PeerRole::Injector {
            return Err(InjectCode::WorkerNotAllowed);
        }
        // 2. 稼働中の隔離 session（登録を信じず broker が確かめる）
        if process_start(session.runtime_pid) != Some(session.runtime_start) {
            return Err(InjectCode::SessionNotLive);
        }
        self.admission
            .admit(&session.session_id, session.runtime_pid as i32)?;
        // 3. CDP 対象と origin（区間が無ければ対象の照合先も無い）
        let Some(section) = section else {
            return Err(InjectCode::AuthSectionRequired);
        };
        let origin = section.exact_origin.as_str();
        if req.cdp_target_id != section.cdp_target_id {
            return Err(InjectCode::TargetMismatch);
        }
        if req.frame_chain.is_empty() {
            return Err(InjectCode::EmptyFrameChain);
        }
        if req.frame_chain.iter().any(|o| o != origin) {
            return Err(InjectCode::CrossOriginFrame);
        }
        if req.redirect_chain.iter().any(|o| o != origin) {
            return Err(InjectCode::Redirected);
        }
        let ok_type = match req.field {
            Field::Password => req.input_type == "password",
            Field::Username => matches!(req.input_type.as_str(), "text" | "email"),
        };
        if !ok_type {
            return Err(InjectCode::RedisplayField);
        }
        // 4. auth_section
        if section.auth_section_id != req.auth_section_id || section.lease_id != req.lease_id {
            return Err(InjectCode::AuthSectionMismatch);
        }
        // 4b. trusted selector（ADR-0110 D2 照合 3）: lease が保持する管理者 policy の selector だけが出所。
        // 要求の selector は byte 一致でなければ拒否し、lease は消費しない。policy に selector が無ければ注入しない。
        // lease が無い場合は順 5 が `lease_invalid` を返す。
        if let Some(pinned) = self.broker.lease_trusted_selector(&req.lease_id) {
            let pinned = pinned.ok_or(InjectCode::TrustedSelectorMissing)?;
            // username 欄の trusted selector は ADR-0110 の範囲外（未解決）。password 欄だけ照合する。
            if req.field == Field::Password && pinned.as_bytes() != req.selector.as_bytes() {
                return Err(InjectCode::SelectorMismatch);
            }
        }
        // 5. lease（ここで消費）
        let (reference, context, provider) = self
            .broker
            .consume_for_injection(&req.lease_id, &req.session_id, origin)
            .map_err(InjectCode::from_lease)?;
        // 6. provider → sink
        let secret = provider_resolve(provider.as_ref(), &reference, &context)?;
        let value = match req.field {
            Field::Username => &secret.username,
            Field::Password => &secret.password,
        };
        let guard = RedisplayGuard::new(value).to_wire();
        let mut frame = cdp_frame(req, origin, value)?;
        drop(secret);
        let reply = sink.exchange(&frame);
        frame.zeroize();
        let mut reply = reply.map_err(|_| InjectCode::SinkFailed)?;
        let verdict = sink_verdict(req.cdp_command_id, &reply);
        reply.zeroize();
        verdict?;
        Ok((
            InjectionReceipt {
                lease_id: req.lease_id.clone(),
                auth_section_id: req.auth_section_id.clone(),
                session_id: req.session_id.clone(),
                cdp_target_id: req.cdp_target_id.clone(),
                frame_id: req.frame_id.clone(),
                loader_id: req.loader_id.clone(),
                field: req.field,
                injected_at: now(),
            },
            guard,
        ))
    }

    fn audit(
        &self,
        peer: PeerCred,
        req: Option<&InjectionRequest>,
        code: &str,
        role: Option<PeerRole>,
    ) {
        let mut record = serde_json::json!({
            "action": "inject",
            "decision_code": code,
            "peer_pid": peer.pid,
            "peer_uid": peer.uid,
            "admission": self.admission.name(),
        });
        if let (Some(o), Some(role)) = (record.as_object_mut(), role) {
            o.insert("peer_role".into(), format!("{role:?}").into());
        }
        if let (Some(o), Some(r)) = (record.as_object_mut(), req) {
            for (k, v) in [
                ("request_id", &r.request_id),
                ("session_id", &r.session_id),
                ("cdp_target_id", &r.cdp_target_id),
                ("frame_id", &r.frame_id),
                ("loader_id", &r.loader_id),
                ("selector", &r.selector),
                ("auth_section_id", &r.auth_section_id),
                ("lease_id", &r.lease_id),
            ] {
                o.insert(k.into(), v.clone().into());
            }
            o.insert("frame_chain".into(), r.frame_chain.clone().into());
            o.insert("redirect_chain".into(), r.redirect_chain.clone().into());
        }
        // audit の失敗は応答を変えない（step 5 の消費は Broker 側で durable に記録済み）。
        let _ = self.broker.audit_injection(&record);
    }
}

fn provider_resolve(
    provider: &dyn CredentialProvider,
    reference: &crate::CredentialRef,
    context: &crate::AuthorizedLeaseContext,
) -> Result<crate::SecretEnvelope, InjectCode> {
    provider
        .resolve(reference, context)
        .map_err(|_| InjectCode::ProviderFailed)
}

// ---- socket（D1）----

/// 1 接続を読む: `u32` BE 長 + JSON、`SCM_RIGHTS` の FD を集める。
pub(crate) fn read_request(fd: i32) -> Result<(Vec<u8>, Vec<OwnedFd>), InjectCode> {
    let mut data = Vec::new();
    let mut fds = Vec::new();
    let mut want: Option<usize> = None;
    loop {
        if let Some(n) = want
            && data.len() >= 4 + n
        {
            break;
        }
        let mut buf = [0u8; 4096];
        let mut cmsg = [0u64; 16];
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr().cast(),
            iov_len: buf.len(),
        };
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(&cmsg) as _;
        let n = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC) };
        // 受け取った FD は、失敗の場合も含めて必ず OwnedFd にして閉じる。
        unsafe {
            let mut c = libc::CMSG_FIRSTHDR(&msg);
            while !c.is_null() {
                if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                    let payload = (*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                    let p = libc::CMSG_DATA(c).cast::<libc::c_int>();
                    for i in 0..payload / std::mem::size_of::<libc::c_int>() {
                        let raw = std::ptr::read_unaligned(p.add(i));
                        if raw >= 0 {
                            fds.push(OwnedFd::from_raw_fd(raw));
                        }
                    }
                }
                c = libc::CMSG_NXTHDR(&msg, c);
            }
        }
        if n <= 0 || msg.msg_flags & libc::MSG_CTRUNC != 0 {
            data.zeroize();
            return Err(InjectCode::InvalidRequest);
        }
        data.extend_from_slice(&buf[..n as usize]);
        buf.zeroize();
        if want.is_none() && data.len() >= 4 {
            let n = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
            if n == 0 || n > MAX_REQUEST {
                data.zeroize();
                return Err(InjectCode::InvalidRequest);
            }
            want = Some(n);
        }
        if data.len() > 4 + MAX_REQUEST {
            data.zeroize();
            return Err(InjectCode::InvalidRequest);
        }
    }
    let n = want.unwrap_or_default();
    if data.len() != 4 + n {
        data.zeroize();
        return Err(InjectCode::InvalidRequest);
    }
    let body = data[4..].to_vec();
    data.zeroize();
    Ok((body, fds))
}

pub(crate) fn write_reply(stream: &mut std::os::unix::net::UnixStream, reply: &InjectionReply) {
    use std::io::Write;
    if let Ok(body) = serde_json::to_vec(reply) {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&body);
        let _ = stream.write_all(&out);
    }
}

/// controller の injection client（と試験）が使う呼び出し。応答の生 byte 列（長さ prefix 除く）を返す。
pub fn call_raw(
    socket: &std::path::Path,
    request: &[u8],
    sink_fds: &[i32],
) -> Result<Vec<u8>, crate::Error> {
    use crate::Error;
    use std::io::Read;
    let stream = std::os::unix::net::UnixStream::connect(socket).map_err(|_| Error::Io)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT + Duration::from_secs(5)))
        .map_err(|_| Error::Io)?;
    let mut out = (request.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(request);
    let mut cmsg = [0u64; 16];
    let mut iov = libc::iovec {
        iov_base: out.as_mut_ptr().cast(),
        iov_len: out.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if !sink_fds.is_empty() {
        let bytes = std::mem::size_of_val(sink_fds);
        let space = unsafe { libc::CMSG_SPACE(bytes as u32) } as usize;
        if space > std::mem::size_of_val(&cmsg) {
            return Err(Error::Invalid);
        }
        msg.msg_control = cmsg.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&msg);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(bytes as u32) as _;
            std::ptr::copy_nonoverlapping(
                sink_fds.as_ptr(),
                libc::CMSG_DATA(c).cast::<libc::c_int>(),
                sink_fds.len(),
            );
        }
    }
    let n = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
    if n < 0 || n as usize != out.len() {
        return Err(Error::Io);
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut reply = Vec::new();
    (&stream)
        .take(4 + 64 * 1024)
        .read_to_end(&mut reply)
        .map_err(|_| Error::Io)?;
    if reply.len() < 4 {
        return Err(Error::Io);
    }
    let n = u32::from_be_bytes([reply[0], reply[1], reply[2], reply[3]]) as usize;
    if reply.len() != 4 + n {
        return Err(Error::Invalid);
    }
    Ok(reply[4..].to_vec())
}

/// [`call_raw`] の応答を読んだもの。
pub fn call(
    socket: &std::path::Path,
    request: &InjectionRequest,
    sink_fd: i32,
) -> Result<InjectionReply, crate::Error> {
    let body = serde_json::to_vec(request).map_err(|_| crate::Error::Invalid)?;
    let raw = call_raw(socket, &body, &[sink_fd])?;
    serde_json::from_slice(&raw).map_err(|_| crate::Error::Invalid)
}

pub fn injection_socket(runtime: &std::path::Path) -> std::path::PathBuf {
    runtime.join("celeris-credentiald/injection.sock")
}
