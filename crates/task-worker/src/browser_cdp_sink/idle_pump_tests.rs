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

/// 付記 2026-10-10h: a dialog on an agent page is answered by the controller (alert and
/// beforeunload accepted, confirm and prompt dismissed), recorded by type only and not passed
/// on; a dialog on the controller's own (private) session is left alone.
#[test]
fn agent_page_dialogs_are_answered_with_fixed_defaults_and_private_ones_left_alone() {
    use std::io::Read as _;
    let (mut c, mut browser) = controller();
    browser
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    let mut answers = Vec::new();
    for (kind, accept, code) in [
        ("alert", true, "dialog_accepted_alert"),
        ("beforeunload", true, "dialog_accepted_beforeunload"),
        ("confirm", false, "dialog_dismissed_confirm"),
        ("prompt", false, "dialog_dismissed_prompt"),
    ] {
        c.queue_event(
            json!({"method":"Page.javascriptDialogOpening","sessionId":"S",
            "params":{"type":kind,"message":"secret-ish page text","url":"https://lms.test/"}}),
        );
        let mut frame = Vec::new();
        let mut byte = [0u8; 1];
        while browser.read_exact(&mut byte).is_ok() && byte[0] != 0 {
            frame.push(byte[0]);
        }
        let sent: Value = serde_json::from_slice(&frame).expect("answer");
        assert_eq!(sent["method"], "Page.handleJavaScriptDialog");
        assert_eq!(sent["sessionId"], "S");
        assert_eq!(sent["params"], json!({"accept": accept}));
        answers.push(("Page.javascriptDialogOpening".to_owned(), code));
    }
    assert_eq!(c.take_agent_denials(), answers);
    assert!(
        c.take_agent_events().is_empty(),
        "opening events are not passed on"
    );
    c.queue_event(
        json!({"method":"Page.javascriptDialogClosed","sessionId":"S",
        "params":{"result":true,"userInput":""}}),
    );
    assert!(
        c.take_agent_events().is_empty(),
        "nor the closing of an answered dialog"
    );
    c.queue_event(
        json!({"method":"Page.javascriptDialogClosed","sessionId":"S",
        "params":{"result":true,"userInput":""}}),
    );
    assert_eq!(
        c.take_agent_events().len(),
        1,
        "a closing the controller did not cause passes"
    );
    c.private_sessions.insert("OWN".into());
    c.queue_event(
        json!({"method":"Page.javascriptDialogOpening","sessionId":"OWN",
        "params":{"type":"alert"}}),
    );
    assert!(c.take_agent_denials().is_empty());
    browser
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("timeout");
    let mut byte = [0u8; 1];
    assert!(
        browser.read_exact(&mut byte).is_err(),
        "nothing sent for a private session"
    );
}

/// 付記 2026-10-10i: a timed-out agent reply records fixed tokens: the timeout, whether an agent
/// page's document response was pending (a private session's is not counted), and what Chrome
/// announced meanwhile. Outside an agent command nothing is recorded.
#[test]
fn agent_reply_timeout_records_pending_navigation_as_fixed_tokens() {
    let (mut c, _browser) = controller();
    c.private_sessions.insert("OWN".into());
    c.queue_event(
        json!({"method":"Network.requestWillBeSent","sessionId":"OWN",
        "params":{"requestId":"r0","type":"Document","request":{"url":"https://idp.test/"}}}),
    );
    c.record_reply_timeout(&["navigation_requested"]);
    assert!(
        c.take_agent_denials().is_empty(),
        "not during an agent command"
    );
    c.agent_call = true;
    c.record_reply_timeout(&[]);
    assert_eq!(
        c.take_agent_denials(),
        vec![("Controller.reply".to_owned(), "cdp_reply_timeout")],
        "a private session's request is not an agent navigation"
    );
    c.queue_event(json!({"method":"Network.requestWillBeSent","sessionId":"S",
        "params":{"requestId":"r1","type":"Document","request":{"url":"https://lms.test/a.pdf?sid=1"}}}));
    assert_eq!(
        wait_event_token(
            &json!({"method":"Network.requestWillBeSent","params":{"type":"Document"}})
        ),
        Some("document_request")
    );
    c.record_reply_timeout(&["navigation_requested", "document_request"]);
    let denials = c.take_agent_denials();
    assert_eq!(
        denials,
        vec![
            ("Controller.reply".to_owned(), "cdp_reply_timeout"),
            (
                "Controller.seenWhileWaiting".to_owned(),
                "document_response_pending"
            ),
            (
                "Controller.seenWhileWaiting".to_owned(),
                "navigation_requested"
            ),
            ("Controller.seenWhileWaiting".to_owned(), "document_request"),
        ]
    );
    assert!(!format!("{denials:?}").contains("sid="));
    c.queue_event(json!({"method":"Network.responseReceived","sessionId":"S",
        "params":{"requestId":"r1","type":"Document"}}));
    c.record_reply_timeout(&[]);
    assert_eq!(
        c.take_agent_denials(),
        vec![("Controller.reply".to_owned(), "cdp_reply_timeout")],
        "the response arrived"
    );
}

