//! ADR-0109 D4: controller owned CDP pipe and one way broker sink.
//!
//! The controller never receives a credential in an IPC response. The broker
//! writes one CDP command to a private seqpacket socket; its bytes are forwarded
//! to the browser without decoding or logging them. Only the CDP reply goes back
//! to the broker. The caller gets a receipt with a fixed, non-secret shape.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use celeris_credentiald::injection::RedisplayGuard;
use celeris_credentiald::injection_ipc::{
    AuthSectionRegistration, Field, InjectionReply, InjectionRequest as WireRequest,
    LiveSessionRegistration, PAIR_REQUEST_VERSION, UsernameTarget,
};
use celeris_credentiald::ipc;
use serde_json::{Value, json};
use url::Url;
use zeroize::Zeroize;

use crate::browser_relay;

/// The trusted login's submit (ADR-0110 D2 `submit_selector`): a submit button / input is the
/// form's submitter, so its `name` / `value` go into the POST (Shibboleth / Spring Web Flow needs
/// `_eventId_proceed`, without it the IdP shows the login form again). Anything that is not a
/// submitter (requestSubmit throws) is clicked instead; an element outside a form is clicked.
/// Returns the fixed `'ok'` / `'missing'` only.
pub fn login_submit_expression(selector: &str) -> Result<String, serde_json::Error> {
    Ok(format!(
        "(()=>{{let e=document.querySelector({});if(!e)return 'missing';\
if(e.form){{try{{e.form.requestSubmit(e);}}catch(_){{e.click();}}}}else e.click();return 'ok'}})()",
        serde_json::to_string(selector)?
    ))
}

/// Bound on one broker sink frame (ADR-0109 D4).
const MAX_FRAME: usize = 65536;
/// Bound on one CDP message read from the browser pipe. Screenshots and large DOM/AX trees are
/// single messages well above the sink bound (ADR 2026-10-09 credential username / post-login D2-6).
const MAX_CDP_MESSAGE: usize = 64 << 20;
const TIMEOUT: Duration = Duration::from_secs(5);
/// Shibboleth IdP attribute-release consent form markers (fixed names; the page text is not read).
const CONSENT_MARKER_JS: &str = "!!document.querySelector('[name=\"_shib_idp_consentIds\"],[name=\"_shib_idp_consentOptions\"],[name=\"_eventId_AttributeReleaseRejected\"]')";
/// The same markers as a JS string literal (for the press and the diagnostics).
const CONSENT_SELECTOR_JS: &str = "'[name=\"_shib_idp_consentIds\"],[name=\"_shib_idp_consentOptions\"],[name=\"_eventId_AttributeReleaseRejected\"]'";
/// Isolated world for the controller's own post-login checks (never visible to the agent).
const CHECK_WORLD: &str = "celeris-post-login";
/// Counts the *live* `input[type=password]` in the document, its open shadow roots and same-origin
/// frames: rendered (a layout box, not `visibility: hidden`) or holding a value. A hidden, empty
/// password input (a collapsed login widget on an otherwise ordinary page) can neither show a
/// password on screen nor hold one (ADR 2026-10-09 credential username / post-login 付記
/// 2026-10-10c). Runs in the controller's isolated world, so page script cannot replace the DOM
/// accessors.
const PASSWORD_COUNT_JS: &str = "(()=>{let n=0;const live=(e)=>{if(String(e.value||'')!=='')return true;\
const r=e.getClientRects();if(!r||r.length===0)return false;\
const v=e.ownerDocument&&e.ownerDocument.defaultView;const s=v?v.getComputedStyle(e):null;\
if(s&&(s.visibility==='hidden'||s.visibility==='collapse'||s.display==='none'))return false;\
for(const q of r){if(q.width>0||q.height>0)return true;}return false;};\
const walk=(root,depth)=>{if(depth>8)return;\
for(const e of root.querySelectorAll('*')){const tag=String(e.localName||'').toLowerCase();\
if(tag==='input'&&String(e.type||'').toLowerCase()==='password'&&live(e))n++;\
if(e.shadowRoot)walk(e.shadowRoot,depth+1);\
if(tag==='iframe'||tag==='frame'){let d=null;try{d=e.contentDocument;}catch(_){}if(d)walk(d,depth+1);}}};\
walk(document,0);return n;})()";
/// Bound on events no agent connection has taken yet (oldest dropped first).
const MAX_QUEUED_EVENTS: usize = 4096;
/// Recent relay refusals kept for the launcher's failure log (付記 2026-10-10f).
pub const MAX_AGENT_DENIALS: usize = 8;

/// A CDP method name as it may appear in a log: `Domain.method` of ASCII letters, else `other`.
pub fn loggable_method(method: &str) -> String {
    let shaped = method.split_once('.').is_some_and(|(d, m)| {
        (1..=32).contains(&d.len())
            && (1..=64).contains(&m.len())
            && d.bytes().all(|b| b.is_ascii_alphabetic())
            && m.bytes().all(|b| b.is_ascii_alphabetic())
    });
    if shaped {
        method.to_owned()
    } else {
        "other".to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionError {
    AuthSectionRequired,
    TargetMismatch,
    CrossOriginFrame,
    Redirected,
    RedisplayField,
    TargetChanged,
    SinkFailed,
    /// ADR-0111: an agent observation re-displayed an injected value; it was discarded.
    RedisplayDetected,
    /// ADR 2026-10-09 credential username / post-login D2-3: after login, the observed top
    /// document is not on a `read_origins` origin (the IdP, another allowed domain, no target).
    ObservationOriginDenied,
    /// Same D2-3: the observed document (or a same-origin frame) has a password input.
    PasswordFieldPresent,
    /// Same D2-2: the auth section close conditions did not hold in time; observation stays stopped.
    PostLoginUnconfirmed,
    BrokerRejected(&'static str),
}

impl InjectionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::AuthSectionRequired => "auth_section_required",
            Self::TargetMismatch => "target_mismatch",
            Self::CrossOriginFrame => "cross_origin_frame",
            Self::Redirected => "redirected",
            Self::RedisplayField => "redisplay_field",
            Self::TargetChanged => "target_changed",
            Self::SinkFailed => "sink_failed",
            Self::RedisplayDetected => "redisplay_detected",
            Self::ObservationOriginDenied => "observation_origin_denied",
            Self::PasswordFieldPresent => "password_field_present",
            Self::PostLoginUnconfirmed => "post_login_unconfirmed",
            Self::BrokerRejected(code) => code,
        }
    }
}

/// Controller-side target intent. This is resolved against live CDP state;
/// the serialized IPC request uses credentiald's shared `WireRequest` type.
#[derive(Debug, Clone)]
pub struct InjectionRequest {
    pub request_id: String,
    pub session_id: String,
    pub cdp_target_id: String,
    pub frame_id: String,
    pub loader_id: String,
    pub exact_origin: String,
    pub redirect_chain: Vec<String>,
    pub selector: String,
    pub field: String,
    pub auth_section_id: String,
    pub lease_id: String,
    /// ADR 2026-10-09 credential username / post-login D1-4: the administrator's username selector,
    /// resolved on the same top document as the password field (only with `field = password`).
    pub username_selector: Option<String>,
}

/// What the login tab shows now (ADR 2026-10-09 credential username / post-login D2-2, 付記
/// 2026-10-10). Only `Ready` closes the auth section; every other state keeps observation stopped.
/// Nothing about the page crosses besides this fixed state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostLoginProbe {
    /// Left the injected document, top on a `read_origins` origin, no password input.
    Ready,
    /// Still the injected login document.
    LoginDocument,
    /// No http(s) top document (blank or in between).
    NoDocument,
    /// An IdP page without a password field (interstitial, SAML auto-POST, error).
    IdpPage,
    /// An IdP attribute-release consent page (needs a human decision).
    IdpConsent,
    /// The IdP shows a login form again (the login was refused).
    IdpLoginForm,
    /// A `read_origins` page that has a password input.
    ReadOriginPasswordField,
    /// Any other origin.
    OtherOrigin,
    /// The controller could not read the tab (a transient CDP failure).
    CheckFailed,
}

/// Why the auth section did not close (fixed codes; progress and the launcher answer carry only
/// these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostLoginHeld {
    ConsentRequired,
    IdpLoginForm,
    IdpTimeout,
    PasswordField,
    OtherOrigin,
    LoginDocument,
    NoDocument,
    CheckFailed,
    /// The conditions held but the store / policy transition failed (daemon side).
    ResumeFailed,
}

impl PostLoginHeld {
    pub fn code(self) -> &'static str {
        match self {
            Self::ConsentRequired => "post_login_consent_required",
            Self::IdpLoginForm => "post_login_idp_login_form",
            Self::IdpTimeout => "post_login_idp_timeout",
            Self::PasswordField => "post_login_password_field",
            Self::OtherOrigin => "post_login_other_origin",
            Self::LoginDocument => "post_login_login_document",
            Self::NoDocument => "post_login_no_document",
            Self::CheckFailed => "post_login_check_failed",
            Self::ResumeFailed => "post_login_resume_failed",
        }
    }

    /// The top document's origin category when the wait ended (`idp` / `read_origin` / `other` /
    /// `none`); never the URL.
    pub fn top_origin(self) -> &'static str {
        match self {
            Self::ConsentRequired | Self::IdpLoginForm | Self::IdpTimeout | Self::LoginDocument => {
                "idp"
            }
            Self::PasswordField => "read_origin",
            Self::OtherOrigin => "other",
            Self::NoDocument | Self::CheckFailed | Self::ResumeFailed => "none",
        }
    }

    fn from_probe(probe: PostLoginProbe) -> Self {
        match probe {
            PostLoginProbe::IdpConsent => Self::ConsentRequired,
            PostLoginProbe::IdpLoginForm => Self::IdpLoginForm,
            PostLoginProbe::IdpPage => Self::IdpTimeout,
            PostLoginProbe::ReadOriginPasswordField => Self::PasswordField,
            PostLoginProbe::OtherOrigin => Self::OtherOrigin,
            PostLoginProbe::LoginDocument => Self::LoginDocument,
            PostLoginProbe::NoDocument => Self::NoDocument,
            PostLoginProbe::CheckFailed | PostLoginProbe::Ready => Self::CheckFailed,
        }
    }
}

/// One control of an IdP consent form, for the operator to choose the fixed consent selector
/// (ADR 2026-10-09 credential username / post-login 付記 2026-10-10b-4). Only the `name`, the kind
/// (`submit` / `button` / `radio`) and the `value` attribute — never labels, text or user data.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentControl {
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// Bounds of the consent diagnostics.
pub const CONSENT_CONTROLS_MAX: usize = 12;
const CONSENT_ATTR_MAX: usize = 64;
const CONSENT_LINE_MAX: usize = 512;

/// Keep only well-formed controls: kind `submit` / `button` / `radio`, name 1..=64 printable ASCII
/// without spaces, value printable ASCII up to 64 (otherwise dropped), at most 12. Applied where the
/// controls are read and again where the daemon receives them from the launcher.
pub fn sanitize_consent_controls(raw: Vec<ConsentControl>) -> Vec<ConsentControl> {
    let name_ok = |n: &str| {
        !n.is_empty() && n.len() <= CONSENT_ATTR_MAX && n.bytes().all(|b| (0x21..0x7f).contains(&b))
    };
    let value_ok =
        |v: &str| v.len() <= CONSENT_ATTR_MAX && v.bytes().all(|b| (0x20..0x7f).contains(&b));
    raw.into_iter()
        .filter(|c| matches!(c.kind.as_str(), "submit" | "button" | "radio") && name_ok(&c.name))
        .map(|c| ConsentControl {
            value: c.value.filter(|v| !v.is_empty() && value_ok(v)),
            ..c
        })
        .take(CONSENT_CONTROLS_MAX)
        .collect()
}

/// `name=value(kind)` / `name(kind)` joined by `,`, at most 512 characters.
pub fn format_consent_controls(controls: &[ConsentControl]) -> String {
    let mut line = controls
        .iter()
        .map(|c| match &c.value {
            Some(v) => format!("{}={}({})", c.name, v, c.kind),
            None => format!("{}({})", c.name, c.kind),
        })
        .collect::<Vec<_>>()
        .join(",");
    // Everything is ASCII after sanitizing; truncate on a char boundary anyway.
    while line.len() > CONSENT_LINE_MAX {
        line.pop();
    }
    line
}

