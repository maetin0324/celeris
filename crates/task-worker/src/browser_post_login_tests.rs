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
    // A fresh document avoids Chromium's multiple-download limiter hiding the cancellation.
    agent.goto(
        &format!("{}/ct/home", w.o.lms),
        &format!("{}/ct/home", w.o.lms),
    );
    let from = agent.seen.len();
    agent.click("dl-other").expect("click other download");
    let other = agent.wait_download_begin(from, &[&lms]).unwrap_or_else(|| {
        panic!(
            "other-origin download began (observation stopped: {}): {:?}",
            w.controller().observation_stopped(),
            agent.tail(6)
        )
    });
    assert!(
        agent.wait_download(from, &other, "canceled"),
        "other-origin download cancelled (observation stopped: {}): {:?}",
        w.controller().observation_stopped(),
        agent.tail(6)
    );
    assert_eq!(
        std::fs::read_dir(download_dir.path()).expect("dir").count(),
        1
    );

    // Refused: the IdP, another allowed origin, a page with a password field.
    agent.goto(&format!("{}/", w.o.idp), &format!("{}/", w.o.idp));
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
        &format!("{}/page", w.o.other),
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
