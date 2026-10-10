//! ADR 2026-10-09 credential username / post-login: real Chromium + loopback HTTPS (IdP, LMS and an
//! unrelated origin). The broker side is a test sink that writes credentiald's production pair
//! function; the launcher e2e (`browser_launcher_run_tests.rs`) uses the real credentiald.
use super::sso_tests::ChromeFixture;
use super::*;
use crate::browser_cdp_sink::{
    BrokerClient, CdpController, InjectionError, InjectionRequest, PendingInjection,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Mutex;
use std::time::Instant;

const FIXTURE_SOURCE: &str = include_str!("browser_post_login_fixture.py");
/// The fixture with the other origin's file made unsniffable. The source sends it as
/// `application/octet-stream` with the body 8 seconds after the headers; Chromium sniffs that type
/// and only begins the download once the body arrives, so the controller's cancel races the
/// completion (a lost race is the D2-6 breach, failing the test). An unsniffable type begins the
/// download at the headers, and a 60-second body delay (the event safety net) keeps the cancel first.
pub(crate) static FIXTURE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let mut script = FIXTURE_SOURCE.to_string();
    for (from, to) in [
        (
            "self.send_header('Content-Type', 'application/octet-stream')",
            "self.send_header('Content-Type', 'application/x-celeris-other')",
        ),
        ("threading.Event().wait(8)", "threading.Event().wait(60)"),
    ] {
        assert_eq!(
            script.matches(from).count(),
            1,
            "fixture line {from:?} moved"
        );
        script = script.replace(from, to);
    }
    script
});
pub(crate) const USER: &str = "s2026001";
const SECRET: &str = "post-login-password-5d1c92";
pub(crate) const COOKIE_VALUE: &str = "lms-session-cookie-7f3a";
/// Safety net for download events; the waits end on the event itself (ADR-0125).
const DOWNLOAD_EVENT_TIMEOUT: Duration = Duration::from_secs(60);

/// The fixture's three origins: IdP, LMS, other.
pub(crate) struct Origins {
    pub(crate) idp: String,
    pub(crate) lms: String,
    pub(crate) other: String,
}

pub(crate) fn origins(fx: &ChromeFixture) -> Origins {
    Origins {
        idp: fx.origins[0].clone(),
        lms: fx.origins[1].clone(),
        other: fx.origins[2].clone(),
    }
}

/// The administrator's site policy for the fixture: username and password on one IdP form that is
/// not at the origin root, and a post-login read of the LMS.
pub(crate) fn trusted(o: &Origins, read: &[&str]) -> task_core::browser_wait::TrustedLogin {
    use task_core::browser_wait::{PostLogin, PostLoginAction};
    task_core::browser_wait::TrustedLogin {
        policy_id: "lms-fixture".into(),
        revision: 1,
        login_url: format!("{}/idp/login", o.idp),
        password_selector: "input[name=j_password]".into(),
        submit_selector: Some("button[name=_eventId_proceed]".into()),
        username_selector: Some("input[name=j_username]".into()),
        post_login: Some(PostLogin {
            read_origins: read.iter().map(|s| s.to_string()).collect(),
            actions: PostLoginAction::ALL.to_vec(),
        }),
        consent: None,
    }
}

#[derive(Default)]
struct PairBroker {
    requests: Vec<Value>,
}
struct Pending(std::thread::JoinHandle<Result<Value, InjectionError>>);
impl PendingInjection for Pending {
    fn finish(self: Box<Self>) -> Result<Value, InjectionError> {
        self.0.join().expect("broker thread")
    }
}
impl BrokerClient for PairBroker {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        assert!(!request.to_string().contains(SECRET));
        assert!(!request.to_string().contains(USER));
        self.requests.push(request.clone());
        Ok(Box::new(Pending(std::thread::spawn(move || {
            let mut frame = serde_json::to_vec(&json!({"id":request["cdp_command_id"],
                "sessionId":request["cdp_session_id"],"method":"Runtime.callFunctionOn",
                "params":{"objectId":request["object_id"],
                "functionDeclaration":celeris_credentiald::injection_ipc::INJECT_PAIR_FUNCTION,
                "arguments":[{"value":request["frame_chain"][0]},{"value":0},
                    {"objectId":request["username"]["object_id"]},{"value":USER},{"value":SECRET}],
                "returnByValue":true,"silent":true}}))
            .expect("frame");
            frame.push(0);
            // SAFETY: a connected seqpacket FD and valid buffers.
            let sent =
                unsafe { nix::libc::send(sink.as_raw_fd(), frame.as_ptr().cast(), frame.len(), 0) };
            assert_eq!(sent, frame.len() as isize);
            let mut bytes = [0u8; 4096];
            // SAFETY: as above.
            let n = unsafe {
                nix::libc::recv(sink.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len(), 0)
            };
            assert!(n > 0);
            let response: Value = serde_json::from_slice(&bytes[..n as usize]).expect("reply");
            let ok = response["result"]["result"]["value"] == "ok";
            Ok(if ok {
                json!({"v":1,"request_id":request["request_id"],"ok":true,"receipt":{"injected_at":1,
                    "lease_id":request["lease_id"],"auth_section_id":request["auth_section_id"],
                    "session_id":request["session_id"],"cdp_target_id":request["cdp_target_id"],
                    "frame_id":request["frame_id"],"loader_id":request["loader_id"],
                    "field":request["field"]},
                    "redisplay_guard":celeris_credentiald::injection::RedisplayGuard::new(SECRET).to_wire()})
            } else {
                json!({"v":1,"request_id":request["request_id"],"ok":false,"code":"target_changed"})
            })
        }))))
    }
}