/// Why observation stayed stopped, with the consent diagnostics when the wait ended on a consent
/// page (付記 2026-10-10b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostLoginHeldInfo {
    pub reason: PostLoginHeld,
    /// The controller pressed the configured consent button once in this login.
    pub consent_pressed: bool,
    pub consent_controls: Vec<ConsentControl>,
}

impl From<PostLoginHeld> for PostLoginHeldInfo {
    fn from(reason: PostLoginHeld) -> Self {
        Self {
            reason,
            consent_pressed: false,
            consent_controls: Vec::new(),
        }
    }
}

/// How long the post-login wait may last in all (several IdP / SAML auto-POST hops).
pub const POST_LOGIN_WAIT: Duration = Duration::from_secs(60);
/// A page that needs a human (consent) or shows the login form again ends the wait once it has
/// stayed for this long.
pub const POST_LOGIN_STABLE: Duration = Duration::from_secs(3);

/// The bounded wait over [`PostLoginProbe`]s: `Ready` ends it at once; a consent page or a
/// repeated login form that stays for [`POST_LOGIN_STABLE`] ends it early with its reason; every
/// other state (interstitials, SAML auto-POST hops, redirects) is waited through until `deadline`,
/// when the last state gives the reason.
#[derive(Debug)]
pub struct PostLoginWaiter {
    deadline: std::time::Instant,
    stable: Duration,
    last: Option<(PostLoginProbe, std::time::Instant)>,
}

impl PostLoginWaiter {
    /// Forget how long the current state has lasted (after the controller pressed consent).
    pub fn restart_stability(&mut self) {
        self.last = None;
    }

    pub fn new(now: std::time::Instant, timeout: Duration, stable: Duration) -> Self {
        Self {
            deadline: now + timeout,
            stable,
            last: None,
        }
    }

    /// `None` = keep waiting; `Some(Ok)` = close the section; `Some(Err)` = stay stopped.
    pub fn observe(
        &mut self,
        probe: PostLoginProbe,
        now: std::time::Instant,
    ) -> Option<Result<(), PostLoginHeld>> {
        if probe == PostLoginProbe::Ready {
            return Some(Ok(()));
        }
        let since = match self.last {
            Some((p, since)) if p == probe => since,
            _ => now,
        };
        self.last = Some((probe, since));
        let terminal = matches!(
            probe,
            PostLoginProbe::IdpConsent | PostLoginProbe::IdpLoginForm
        );
        if (terminal && now.saturating_duration_since(since) >= self.stable) || now >= self.deadline
        {
            return Some(Err(PostLoginHeld::from_probe(probe)));
        }
        None
    }
}

/// A completed broker call returns only a receipt. The FD carries the CDP
/// command in the other direction and is never embedded in this response.
pub trait PendingInjection: Send {
    fn finish(self: Box<Self>) -> Result<Value, InjectionError>;
}

/// IPC transport is supplied by credentiald integration. A fake can use the
/// same sink protocol without opening a production broker socket.
pub trait BrokerClient {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError>;
}

/// The production injection-only IPC client. It never opens the retired
/// resolve endpoint; the path must be the broker's injection.sock.
pub struct UnixInjectionClient {
    socket: PathBuf,
}

impl UnixInjectionClient {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    fn control(&self, request: Value) -> Result<(), InjectionError> {
        self.check_socket()?;
        let path = self.socket.with_file_name("control.sock");
        let bytes = serde_json::to_vec(&request).map_err(|_| InjectionError::SinkFailed)?;
        let reply = ipc::call(&path, &bytes).map_err(|_| InjectionError::SinkFailed)?;
        if reply.success {
            Ok(())
        } else {
            let code = json!({"code":reply.code});
            Err(broker_denial(&code))
        }
    }

    fn check_socket(&self) -> Result<(), InjectionError> {
        if self
            .socket
            .file_name()
            .is_some_and(|name| name == "injection.sock")
        {
            Ok(())
        } else {
            Err(InjectionError::SinkFailed)
        }
    }

    pub fn register_live_session(
        &self,
        registration: LiveSessionRegistration,
    ) -> Result<(), InjectionError> {
        self.control(json!({"op":"register_live_session","session_id":registration.session_id,
            "controller_pid":registration.controller_pid,"controller_start":registration.controller_start,
            "runtime_pid":registration.runtime_pid,"runtime_start":registration.runtime_start}))
    }

    /// Bind the launcher proof to the live broker session using the credentiald control wire shape.
    /// The proof contains process identity only; no credential material crosses this call.
    pub fn attach_launcher_proof(
        &self,
        registration: celeris_credentiald::injection_ipc::LauncherProofRegistration,
    ) -> Result<(), InjectionError> {
        self.control(
            json!({"op":"attach_launcher_proof","session_id":registration.session_id,
            "instance_id":registration.instance_id,"peer_uid":registration.peer_uid,
            "proof":registration.proof}),
        )
    }

    pub fn open_auth_section(
        &self,
        section: AuthSectionRegistration,
    ) -> Result<(), InjectionError> {
        self.control(
            json!({"op":"open_auth_section","session_id":section.session_id,
            "auth_section_id":section.auth_section_id,"lease_id":section.lease_id,
            "exact_origin":section.exact_origin,"cdp_target_id":section.cdp_target_id}),
        )
    }

    pub fn close_auth_section(
        &self,
        session_id: &str,
        auth_section_id: &str,
    ) -> Result<(), InjectionError> {
        self.control(json!({"op":"close_auth_section","session_id":session_id,"auth_section_id":auth_section_id}))
    }

    pub fn unregister_live_session(&self, session_id: &str) -> Result<(), InjectionError> {
        self.control(json!({"op":"unregister_live_session","session_id":session_id}))
    }
}

struct UnixPending(UnixStream);

impl PendingInjection for UnixPending {
    fn finish(mut self: Box<Self>) -> Result<Value, InjectionError> {
        let mut size = [0u8; 4];
        self.0
            .read_exact(&mut size)
            .map_err(|_| InjectionError::SinkFailed)?;
        let len = u32::from_be_bytes(size) as usize;
        if len == 0 || len > 16 * 1024 {
            return Err(InjectionError::SinkFailed);
        }
        let mut body = vec![0u8; len];
        self.0
            .read_exact(&mut body)
            .map_err(|_| InjectionError::SinkFailed)?;
        let response = serde_json::from_slice(&body).map_err(|_| InjectionError::SinkFailed);
        body.zeroize();
        response
    }
}

impl BrokerClient for UnixInjectionClient {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        self.check_socket()?;
        let stream = UnixStream::connect(&self.socket).map_err(|_| InjectionError::SinkFailed)?;
        start_on_stream(stream, request, sink)
    }
}

/// ADR 2026-10-09 付記（launcher の Authenticate 経路）2: an `injection.sock` connection the
/// daemon opened (so credentiald sees the registered controller as the peer) and handed to the
/// launcher with `SCM_RIGHTS`. It carries exactly one injection; the launcher never learns the
/// broker's path and never talks to its control socket.
pub struct PassedInjectionStream(Option<UnixStream>);

impl PassedInjectionStream {
    pub fn new(stream: UnixStream) -> Self {
        Self(Some(stream))
    }
}

impl BrokerClient for PassedInjectionStream {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        let stream = self.0.take().ok_or(InjectionError::SinkFailed)?;
        start_on_stream(stream, request, sink)
    }
}

/// Send one injection request with its sink FD on a connected `injection.sock` stream.
fn start_on_stream(
    stream: UnixStream,
    request: Value,
    sink: OwnedFd,
) -> Result<Box<dyn PendingInjection>, InjectionError> {
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| InjectionError::SinkFailed)?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| InjectionError::SinkFailed)?;
    let mut body = serde_json::to_vec(&request).map_err(|_| InjectionError::SinkFailed)?;
    if body.len() > 16 * 1024 {
        body.zeroize();
        return Err(InjectionError::SinkFailed);
    }
    let mut header = (body.len() as u32).to_be_bytes();
    let mut iov = [
        nix::libc::iovec {
            iov_base: header.as_mut_ptr().cast(),
            iov_len: header.len(),
        },
        nix::libc::iovec {
            iov_base: body.as_mut_ptr().cast(),
            iov_len: body.len(),
        },
    ];
    let mut control = [0u64; 4];
    // SAFETY: msghdr and aligned CMSG buffer are valid for one SCM_RIGHTS fd.
    let sent = unsafe {
        let mut msg: nix::libc::msghdr = std::mem::zeroed();
        msg.msg_iov = iov.as_mut_ptr();
        msg.msg_iovlen = iov.len();
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = nix::libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as _;
        let cmsg = nix::libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = nix::libc::SOL_SOCKET;
        (*cmsg).cmsg_type = nix::libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = nix::libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as _;
        std::ptr::write_unaligned(nix::libc::CMSG_DATA(cmsg).cast::<i32>(), sink.as_raw_fd());
        nix::libc::sendmsg(stream.as_raw_fd(), &msg, nix::libc::MSG_NOSIGNAL)
    };
    body.zeroize();
    if sent != (header.len() + iov[1].iov_len) as isize {
        return Err(InjectionError::SinkFailed);
    }
    Ok(Box::new(UnixPending(stream)))
}

pub struct CdpController {
    write: File,
    read: File,
    buffered: Vec<u8>,
    next_id: u64,
    auth_section: Option<String>,
    /// ADR-0080 H3 / ADR-0101 D4: identity の復元を受けた session。session（= controller）の
    /// 終わりまで agent の観測を止める。解除する口は無い。
    restored: bool,
    injected: Vec<(String, String, String, String)>,
    events: Vec<Value>,
    login_navigation: Option<LoginNavigation>,
    /// ADR-0111: one guard per injected value, kept for the controller's (= session's) lifetime.
    guards: Vec<RedisplayGuard>,
    /// ADR 2026-10-09 credential username / post-login D2: the `read_origins` once the auth
    /// section closed on the post-login conditions. Every agent observation is checked against it.
    post_login: Option<Vec<String>>,
    /// Agent CDP session -> the target it attached to (recorded from the agent's own attach).
    agent_targets: std::collections::HashMap<String, String>,
    /// Target -> the controller's own flatten session used for post-login checks.
    check_sessions: std::collections::HashMap<String, String>,
    /// Controller-only sessions (login target, check sessions): their events are never queued.
    private_sessions: std::collections::HashSet<String>,
    /// Downloads that began outside `read_origins` (cancelled the moment they begin).
    denied_downloads: std::collections::HashSet<String>,
    /// A denied download completed before its cancel took effect: observation stops for the rest
    /// of the session (fail closed; the file may already exist).
    download_breach: bool,
    /// The most recent agent commands the relay refused, as (CDP method, fixed code), at most
    /// [`MAX_AGENT_DENIALS`]. The launcher logs them when an action fails so a production failure
    /// says which gate refused what (付記 2026-10-10f). Methods are reduced to a fixed shape.
    agent_denials: std::collections::VecDeque<(String, &'static str)>,
    /// Test-only: every agent command method, in order.
    #[cfg(test)]
    pub(crate) agent_log: Vec<String>,
    /// Test-only (ADR-0109 A1): navigate the page after all checks and before the
    /// broker's `Runtime.callFunctionOn` frame is written. Absent from production builds.
    #[cfg(feature = "attack-test-hooks")]
    retarget_before_sink: Option<String>,
    #[cfg(feature = "attack-test-hooks")]
    response_timeout: Duration,
}

/// Non-secret, bounded history for the trusted login top document only.
struct LoginNavigation {
    session: String,
    frame: Option<String>,
    origins: Vec<String>,
    redirects: usize,
    documents: usize,
    invalid: bool,
    deadline: std::time::Instant,
    waiting: bool,
}

impl CdpController {
    pub fn new(write: File, read: File) -> Self {
        Self {
            write,
            read,
            buffered: Vec::new(),
            next_id: 1,
            auth_section: None,
            restored: false,
            injected: Vec::new(),
            events: Vec::new(),
            login_navigation: None,
            guards: Vec::new(),
            post_login: None,
            agent_targets: std::collections::HashMap::new(),
            check_sessions: std::collections::HashMap::new(),
            private_sessions: std::collections::HashSet::new(),
            denied_downloads: std::collections::HashSet::new(),
            download_breach: false,
            agent_denials: std::collections::VecDeque::new(),
            #[cfg(test)]
            agent_log: Vec::new(),
            #[cfg(feature = "attack-test-hooks")]
            retarget_before_sink: None,
            #[cfg(feature = "attack-test-hooks")]
            response_timeout: TIMEOUT,
        }
    }

