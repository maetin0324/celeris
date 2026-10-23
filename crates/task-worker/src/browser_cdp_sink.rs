//! ADR-0109 D4: controller owned CDP pipe and one way broker sink.
//!
//! The controller never receives a credential in an IPC response. The broker
//! writes one CDP command to a private seqpacket socket; its bytes are forwarded
//! to the browser without decoding or logging them. Only the CDP reply goes back
//! to the broker. The caller gets a receipt with a fixed, non-secret shape.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::Duration;

use celeris_credentiald::injection::RedisplayGuard;
use celeris_credentiald::injection_ipc::{
    Field, InjectionReply, InjectionRequest as WireRequest, PAIR_REQUEST_VERSION, UsernameTarget,
};
use serde_json::{Value, json};
use url::Url;
use zeroize::Zeroize;

use crate::browser_relay;

#[path = "browser_cdp_credential_ipc.rs"]
mod credential_ipc;
pub use credential_ipc::*;
#[path = "browser_cdp_post_login.rs"]
mod post_login;
pub use post_login::*;

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
/// 付記 2026-10-10i: how long an agent command (and its post-login checks) may wait for Chrome.
/// While a page waits for a navigation's response (a slow PDF), Chrome answers commands on that
/// page only after the response arrives; 5 s failed a download click on a slow server.
/// agent-browser's own command timeout is longer, so the relay answers first.
const AGENT_TIMEOUT: Duration = Duration::from_secs(25);
/// 付記 2026-10-10m: the controller's own world for a download it fetches (page JS cannot reach it).
const FETCH_WORLD: &str = "celeris-download";
/// How long the controller's fetch of a link may take (the launcher's action deadline is longer).
const FETCH_TIMEOUT: Duration = Duration::from_secs(40);
/// Bytes per read of a fetched file out of the page (base64 in one CDP reply).
const FETCH_CHUNK: u64 = 1 << 20;
/// The element (or its closest `<a href>` ancestor): its absolute href, or null.
const LINK_HREF_FN: &str = "function(){const a=this.closest&&this.closest('a[href]');\
return a&&typeof a.href==='string'?a.href:null}";
/// Fetch `href` with the page's same-origin credentials, counting the body against `max`; the
/// bytes stay in this world until read out in chunks. Only fixed fields return. `same-origin` mode
/// makes a redirect to another origin a failure before any request there; a first `manual` request
/// tells such a failure (`redirect`) from a network error.
const FETCH_FN: &str = "async function(href,max,origins){const opt={credentials:'same-origin',mode:'same-origin'};\
let r;try{r=await fetch(href,Object.assign({redirect:'manual'},opt));}catch(e){return {e:'network'};}\
if(r.type==='opaqueredirect'){try{r=await fetch(href,Object.assign({redirect:'follow'},opt));}\
catch(e){return {e:'redirect'};}}const m={url:r.url,status:r.status};\
let o0='';try{o0=new URL(r.url).origin;}catch(e){}if(!origins.includes(o0)){try{r.body&&r.body.cancel();}\
catch(e){}return Object.assign(m,{e:'redirect'});}\
if(!r.ok)return Object.assign(m,{e:'status'});const reader=r.body.getReader();const parts=[];let n=0;\
for(;;){const x=await reader.read();if(x.done)break;n+=x.value.length;if(n>max){try{reader.cancel();}\
catch(e){}return Object.assign(m,{e:'size'});}parts.push(x.value);}const out=new Uint8Array(n);let o=0;\
for(const p of parts){out.set(p,o);o+=p.length;}globalThis.__celerisDownload=out;\
return Object.assign(m,{size:n});}";
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
    /// True while an agent command (and its post-login checks) runs: replies may take
    /// [`AGENT_TIMEOUT`], and a timeout records what Chrome announced meanwhile.
    agent_call: bool,
    /// 付記 2026-10-10m: a longer reply bound for the controller's own fetch.
    call_timeout: Option<Duration>,
    /// Armed by the launcher before the runner reads a download's ref: the agent session and
    /// `backendNodeId` of the first element agent-browser resolves.
    ref_capture: Option<Option<(String, i64)>>,
    /// Document requests of agent pages still waiting for their response (request ids, at most
    /// 64): a reply timeout says whether a navigation was pending (付記 2026-10-10i).
    pending_documents: std::collections::HashSet<String>,
    /// 付記 2026-10-10j: downloads Chrome completed that the post-login rules let through (guids,
    /// at most 64). The launcher adopts a file Chrome saved on its own only if its guid is here.
    completed_downloads: std::collections::HashSet<String>,
    /// What Chrome did during one launcher action (fixed tokens), while the launcher watches.
    action_watch: Option<Vec<&'static str>>,
    /// Page targets created during the watched action (to close the extra tab of an adopted
    /// download).
    watch_targets: Vec<String>,
    /// The watched action is a `download`: a new tab its click opens is followed in the opener.
    download_watch: bool,
    /// The page (target, or session if unknown), session and time of the last dialog answered.
    last_dialog: Option<(String, String, std::time::Instant)>,
    /// Agent sessions whose dialog the controller answered and whose closing is still to come.
    answered_dialogs: std::collections::HashSet<String>,
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
            agent_call: false,
            call_timeout: None,
            ref_capture: None,
            pending_documents: std::collections::HashSet::new(),
            completed_downloads: std::collections::HashSet::new(),
            action_watch: None,
            watch_targets: Vec::new(),
            download_watch: false,
            last_dialog: None,
            answered_dialogs: std::collections::HashSet::new(),
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
        // 付記 2026-10-10m: the element the runner's ref read resolved (the download's link).
        if method == "DOM.resolveNode"
            && let Some(None) = self.ref_capture
            && let (Some(node), Some(s)) = (params["backendNodeId"].as_i64(), session)
        {
            self.ref_capture = Some(Some((s.to_owned(), node)));
        }
        self.agent_call = true;
        let result = self.agent_command_inner(method, params, session);
        self.agent_call = false;
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

    /// 付記 2026-10-10h: a JavaScript dialog on an agent page blocks the page — and the agent's
    /// `Input.dispatchMouseEvent` that opened it does not answer until the dialog closes. The relay
    /// runs one agent command at a time, so agent-browser could never send its own
    /// `Page.handleJavaScriptDialog`: the click timed out (`sink_failed`) and the tab stayed blocked.
    /// The controller answers the dialog itself, with agent-browser's default: `alert` and
    /// `beforeunload` are accepted, `confirm` and `prompt` are dismissed (a site's confirmation is
    /// never given on the agent's behalf). Only the dialog type is recorded, never its text; the
    /// opening event is not passed on (the dialog is already answered). The controller's own
    /// (private) sessions are left alone. Returns whether the dialog was answered here.
    fn resolve_agent_dialog(&mut self, event: &Value) -> bool {
        let Some(session) = event["sessionId"].as_str().map(str::to_owned) else {
            return false;
        };
        if self.private_sessions.contains(&session) {
            return false;
        }
        // The same dialog is announced on every agent session attached to its page (agent-browser
        // can hold more than one): answer and record it once (付記 2026-10-10j).
        let page = self
            .agent_targets
            .get(&session)
            .cloned()
            .unwrap_or_else(|| session.clone());
        let now = std::time::Instant::now();
        // Another session of the same page announcing the dialog just answered (a second dialog
        // on one session is a new dialog).
        if self.answered_dialogs.len() >= 64 {
            self.answered_dialogs.clear();
        }
        self.answered_dialogs.insert(session.clone());
        if self.last_dialog.as_ref().is_some_and(|(p, s, at)| {
            *p == page && *s != session && now.duration_since(*at) < Duration::from_secs(1)
        }) {
            return true;
        }
        self.last_dialog = Some((page, session.clone(), now));
        let (accept, code) = match event["params"]["type"].as_str() {
            Some("alert") => (true, "dialog_accepted_alert"),
            Some("beforeunload") => (true, "dialog_accepted_beforeunload"),
            Some("confirm") => (false, "dialog_dismissed_confirm"),
            Some("prompt") => (false, "dialog_dismissed_prompt"),
            _ => (false, "dialog_dismissed_other"),
        };
        self.send_untracked_on(
            "Page.handleJavaScriptDialog",
            json!({"accept": accept}),
            Some(&session),
        );
        self.record_agent_denial("Page.javascriptDialogOpening", code);
        true
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

    /// Write a CDP command without waiting for its reply (the reply is dropped as unsolicited).
    /// Used from the event path, where no call can be made. A failed write stops observation.
    fn send_untracked(&mut self, method: &str, params: Value) {
        self.send_untracked_on(method, params, None);
    }

    /// [`Self::send_untracked`] on a page session.
    fn send_untracked_on(&mut self, method: &str, params: Value, session: Option<&str>) {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut message = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let sent = serde_json::to_vec(&message)
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
        self.watch_event(&value);
        if value["method"] == "Page.javascriptDialogOpening" && self.resolve_agent_dialog(&value) {
            return;
        }
        // The closing of a dialog the controller answered is not passed on either: agent-browser
        // never saw it open (付記 2026-10-10l).
        if value["method"] == "Page.javascriptDialogClosed"
            && let Some(session) = value["sessionId"].as_str()
            && self.answered_dialogs.remove(session)
        {
            return;
        }
        self.track_document_request(&value);
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
        } else if matches!(
            value["method"].as_str(),
            Some("Browser.downloadProgress" | "Page.downloadProgress")
        ) && value["params"]["state"] == "completed"
            && let Some(guid) = value["params"]["guid"].as_str()
        {
            if self.completed_downloads.len() >= 64 {
                self.completed_downloads.clear();
            }
            self.completed_downloads.insert(guid.to_owned());
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
        let timeout = if let Some(long) = self.call_timeout {
            long
        } else if self.agent_call {
            timeout.max(AGENT_TIMEOUT)
        } else {
            timeout
        };
        // 付記 2026-10-10i: what Chrome announced while this reply was awaited (fixed tokens,
        // recorded only if the wait times out).
        let mut seen: Vec<&'static str> = Vec::new();
        let deadline = std::time::Instant::now() + timeout;
        let deadline = self
            .login_navigation
            .as_ref()
            .filter(|nav| nav.waiting)
            .map_or(deadline, |nav| deadline.min(nav.deadline));
        loop {
            if std::time::Instant::now() >= deadline {
                self.record_reply_timeout(&seen);
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
                if self.agent_call
                    && let Some(token) = wait_event_token(&value)
                    && !seen.contains(&token)
                {
                    seen.push(token);
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
                self.record_reply_timeout(&seen);
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

    /// 付記 2026-10-10m: remember the next element agent-browser resolves (the runner reads the
    /// download's ref right after this).
    pub fn arm_ref_capture(&mut self) {
        self.ref_capture = Some(None);
    }

    /// 付記 2026-10-10m: fetch the captured link's href in the page's context. `Err` is a fixed
    /// token; `href_missing` / `ref_unresolved` mean the caller may click instead.
    pub fn fetch_captured_download(&mut self, max: u64) -> Result<Vec<u8>, &'static str> {
        let captured = self.ref_capture.take().flatten();
        let read_origins = self.post_login.clone().ok_or("not_after_login")?;
        let (session, node) = captured.ok_or("ref_unresolved")?;
        let target = self
            .agent_targets
            .get(&session)
            .cloned()
            .ok_or("ref_unresolved")?;
        if self.post_login_gate(&session).is_err() {
            return Err("page_denied");
        }
        let check = self.check_session(&target).map_err(|_| "page_denied")?;
        let tree = self
            .call("Page.getFrameTree", json!({}), Some(&check))
            .map_err(|_| "page_denied")?;
        let frame = tree["result"]["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or("page_denied")?
            .to_owned();
        let world = self
            .call(
                "Page.createIsolatedWorld",
                json!({"frameId": frame, "worldName": FETCH_WORLD}),
                Some(&check),
            )
            .map_err(|_| "page_denied")?;
        let context = world["result"]["executionContextId"]
            .as_i64()
            .ok_or("page_denied")?;
        let resolved = self
            .call(
                "DOM.resolveNode",
                json!({"backendNodeId": node, "executionContextId": context}),
                Some(&check),
            )
            .map_err(|_| "ref_unresolved")?;
        let object = resolved["result"]["object"]["objectId"]
            .as_str()
            .ok_or("ref_unresolved")?
            .to_owned();
        let href = self
            .call(
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": LINK_HREF_FN,
                       "returnByValue": true, "silent": true}),
                Some(&check),
            )
            .map_err(|_| "ref_unresolved")?;
        let Some(href) = href["result"]["result"]["value"]
            .as_str()
            .map(str::to_owned)
        else {
            return Err("href_missing");
        };
        if origin(Some(&href)).is_none_or(|o| !read_origins.contains(&o)) {
            return Err("origin_denied");
        }
        self.call_timeout = Some(FETCH_TIMEOUT);
        let fetched = self.call(
            "Runtime.callFunctionOn",
            json!({"executionContextId": context, "functionDeclaration": FETCH_FN,
                   "arguments": [{"value": href}, {"value": max}, {"value": read_origins}],
                   "awaitPromise": true, "returnByValue": true, "silent": true}),
            Some(&check),
        );
        self.call_timeout = None;
        let fetched = fetched.map_err(|_| "fetch_failed_timeout")?;
        if !fetched["result"]["exceptionDetails"].is_null() {
            return Err("fetch_failed_script");
        }
        let meta = &fetched["result"]["result"]["value"];
        let result = (|| {
            match meta["e"].as_str() {
                Some("network") => return Err("fetch_failed_network"),
                Some("size") => return Err("size_exceeded"),
                Some("redirect") => return Err("redirect_denied"),
                Some("status") => {
                    return Err(match meta["status"].as_u64().unwrap_or(0) / 100 {
                        1 => "fetch_failed_1xx",
                        3 => "fetch_failed_3xx",
                        4 => "fetch_failed_4xx",
                        5 => "fetch_failed_5xx",
                        _ => "fetch_failed_other",
                    });
                }
                Some(_) => return Err("fetch_failed_other"),
                None => {}
            }
            if origin(meta["url"].as_str()).is_none_or(|o| !read_origins.contains(&o)) {
                return Err("redirect_denied");
            }
            let size = meta["size"].as_u64().ok_or("fetch_failed_other")?;
            if size > max {
                return Err("size_exceeded");
            }
            let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
            let mut offset = 0u64;
            while offset < size {
                let chunk = self
                    .call(
                        "Runtime.evaluate",
                        json!({"expression": format!(
                            "(()=>{{const b=globalThis.__celerisDownload.subarray({offset},{end});\
                             let s='';for(let i=0;i<b.length;i+=32768)\
                             s+=String.fromCharCode.apply(null,b.subarray(i,i+32768));return btoa(s);}})()",
                            end = offset + FETCH_CHUNK
                        ), "contextId": context, "returnByValue": true, "silent": true}),
                        Some(&check),
                    )
                    .map_err(|_| "fetch_failed_read")?;
                let data = chunk["result"]["result"]["value"]
                    .as_str()
                    .ok_or("fetch_failed_read")?;
                use base64::Engine as _;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| "fetch_failed_read")?;
                if decoded.is_empty() {
                    return Err("fetch_failed_read");
                }
                offset += decoded.len() as u64;
                bytes.extend_from_slice(&decoded);
            }
            if bytes.len() as u64 != size {
                return Err("fetch_failed_read");
            }
            Ok(bytes)
        })();
        let _ = self.call(
            "Runtime.evaluate",
            json!({"expression": "delete globalThis.__celerisDownload", "contextId": context,
                   "silent": true}),
            Some(&check),
        );
        result
    }

    /// 付記 2026-10-10j: start recording what Chrome does during one launcher action.
    pub fn begin_action_watch(&mut self, download: bool) {
        self.action_watch = Some(Vec::new());
        self.watch_targets.clear();
        self.download_watch = download;
    }

    /// The fixed tokens recorded since [`Self::begin_action_watch`] (`none` if Chrome announced
    /// nothing worth reporting); the watch ends.
    pub fn take_action_watch(&mut self) -> Vec<&'static str> {
        self.download_watch = false;
        let seen = self.action_watch.take().unwrap_or_default();
        if seen.is_empty() { vec!["none"] } else { seen }
    }

    /// Whether Chrome completed the download `guid` and the post-login rules let it through (no
    /// breach in this session).
    pub fn download_completed_allowed(&self, guid: &str) -> bool {
        !self.download_breach
            && self.completed_downloads.contains(guid)
            && !self.denied_downloads.contains(guid)
    }

    /// Close the page targets the watched action opened (the extra tab of a download that a link
    /// started in a new tab). Agent targets the agent attached to stay untouched otherwise.
    pub fn close_watch_targets(&mut self) {
        for target in std::mem::take(&mut self.watch_targets) {
            let _ = self.call("Target.closeTarget", json!({"targetId": target}), None);
        }
    }

    fn watch_event(&mut self, event: &Value) {
        if self.action_watch.is_none() {
            return;
        }
        let params = &event["params"];
        let token = match event["method"].as_str() {
            Some("Page.frameRequestedNavigation" | "Page.frameStartedNavigating") => {
                "navigation_requested"
            }
            Some("Page.navigatedWithinDocument") => "same_document_navigation",
            Some("Page.frameNavigated") if params["frame"]["parentId"].is_null() => {
                let url = params["frame"]["url"].as_str().unwrap_or_default();
                if url.starts_with("chrome-extension:")
                    || params["frame"]["mimeType"] == "application/pdf"
                {
                    "viewer_page"
                } else {
                    "frame_navigated"
                }
            }
            Some("Network.requestWillBeSent") if params["type"] == "Document" => "document_request",
            Some("Network.responseReceived") if params["type"] == "Document" => "document_response",
            Some("Browser.downloadWillBegin" | "Page.downloadWillBegin") => "download_will_begin",
            Some("Browser.downloadProgress" | "Page.downloadProgress") => {
                match params["state"].as_str() {
                    Some("completed") => "download_completed",
                    Some("canceled") => "download_canceled",
                    _ => return,
                }
            }
            Some("Page.windowOpen") => self.download_in_opener(event),
            Some("Target.targetCreated") if params["targetInfo"]["type"] == "page" => {
                let opened_by_page = params["targetInfo"]["openerId"].is_string();
                if let Some(id) = params["targetInfo"]["targetId"].as_str() {
                    if self.download_watch && opened_by_page {
                        // The link's download is started in the opener (download_in_opener); the
                        // new tab would download it a second time, out of the controller's sight.
                        self.send_untracked("Target.closeTarget", json!({"targetId": id}));
                    } else if self.watch_targets.len() < 8 {
                        self.watch_targets.push(id.to_owned());
                    }
                }
                if opened_by_page {
                    "new_tab_from_page"
                } else {
                    "new_tab"
                }
            }
            Some("Target.targetDestroyed") => "target_destroyed",
            Some("Page.javascriptDialogOpening") => "dialog_opening",
            _ => return,
        };
        if let Some(seen) = self.action_watch.as_mut()
            && !seen.contains(&token)
            && seen.len() < 16
        {
            seen.push(token);
        }
    }

    /// 付記 2026-10-10j: during a `download`, a link that opens a new tab (`target=_blank`,
    /// `window.open`) would download in a tab neither agent-browser nor the controller observes:
    /// agent-browser waits on its own page until its timeout, and the controller cannot apply the
    /// read_origins rule to the download. The controller closes the new tab (above) and moves the
    /// opener page itself to the link's URL by script (a renderer-initiated navigation: the download
    /// raises its events on the agent's page, agent-browser saves it, and a download from outside
    /// `read_origins` is cancelled as before). Only an http(s) URL on a read origin (after login) is
    /// followed; nothing of the URL is recorded.
    fn download_in_opener(&mut self, event: &Value) -> &'static str {
        if !self.download_watch {
            return "window_open";
        }
        let Some(session) = event["sessionId"].as_str() else {
            return "window_open_unattributed";
        };
        if self.private_sessions.contains(session) {
            return "window_open_unattributed";
        }
        let url = event["params"]["url"].as_str().unwrap_or_default();
        let allowed = match (origin(Some(url)), &self.post_login) {
            (None, _) => false,
            (Some(o), Some(read_origins)) => read_origins.contains(&o),
            (Some(_), None) => true,
        };
        if !allowed {
            return "window_open_origin_denied";
        }
        let Ok(literal) = serde_json::to_string(url) else {
            return "window_open_origin_denied";
        };
        let session = session.to_owned();
        self.send_untracked_on(
            "Runtime.evaluate",
            json!({"expression": format!("window.location.assign({literal})"), "silent": true}),
            Some(&session),
        );
        "window_open_followed_in_opener"
    }

    /// Follow agent pages' document requests until their response (or failure) arrives.
    fn track_document_request(&mut self, event: &Value) {
        let Some(request) = event["params"]["requestId"].as_str() else {
            return;
        };
        match event["method"].as_str() {
            Some("Network.requestWillBeSent")
                if event["params"]["type"] == "Document"
                    && event["sessionId"]
                        .as_str()
                        .is_some_and(|s| !self.private_sessions.contains(s)) =>
            {
                if self.pending_documents.len() >= 64 {
                    self.pending_documents.clear();
                }
                self.pending_documents.insert(request.to_owned());
            }
            Some(
                "Network.responseReceived" | "Network.loadingFailed" | "Network.loadingFinished",
            ) => {
                self.pending_documents.remove(request);
            }
            _ => {}
        }
    }

    /// 付記 2026-10-10i: an agent command's reply (or one of its post-login checks) did not come in
    /// time. Record that, whether a document response was still pending, and what Chrome announced
    /// meanwhile, as fixed tokens.
    fn record_reply_timeout(&mut self, seen: &[&'static str]) {
        if !self.agent_call {
            return;
        }
        self.record_agent_denial("Controller.reply", "cdp_reply_timeout");
        if !self.pending_documents.is_empty() {
            self.record_agent_denial("Controller.seenWhileWaiting", "document_response_pending");
        }
        for token in seen {
            self.record_agent_denial("Controller.seenWhileWaiting", token);
        }
    }
}

/// The fixed token for an event worth reporting when a reply times out (付記 2026-10-10i).
fn wait_event_token(event: &Value) -> Option<&'static str> {
    let document = event["params"]["type"] == "Document";
    match event["method"].as_str()? {
        "Page.frameRequestedNavigation" | "Page.frameStartedNavigating" => {
            Some("navigation_requested")
        }
        "Page.frameStartedLoading" => Some("frame_started_loading"),
        "Network.requestWillBeSent" if document => Some("document_request"),
        "Network.responseReceived" if document => Some("document_response"),
        "Browser.downloadWillBegin" | "Page.downloadWillBegin" => Some("download_will_begin"),
        "Browser.downloadProgress" | "Page.downloadProgress" => Some("download_progress"),
        "Target.targetCreated" | "Target.attachedToTarget" => Some("target_created"),
        "Page.javascriptDialogOpening" => Some("dialog_opening"),
        "Page.frameNavigated" => Some("frame_navigated"),
        _ => None,
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
#[path = "browser_cdp_sink/idle_pump_tests.rs"]
mod idle_pump_tests;