struct World {
    fx: ChromeFixture,
    o: Origins,
    target: String,
    own: String,
}

fn world() -> World {
    let fx = ChromeFixture::start_with(&FIXTURE);
    let o = origins(&fx);
    let (target, own) = {
        let mut c = fx.controller.lock().expect("lock");
        let target = c
            .controller_command("Target.createTarget", json!({"url":"about:blank"}), None)
            .expect("target")["result"]["targetId"]
            .as_str()
            .expect("id")
            .to_owned();
        let own = c
            .controller_command(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )
            .expect("attach")["result"]["sessionId"]
            .as_str()
            .expect("session")
            .to_owned();
        c.open_auth_section("auth".into());
        c.controller_command("Network.enable", json!({}), Some(&own))
            .expect("network");
        (target, own)
    };
    World { fx, o, target, own }
}

impl World {
    fn controller(&self) -> std::sync::MutexGuard<'_, CdpController> {
        self.fx.controller.lock().expect("lock")
    }

    async fn login(
        &self,
        trusted: &task_core::browser_wait::TrustedLogin,
        broker: &mut PairBroker,
    ) -> Result<LoginTab, &'static str> {
        {
            let mut c = self.controller();
            c.begin_login_navigation(&self.own, Duration::from_secs(15))
                .expect("begin");
            c.controller_command(
                "Page.navigate",
                json!({"url":trusted.login_url}),
                Some(&self.own),
            )
            .map_err(|_| "navigation_failed")?;
        }
        let request = InjectionRequest {
            request_id: "request".into(),
            session_id: "test".into(),
            cdp_target_id: self.target.clone(),
            frame_id: String::new(),
            loader_id: String::new(),
            exact_origin: self.o.idp.clone(),
            redirect_chain: Vec::new(),
            selector: trusted.password_selector.clone(),
            field: "password".into(),
            auth_section_id: "auth".into(),
            lease_id: "lease".into(),
            username_selector: trusted.username_selector.clone(),
        };
        let result =
            complete_trusted_login(&self.fx.controller, broker, &self.own, request, trusted).await;
        self.controller()
            .clear_injected_values()
            .map_err(|_| "clear_failed")?;
        result
    }

    fn received(&self) -> String {
        let path = self.fx.directory.path().join("received");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(Instant::now() < deadline, "IdP never received the form");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::read_to_string(path).expect("received")
    }
}

/// The agent side of the relay: every command goes through `agent_command` (what the relay calls).
pub(crate) struct Agent<'a> {
    pub(crate) controller: &'a Arc<Mutex<CdpController>>,
    pub(crate) session: String,
    pub(crate) target: String,
    /// Every reply and event the agent received (for the secret scan).
    pub(crate) seen: Vec<String>,
}

impl<'a> Agent<'a> {
    pub(crate) fn attach(controller: &'a Arc<Mutex<CdpController>>, target: &str) -> Self {
        let reply = controller
            .lock()
            .expect("lock")
            .agent_command(
                "Target.attachToTarget",
                json!({"targetId":target,"flatten":true}),
                None,
            )
            .expect("agent attach");
        let session = reply["result"]["sessionId"]
            .as_str()
            .expect("session")
            .to_owned();
        Self {
            controller,
            session,
            target: target.into(),
            seen: Vec::new(),
        }
    }

    pub(crate) fn cmd(&mut self, method: &str, params: Value) -> Result<Value, InjectionError> {
        let mut c = self.controller.lock().expect("lock");
        let result = c.agent_command(method, params, Some(&self.session));
        let sessions: HashSet<String> = [self.session.clone()].into_iter().collect();
        for mut event in c.take_agent_events_for(&sessions) {
            crate::browser_shared_cdp::sanitize_agent_event(&mut event);
            self.seen.push(event.to_string());
        }
        if let Ok(reply) = &result {
            self.seen.push(reply.to_string());
        }
        result
    }

    pub(crate) fn eval(&mut self, expression: &str) -> Result<Value, InjectionError> {
        self.cmd(
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
        )
        .map(|r| r["result"]["result"]["value"].clone())
    }

    /// The agent's navigation (no page data), then wait for the target to settle on `url` (the
    /// controller reads only the target's URL).
    pub(crate) fn goto(&mut self, url: &str, settles_on: &str) {
        self.cmd("Page.navigate", json!({"url":url}))
            .expect("navigate");
        self.wait_url(settles_on);
    }

    pub(crate) fn wait_url(&mut self, settles_on: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let info = self
                .controller
                .lock()
                .expect("lock")
                .controller_command(
                    "Target.getTargetInfo",
                    json!({"targetId":self.target}),
                    None,
                )
                .expect("target info");
            if info["result"]["targetInfo"]["url"]
                .as_str()
                .is_some_and(|u| u.starts_with(settles_on))
            {
                break;
            }
            assert!(Instant::now() < deadline, "never reached {settles_on}");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Let the document finish (load events are not page data).
        std::thread::sleep(Duration::from_millis(200));
    }