    /// Test-only bound for a CDP response while a real browser is being started.
    #[cfg(feature = "attack-test-hooks")]
    pub fn response_timeout_for_test(&mut self, timeout: Duration) {
        self.response_timeout = timeout;
    }

    /// Test-only (ADR-0109 A1): the next injection navigates its page session to `url`
    /// after the controller's checks and before the broker's command reaches CDP.
    #[cfg(feature = "attack-test-hooks")]
    pub fn retarget_before_sink_for_test(&mut self, url: String) {
        self.retarget_before_sink = Some(url);
    }

    #[cfg(feature = "attack-test-hooks")]
    fn retarget_for_test(&mut self, url: &str, session: &str) -> Result<(), InjectionError> {
        self.call("Page.navigate", json!({"url":url}), Some(session))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
            let ready = self.call(
                "Runtime.evaluate",
                json!({"expression":"document.readyState","returnByValue":true}),
                Some(session),
            );
            if tree["result"]["frameTree"]["frame"]["url"] == url
                && ready.is_ok_and(|r| r["result"]["result"]["value"] == "complete")
            {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err(InjectionError::SinkFailed)
    }

    pub fn open_auth_section(&mut self, id: String) {
        self.events.clear();
        self.login_navigation = None;
        self.auth_section = Some(id);
    }

    /// Start before Page.navigate; history survives same-origin JS relay documents.
    pub fn begin_login_navigation(
        &mut self,
        session: &str,
        timeout: Duration,
    ) -> Result<(), InjectionError> {
        let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
        let frame = tree["result"]["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or(InjectionError::TargetMismatch)?
            .to_owned();
        self.login_navigation = Some(LoginNavigation {
            session: session.into(),
            frame: Some(frame),
            origins: Vec::new(),
            redirects: 0,
            documents: 0,
            invalid: false,
            deadline: std::time::Instant::now() + timeout,
            waiting: true,
        });
        Ok(())
    }

    pub fn login_deadline(&self) -> Option<std::time::Instant> {
        self.login_navigation.as_ref().map(|nav| nav.deadline)
    }

    pub fn login_redirect_chain(&self, exact_origin: &str) -> Result<Vec<String>, InjectionError> {
        let nav = self
            .login_navigation
            .as_ref()
            .ok_or(InjectionError::AuthSectionRequired)?;
        if nav.invalid || nav.origins.iter().any(|o| o != exact_origin) {
            return Err(InjectionError::Redirected);
        }
        Ok(nav.origins.clone())
    }

    /// Only readiness is returned; no input value or page text crosses H3.
    pub fn login_password_document(
        &mut self,
        session: &str,
        exact_origin: &str,
        selector: &str,
        username_selector: Option<&str>,
    ) -> Result<Option<(String, String)>, InjectionError> {
        let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
        self.login_redirect_chain(exact_origin)?;
        let frame = &tree["result"]["frameTree"]["frame"];
        let url = frame["url"].as_str();
        if url == Some("about:blank") {
            return Ok(None);
        }
        if origin(url).as_deref() != Some(exact_origin) {
            return Err(InjectionError::Redirected);
        }
        let (Some(id), Some(loader)) = (frame["id"].as_str(), frame["loaderId"].as_str()) else {
            return Ok(None);
        };
        let world = match self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":id,"worldName":"celeris-credential"}),
            Some(session),
        ) {
            Ok(world) => world,
            Err(InjectionError::TargetChanged) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(context) = world["result"]["executionContextId"].as_i64() else {
            return Ok(None);
        };
        // The username field (when the policy has one) must be on the same document: exactly one
        // text/email input next to exactly one password input.
        let user = match username_selector {
            Some(u) => format!(
                "const us=document.querySelectorAll({});if(us.length!==1||!(us[0] instanceof HTMLInputElement)||(us[0].type!=='text'&&us[0].type!=='email'))return false;",
                serde_json::to_string(u).map_err(|_| InjectionError::TargetMismatch)?
            ),
            None => String::new(),
        };
        let expr = format!(
            "(()=>{{{user}const es=document.querySelectorAll({});return es.length===1 && es[0] instanceof HTMLInputElement && es[0].type==='password';}})()",
            serde_json::to_string(selector).map_err(|_| InjectionError::TargetMismatch)?
        );
        let ready = self.call(
            "Runtime.evaluate",
            json!({"expression":expr,"contextId":context,"returnByValue":true,"silent":true}),
            Some(session),
        );
        // Even a destroyed context can carry navigation events: inspect the history first.
        self.login_redirect_chain(exact_origin)?;
        if !ready.is_ok_and(|r| r["result"]["result"]["value"] == true) {
            return Ok(None);
        }
        let after = self.call("Page.getFrameTree", json!({}), Some(session))?;
        self.login_redirect_chain(exact_origin)?;
        let after = &after["result"]["frameTree"]["frame"];
        if origin(after["url"].as_str()).as_deref() != Some(exact_origin) {
            return Err(InjectionError::Redirected);
        }
        if after["id"] != id || after["loaderId"] != loader {
            return Ok(None);
        }
        if let Some(nav) = self.login_navigation.as_mut() {
            nav.waiting = false;
        }
        Ok(Some((id.into(), loader.into())))
    }

    /// Remove injected values while the agent relay remains blocked.
    pub fn clear_injected_values(&mut self) -> Result<(), InjectionError> {
        // Clear on the same document before observations become available.
        for (object_id, session, frame_id, loader_id) in self.injected.clone() {
            let tree = self.call("Page.getFrameTree", json!({}), Some(&session))?;
            let mut chain = Vec::new();
            if find_frame(&tree["result"]["frameTree"], &frame_id, &mut chain)
                && chain.last().is_some_and(|f| f["loaderId"] == loader_id)
            {
                let cleared = self.call("Runtime.callFunctionOn", json!({"objectId":object_id,"functionDeclaration":"function(){if(this.isConnected){Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(this,'');this.dispatchEvent(new Event('input',{bubbles:true}));}return 'ok';}","returnByValue":true,"silent":true}), Some(&session))?;
                if cleared["result"]["result"]["value"] != "ok" {
                    return Err(InjectionError::TargetChanged);
                }
            }
        }
        self.injected.clear();
        Ok(())
    }

    /// identity 復元の state を投入する前に呼ぶ。以後この controller が生きている間、
    /// agent 由来の観測は拒否され、CDP event（console を含む）は捨てられる。
    pub fn enter_restored_observation_stop(&mut self) {
        self.events.clear();
        self.restored = true;
    }

    /// 認証区間中か、復元を受けた session か（どちらも agent の観測を止める）。
    pub fn observation_stopped(&self) -> bool {
        self.auth_section.is_some() || self.restored || self.download_breach
    }

    pub fn close_auth_section(&mut self) -> Result<(), InjectionError> {
        self.clear_injected_values()?;
        // Drain frames produced during the stopped interval while suppression is
        // still active, so the relay cannot forward them after the section closes.
        self.pump_events()?;
        self.auth_section = None;
        self.login_navigation = None;
        Ok(())
    }

    /// Agent-originating observation commands are refused throughout H3. CDP
    /// events (including console output) are consumed internally and discarded.
    pub fn agent_command(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        let result = self.agent_command_inner(method, params, session);
        #[cfg(test)]
        self.agent_log.push(match &result {
            Ok(_) => method.to_owned(),
            Err(e) => format!("{method}!{}", e.code()),
        });
        result
    }

    fn agent_command_inner(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        if self.observation_stopped() {
            return Err(InjectionError::AuthSectionRequired);
        }
        // ADR 2026-10-09 credential username / post-login D2-3: after login every agent command on a
        // page session that can read or act on the page is checked before it runs and its result is
        // checked again before it crosses (the page may have navigated meanwhile).
        // 付記 2026-10-10c: after login the agent may navigate only to `read_origins` (or a blank
        // tab). A browser-initiated navigation that turns into a download raises no download event,
        // so the cancellation below could not hold it; pages elsewhere were unreadable anyway.
        if let Some(read_origins) = &self.post_login
            && matches!(method, "Page.navigate" | "Target.createTarget")
        {
            let url = params["url"].as_str();
            let blank = url.is_none_or(|u| u == "about:blank");
            if !blank && origin(url).is_none_or(|o| !read_origins.contains(&o)) {
                return Err(InjectionError::ObservationOriginDenied);
            }
        }
        let gated = self.post_login.is_some()
            && session.is_some_and(|_| !post_login_control_method(method));
        if gated && let Some(s) = session {
            self.post_login_gate(s)?;
        }
        let attach_target = (method == "Target.attachToTarget")
            .then(|| params["targetId"].as_str().map(str::to_owned))
            .flatten();
        let reply = self.call(method, params, session)?;
        if self.download_breach {
            return Err(InjectionError::ObservationOriginDenied);
        }
        if let (Some(target), Some(agent_session)) =
            (attach_target, reply["result"]["sessionId"].as_str())
        {
            self.agent_targets.insert(agent_session.to_owned(), target);
        }
        if self.redisplayed(&reply) {
            // The whole observation is dropped; only the fixed reason crosses.
            drop(reply);
            return Err(InjectionError::RedisplayDetected);
        }
        if gated && let Some(s) = session {
            self.post_login_gate(s)?;
        }
        Ok(reply)
    }

    /// Record a relay refusal of an agent command (fixed code; the method is reduced by
    /// [`loggable_method`]). Only the last [`MAX_AGENT_DENIALS`] are kept.
    pub fn record_agent_denial(&mut self, method: &str, code: &'static str) {
        if self.agent_denials.len() >= MAX_AGENT_DENIALS {
            self.agent_denials.pop_front();
        }
        self.agent_denials
            .push_back((loggable_method(method), code));
    }

    /// The refusals recorded since the last call, oldest first.
    pub fn take_agent_denials(&mut self) -> Vec<(String, &'static str)> {
        self.agent_denials.drain(..).collect()
    }

    /// Whether the session's auth section closed on the post-login conditions.
    pub fn post_login_active(&self) -> bool {
        self.post_login.is_some()
    }

    /// ADR 2026-10-09 credential username / post-login D2-2: what the login tab (`own`) shows now. The
    /// section may close only on `Ready`: the tab left the injected document (`login_loader`), its
    /// top document is on a `read_origins` origin and no password input remains. IdP pages are
    /// classified (consent, login form again, other) for the held reason; the controller reads only
    /// fixed markers in its isolated world. A failed read is `CheckFailed` (the caller keeps waiting).
    pub fn post_login_probe(
        &mut self,
        own: &str,
        login_loader: &str,
        idp_origin: &str,
        read_origins: &[String],
    ) -> Result<PostLoginProbe, InjectionError> {
        if self.auth_section.is_none() || self.restored {
            return Err(InjectionError::AuthSectionRequired);
        }
        let Ok(tree) = self.call("Page.getFrameTree", json!({}), Some(own)) else {
            return Ok(PostLoginProbe::CheckFailed);
        };
        let frame = &tree["result"]["frameTree"]["frame"];
        let (Some(loader), Some(frame_id)) = (frame["loaderId"].as_str(), frame["id"].as_str())
        else {
            return Ok(PostLoginProbe::CheckFailed);
        };
        if loader == login_loader {
            return Ok(PostLoginProbe::LoginDocument);
        }
        let Some(top) = origin(frame["url"].as_str()) else {
            return Ok(PostLoginProbe::NoDocument);
        };
        let frame_id = frame_id.to_owned();
        let loader = loader.to_owned();
        if top == idp_origin {
            return Ok(match self.idp_markers(own, &frame_id) {
                Ok((_, true)) => PostLoginProbe::IdpConsent,
                Ok((n, false)) if n > 0 => PostLoginProbe::IdpLoginForm,
                Ok(_) => PostLoginProbe::IdpPage,
                Err(_) => PostLoginProbe::CheckFailed,
            });
        }
        if !read_origins.contains(&top) {
            return Ok(PostLoginProbe::OtherOrigin);
        }
        match self.password_inputs(own, &frame_id) {
            Ok(0) => {}
            Ok(_) => return Ok(PostLoginProbe::ReadOriginPasswordField),
            Err(_) => return Ok(PostLoginProbe::CheckFailed),
        }
        // The same document must still be the top document after the count.
        let Ok(after) = self.call("Page.getFrameTree", json!({}), Some(own)) else {
            return Ok(PostLoginProbe::CheckFailed);
        };
        if after["result"]["frameTree"]["frame"]["loaderId"].as_str() != Some(loader.as_str()) {
            return Ok(PostLoginProbe::CheckFailed);
        }
        Ok(PostLoginProbe::Ready)
    }

    /// Password inputs and whether a Shibboleth attribute-release consent form is present on an
    /// IdP page (counted in the controller's isolated world; nothing else is read).
    fn idp_markers(
        &mut self,
        session: &str,
        frame_id: &str,
    ) -> Result<(u64, bool), InjectionError> {
        let passwords = self.password_inputs(session, frame_id)?;
        let world = self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":frame_id,"worldName":CHECK_WORLD}),
            Some(session),
        )?;
        let context = world["result"]["executionContextId"]
            .as_i64()
            .ok_or(InjectionError::TargetChanged)?;
        let consent = self.call(
            "Runtime.evaluate",
            json!({"expression":CONSENT_MARKER_JS,"contextId":context,"returnByValue":true,"silent":true}),
            Some(session),
        )?;
        Ok((
            passwords,
            consent["result"]["result"]["value"] == Value::Bool(true),
        ))
    }

    /// 付記 2026-10-10b: press the administrator's fixed consent button once — only on the IdP
    /// (`idp_origin`) top document of the login tab, only on a consent page, only when `selector`
    /// matches exactly one submit button inside the consent form (and `choice_selector`, if any,
    /// exactly one radio of the same form, which is checked first). Runs in the controller's
    /// isolated world; returns whether it submitted.
    pub fn press_consent(
        &mut self,
        own: &str,
        idp_origin: &str,
        consent: &task_core::browser_wait::ConsentPolicy,
    ) -> Result<bool, InjectionError> {
        if self.auth_section.is_none() || self.restored {
            return Err(InjectionError::AuthSectionRequired);
        }
        let tree = self.call("Page.getFrameTree", json!({}), Some(own))?;
        let frame = &tree["result"]["frameTree"]["frame"];
        if origin(frame["url"].as_str()).as_deref() != Some(idp_origin) {
            return Ok(false);
        }
        let frame_id = frame["id"]
            .as_str()
            .ok_or(InjectionError::TargetChanged)?
            .to_owned();
        let selector =
            serde_json::to_string(&consent.selector).map_err(|_| InjectionError::TargetMismatch)?;
        let choice = match &consent.choice_selector {
            Some(c) => format!(
                "const cs=document.querySelectorAll({});if(cs.length!==1)return 'no_match';const c=cs[0];\
if(String(c.localName)!=='input'||String(c.type).toLowerCase()!=='radio'||c.form!==e.form)return 'no_match';\
c.checked=true;",
                serde_json::to_string(c).map_err(|_| InjectionError::TargetMismatch)?
            ),
            None => String::new(),
        };
        let expr = format!(
            "(()=>{{const marker={CONSENT_SELECTOR_JS};if(!document.querySelector(marker))return 'no_consent';\
const es=document.querySelectorAll({selector});if(es.length!==1)return 'no_match';const e=es[0];\
const t=String(e.type||'').toLowerCase();\
const button=(String(e.localName)==='button'&&(t===''||t==='submit'))||(String(e.localName)==='input'&&t==='submit');\
if(!button||!e.form||!e.form.querySelector(marker))return 'no_match';\
{choice}e.form.requestSubmit(e);return 'ok';}})()"
        );
        let world = self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":frame_id,"worldName":CHECK_WORLD}),
            Some(own),
        )?;
        let context = world["result"]["executionContextId"]
            .as_i64()
            .ok_or(InjectionError::TargetChanged)?;
        let pressed = self.call(
            "Runtime.evaluate",
            json!({"expression":expr,"contextId":context,"returnByValue":true,"silent":true}),
            Some(own),
        )?;
        Ok(pressed["result"]["result"]["value"] == "ok")
    }

