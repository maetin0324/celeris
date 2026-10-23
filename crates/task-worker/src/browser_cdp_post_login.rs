//! Post-login observation, consent and the gate for resuming agent access.
use super::{CdpController, InjectionError, origin};
use serde_json::{Value, json};
use std::time::Duration;

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

    pub(super) fn from_probe(probe: PostLoginProbe) -> Self {
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

impl CdpController {
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
    pub(super) fn idp_markers(
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
    pub(super) fn check_session(&mut self, target: &str) -> Result<String, InjectionError> {
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
    pub(super) fn password_inputs(
        &mut self,
        session: &str,
        frame_id: &str,
    ) -> Result<u64, InjectionError> {
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
    pub(super) fn post_login_gate(&mut self, agent_session: &str) -> Result<(), InjectionError> {
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