    /// Click the centre of `#id` with real input events (what agent-browser's click does).
    pub(crate) fn click(&mut self, id: &str) -> Result<(), InjectionError> {
        let rect = self.eval(&format!(
            "(()=>{{const r=document.getElementById('{id}').getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2];}})()"
        ))?;
        let (x, y) = (
            rect[0].as_f64().unwrap_or(0.0),
            rect[1].as_f64().unwrap_or(0.0),
        );
        for kind in ["mousePressed", "mouseReleased"] {
            self.cmd(
                "Input.dispatchMouseEvent",
                json!({"type":kind,"x":x,"y":y,"button":"left","clickCount":1}),
            )?;
        }
        Ok(())
    }

    /// Open a fresh tab for this agent (a new target, attached the way the agent attaches).
    pub(crate) fn new_tab(controller: &'a Arc<Mutex<CdpController>>) -> Self {
        let created = controller
            .lock()
            .expect("lock")
            .agent_command("Target.createTarget", json!({"url":"about:blank"}), None)
            .expect("new tab");
        let target = created["result"]["targetId"]
            .as_str()
            .expect("target")
            .to_owned();
        Self::attach(controller, &target)
    }

    /// Start the download behind `#id` with a page-initiated click (in this tab's page) and wait on
    /// the browser's own events for it to begin and then reach `state`. Use a tab that has not
    /// downloaded yet: Chromium's per-tab download limiter silently holds a second download without
    /// fresh user activation (which made the earlier mouse-driven check depend on timing).
    pub(crate) fn download_by_script(&mut self, id: &str, state: &str) -> Result<(), String> {
        let from = self.seen.len();
        self.eval(&format!("document.getElementById('{id}').click()"))
            .map_err(|e| format!("click refused: {}", e.code()))?;
        let guid = self
            .wait_download_begin(from, &[])
            .ok_or_else(|| format!("the download never began: {:?}", self.tail(6)))?;
        if self.wait_download(from, &guid, state) {
            Ok(())
        } else {
            Err(format!(
                "no {state} after the download began: {:?}",
                self.tail(6)
            ))
        }
    }