    /// 付記 2026-10-10b-4: the consent form's `button` / `input[type=submit|radio]` names, kinds and
    /// `value` attributes (sanitized) on the login tab's IdP top document; empty otherwise.
    pub fn consent_controls(&mut self, own: &str, idp_origin: &str) -> Vec<ConsentControl> {
        let read = (|| -> Result<Vec<ConsentControl>, InjectionError> {
            let tree = self.call("Page.getFrameTree", json!({}), Some(own))?;
            let frame = &tree["result"]["frameTree"]["frame"];
            if origin(frame["url"].as_str()).as_deref() != Some(idp_origin) {
                return Ok(Vec::new());
            }
            let frame_id = frame["id"]
                .as_str()
                .ok_or(InjectionError::TargetChanged)?
                .to_owned();
            let world = self.call(
                "Page.createIsolatedWorld",
                json!({"frameId":frame_id,"worldName":CHECK_WORLD}),
                Some(own),
            )?;
            let context = world["result"]["executionContextId"]
                .as_i64()
                .ok_or(InjectionError::TargetChanged)?;
            let expr = format!(
                "(()=>{{const m=document.querySelector({CONSENT_SELECTOR_JS});if(!m)return [];\
const root=m.form||document;const out=[];\
for(const e of root.querySelectorAll('button,input[type=submit],input[type=radio]')){{\
if(out.length>={CONSENT_CONTROLS_MAX})break;const t=String(e.type||'').toLowerCase();\
const kind=t==='radio'?'radio':(t==='submit'||(String(e.localName)==='button'&&t==='')?'submit':'button');\
out.push({{name:String(e.getAttribute('name')||''),kind:kind,value:String(e.getAttribute('value')||'')}});}}\
return out;}})()"
            );
            let reply = self.call(
                "Runtime.evaluate",
                json!({"expression":expr,"contextId":context,"returnByValue":true,"silent":true}),
                Some(own),
            )?;
            Ok(serde_json::from_value::<Vec<ConsentControl>>(
                reply["result"]["result"]["value"].clone(),
            )
            .unwrap_or_default())
        })();
        sanitize_consent_controls(read.unwrap_or_default())
    }

    /// Close the auth section after [`Self::post_login_probe`] returned `Ready`: clear injected values
    /// (skipped when their document is gone), discard everything produced while stopped, and resume
    /// agent observation under the post-login checks for the rest of the session. `own` (the login
    /// tab's controller session) stays private: its events are never queued for the agent.
    pub fn resume_after_login(
        &mut self,
        own: &str,
        read_origins: Vec<String>,
    ) -> Result<(), InjectionError> {
        if self.auth_section.is_none() || self.restored || read_origins.is_empty() {
            return Err(InjectionError::AuthSectionRequired);
        }
        self.clear_injected_values()?;
        self.private_sessions.insert(own.to_owned());
        // The login tab's session was attached during the auth section (its events were never
        // queued); keep it private for the rest of the session.
        self.pump_events()?;
        self.events.clear();
        self.post_login = Some(read_origins);
        self.auth_section = None;
        self.login_navigation = None;
        Ok(())
    }

    /// The controller's own session on `target`, attached once (flatten) and kept private.
    fn check_session(&mut self, target: &str) -> Result<String, InjectionError> {
        if let Some(s) = self.check_sessions.get(target) {
            return Ok(s.clone());
        }
        let attached = self.call(
            "Target.attachToTarget",
            json!({"targetId":target,"flatten":true}),
            None,
        )?;
        let session = attached["result"]["sessionId"]
            .as_str()
            .ok_or(InjectionError::ObservationOriginDenied)?
            .to_owned();
        self.private_sessions.insert(session.clone());
        // The attach announced the new session (`Target.attachedToTarget`) before the reply named it;
        // agent connections never learn about controller-only sessions.
        self.events
            .retain(|e| e["params"]["sessionId"].as_str() != Some(session.as_str()));
        self.check_sessions
            .insert(target.to_owned(), session.clone());
        Ok(session)
    }

    /// Number of password inputs in `frame_id`'s document (open shadow roots and same-origin frames
    /// included), counted in the controller's isolated world on `session`.
    fn password_inputs(&mut self, session: &str, frame_id: &str) -> Result<u64, InjectionError> {
        let world = self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":frame_id,"worldName":CHECK_WORLD}),
            Some(session),
        )?;
        let context = world["result"]["executionContextId"]
            .as_i64()
            .ok_or(InjectionError::PasswordFieldPresent)?;
        let counted = self.call(
            "Runtime.evaluate",
            json!({"expression":PASSWORD_COUNT_JS,"contextId":context,"returnByValue":true,"silent":true}),
            Some(session),
        )?;
        counted["result"]["result"]["value"]
            .as_u64()
            .ok_or(InjectionError::PasswordFieldPresent)
    }

    /// ADR 2026-10-09 credential username / post-login D2-3: the agent session's top document must be
    /// on a `read_origins` origin (an empty `about:blank` tab carries no page data) and have no
    /// password input. Any failure to check is a denial.
    fn post_login_gate(&mut self, agent_session: &str) -> Result<(), InjectionError> {
        let read_origins = self
            .post_login
            .clone()
            .ok_or(InjectionError::AuthSectionRequired)?;
        let target = self
            .agent_targets
            .get(agent_session)
            .cloned()
            .ok_or(InjectionError::ObservationOriginDenied)?;
        let check = self
            .check_session(&target)
            .map_err(|_| InjectionError::ObservationOriginDenied)?;
        let tree = self
            .call("Page.getFrameTree", json!({}), Some(&check))
            .map_err(|_| InjectionError::ObservationOriginDenied)?;
        let frame = &tree["result"]["frameTree"]["frame"];
        let url = frame["url"].as_str();
        if url == Some("about:blank") && frame["childFrames"].is_null() {
            return Ok(());
        }
        match origin(url) {
            Some(o) if read_origins.contains(&o) => {}
            _ => return Err(InjectionError::ObservationOriginDenied),
        }
        let frame_id = frame["id"]
            .as_str()
            .ok_or(InjectionError::ObservationOriginDenied)?
            .to_owned();
        match self.password_inputs(&check, &frame_id) {
            Ok(0) => Ok(()),
            _ => Err(InjectionError::PasswordFieldPresent),
        }
    }