fn read_frame(browser: &mut UnixStream) -> Option<Value> {
    use std::io::Read as _;
    let mut frame = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        browser.read_exact(&mut byte).ok()?;
        if byte[0] == 0 {
            return serde_json::from_slice(&frame).ok();
        }
        frame.push(byte[0]);
    }
}

/// 付記 2026-10-10j: during a watched `download`, a new tab opened by the agent's page is
/// closed and its URL followed in the opener by script, only on a read origin after login;
/// what happened is recorded as fixed tokens. Outside a download watch nothing is sent.
#[test]
fn download_watch_follows_a_new_tab_in_the_opener_on_read_origins_only() {
    let (mut c, mut browser) = controller();
    browser
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("timeout");
    c.post_login = Some(vec!["https://lms.test".into()]);
    let open = |url: &str| {
        json!({"method":"Page.windowOpen","sessionId":"S",
            "params":{"url":url,"windowName":"_blank","windowFeatures":[],"userGesture":true}})
    };
    let created = json!({"method":"Target.targetCreated","params":{"targetInfo":{
        "targetId":"NEW","type":"page","url":"","openerId":"T"}}});
    // Not watched: the events pass, nothing is sent.
    c.queue_event(open("https://lms.test/ct/a.pdf"));
    assert!(read_frame(&mut browser).is_none());
    c.begin_action_watch(true);
    c.queue_event(open("https://lms.test/ct/page_1/Slides1.pdf?view=full"));
    let sent = read_frame(&mut browser).expect("follow in the opener");
    assert_eq!(sent["method"], "Runtime.evaluate");
    assert_eq!(sent["sessionId"], "S");
    assert_eq!(
        sent["params"]["expression"],
        r#"window.location.assign("https://lms.test/ct/page_1/Slides1.pdf?view=full")"#
    );
    c.queue_event(created.clone());
    let closed = read_frame(&mut browser).expect("close the new tab");
    assert_eq!(closed["method"], "Target.closeTarget");
    assert_eq!(closed["params"]["targetId"], "NEW");
    // Another origin is not followed (the new tab is still closed).
    c.queue_event(open("https://files.other.test/x.pdf"));
    c.queue_event(open("javascript:alert(1)"));
    c.queue_event(created);
    let closed = read_frame(&mut browser).expect("close");
    assert_eq!(closed["method"], "Target.closeTarget");
    assert!(read_frame(&mut browser).is_none(), "nothing else sent");
    assert_eq!(
        c.take_action_watch(),
        vec![
            "window_open_followed_in_opener",
            "new_tab_from_page",
            "window_open_origin_denied"
        ]
    );
    // A click watch records the new tab and sends nothing.
    c.begin_action_watch(false);
    c.queue_event(open("https://lms.test/ct/a.pdf"));
    assert!(read_frame(&mut browser).is_none());
    assert_eq!(c.take_action_watch(), vec!["window_open"]);
    c.begin_action_watch(true);
    assert_eq!(c.take_action_watch(), vec!["none"]);
}

/// 付記 2026-10-10j: an adopted download counts only if Chrome completed it, it was not
/// cancelled for its origin, and no breach stopped the session.
#[test]
fn adopted_downloads_must_have_completed_through_the_post_login_rules() {
    let (mut c, _browser) = controller();
    c.post_login = Some(vec!["https://lms.test".into()]);
    let progress = |guid: &str| json!({"method":"Browser.downloadProgress","params":{"guid":guid,"state":"completed"}});
    c.queue_event(progress("g-ok"));
    assert!(c.download_completed_allowed("g-ok"));
    assert!(!c.download_completed_allowed("g-unknown"));
    c.queue_event(json!({"method":"Browser.downloadWillBegin","params":{
        "guid":"g-other","url":"https://files.other.test/x.pdf","frameId":"F"}}));
    assert!(!c.download_completed_allowed("g-other"));
    c.queue_event(progress("g-other"));
    assert!(!c.download_completed_allowed("g-other"));
    assert!(
        !c.download_completed_allowed("g-ok"),
        "a breach stops every adoption"
    );
}

/// 付記 2026-10-10m: the href fetch runs only after login and only for an element the runner's
/// ref read resolved; otherwise the caller falls back (no request is sent).
#[test]
fn fetch_href_needs_login_and_a_captured_element() {
    let (mut c, _browser) = controller();
    c.arm_ref_capture();
    assert_eq!(c.fetch_captured_download(1 << 20), Err("not_after_login"));
    c.post_login = Some(vec!["https://lms.test".into()]);
    assert_eq!(
        c.fetch_captured_download(1 << 20),
        Err("ref_unresolved"),
        "not armed"
    );
    c.arm_ref_capture();
    assert_eq!(
        c.fetch_captured_download(1 << 20),
        Err("ref_unresolved"),
        "nothing resolved"
    );
    c.ref_capture = Some(Some(("UNKNOWN".into(), 7)));
    assert_eq!(
        c.fetch_captured_download(1 << 20),
        Err("ref_unresolved"),
        "no agent target"
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