    /// Wait for the first event since `from` (an index into `seen`) that `pick` maps to a value.
    /// Events are browser-level (no page data) and carry no session, so they reach the agent's
    /// queue; the controller has no other consumer here. Waits on events (ADR-0125): the bound is
    /// only a long safety net, and a stopped observation (D2-6 breach) ends the wait at once.
    fn wait_event<T>(&mut self, from: usize, pick: impl Fn(&Value) -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + DOWNLOAD_EVENT_TIMEOUT;
        let mut next = from.min(self.seen.len());
        loop {
            while next < self.seen.len() {
                let found = serde_json::from_str::<Value>(&self.seen[next])
                    .ok()
                    .and_then(|v| pick(&v));
                next += 1;
                if found.is_some() {
                    return found;
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            let mut c = self.controller.lock().expect("lock");
            let _ = c.pump_events();
            let sessions: HashSet<String> = [self.session.clone()].into_iter().collect();
            let events = c.take_agent_events_for(&sessions);
            let stopped = c.observation_stopped();
            drop(c);
            let idle = events.is_empty();
            for mut event in events {
                crate::browser_shared_cdp::sanitize_agent_event(&mut event);
                self.seen.push(event.to_string());
            }
            if stopped && idle {
                return None;
            }
            if idle {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Wait for a download to begin among the events received since `from` and return its guid
    /// (`Browser.downloadWillBegin`, or the first progress of a guid not in `known`).
    pub(crate) fn wait_download_begin(&mut self, from: usize, known: &[&str]) -> Option<String> {
        self.wait_event(from, |v| {
            let guid = v["params"]["guid"].as_str()?;
            match v["method"].as_str()? {
                "Browser.downloadWillBegin" | "Page.downloadWillBegin" => Some(guid.to_owned()),
                "Browser.downloadProgress" | "Page.downloadProgress" if !known.contains(&guid) => {
                    Some(guid.to_owned())
                }
                _ => None,
            }
        })
    }

    /// Wait for a progress event of the download `guid` reaching `state` (searched from `from`, so
    /// an event that arrived with the begin is not missed).
    pub(crate) fn wait_download(&mut self, from: usize, guid: &str, state: &str) -> bool {
        self.wait_event(from, |v| {
            (matches!(
                v["method"].as_str(),
                Some("Browser.downloadProgress" | "Page.downloadProgress")
            ) && v["params"]["guid"] == guid
                && v["params"]["state"] == state)
                .then_some(())
        })
        .is_some()
    }

    /// The last `n` events the agent received (for failure messages).
    pub(crate) fn tail(&self, n: usize) -> Vec<String> {
        self.seen
            .iter()
            .rev()
            .take(n)
            .map(|e| e.chars().take(300).collect())
            .collect()
    }
}

fn code(r: Result<Value, InjectionError>) -> &'static str {
    match r {
        Ok(_) => "ok",
        Err(e) => e.code(),
    }
}

/// D1: username and password go in with one pair injection; D2: the auth section closes only on
/// the LMS without a password field, then every observation is checked (read origins only, no
/// password field, redisplay guard), downloads outside the read origins are cancelled, and neither
/// the password nor the session cookie reaches the agent. The username shown by the LMS is readable.
#[tokio::test]
async fn daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages() {
    let w = world();
    let read = [w.o.lms.as_str()];
    let trusted = trusted(&w.o, &read);
    let mut broker = PairBroker::default();
    let tab = w.login(&trusted, &mut broker).await.expect("login");
    assert_eq!(w.received(), format!("{USER}\n{SECRET}"));
    assert_eq!(broker.requests.len(), 1);
    let req = &broker.requests[0];
    assert_eq!(req["v"], 2);
    assert_eq!(req["field"], "password");
    assert_eq!(req["username"]["selector"], "input[name=j_username]");
    assert_eq!(req["username"]["input_type"], "text");
    assert_ne!(req["username"]["object_id"], req["object_id"]);
    // Still H3: no agent command before the close conditions hold.
    assert_eq!(
        code(
            w.controller()
                .agent_command("Target.getTargets", json!({}), None)
        ),
        "auth_section_required"
    );
    let owned: Vec<String> = read.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        await_post_login(
            &w.fx.controller,
            &tab,
            &w.o.idp,
            &owned,
            None,
            POST_LOGIN_TIMEOUT
        )
        .await,
        Ok(false),
        "post-login conditions (no consent page, nothing pressed)"
    );
    assert!(w.controller().post_login_active());
    assert!(!w.controller().auth_section_active());

    let download_dir = tempfile::tempdir().expect("downloads");
    let mut agent = Agent::attach(&w.fx.controller, &w.target);
    agent.cmd("Network.enable", json!({})).expect("network");
    // extract / snapshot / screenshot on the LMS.
    let text = agent.eval("document.body.innerText").expect("extract");
    let text = text.as_str().unwrap_or_default().to_owned();
    assert!(text.contains("Report 1: Fluid dynamics essay"), "{text}");
    assert!(
        text.contains(&format!("Signed in as {USER}")),
        "username is readable: {text}"
    );
    assert_eq!(
        code(agent.cmd("Accessibility.getFullAXTree", json!({}))),
        "ok"
    );
    let shot = agent
        .cmd("Page.captureScreenshot", json!({"format":"png"}))
        .expect("screenshot");
    assert!(
        shot["result"]["data"]
            .as_str()
            .is_some_and(|d| d.len() > 100)
    );
    // click within the LMS.
    agent.click("report").expect("click");
    agent.wait_url(&format!("{}/ct/report_1", w.o.lms));
    let detail = agent.eval("document.body.innerText").expect("detail");
    assert!(
        detail
            .as_str()
            .is_some_and(|t| t.contains("Report 1 detail"))
    );
    // download from the LMS completes; one from another origin is cancelled.
    agent.goto(
        &format!("{}/ct/home", w.o.lms),
        &format!("{}/ct/home", w.o.lms),
    );
    w.controller()
        .agent_command(
            "Browser.setDownloadBehavior",
            json!({"behavior":"allowAndName","downloadPath":download_dir.path(),"eventsEnabled":true}),
            None,
        )
        .expect("download behavior");
    let from = agent.seen.len();
    agent.click("dl").expect("download click");
    let lms = agent
        .wait_download_begin(from, &[])
        .unwrap_or_else(|| panic!("LMS download began: {:?}", agent.tail(6)));
    assert!(
        agent.wait_download(from, &lms, "completed"),
        "LMS download completed: {:?}",
        agent.tail(6)
    );
    let files: Vec<_> = std::fs::read_dir(download_dir.path())
        .expect("dir")
        .flatten()
        .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
        .collect();
    assert_eq!(files, vec!["%PDF-1.4 handout for report 1".to_string()]);
    // Deterministic trigger: navigate the tab to the other origin's attachment and wait on the
    // browser's own download events (no timing guess, no click hit-testing).
    // An agent navigation straight to another origin's file is refused (a navigation download
    // raises no download event, so it could not be cancelled) …
    let mut tab2 = Agent::new_tab(&w.fx.controller);
    assert_eq!(
        code(tab2.cmd(
            "Page.navigate",
            json!({"url":format!("{}/files/other.bin", w.o.other)})
        )),
        "observation_origin_denied"
    );
    // … and a page-initiated one from a read-origin page is cancelled. A fresh tab's first download
    // is not held by Chromium's per-tab download limiter, and the waits are on the browser's own
    // download events.
    tab2.goto(
        &format!("{}/ct/home", w.o.lms),
        &format!("{}/ct/home", w.o.lms),
    );
    tab2.download_by_script("dl-other", "canceled")
        .expect("other-origin download cancelled");
    assert_eq!(
        std::fs::read_dir(download_dir.path()).expect("dir").count(),
        1
    );

    // Refused: the IdP, another allowed origin, a page with a password field.
    // After login the agent navigates only to read origins …
    for url in [format!("{}/", w.o.idp), format!("{}/page", w.o.other)] {
        assert_eq!(
            code(agent.cmd("Page.navigate", json!({"url":url}))),
            "observation_origin_denied"
        );
    }
    // … but a read-origin page may redirect anywhere; nothing there is readable.
    agent.goto(&format!("{}/ct/go_idp", w.o.lms), &format!("{}/", w.o.idp));
    assert_eq!(
        code(agent.eval("document.body.innerText")),
        "observation_origin_denied"
    );
    assert_eq!(
        code(agent.cmd("Page.captureScreenshot", json!({}))),
        "observation_origin_denied"
    );
    assert_eq!(
        code(agent.cmd(
            "Input.dispatchMouseEvent",
            json!({"type":"mousePressed","x":5,"y":5,"button":"left","clickCount":1})
        )),
        "observation_origin_denied"
    );
    agent.goto(
        &format!("{}/ct/go_other", w.o.lms),
        &format!("{}/page", w.o.other),
    );
    assert_eq!(
        code(agent.eval("document.title")),
        "observation_origin_denied"
    );
    agent.goto(
        &format!("{}/ct/settings", w.o.lms),
        &format!("{}/ct/settings", w.o.lms),
    );
    assert_eq!(code(agent.eval("document.title")), "password_field_present");
    assert_eq!(
        code(agent.cmd("Page.captureScreenshot", json!({}))),
        "password_field_present"
    );
    // A page that re-displays the password is dropped whole (ADR-0111).
    agent.goto(
        &format!("{}/ct/leak", w.o.lms),
        &format!("{}/ct/leak", w.o.lms),
    );
    assert_eq!(
        code(agent.eval("document.body.innerText")),
        "redisplay_detected"
    );
    // Session expiry sends the tab back to the IdP form: nothing is readable there.
    agent.goto(
        &format!("{}/ct/logout", w.o.lms),
        &format!("{}/idp/profile/SAML2/Unsolicited/SSO", w.o.idp),
    );
    assert_eq!(
        code(agent.eval("document.body.innerText")),
        "observation_origin_denied"
    );

    let seen = agent.seen.join("\n");
    // The controller's own sessions (login tab, check sessions) were never announced to the agent.
    assert!(!seen.contains(&tab.own), "login tab session leaked");
    for event in &agent.seen {
        let v: Value = serde_json::from_str(event).expect("json");
        if v["method"] == "Target.attachedToTarget" {
            assert_eq!(
                v["params"]["sessionId"], agent.session,
                "a controller-only attach reached the agent"
            );
        }
    }
    assert!(!seen.contains(SECRET), "password reached the agent");
    assert!(
        !seen.contains(COOKIE_VALUE),
        "session cookie reached the agent"
    );
    assert!(seen.contains("Report 1"));
}

/// D2-2 / 付記 2026-10-10: the auth section stays closed to observation when the login does not
/// land on a read origin without a password field, and the held reason names the condition: an
/// IdP consent page (ends early), the IdP's login form again (ends early), a password field on the
/// landing page, a landing origin that is not a read origin.
#[tokio::test]
async fn daemon_post_login_unconfirmed_keeps_observation_stopped_and_names_the_reason() {
    use crate::browser_cdp_sink::PostLoginHeld;
    for (case, timeout, want) in [
        (
            "consent",
            Duration::from_secs(30),
            PostLoginHeld::ConsentRequired,
        ),
        (
            "reject",
            Duration::from_secs(30),
            PostLoginHeld::IdpLoginForm,
        ),
        (
            "landing_pw",
            Duration::from_secs(4),
            PostLoginHeld::PasswordField,
        ),
        (
            "other_origin",
            Duration::from_secs(4),
            PostLoginHeld::OtherOrigin,
        ),
    ] {
        let w = world();
        if case != "other_origin" {
            std::fs::write(w.fx.directory.path().join(case), "1").expect("flag");
        }
        let read = if case == "other_origin" {
            vec![w.o.other.clone()]
        } else {
            vec![w.o.lms.clone()]
        };
        let refs: Vec<&str> = read.iter().map(String::as_str).collect();
        let trusted = trusted(&w.o, &refs);
        let mut broker = PairBroker::default();
        let tab = w
            .login(&trusted, &mut broker)
            .await
            .expect("login submitted");
        assert_eq!(w.received(), format!("{USER}\n{SECRET}"), "{case}");
        let started = Instant::now();
        assert_eq!(
            await_post_login(&w.fx.controller, &tab, &w.o.idp, &read, None, timeout)
                .await
                .map_err(|held| held.reason),
            Err(want),
            "{case}"
        );
        if matches!(
            want,
            PostLoginHeld::ConsentRequired | PostLoginHeld::IdpLoginForm
        ) {
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "{case} ends early"
            );
        }
        let mut c = w.controller();
        assert!(c.auth_section_active(), "{case}");
        assert!(!c.post_login_active(), "{case}");
        assert_eq!(
            code(c.agent_command("Target.getTargets", json!({}), None)),
            "auth_section_required",
            "{case}"
        );
        assert!(c.take_agent_events().is_empty(), "{case}");
    }
}

/// 付記 2026-10-10 (production 2026-10-10): a Shibboleth-shaped flow — localStorage interstitial
/// auto-POST before the form, then a localStorage write interstitial and a SAML auto-POST after
/// it, each post-login hop taking 9 s — still closes the section once the LMS is reached (the old
/// 15 s limit would have held it).
#[tokio::test]
async fn daemon_post_login_waits_through_slow_shibboleth_hops() {
    let w = world();
    std::fs::write(w.fx.directory.path().join("slow_ms"), "9000").expect("slow");
    let read = vec![w.o.lms.clone()];
    let trusted = trusted(&w.o, &[w.o.lms.as_str()]);
    let mut broker = PairBroker::default();
    let tab = w.login(&trusted, &mut broker).await.expect("login");
    assert_eq!(w.received(), format!("{USER}\n{SECRET}"));
    let started = Instant::now();
    await_post_login(
        &w.fx.controller,
        &tab,
        &w.o.idp,
        &read,
        None,
        POST_LOGIN_TIMEOUT,
    )
    .await
    .expect("reaches the LMS through the IdP hops");
    assert!(
        started.elapsed() > Duration::from_secs(15),
        "the hops took longer than 15 s"
    );
    assert!(w.controller().post_login_active());
    let mut agent = Agent::attach(&w.fx.controller, &w.target);
    let text = agent.eval("document.body.innerText").expect("extract");
    assert!(text.as_str().is_some_and(|t| t.contains("Report 1")));
}

/// 付記 2026-10-10b: with the pinned consent button, the controller presses it exactly once on the
/// IdP's consent page (choosing the one-time option), the login continues to the LMS and the agent
/// reads it. The agent cannot act while the consent page is up (the auth section is open), and the
/// controller never presses on a page that is not the IdP's.
#[tokio::test]
async fn daemon_post_login_presses_the_pinned_consent_button_once() {
    use task_core::browser_wait::ConsentPolicy;
    let w = world();
    std::fs::write(w.fx.directory.path().join("consent"), "1").expect("flag");
    let read = vec![w.o.lms.clone()];
    let mut trusted = trusted(&w.o, &[w.o.lms.as_str()]);
    let consent = ConsentPolicy {
        selector: "input[name=_eventId_proceed]".into(),
        choice_selector: Some(
            "input[name=_shib_idp_consentOptions][value=_shib_idp_doNotRememberConsent]".into(),
        ),
    };
    trusted.consent = Some(consent.clone());
    let mut broker = PairBroker::default();
    let tab = w.login(&trusted, &mut broker).await.expect("login");
    // Wait for the consent page (no press yet: only the post-login wait presses).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let probe = w
            .controller()
            .post_login_probe(&tab.own, &tab.loader, &w.o.idp, &read)
            .expect("probe");
        if probe == crate::browser_cdp_sink::PostLoginProbe::IdpConsent {
            break;
        }
        assert!(Instant::now() < deadline, "consent page never shown");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        code(
            w.controller()
                .agent_command("Target.getTargets", json!({}), None)
        ),
        "auth_section_required",
        "the agent cannot reach the consent page"
    );
    // Never on another origin's page.
    assert_eq!(
        w.controller().press_consent(&tab.own, &w.o.lms, &consent),
        Ok(false)
    );
    assert!(!w.fx.directory.path().join("consent_posts").exists());
    assert_eq!(
        await_post_login(
            &w.fx.controller,
            &tab,
            &w.o.idp,
            &read,
            Some(&consent),
            POST_LOGIN_TIMEOUT
        )
        .await,
        Ok(true)
    );
    assert_eq!(
        std::fs::read_to_string(w.fx.directory.path().join("consent_posts")).expect("posts"),
        "_eventId_proceed _shib_idp_doNotRememberConsent\n",
        "pressed exactly once, with the one-time option"
    );
    let mut agent = Agent::attach(&w.fx.controller, &w.target);
    let text = agent.eval("document.body.innerText").expect("extract");
    assert!(text.as_str().is_some_and(|t| t.contains("Report 1")));
}

/// 付記 2026-10-10b: a consent page that comes back after the one press, a selector that matches
/// nothing, or no consent setting at all ends with `consent_required`; the diagnostics carry only
/// the consent form's control names / kinds / values — no labels, no user data, no secrets.
#[tokio::test]
async fn daemon_post_login_consent_not_pressed_twice_and_diagnostics_hold_no_page_data() {
    use crate::browser_cdp_sink::PostLoginHeld;
    use task_core::browser_wait::ConsentPolicy;
    for (case, consent, posts) in [
        (
            "again",
            Some(ConsentPolicy {
                selector: "input[name=_eventId_proceed]".into(),
                choice_selector: None,
            }),
            1,
        ),
        (
            "mismatch",
            Some(ConsentPolicy {
                selector: "#no-such-button".into(),
                choice_selector: None,
            }),
            0,
        ),
        ("unset", None, 0),
    ] {
        let w = world();
        std::fs::write(w.fx.directory.path().join("consent"), "1").expect("flag");
        std::fs::write(w.fx.directory.path().join("consent_again"), "1").expect("flag");
        let read = vec![w.o.lms.clone()];
        let mut trusted = trusted(&w.o, &[w.o.lms.as_str()]);
        trusted.consent = consent.clone();
        let mut broker = PairBroker::default();
        let tab = w.login(&trusted, &mut broker).await.expect("login");
        let held = await_post_login(
            &w.fx.controller,
            &tab,
            &w.o.idp,
            &read,
            consent.as_ref(),
            Duration::from_secs(30),
        )
        .await
        .expect_err(case);
        assert_eq!(held.reason, PostLoginHeld::ConsentRequired, "{case}");
        assert_eq!(held.consent_pressed, posts == 1, "{case}");
        let recorded = std::fs::read_to_string(w.fx.directory.path().join("consent_posts"))
            .unwrap_or_default();
        assert_eq!(recorded.lines().count(), posts, "{case}: at most one press");
        let line = crate::browser_cdp_sink::format_consent_controls(&held.consent_controls);
        assert_eq!(
            line,
            "_shib_idp_consentOptions=_shib_idp_doNotRememberConsent(radio),\
_shib_idp_consentOptions=_shib_idp_rememberConsent(radio),\
_eventId_AttributeReleaseRejected=Reject(submit),_eventId_proceed=Accept(submit)",
            "{case}"
        );
        for leak in [
            USER,
            SECRET,
            "Ask me again",
            "Information",
            "uid",
            "@u.example",
            "127.0.0.1",
        ] {
            assert!(!line.contains(leak), "{case}: {leak} in {line}");
        }
        assert!(w.controller().auth_section_active(), "{case}");
    }
}

// ---- 2026-10-10: the real agent-browser 0.38.1 through the relay after login ----

/// The agent-browser 0.38.1 the sandbox runs (skip when this host does not have it).
fn agent_browser_binary() -> Option<std::path::PathBuf> {
    let candidates = [
        std::env::var_os("CELERIS_TEST_AGENT_BROWSER").map(std::path::PathBuf::from),
        std::env::var_os("HOME").map(|h| {
            std::path::PathBuf::from(h)
                .join(".local/celeris/npm/agent-browser-0.38.1/node_modules/.bin/agent-browser")
        }),
    ];
    candidates.into_iter().flatten().find(|p| {
        std::process::Command::new(p)
            .arg("--version")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "agent-browser 0.38.1")
    })
}