    /// Write a CDP command without waiting for its reply (the reply is dropped as unsolicited).
    /// Used from the event path, where no call can be made. A failed write stops observation.
    fn send_untracked(&mut self, method: &str, params: Value) {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let sent = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))
            .map(|mut bytes| {
                bytes.push(0);
                bytes
            })
            .ok()
            .is_some_and(|bytes| self.write.write_all(&bytes).is_ok());
        if !sent {
            self.download_breach = true;
        }
    }

    /// ADR-0111: whether an observation carries an injected value in any
    /// representation the guard decodes (raw, percent, UTF-16LE, base64, JSON escape).
    fn redisplayed(&self, observation: &Value) -> bool {
        self.guards.iter().any(|g| g.exposes_json(observation))
    }

    /// Number of redisplay guards received from the broker for this session.
    pub fn redisplay_guards(&self) -> usize {
        self.guards.len()
    }

    /// Trusted controller operations needed to prepare and finish H3. The
    /// caller must hold the controller handle, never an agent connection.
    pub fn controller_command(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        self.call(method, params, session)
    }

    /// Events collected while a command was in flight. An auth section discards
    /// them before the relay can expose them to an agent connection.
    pub fn take_agent_events(&mut self) -> Vec<Value> {
        if self.observation_stopped() {
            self.events.clear();
            Vec::new()
        } else {
            let mut events = std::mem::take(&mut self.events);
            events.retain(|e| !self.redisplayed(e));
            events
        }
    }

    /// Like [`Self::take_agent_events`], but only the events of `sessions`; events of
    /// other connections' sessions stay queued for them (F1: idle pumping per connection).
    pub fn take_agent_events_for(
        &mut self,
        sessions: &std::collections::HashSet<String>,
    ) -> Vec<Value> {
        if self.observation_stopped() {
            self.events.clear();
            return Vec::new();
        }
        let (mine, rest): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.events)
            .into_iter()
            .partition(|e| e["sessionId"].as_str().is_none_or(|s| sessions.contains(s)));
        self.events = rest;
        mine.into_iter().filter(|e| !self.redisplayed(e)).collect()
    }

    pub fn auth_section_active(&self) -> bool {
        self.auth_section.is_some()
    }

    pub fn inject(
        &mut self,
        request: &InjectionRequest,
        cdp_session: &str,
        broker: &mut dyn BrokerClient,
    ) -> Result<Value, InjectionError> {
        if self.auth_section.as_deref() != Some(request.auth_section_id.as_str()) {
            return Err(InjectionError::AuthSectionRequired);
        }
        let target = self.call(
            "Target.getTargetInfo",
            json!({"targetId":request.cdp_target_id}),
            None,
        )?;
        let info = &target["result"]["targetInfo"];
        if info["targetId"] != request.cdp_target_id || info["type"] != "page" {
            return Err(InjectionError::TargetMismatch);
        }
        if origin(info["url"].as_str()) != Some(request.exact_origin.clone()) {
            return Err(InjectionError::TargetMismatch);
        }
        let tree = self.call("Page.getFrameTree", json!({}), Some(cdp_session))?;
        let root = &tree["result"]["frameTree"];
        let mut chain = Vec::new();
        if !find_frame(root, &request.frame_id, &mut chain) {
            return Err(InjectionError::TargetMismatch);
        }
        if chain
            .iter()
            .any(|frame| origin(frame["url"].as_str()) != Some(request.exact_origin.clone()))
        {
            return Err(InjectionError::CrossOriginFrame);
        }
        let frame = chain.last().ok_or(InjectionError::TargetMismatch)?;
        if frame["loaderId"] != request.loader_id {
            return Err(InjectionError::TargetChanged);
        }
        if request
            .redirect_chain
            .iter()
            .any(|o| o != &request.exact_origin)
        {
            return Err(InjectionError::Redirected);
        }
        // The DOM domain addresses the top document for this page session.
        // A child frame needs its own CDP session to resolve nodes safely.
        if chain.len() != 1 {
            return Err(InjectionError::TargetMismatch);
        }
        let world = self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":request.frame_id,"worldName":"celeris-credential"}),
            Some(cdp_session),
        )?;
        let context = world["result"]["executionContextId"]
            .as_i64()
            .ok_or(InjectionError::TargetChanged)?;
        let document = self.call("DOM.getDocument", json!({}), Some(cdp_session))?;
        let root_id = document["result"]["root"]["nodeId"]
            .as_i64()
            .ok_or(InjectionError::TargetChanged)?;
        let queried = self.call(
            "DOM.querySelectorAll",
            json!({"nodeId":root_id,"selector":request.selector}),
            Some(cdp_session),
        )?;
        let nodes = queried["result"]["nodeIds"]
            .as_array()
            .ok_or(InjectionError::TargetMismatch)?;
        if nodes.len() != 1 {
            return Err(InjectionError::TargetMismatch);
        }
        let node_id = nodes[0]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or(InjectionError::TargetMismatch)?;
        let resolved = self.call(
            "DOM.resolveNode",
            json!({"nodeId":node_id,"executionContextId":context}),
            Some(cdp_session),
        )?;
        let object_id = resolved["result"]["object"]["objectId"]
            .as_str()
            .ok_or(InjectionError::TargetChanged)?;
        let object_id = object_id.to_owned();
        let input_type = self.input_type(&object_id, cdp_session)?;
        let valid = match request.field.as_str() {
            "password" => input_type == "password",
            "username" => matches!(input_type.as_str(), "text" | "email"),
            _ => false,
        };
        if !valid {
            return Err(InjectionError::RedisplayField);
        }
        // ADR 2026-10-09 credential username / post-login D1-4: the username field is resolved with
        // the administrator's selector on the same top document (same frame id and loader id as the
        // password field, checked above), exactly one text/email input that is not the password field.
        let username = match &request.username_selector {
            None => None,
            Some(_) if request.field != "password" => return Err(InjectionError::TargetMismatch),
            Some(selector) => {
                let queried = self.call(
                    "DOM.querySelectorAll",
                    json!({"nodeId":root_id,"selector":selector}),
                    Some(cdp_session),
                )?;
                let nodes = queried["result"]["nodeIds"]
                    .as_array()
                    .ok_or(InjectionError::TargetMismatch)?;
                if nodes.len() != 1 {
                    return Err(InjectionError::TargetMismatch);
                }
                let user_node = nodes[0]
                    .as_i64()
                    .filter(|id| *id > 0 && *id != node_id)
                    .ok_or(InjectionError::TargetMismatch)?;
                let resolved = self.call(
                    "DOM.resolveNode",
                    json!({"nodeId":user_node,"executionContextId":context}),
                    Some(cdp_session),
                )?;
                let user_object = resolved["result"]["object"]["objectId"]
                    .as_str()
                    .ok_or(InjectionError::TargetChanged)?
                    .to_owned();
                let user_type = self.input_type(&user_object, cdp_session)?;
                if !matches!(user_type.as_str(), "text" | "email") {
                    return Err(InjectionError::RedisplayField);
                }
                Some(UsernameTarget {
                    selector: selector.clone(),
                    object_id: user_object,
                    input_type: user_type,
                })
            }
        };
        // The broker's frame must not be confused with the frame tree re-read above.
        let object_id = object_id.as_str();
        let command_id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(InjectionError::SinkFailed)?;
        let (controller_fd, broker_fd) =
            browser_relay::seqpacket_pair().map_err(|_| InjectionError::SinkFailed)?;
        let field = match request.field.as_str() {
            "password" => Field::Password,
            "username" => Field::Username,
            _ => return Err(InjectionError::RedisplayField),
        };
        let redirect_chain = if self.login_navigation.is_some() {
            let chain = self.login_redirect_chain(&request.exact_origin)?;
            if chain.is_empty() {
                return Err(InjectionError::Redirected);
            }
            chain
        } else {
            request.redirect_chain.clone()
        };
        let username_object = username.as_ref().map(|u| u.object_id.clone());
        let wire = serde_json::to_value(WireRequest {
            v: if username.is_some() {
                PAIR_REQUEST_VERSION
            } else {
                1
            },
            request_id: request.request_id.clone(),
            session_id: request.session_id.clone(),
            cdp_target_id: request.cdp_target_id.clone(),
            cdp_session_id: Some(cdp_session.to_owned()),
            frame_id: request.frame_id.clone(),
            loader_id: request.loader_id.clone(),
            frame_chain: chain
                .iter()
                .filter_map(|f| origin(f["url"].as_str()))
                .collect(),
            redirect_chain,
            selector: request.selector.clone(),
            object_id: object_id.to_owned(),
            field,
            input_type: input_type.to_owned(),
            auth_section_id: request.auth_section_id.clone(),
            lease_id: request.lease_id.clone(),
            cdp_command_id: command_id,
            username,
        })
        .map_err(|_| InjectionError::SinkFailed)?;
        let pending = broker.start(wire, broker_fd)?;
        let mut command = match recv_packet(&controller_fd) {
            Ok(command) => command,
            Err(_) => {
                let reply = pending.finish()?;
                return Err(broker_denial(&reply));
            }
        };
        if command.last() != Some(&0) || command.len() > MAX_FRAME {
            command.zeroize();
            return Err(InjectionError::SinkFailed);
        }
        // ADR 2026-10-09 付記（launcher の Authenticate 経路）3: only the fixed injection call on
        // the resolved element may reach Chrome, whoever holds the broker end of the sink.
        if !fixed_injection_frame(
            &command,
            &SinkExpectation {
                command_id,
                cdp_session,
                object_id,
                origin: &request.exact_origin,
                field: request.field.as_str(),
                username_object: username_object.as_deref(),
            },
        ) {
            command.zeroize();
            // Close the sink unanswered and let the broker finish (it records its own failure).
            drop(controller_fd);
            let _ = pending.finish();
            return Err(InjectionError::SinkFailed);
        }
        #[cfg(feature = "attack-test-hooks")]
        if let Some(url) = self.retarget_before_sink.take()
            && let Err(e) = self.retarget_for_test(&url, cdp_session)
        {
            command.zeroize();
            return Err(e);
        }
        self.write
            .write_all(&command)
            .map_err(|_| InjectionError::SinkFailed)?;
        command.zeroize();
        let reply = self.read_response(command_id)?;
        let status = if reply["error"].is_null() && reply["result"]["result"]["value"] == "ok" {
            Ok(())
        } else {
            Err(InjectionError::TargetChanged)
        };
        let mut raw = serde_json::to_vec(&reply).map_err(|_| InjectionError::SinkFailed)?;
        send_packet(&controller_fd, &raw)?;
        raw.zeroize();
        if status.is_err() {
            // The broker has already consumed the lease; report its own verdict.
            let receipt = pending.finish()?;
            return Err(if receipt["ok"] == false {
                broker_denial(&receipt)
            } else {
                InjectionError::TargetChanged
            });
        }
        let receipt = pending.finish()?;
        let typed: InjectionReply =
            serde_json::from_value(receipt.clone()).map_err(|_| InjectionError::SinkFailed)?;
        if typed.v != 1 {
            return Err(InjectionError::SinkFailed);
        }
        if !typed.ok {
            return Err(broker_denial(&receipt));
        }
        if typed.code.is_some()
            || typed.receipt.is_none()
            || receipt["ok"] != true
            || receipt["request_id"] != request.request_id
            || receipt["receipt"]["lease_id"] != request.lease_id
            || receipt["receipt"]["auth_section_id"] != request.auth_section_id
            || receipt["receipt"]["session_id"] != request.session_id
            || receipt["receipt"]["cdp_target_id"] != request.cdp_target_id
            || receipt["receipt"]["frame_id"] != request.frame_id
            || receipt["receipt"]["loader_id"] != request.loader_id
            || receipt["receipt"]["field"] != request.field
        {
            return Err(InjectionError::SinkFailed);
        }
        let injected_at = receipt["receipt"]["injected_at"]
            .as_u64()
            .ok_or(InjectionError::SinkFailed)?;
        // A receipt without a well-formed guard is a failed injection (fail closed).
        let guard = typed
            .redisplay_guard
            .as_ref()
            .and_then(RedisplayGuard::from_wire)
            .ok_or(InjectionError::SinkFailed)?;
        self.guards.push(guard);
        self.login_navigation = None;
        self.injected.push((
            object_id.to_owned(),
            cdp_session.to_owned(),
            request.frame_id.clone(),
            request.loader_id.clone(),
        ));
        if let Some(user) = username_object {
            self.injected.push((
                user,
                cdp_session.to_owned(),
                request.frame_id.clone(),
                request.loader_id.clone(),
            ));
        }
        Ok(
            json!({"v":1,"request_id":request.request_id,"ok":true,"receipt":{
                "lease_id":request.lease_id,"auth_section_id":request.auth_section_id,
                "session_id":request.session_id,"cdp_target_id":request.cdp_target_id,
                "frame_id":request.frame_id,"loader_id":request.loader_id,
                "field":request.field,"injected_at":injected_at
            }}),
        )
    }

    /// The resolved element's input type (lower case), `""` for a non-input.
    fn input_type(&mut self, object_id: &str, session: &str) -> Result<String, InjectionError> {
        let check = self.call("Runtime.callFunctionOn", json!({"objectId":object_id,"functionDeclaration":"function(){return this instanceof HTMLInputElement ? this.type.toLowerCase() : '';}","returnByValue":true,"silent":true}), Some(session))?;
        check["result"]["result"]["value"]
            .as_str()
            .map(str::to_owned)
            .ok_or(InjectionError::TargetChanged)
    }

    fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(InjectionError::SinkFailed)?;
        let mut message = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let mut bytes = serde_json::to_vec(&message).map_err(|_| InjectionError::SinkFailed)?;
        bytes.push(0);
        self.write
            .write_all(&bytes)
            .map_err(|_| InjectionError::SinkFailed)?;
        bytes.zeroize();
        let reply = self.read_response(id)?;
        if !reply["error"].is_null() {
            return Err(InjectionError::TargetChanged);
        }
        Ok(reply)
    }

    fn queue_event(&mut self, value: Value) {
        if value.get("method").is_none() {
            return;
        }
        if self.auth_section.is_some()
            && value["method"] == "Network.requestWillBeSent"
            && let Some(nav) = self.login_navigation.as_mut()
            && value["sessionId"] == nav.session
            && value["params"]["type"] == "Document"
        {
            let params = &value["params"];
            if let Some(frame) = params["frameId"].as_str() {
                if nav.frame.as_deref() == Some(frame) {
                    nav.documents += 1;
                    if params["redirectResponse"].is_object() {
                        nav.redirects += 1;
                    }
                    if nav.redirects > 32 || nav.documents > 32 {
                        nav.invalid = true;
                    } else {
                        if params["redirectResponse"].is_object() {
                            match origin(params["redirectResponse"]["url"].as_str()) {
                                Some(o) => {
                                    if nav.origins.last() != Some(&o) {
                                        nav.origins.push(o);
                                    }
                                }
                                None => nav.invalid = true,
                            }
                        }
                        match origin(params["request"]["url"].as_str()) {
                            Some(o) => nav.origins.push(o),
                            None => nav.invalid = true,
                        }
                        if nav.origins.len() > 32 {
                            nav.origins.truncate(32);
                            nav.invalid = true;
                        }
                    }
                }
            } else {
                nav.invalid = true;
            }
        }
        // ADR 2026-10-09 credential username / post-login D2-6: after login a download may only come
        // from a `read_origins` origin; any other is cancelled the moment it begins. If it still
        // completes (its body arrived before the cancel), observation stops for the session.
        if let Some(read_origins) = &self.post_login
            && matches!(
                value["method"].as_str(),
                Some("Browser.downloadWillBegin" | "Page.downloadWillBegin")
            )
            && origin(value["params"]["url"].as_str()).is_none_or(|o| !read_origins.contains(&o))
            && let Some(guid) = value["params"]["guid"].as_str()
        {
            let guid = guid.to_owned();
            self.send_untracked("Browser.cancelDownload", json!({"guid":guid}));
            self.denied_downloads.insert(guid);
            self.record_agent_denial("Browser.downloadWillBegin", "download_origin_denied");
        }
        if matches!(
            value["method"].as_str(),
            Some("Browser.downloadProgress" | "Page.downloadProgress")
        ) && value["params"]["state"] == "completed"
            && value["params"]["guid"]
                .as_str()
                .is_some_and(|g| self.denied_downloads.contains(g))
        {
            self.download_breach = true;
            self.events.clear();
            self.record_agent_denial("Browser.downloadProgress", "download_breach");
        }
        if [&value["sessionId"], &value["params"]["sessionId"]]
            .iter()
            .any(|s| {
                s.as_str()
                    .is_some_and(|s| self.private_sessions.contains(s))
            })
        {
            return;
        }
        if !self.observation_stopped() {
            if self.events.len() >= MAX_QUEUED_EVENTS {
                self.events.remove(0);
            }
            self.events.push(value);
        }
    }

    /// F1: queue CDP events the browser emitted while no command was in flight
    /// (e.g. `Page.loadEventFired` after the `Page.navigate` reply). Never waits.
    pub fn pump_events(&mut self) -> Result<(), InjectionError> {
        loop {
            while let Some(end) = self.buffered.iter().position(|b| *b == 0) {
                let mut frame: Vec<u8> = self.buffered.drain(..=end).collect();
                let parsed = serde_json::from_slice::<Value>(&frame[..end]);
                frame.zeroize();
                // A reply nobody waits for (all calls hold the lock until answered) is dropped.
                if let Ok(value) = parsed {
                    self.queue_event(value);
                }
            }

            if self.buffered.len() >= MAX_CDP_MESSAGE {
                return Err(InjectionError::SinkFailed);
            }
            let mut pollfd = nix::libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pollfd points to one valid descriptor and structure.
            if unsafe { nix::libc::poll(&mut pollfd, 1, 0) } <= 0 {
                return Ok(());
            }
            let mut chunk = [0u8; 4096];
            let n = self
                .read
                .read(&mut chunk)
                .map_err(|_| InjectionError::SinkFailed)?;
            if n == 0 {
                return Err(InjectionError::SinkFailed);
            }
            self.buffered.extend_from_slice(&chunk[..n]);
            chunk.zeroize();
        }
    }

    fn read_response(&mut self, id: u64) -> Result<Value, InjectionError> {
        #[cfg(feature = "attack-test-hooks")]
        let timeout = self.response_timeout;
        #[cfg(not(feature = "attack-test-hooks"))]
        let timeout = TIMEOUT;
        let deadline = std::time::Instant::now() + timeout;
        let deadline = self
            .login_navigation
            .as_ref()
            .filter(|nav| nav.waiting)
            .map_or(deadline, |nav| deadline.min(nav.deadline));
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(InjectionError::SinkFailed);
            }
            if let Some(end) = self.buffered.iter().position(|b| *b == 0) {
                let mut frame: Vec<u8> = self.buffered.drain(..=end).collect();
                let parsed = serde_json::from_slice::<Value>(&frame[..end])
                    .map_err(|_| InjectionError::SinkFailed);
                frame.zeroize();
                let value = parsed?;
                if value["id"] == id {
                    return Ok(value);
                }
                self.queue_event(value);
                continue;
            }
            if self.buffered.len() >= MAX_CDP_MESSAGE {
                return Err(InjectionError::SinkFailed);
            }
            let mut pollfd = nix::libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pollfd points to one valid descriptor and structure.
            if unsafe {
                nix::libc::poll(
                    &mut pollfd,
                    1,
                    timeout
                        .min(deadline.saturating_duration_since(std::time::Instant::now()))
                        .as_millis() as i32,
                )
            } <= 0
            {
                return Err(InjectionError::SinkFailed);
            }
            let mut chunk = [0u8; 4096];
            let n = self
                .read
                .read(&mut chunk)
                .map_err(|_| InjectionError::SinkFailed)?;
            if n == 0 {
                return Err(InjectionError::SinkFailed);
            }
            self.buffered.extend_from_slice(&chunk[..n]);
            chunk.zeroize();
        }
    }
}

/// One post-login wait in progress (both runtimes drive it; the launcher blocks, the daemon awaits).
/// 付記 2026-10-10b: on a consent page with a configured `consent`, the controller presses it once
/// and the wait restarts its stability timer; a consent page that stays ends the wait with
/// `consent_required` and the form's control names for the operator.
pub struct PostLoginWait<'a> {
    pub own: &'a str,
    pub login_loader: &'a str,
    pub idp_origin: &'a str,
    pub read_origins: &'a [String],
    pub consent: Option<&'a task_core::browser_wait::ConsentPolicy>,
    waiter: PostLoginWaiter,
    /// The one press per login was attempted (whatever the outcome).
    attempted: bool,
    /// The press submitted the consent form.
    pressed: bool,
}

impl<'a> PostLoginWait<'a> {
    pub fn new(
        own: &'a str,
        login_loader: &'a str,
        idp_origin: &'a str,
        read_origins: &'a [String],
        consent: Option<&'a task_core::browser_wait::ConsentPolicy>,
        timeout: Duration,
    ) -> Self {
        Self {
            own,
            login_loader,
            idp_origin,
            read_origins,
            consent,
            waiter: PostLoginWaiter::new(std::time::Instant::now(), timeout, POST_LOGIN_STABLE),
            attempted: false,
            pressed: false,
        }
    }

    /// One probe. `None` = poll again after a short sleep. `Ok(pressed)` = the section closed;
    /// `pressed` says whether the consent button was pressed on the way.
    pub fn step(
        &mut self,
        controller: &std::sync::Mutex<CdpController>,
    ) -> Option<Result<bool, PostLoginHeldInfo>> {
        let mut c = match controller.lock() {
            Ok(c) => c,
            Err(_) => return Some(Err(PostLoginHeld::CheckFailed.into())),
        };
        let probe = match c.post_login_probe(
            self.own,
            self.login_loader,
            self.idp_origin,
            self.read_origins,
        ) {
            Ok(p) => p,
            Err(_) => return Some(Err(PostLoginHeld::CheckFailed.into())),
        };
        if probe == PostLoginProbe::IdpConsent
            && !self.attempted
            && let Some(consent) = self.consent
        {
            // At most once per login, whatever the outcome.
            self.attempted = true;
            if c.press_consent(self.own, self.idp_origin, consent)
                .unwrap_or(false)
            {
                self.pressed = true;
                self.waiter.restart_stability();
                return None;
            }
        }
        match self.waiter.observe(probe, std::time::Instant::now())? {
            Ok(()) => Some(
                c.resume_after_login(self.own, self.read_origins.to_vec())
                    .map(|()| self.pressed)
                    .map_err(|_| PostLoginHeld::CheckFailed.into()),
            ),
            Err(reason) => {
                let consent_controls = if reason == PostLoginHeld::ConsentRequired {
                    c.consent_controls(self.own, self.idp_origin)
                } else {
                    Vec::new()
                };
                Some(Err(PostLoginHeldInfo {
                    reason,
                    consent_pressed: self.pressed,
                    consent_controls,
                }))
            }
        }
    }
}