/// agent-browser driving the shared Chromium through the controller's relay (as in the sandbox:
/// `--cdp ws://127.0.0.1:<port>/<token>`, the shim's fixed argv shape).
struct RealAgentBrowser {
    exe: std::path::PathBuf,
    dir: tempfile::TempDir,
    /// A short socket dir: agent-browser's session socket path must fit in sun_path.
    sockets: tempfile::TempDir,
    endpoint: String,
    _relay: crate::browser_shared_cdp::SharedCdp,
}

impl RealAgentBrowser {
    fn start(
        exe: std::path::PathBuf,
        controller: &Arc<Mutex<CdpController>>,
        domains: Vec<String>,
        allow: &[&str],
    ) -> Self {
        let dir = tempfile::tempdir().expect("agent dir");
        let sockets = tempfile::Builder::new()
            .prefix("ab")
            .tempdir_in("/tmp")
            .expect("socket dir");
        let token = "ab".repeat(32);
        let socket = sockets.path().join("relay.sock");
        let relay = crate::browser_shared_cdp::SharedCdp::start_shared(
            Arc::clone(controller),
            &socket,
            token.clone(),
            domains,
            0o600,
        )
        .expect("relay");
        // TCP → unix bridge (the sandbox's 127.0.0.1:9223).
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").expect("bridge");
        let port = tcp.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for conn in tcp.incoming().flatten() {
                let Ok(unix) = std::os::unix::net::UnixStream::connect(&socket) else {
                    continue;
                };
                let (mut a, mut b) = (conn, unix);
                let (mut a2, mut b2) =
                    (a.try_clone().expect("clone"), b.try_clone().expect("clone"));
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut a, &mut b);
                    let _ = b.shutdown(std::net::Shutdown::Write);
                });
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut b2, &mut a2);
                    let _ = a2.shutdown(std::net::Shutdown::Write);
                });
            }
        });
        std::fs::create_dir_all(dir.path().join("home")).expect("home");
        std::fs::write(
            dir.path().join("upstream.json"),
            br#"{"idleTimeout":"5m","noWebmcp":true}"#,
        )
        .expect("upstream");
        let mut allow: Vec<&str> = allow.to_vec();
        allow.extend(["launch", "close"]);
        std::fs::write(
            dir.path().join("policy.json"),
            serde_json::to_vec(&json!({"default":"deny","allow":allow})).expect("policy"),
        )
        .expect("policy");
        Self {
            exe,
            endpoint: format!("ws://127.0.0.1:{port}/{token}"),
            dir,
            sockets,
            _relay: relay,
        }
    }

    /// One shim-shaped command; returns (exit status, stdout).
    fn run(&self, action: &[&str]) -> (i32, String) {
        let d = self.dir.path();
        let out = std::process::Command::new(&self.exe)
            .current_dir(d)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", d.join("home"))
            .env("AGENT_BROWSER_NAMESPACE", "celeris-test")
            .env("AGENT_BROWSER_SOCKET_DIR", self.sockets.path())
            // No traffic leaves the host: any proxy use fails closed.
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .args(["--config"])
            .arg(d.join("upstream.json"))
            .args(["--session", "celeris-test"])
            .arg("--action-policy")
            .arg(d.join("policy.json"))
            .args(["--cdp", &self.endpoint])
            .args(["--content-boundaries", "--max-output", "16000", "--json"])
            .args(action)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("agent-browser runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    }
}

impl Drop for RealAgentBrowser {
    fn drop(&mut self) {
        let _ = self.run(&["close"]);
        // agent-browser keeps a per-session daemon until its idle timeout; stop this test's daemon
        // (found by its private socket dir in its environment, so nothing else is touched).
        let marker = format!("AGENT_BROWSER_SOCKET_DIR={}", self.sockets.path().display());
        for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<i32>().ok())
            else {
                continue;
            };
            let ours = std::fs::read(entry.path().join("environ"))
                .is_ok_and(|env| env.split(|b| *b == 0).any(|kv| kv == marker.as_bytes()));
            if ours {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
        }
    }
}

/// 付記 2026-10-10c (production run 01M4J3S5706DNVEZA5EASGV10C): the sandbox's real agent-browser
/// 0.38.1, with the shim's argv shape, through the controller's relay after the login. On the LMS
/// home page with a collapsed (hidden, empty) login widget, snapshot lists the links with their URLs,
/// click follows a link, extract and screenshot work; a page with a visible password field, a hidden
/// password field that holds a value, the IdP and another origin are refused. Skips when this host has
/// no agent-browser 0.38.1.
#[tokio::test]
async fn real_agent_browser_reads_clicks_and_is_refused_after_login() {
    let Some(exe) = agent_browser_binary() else {
        eprintln!("SKIP: agent-browser 0.38.1 not installed");
        return;
    };
    let w = world();
    let read = vec![w.o.lms.clone()];
    let trusted = trusted(&w.o, &[w.o.lms.as_str()]);
    let mut broker = PairBroker::default();
    let tab = w.login(&trusted, &mut broker).await.expect("login");
    await_post_login(
        &w.fx.controller,
        &tab,
        &w.o.idp,
        &read,
        None,
        POST_LOGIN_TIMEOUT,
    )
    .await
    .expect("resumed");
    let ab = RealAgentBrowser::start(
        exe,
        &w.fx.controller,
        vec![w.o.idp.clone(), w.o.lms.clone(), w.o.other.clone()],
        &[
            "navigate",
            "snapshot",
            "gettext",
            "click",
            "screenshot",
            "scroll",
        ],
    );
    let lms = |p: &str| format!("{}{p}", w.o.lms);
    let ok = |(code, out): (i32, String)| {
        assert_eq!(code, 0, "{out}");
        let v: Value = serde_json::from_str(out.trim()).expect("json");
        assert_eq!(v["success"], true, "{out}");
        v
    };
    let refused = |(code, out): (i32, String)| {
        assert_ne!(code, 0, "{out}");
        out
    };
    ok(ab.run(&["open", &lms("/ct/home")]));
    // The shim's snapshot argv: interactive refs with link URLs.
    let snap = ok(ab.run(&["snapshot", "-i", "--urls"]));
    let tree = snap["data"]["snapshot"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(tree.contains("Report 1: Fluid dynamics essay"), "{tree}");
    assert!(
        tree.contains(&lms("/ct/report_1")),
        "link URL in the snapshot: {tree}"
    );
    let report_ref = snap["data"]["refs"]
        .as_object()
        .expect("refs")
        .iter()
        .find(|(_, r)| r["name"] == "Report 1: Fluid dynamics essay")
        .map(|(k, _)| format!("@{k}"))
        .expect("report ref");
    let before = w.controller().agent_log.len();
    ok(ab.run(&["click", &report_ref]));
    let clicked: Vec<String> = w.controller().agent_log[before..].to_vec();
    assert!(
        clicked.iter().any(|m| m == "Input.dispatchMouseEvent"),
        "agent-browser's click path passed the post-login gate: {clicked:?}"
    );
    assert!(!clicked.iter().any(|m| m.contains('!')), "{clicked:?}");
    let detail = ok(ab.run(&["snapshot", "-i", "--urls"]));
    assert_eq!(detail["data"]["origin"], lms("/ct/report_1"));
    let heading = detail["data"]["refs"]
        .as_object()
        .expect("refs")
        .keys()
        .next()
        .map(|k| format!("@{k}"))
        .expect("a ref");
    let text = ok(ab.run(&["get", "text", &heading]));
    assert!(text.to_string().contains("Report 1 detail"), "{text}");
    let shot = ab.dir.path().join("shot.png");
    ok(ab.run(&["screenshot", &shot.to_string_lossy()]));
    assert!(std::fs::metadata(&shot).is_ok_and(|m| m.len() > 100));
    // Refused pages.
    // `open` of another origin is refused outright after login.
    refused(ab.run(&["open", &format!("{}/page", w.o.other)]));
    for (page, why) in [
        (lms("/ct/settings"), "password_field_present"),
        (lms("/ct/autofilled"), "password_field_present"),
        (lms("/ct/go_idp"), "observation_origin_denied"),
        (lms("/ct/go_other"), "observation_origin_denied"),
    ] {
        ok(ab.run(&["open", &page]));
        let before = w.controller().agent_log.len();
        let out = refused(ab.run(&["snapshot", "-i", "--urls"]));
        assert!(!out.contains("filled-by-page"), "{out}");
        let log: Vec<String> = w.controller().agent_log[before..].to_vec();
        assert!(
            log.iter().any(|m| m.ends_with(&format!("!{why}"))),
            "{page}: {why} expected in {log:?}"
        );
        let shot = ab.dir.path().join("refused.png");
        refused(ab.run(&["screenshot", &shot.to_string_lossy()]));
    }
}