/// ADR 2026-10-09 credential username / post-login D2-2 for a blocking caller (the launcher): run
/// a [`PostLoginWait`] probing every 100 ms. `Ok` = the auth section closed (observation resumed);
/// `Err` keeps observation stopped and says why.
pub fn resume_after_login_blocking(
    controller: &std::sync::Arc<std::sync::Mutex<CdpController>>,
    mut wait: PostLoginWait<'_>,
) -> Result<bool, PostLoginHeldInfo> {
    loop {
        if let Some(outcome) = wait.step(controller) {
            return outcome;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the controller reserved for one injection; the broker's sink frame must match it.
struct SinkExpectation<'a> {
    command_id: u64,
    cdp_session: &'a str,
    object_id: &'a str,
    origin: &'a str,
    field: &'a str,
    /// ADR 2026-10-09 credential username / post-login D1-4: the username element the controller
    /// resolved itself; a pair frame must name exactly this object.
    username_object: Option<&'a str>,
}

/// ADR 2026-10-09 付記（launcher の Authenticate 経路）3: the sink frame is exactly
/// `Runtime.callFunctionOn` of credentiald's fixed `INJECT_FUNCTION` on the resolved element in
/// the injection session, with the reserved id and `[origin, 0, field, <string>]`. Anything else
/// (another method, extra keys, another object/session) is refused before it reaches Chrome.
/// The parsed copy of the value is zeroized here.
fn fixed_injection_frame(command: &[u8], want: &SinkExpectation<'_>) -> bool {
    let Some(body) = command.strip_suffix(&[0]) else {
        return false;
    };
    let Ok(mut frame) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let ok = (|| {
        let top = frame.as_object()?;
        let params = top.get("params")?.as_object()?;
        let args = params.get("arguments")?.as_array()?;
        let keys_ok = top.len() == 4
            && ["id", "method", "params", "sessionId"]
                .iter()
                .all(|k| top.contains_key(*k))
            && params.len() == 5
            && [
                "objectId",
                "functionDeclaration",
                "arguments",
                "returnByValue",
                "silent",
            ]
            .iter()
            .all(|k| params.contains_key(*k));
        let arg = |i: usize| -> Option<&Value> {
            let a = args.get(i)?.as_object()?;
            (a.len() == 1).then(|| a.get("value")).flatten()
        };
        let common = keys_ok
            && top.get("id")?.as_u64() == Some(want.command_id)
            && top.get("method")?.as_str() == Some("Runtime.callFunctionOn")
            && top.get("sessionId")?.as_str() == Some(want.cdp_session)
            && params.get("objectId")?.as_str() == Some(want.object_id)
            && params.get("returnByValue")? == &Value::Bool(true)
            && params.get("silent")? == &Value::Bool(true)
            && arg(0)?.as_str() == Some(want.origin)
            && arg(1)?.as_u64() == Some(0);
        let shape = match want.username_object {
            None => {
                params.get("functionDeclaration")?.as_str()
                    == Some(celeris_credentiald::injection_ipc::INJECT_FUNCTION)
                    && args.len() == 4
                    && arg(2)?.as_str() == Some(want.field)
                    && arg(3)?.is_string()
            }
            Some(user) => {
                let by_object = args.get(2)?.as_object()?;
                params.get("functionDeclaration")?.as_str()
                    == Some(celeris_credentiald::injection_ipc::INJECT_PAIR_FUNCTION)
                    && want.field == "password"
                    && args.len() == 5
                    && by_object.len() == 1
                    && by_object.get("objectId")?.as_str() == Some(user)
                    && arg(3)?.is_string()
                    && arg(4)?.is_string()
            }
        };
        Some(common && shape)
    })()
    .unwrap_or(false);
    for at in ["/params/arguments/3/value", "/params/arguments/4/value"] {
        if let Some(Value::String(value)) = frame.pointer_mut(at) {
            value.zeroize();
        }
    }
    ok
}

/// ADR 2026-10-09 credential username / post-login D2-3: page-session commands that neither read
/// the page nor act on its content (domain enables, navigation, emulation, target plumbing). Every
/// other page-session command passes the post-login gate before and after it runs.
fn post_login_control_method(method: &str) -> bool {
    method.starts_with("Target.")
        || method.starts_with("Emulation.")
        || method.starts_with("Browser.")
        || matches!(
            method,
            "Page.enable"
                | "Page.disable"
                | "Page.navigate"
                | "Page.reload"
                | "Page.stopLoading"
                | "Page.getFrameTree"
                | "Page.setLifecycleEventsEnabled"
                | "Page.addScriptToEvaluateOnNewDocument"
                | "Page.removeScriptToEvaluateOnNewDocument"
                | "Page.createIsolatedWorld"
                | "Page.setInterceptFileChooserDialog"
                | "Page.bringToFront"
                | "Page.setBypassCSP"
                | "Page.setDownloadBehavior"
                | "Network.enable"
                | "Network.disable"
                | "Network.setCacheDisabled"
                | "Network.setExtraHTTPHeaders"
                | "Network.setUserAgentOverride"
                | "Network.setBypassServiceWorker"
                | "Runtime.enable"
                | "Runtime.disable"
                | "Runtime.runIfWaitingForDebugger"
                | "Runtime.releaseObject"
                | "Runtime.releaseObjectGroup"
                | "Runtime.discardConsoleEntries"
                | "Log.enable"
                | "Log.disable"
                | "Inspector.enable"
                | "Performance.enable"
                | "Performance.disable"
                | "Security.enable"
                | "Security.disable"
                | "DOM.enable"
                | "DOM.disable"
                | "CSS.enable"
                | "CSS.disable"
                | "Accessibility.enable"
                | "Accessibility.disable"
                | "Overlay.enable"
                | "Overlay.disable"
        )
}

fn broker_denial(reply: &Value) -> InjectionError {
    // Only the broker's fixed ADR-0109 vocabulary may cross this boundary.
    let code = match reply["code"].as_str() {
        Some("invalid_request") => "invalid_request",
        Some("unsupported_version") => "unsupported_version",
        Some("peer_uid_mismatch") => "peer_uid_mismatch",
        Some("injection_worker_not_allowed") => "injection_worker_not_allowed",
        Some("session_not_live") => "session_not_live",
        Some("isolation_required") => "isolation_required",
        Some("target_mismatch") => "target_mismatch",
        Some("empty_frame_chain") => "empty_frame_chain",
        Some("cross_origin_frame") => "cross_origin_frame",
        Some("redirected") => "redirected",
        Some("redisplay_field") => "redisplay_field",
        Some("auth_section_required") => "auth_section_required",
        Some("auth_section_mismatch") => "auth_section_mismatch",
        Some("selector_mismatch") => "selector_mismatch",
        Some("lease_expired") => "lease_expired",
        Some("lease_used") => "lease_used",
        Some("other_session") => "other_session",
        Some("lease_invalid") => "lease_invalid",
        Some("audit_unavailable") => "audit_unavailable",
        Some("provider_failed") => "provider_failed",
        Some("target_changed") => "target_changed",
        Some("sink_failed") => "sink_failed",
        _ => return InjectionError::SinkFailed,
    };
    InjectionError::BrokerRejected(code)
}

fn origin(url: Option<&str>) -> Option<String> {
    let parsed = Url::parse(url?).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port();
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

fn find_frame<'a>(node: &'a Value, id: &str, chain: &mut Vec<&'a Value>) -> bool {
    chain.push(&node["frame"]);
    if node["frame"]["id"] == id {
        return true;
    }
    if let Some(children) = node["childFrames"].as_array() {
        for child in children {
            if find_frame(child, id, chain) {
                return true;
            }
        }
    }
    chain.pop();
    false
}

fn recv_packet(fd: &OwnedFd) -> Result<Vec<u8>, InjectionError> {
    let mut pollfd = nix::libc::pollfd {
        fd: fd.as_raw_fd(),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to one valid descriptor and structure.
    if unsafe { nix::libc::poll(&mut pollfd, 1, TIMEOUT.as_millis() as i32) } <= 0 {
        return Err(InjectionError::SinkFailed);
    }
    let mut buf = vec![0u8; MAX_FRAME];
    // SAFETY: buffer is writable and fd is a connected seqpacket socket.
    let n = unsafe {
        nix::libc::recv(
            fd.as_raw_fd(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            nix::libc::MSG_TRUNC,
        )
    };
    if n <= 0 || n as usize > buf.len() {
        buf.zeroize();
        return Err(InjectionError::SinkFailed);
    }
    buf.truncate(n as usize);
    Ok(buf)
}

fn send_packet(fd: &OwnedFd, bytes: &[u8]) -> Result<(), InjectionError> {
    // SAFETY: bytes is a valid buffer and fd is a connected seqpacket socket.
    let n = unsafe {
        nix::libc::send(
            fd.as_raw_fd(),
            bytes.as_ptr().cast(),
            bytes.len(),
            nix::libc::MSG_NOSIGNAL,
        )
    };
    if n != bytes.len() as isize {
        return Err(InjectionError::SinkFailed);
    }
    Ok(())
}

#[cfg(test)]
mod idle_pump_tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn controller() -> (CdpController, UnixStream) {
        let (controller_end, browser) = UnixStream::pair().expect("socket pair");
        let read = File::from(OwnedFd::from(controller_end.try_clone().expect("clone")));
        let write = File::from(OwnedFd::from(controller_end));
        (CdpController::new(write, read), browser)
    }

    fn login_controller() -> (CdpController, UnixStream) {
        let (mut c, stream) = controller();
        c.open_auth_section("auth".into());
        c.login_navigation = Some(LoginNavigation {
            session: "S".into(),
            frame: Some("F".into()),
            origins: Vec::new(),
            redirects: 0,
            documents: 0,
            invalid: false,
            deadline: std::time::Instant::now() + Duration::from_secs(15),
            waiting: true,
        });
        (c, stream)
    }

    fn document(url: &str) -> Value {
        json!({"method":"Network.requestWillBeSent","sessionId":"S",
            "params":{"type":"Document","frameId":"F","request":{"url":url}}})
    }

    #[test]
    fn browser_trusted_login_sso_origin_history_is_session_and_top_document_scoped() {
        let (mut c, _stream) = login_controller();
        let mut other = document("https://other.test/secret?SAMLRequest=opaque");
        other["sessionId"] = "OTHER".into();
        c.queue_event(other);
        let mut subresource = document("https://other.test/script");
        subresource["params"]["type"] = "Script".into();
        c.queue_event(subresource);
        let mut iframe = document("https://other.test/frame");
        iframe["params"]["frameId"] = "CHILD".into();
        c.queue_event(iframe);
        c.queue_event(document("https://idp.test/entry?SAMLRequest=opaque"));
        let mut redirect = document("https://idp.test/entry?execution=e1s1");
        redirect["params"]["redirectResponse"] =
            json!({"url":"https://idp.test/entry?SAMLRequest=opaque"});
        c.queue_event(redirect);
        c.queue_event(document("https://idp.test/form?execution=e1s2"));
        assert_eq!(
            c.login_redirect_chain("https://idp.test").unwrap(),
            vec!["https://idp.test"; 3]
        );
        assert!(c.take_agent_events().is_empty());
    }

    #[test]
    fn browser_trusted_login_sso_cross_origin_source_and_js_history_stay_rejected() {
        let (mut c, _stream) = login_controller();
        let mut redirect = document("https://idp.test/form");
        redirect["params"]["redirectResponse"] = json!({"url":"https://other.test/entry"});
        c.queue_event(redirect);
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
        let (mut c, _stream) = login_controller();
        c.queue_event(document("https://idp.test/relay"));
        c.queue_event(document("https://other.test/relay"));
        c.queue_event(document("https://idp.test/form"));
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
    }

    #[test]
    fn browser_trusted_login_sso_history_limit_and_invalid_origin_fail_closed() {
        let (mut c, _stream) = login_controller();
        for _ in 0..32 {
            c.queue_event(document("https://idp.test/loop"));
        }
        assert_eq!(
            c.login_redirect_chain("https://idp.test").unwrap().len(),
            32
        );
        for _ in 0..100 {
            c.queue_event(document("https://idp.test/loop"));
        }
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
        assert_eq!(c.login_navigation.as_ref().unwrap().origins.len(), 32);
        let (mut c, _stream) = login_controller();
        c.queue_event(document("not-a-url"));
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
    }

    #[test]
    fn event_queue_drops_oldest_after_4096_entries() {
        let (mut controller, _browser) = controller();
        for index in 0..=MAX_QUEUED_EVENTS {
            controller.queue_event(
                json!({"method":"Page.loadEventFired", "sessionId":"S", "params":{"index":index}}),
            );
        }
        assert_eq!(controller.events.len(), MAX_QUEUED_EVENTS);
        let sessions = ["S".to_owned()].into_iter().collect();
        let events = controller.take_agent_events_for(&sessions);
        assert_eq!(events.len(), MAX_QUEUED_EVENTS);
        assert_eq!(events[0]["params"]["index"], 1);
        assert_eq!(
            events.last().expect("last event")["params"]["index"],
            MAX_QUEUED_EVENTS
        );
    }

    /// 付記 2026-10-10f: the relay's refusals are kept for the launcher's failure line, bounded and
    /// with methods reduced to `Domain.method`; an other-origin download is recorded when cancelled.
    #[test]
    fn agent_denials_are_bounded_fixed_and_include_cancelled_downloads() {
        let (mut controller, _browser) = controller();
        controller.record_agent_denial("Page.captureScreenshot", "observation_origin_denied");
        controller.record_agent_denial("Runtime.evaluate\nhttps://x/?sid=1", "cdp_command_denied");
        assert_eq!(
            controller.take_agent_denials(),
            vec![
                (
                    "Page.captureScreenshot".to_owned(),
                    "observation_origin_denied"
                ),
                ("other".to_owned(), "cdp_command_denied"),
            ]
        );
        assert!(controller.take_agent_denials().is_empty());
        for _ in 0..(MAX_AGENT_DENIALS + 3) {
            controller.record_agent_denial("DOM.getDocument", "password_field_present");
        }
        assert_eq!(controller.take_agent_denials().len(), MAX_AGENT_DENIALS);
        controller.post_login = Some(vec!["https://lms.test".into()]);
        controller.queue_event(json!({"method":"Browser.downloadWillBegin","params":{
            "guid":"g1","url":"https://files.other.test/a.pdf","frameId":"F"}}));
        assert_eq!(
            controller.take_agent_denials(),
            vec![(
                "Browser.downloadWillBegin".to_owned(),
                "download_origin_denied"
            )]
        );
    }

    #[test]
    fn taking_one_session_preserves_another_sessions_events() {
        let (mut controller, _browser) = controller();
        controller.queue_event(json!({"method":"Page.loadEventFired","sessionId":"OTHER"}));
        controller.queue_event(json!({"method":"Page.loadEventFired","sessionId":"S"}));
        let mine = ["S".to_owned()].into_iter().collect();
        let other = ["OTHER".to_owned()].into_iter().collect();
        assert_eq!(controller.take_agent_events_for(&mine).len(), 1);
        assert_eq!(controller.take_agent_events_for(&other).len(), 1);
    }

    fn frame(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut v = json!({
            "id": 77, "sessionId": "S", "method": "Runtime.callFunctionOn",
            "params": {
                "objectId": "OBJ",
                "functionDeclaration": celeris_credentiald::injection_ipc::INJECT_FUNCTION,
                "arguments": [{"value":"https://idp.test"},{"value":0},{"value":"password"},{"value":"s3cret"}],
                "returnByValue": true, "silent": true
            }
        });
        edit(&mut v);
        let mut bytes = serde_json::to_vec(&v).expect("json");
        bytes.push(0);
        bytes
    }

    /// ADR 2026-10-09 付記「launcher の Authenticate 経路」3: only credentiald's fixed injection
    /// call on the resolved element passes the sink; any other frame is refused.
    #[test]
    fn launcher_credential_sink_accepts_only_the_fixed_injection_frame() {
        let want = SinkExpectation {
            command_id: 77,
            cdp_session: "S",
            object_id: "OBJ",
            origin: "https://idp.test",
            field: "password",
            username_object: None,
        };
        assert!(fixed_injection_frame(&frame(|_| {}), &want));
        type Edit = Box<dyn Fn(&mut Value)>;
        let forged: Vec<Edit> = vec![
            Box::new(|v| v["method"] = "Network.getAllCookies".into()),
            Box::new(|v| v["method"] = "Runtime.evaluate".into()),
            Box::new(|v| v["id"] = 78.into()),
            Box::new(|v| v["sessionId"] = "OTHER".into()),
            Box::new(|v| {
                v.as_object_mut().expect("obj").remove("sessionId");
            }),
            Box::new(|v| v["extra"] = 1.into()),
            Box::new(|v| v["params"]["objectId"] = "OTHER".into()),
            Box::new(|v| {
                v["params"]["functionDeclaration"] = "function(){return document.cookie}".into()
            }),
            Box::new(|v| v["params"]["returnByValue"] = false.into()),
            Box::new(|v| v["params"]["expression"] = "1".into()),
            Box::new(|v| v["params"]["arguments"][0]["value"] = "https://evil.test".into()),
            Box::new(|v| v["params"]["arguments"][1]["value"] = 1.into()),
            Box::new(|v| v["params"]["arguments"][2]["value"] = "username".into()),
            Box::new(|v| v["params"]["arguments"][3]["value"] = 5.into()),
            Box::new(|v| v["params"]["arguments"][3]["objectId"] = "X".into()),
            Box::new(|v| {
                v["params"]["arguments"]
                    .as_array_mut()
                    .expect("args")
                    .push(json!({"value":1}))
            }),
        ];
        for (i, edit) in forged.iter().enumerate() {
            assert!(
                !fixed_injection_frame(&frame(|v| edit(v)), &want),
                "forged frame {i} accepted"
            );
        }
        let mut no_nul = frame(|_| {});
        no_nul.pop();
        assert!(!fixed_injection_frame(&no_nul, &want));
        assert!(!fixed_injection_frame(b"not json\0", &want));
    }

    fn pair_frame(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        frame(|v| {
            v["params"]["functionDeclaration"] =
                celeris_credentiald::injection_ipc::INJECT_PAIR_FUNCTION.into();
            v["params"]["arguments"] = json!([
                {"value":"https://idp.test"},{"value":0},{"objectId":"USER"},
                {"value":"student-1"},{"value":"s3cret"}
            ]);
            edit(v);
        })
    }

    /// ADR 2026-10-09 credential username / post-login D1-4: a pair frame passes only with the fixed
    /// pair function, the username element the controller resolved, and two string values.
    #[test]
    fn launcher_credential_sink_accepts_only_the_fixed_pair_injection_frame() {
        let want = SinkExpectation {
            command_id: 77,
            cdp_session: "S",
            object_id: "OBJ",
            origin: "https://idp.test",
            field: "password",
            username_object: Some("USER"),
        };
        assert!(fixed_injection_frame(&pair_frame(|_| {}), &want));
        // The single-field frame is refused where a pair is expected, and vice versa.
        assert!(!fixed_injection_frame(&frame(|_| {}), &want));
        let single = SinkExpectation {
            username_object: None,
            ..want
        };
        assert!(!fixed_injection_frame(&pair_frame(|_| {}), &single));
        type Edit = Box<dyn Fn(&mut Value)>;
        let forged: Vec<Edit> = vec![
            Box::new(|v| v["params"]["arguments"][2] = json!({"objectId":"OTHER"})),
            Box::new(|v| v["params"]["arguments"][2] = json!({"objectId":"USER","value":1})),
            Box::new(|v| v["params"]["arguments"][2] = json!({"value":"USER"})),
            Box::new(|v| v["params"]["arguments"][3] = json!({"value":5})),
            Box::new(|v| v["params"]["arguments"][4] = json!({"objectId":"X"})),
            Box::new(|v| {
                v["params"]["arguments"]
                    .as_array_mut()
                    .expect("args")
                    .push(json!({"value":1}))
            }),
            Box::new(|v| {
                v["params"]["functionDeclaration"] =
                    celeris_credentiald::injection_ipc::INJECT_FUNCTION.into()
            }),
            Box::new(|v| v["params"]["objectId"] = "USER".into()),
            Box::new(|v| v["method"] = "Runtime.evaluate".into()),
        ];
        for (i, edit) in forged.iter().enumerate() {
            assert!(
                !fixed_injection_frame(&pair_frame(|v| edit(v)), &want),
                "forged pair frame {i} accepted"
            );
        }
        let username_field = SinkExpectation {
            field: "username",
            ..want
        };
        assert!(!fixed_injection_frame(&pair_frame(|_| {}), &username_field));
    }

    /// D2-3: only page-session commands that neither read nor act on the page skip the gate.
    #[test]
    fn post_login_gate_skips_only_plumbing_methods() {
        for m in [
            "Page.enable",
            "Page.navigate",
            "Target.attachToTarget",
            "Emulation.setDeviceMetricsOverride",
            "Network.enable",
            "Runtime.enable",
            "Browser.setDownloadBehavior",
        ] {
            assert!(post_login_control_method(m), "{m}");
        }
        for m in [
            "Runtime.evaluate",
            "Runtime.callFunctionOn",
            "DOM.getDocument",
            "DOM.getOuterHTML",
            "Accessibility.getFullAXTree",
            "DOMSnapshot.captureSnapshot",
            "Page.captureScreenshot",
            "Page.printToPDF",
            "Input.dispatchMouseEvent",
            "Input.insertText",
            "Runtime.getProperties",
            "Fetch.continueRequest",
        ] {
            assert!(!post_login_control_method(m), "{m}");
        }
    }

    /// D2-6: after login a download outside `read_origins` is recorded for cancellation; events of
    /// controller-private sessions (login tab, check sessions) are never queued for the agent.
    #[test]
    fn post_login_downloads_outside_read_origins_are_cancelled_and_private_events_dropped() {
        let (mut c, mut browser) = controller();
        c.post_login = Some(vec!["https://lms.test".into()]);
        c.private_sessions.insert("OWN".into());
        c.queue_event(json!({"method":"Browser.downloadWillBegin",
            "params":{"guid":"g1","url":"https://other.test/f.bin"}}));
        c.queue_event(json!({"method":"Page.downloadWillBegin","sessionId":"S",
            "params":{"guid":"g2","url":"blob:https://lms.test/abc"}}));
        c.queue_event(json!({"method":"Browser.downloadWillBegin",
            "params":{"guid":"g3","url":"https://lms.test/ct/a.pdf"}}));
        c.queue_event(json!({"method":"Network.requestWillBeSent","sessionId":"OWN","params":{}}));
        let mut denied: Vec<_> = c.denied_downloads.iter().cloned().collect();
        denied.sort();
        assert_eq!(denied, vec!["g1".to_string(), "g2".to_string()]);
        assert!(c.events.iter().all(|e| e["sessionId"] != "OWN"));
        assert_eq!(c.events.len(), 3);
        // The cancels were written to the browser at once (no reply awaited).
        browser
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("timeout");
        let mut written = Vec::new();
        let mut buf = [0u8; 4096];
        while written.iter().filter(|b| **b == 0).count() < 2 {
            let n = browser.read(&mut buf).expect("cancel written");
            written.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&written);
        assert_eq!(text.matches("Browser.cancelDownload").count(), 2, "{text}");
        assert!(text.contains("\"g1\"") && text.contains("\"g2\""));
        // An allowed download completing is fine; a denied one completing stops observation.
        c.queue_event(
            json!({"method":"Browser.downloadProgress","params":{"guid":"g3","state":"completed"}}),
        );
        assert!(!c.observation_stopped());
        c.queue_event(
            json!({"method":"Browser.downloadProgress","params":{"guid":"g1","state":"completed"}}),
        );
        assert!(c.observation_stopped());
        assert!(c.take_agent_events().is_empty());
    }

    /// 付記 2026-10-10: the wait goes through interstitial / SAML hops, ends early only on a stable
    /// consent page or repeated login form, and reports the last state at the deadline.
    #[test]
    fn post_login_waiter_waits_through_hops_and_names_the_failed_condition() {
        use PostLoginProbe as P;
        let t0 = std::time::Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut w = PostLoginWaiter::new(t0, Duration::from_secs(60), Duration::from_secs(3));
        for (ms, p) in [
            (0, P::LoginDocument),
            (500, P::IdpPage),
            (20_000, P::IdpPage),
            (30_000, P::NoDocument),
            (30_100, P::OtherOrigin),
            (31_000, P::CheckFailed),
        ] {
            assert_eq!(w.observe(p, at(ms)), None, "{p:?}");
        }
        assert_eq!(w.observe(P::Ready, at(40_000)), Some(Ok(())));
        let mut w = PostLoginWaiter::new(t0, Duration::from_secs(60), Duration::from_secs(3));
        assert_eq!(w.observe(P::IdpConsent, at(1_000)), None);
        assert_eq!(w.observe(P::IdpConsent, at(3_000)), None);
        assert_eq!(
            w.observe(P::IdpConsent, at(4_000)),
            Some(Err(PostLoginHeld::ConsentRequired))
        );
        let mut w = PostLoginWaiter::new(t0, Duration::from_secs(60), Duration::from_secs(3));
        assert_eq!(w.observe(P::IdpLoginForm, at(0)), None);
        assert_eq!(w.observe(P::IdpPage, at(2_000)), None);
        assert_eq!(
            w.observe(P::IdpLoginForm, at(3_500)),
            None,
            "stability restarts"
        );
        assert_eq!(
            w.observe(P::IdpLoginForm, at(6_600)),
            Some(Err(PostLoginHeld::IdpLoginForm))
        );
        for (p, held) in [
            (P::IdpPage, PostLoginHeld::IdpTimeout),
            (P::ReadOriginPasswordField, PostLoginHeld::PasswordField),
            (P::OtherOrigin, PostLoginHeld::OtherOrigin),
            (P::LoginDocument, PostLoginHeld::LoginDocument),
        ] {
            let mut w = PostLoginWaiter::new(t0, Duration::from_secs(60), Duration::from_secs(3));
            assert_eq!(w.observe(p, at(59_000)), None);
            assert_eq!(w.observe(p, at(60_000)), Some(Err(held)));
        }
        assert_eq!(
            PostLoginHeld::ConsentRequired.code(),
            "post_login_consent_required"
        );
        assert_eq!(PostLoginHeld::PasswordField.top_origin(), "read_origin");
        assert_eq!(
            serde_json::to_value(PostLoginHeld::IdpTimeout).unwrap(),
            json!("idp_timeout")
        );
    }

    /// 付記 2026-10-10b-4: the consent diagnostics keep only names / kinds / short ASCII values.
    #[test]
    fn consent_controls_are_sanitized_and_bounded() {
        let c = |name: &str, kind: &str, value: Option<&str>| ConsentControl {
            name: name.into(),
            kind: kind.into(),
            value: value.map(String::from),
        };
        let raw = vec![
            c("_eventId_proceed", "submit", Some("Accept")),
            c(
                "_shib_idp_consentOptions",
                "radio",
                Some("_shib_idp_doNotRememberConsent"),
            ),
            c("label", "text", Some("s2026001")),
            c("", "submit", Some("x")),
            c("bad name", "submit", None),
            c(
                "_eventId_AttributeReleaseRejected",
                "submit",
                Some("同意しない"),
            ),
            c("long", "button", Some(&"v".repeat(65))),
        ];
        let kept = sanitize_consent_controls(raw);
        assert_eq!(
            format_consent_controls(&kept),
            "_eventId_proceed=Accept(submit),_shib_idp_consentOptions=_shib_idp_doNotRememberConsent(radio),_eventId_AttributeReleaseRejected(submit),long(button)"
        );
        let many: Vec<_> = (0..40)
            .map(|i| {
                c(
                    &format!("n{i:03}{}", "x".repeat(60)),
                    "radio",
                    Some(&"v".repeat(64)),
                )
            })
            .collect();
        let kept = sanitize_consent_controls(many);
        assert_eq!(kept.len(), CONSENT_CONTROLS_MAX);
        assert!(format_consent_controls(&kept).len() <= 512);
    }

    /// The login submit names the submitter (its `name` / `value` reach the IdP) and falls back to a
    /// click only for a non-submitter.
    #[test]
    fn login_submit_expression_submits_with_the_submitter() {
        let expr = login_submit_expression("button[name=\"_eventId_proceed\"]").expect("expr");
        assert!(expr.contains("e.form.requestSubmit(e)"), "{expr}");
        assert!(expr.contains("catch(_){e.click();}"), "{expr}");
        assert!(!expr.contains("requestSubmit()"), "{expr}");
        assert!(
            expr.contains("document.querySelector(\"button[name=\\\"_eventId_proceed\\\"]\")"),
            "{expr}"
        );
    }

    #[test]
    fn auth_section_discards_pending_events_before_relay_resumes() {
        let (mut controller, mut browser) = controller();
        let sessions = ["S".to_owned()].into_iter().collect();
        controller.open_auth_section("auth".into());
        browser
            .write_all(b"{\"method\":\"Page.loadEventFired\",\"sessionId\":\"S\"}\0")
            .expect("auth event");
        controller.close_auth_section().expect("close auth");
        assert!(controller.take_agent_events_for(&sessions).is_empty());
        browser
            .write_all(b"{\"method\":\"Page.loadEventFired\",\"sessionId\":\"S\"}\0")
            .expect("normal event");
        controller.pump_events().expect("pump after auth");
        assert_eq!(controller.take_agent_events_for(&sessions).len(), 1);
    }
}
