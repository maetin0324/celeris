use super::*;
use serde_json::json;
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use task_worker::browser::SUPPORTED_VERSION;

fn fixture(dir: &std::path::Path) -> (DoctorConfig, SqliteStore) {
    let config_file = dir.join("config.toml");
    std::fs::write(
        &config_file,
        "[db]\npath = 'test.db'\n[[providers]]\nid = 'fake-local'\nadapter = 'fake'\n",
    )
    .unwrap();
    let config = Config::load(&config_file).unwrap();
    let store = SqliteStore::open(&config.db.path).unwrap();
    let doctor = DoctorConfig {
        config,
        release: Some("123456abcdef".into()),
        ledger: Some(dir.join("conformance.json")),
        agent_browser: dir.join("agent-browser"),
        bwrap: dir.join("bwrap"),
        bin_dir: dir.to_path_buf(),
        resolver: None,
    };
    (doctor, store)
}

fn status<'a>(report: &'a BrowserReadiness, check: &str) -> &'a str {
    report
        .items
        .iter()
        .find(|i| i.check == check)
        .unwrap()
        .status
        .as_str()
}

fn install_ledger(doctor: &DoctorConfig) {
    use task_core::browser_backend::{FixtureCase, required_evidence};
    let evidence: Vec<_> = [
        FixtureCase::IsolationSuite,
        FixtureCase::EgressNegativeSuite,
        FixtureCase::InjectionAttackSuite,
        FixtureCase::AuthSectionObservationStop,
    ]
    .into_iter()
    .flat_map(|case| {
        required_evidence(case)
            .into_iter()
            .map(move |test| json!({"case":case,"test":test,"outcome":"passed","runtime": if matches!(case, FixtureCase::IsolationSuite | FixtureCase::EgressNegativeSuite) { "launcher" } else { "daemon" }}))
    })
    .collect();
    std::fs::write(doctor.ledger.as_ref().unwrap(), serde_json::to_vec(&json!({
        "schema":1, "source":"celeris-browser-conformance",
        "generated_for":{"celeris_release":"123456abcdef","agent_browser":SUPPORTED_VERSION},
        "results":[{"backend_id":"claude-code","version":SUPPORTED_VERSION,"passed":[
            "open_allowed_origin","refuse_denied_origin","resume_after_crash","snapshot_has_refs",
            "click_by_ref","screenshot_artifact","download_to_artifacts",
            "isolation_suite","egress_negative_suite","injection_attack_suite","auth_section_observation_stop"
        ], "evidence": evidence}]
    })).unwrap()).unwrap();
    std::fs::write(
        &doctor.agent_browser,
        format!("#!/bin/sh\nprintf 'agent-browser {SUPPORTED_VERSION}\\n'\n"),
    )
    .unwrap();
    std::fs::set_permissions(
        &doctor.agent_browser,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}

fn fake_dns() -> (SocketAddr, std::thread::JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    (
        addr,
        std::thread::spawn(move || {
            let mut query = [0u8; 4096];
            let (n, peer) = socket.recv_from(&mut query).unwrap();
            assert_eq!(&query[12..n], b"\x07example\x03com\x00\x00\x01\x00\x01");
            query[2] = 0x81;
            query[3] = 0x80;
            socket.send_to(&query[..n], peer).unwrap();
        }),
    )
}

fn fake_credentiald(path: &std::path::Path, success: bool) -> std::thread::JoinHandle<()> {
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request).unwrap(),
            json!({"op":"ping"})
        );
        stream
            .write_all(format!("{{\"success\":{success}}}").as_bytes())
            .unwrap();
    })
}

fn fake_launcher(path: &std::path::Path, loopback: bool) -> std::thread::JoinHandle<()> {
    use task_worker::browser_launcher::protocol::{DEFAULT_MAX_FRAME, read_frame, write_message};
    use task_worker::browser_launcher::{PROTOCOL_VERSION, Request, Response};
    let listener = UnixListener::bind(path).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = read_frame(&mut stream, DEFAULT_MAX_FRAME).unwrap();
        assert_eq!(
            serde_json::from_slice::<Request>(&request).unwrap(),
            Request::Hello {}
        );
        write_message(
            &mut stream,
            &Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                test_loopback_allow: if loopback {
                    vec!["127.0.0.1".into()]
                } else {
                    vec![]
                },
            },
            DEFAULT_MAX_FRAME,
        )
        .unwrap();
    })
}

fn add_policy_grant(store: &SqliteStore) {
    let policy = task_core::BrowserSitePolicy {
        policy_id: "manaba".into(),
        exact_origin: "https://example.com".into(),
        login_url: "https://example.com/login".into(),
        password_selector: "#password".into(),
        submit_selector: None,
        username_selector: None,
        post_login: None,
    };
    let now = time::OffsetDateTime::now_utc();
    store
        .browser_site_policy_upsert(&policy, "admin", now)
        .unwrap();
    let node: task_core::OrgNode = serde_json::from_value(json!({
        "id":"browser-execution", "parent_id":null, "name":"Browser", "kind":"secretary",
        "created_at":"2026-10-08T00:00:00Z", "updated_at":"2026-10-08T00:00:00Z",
        "profile":{"browser":{"allowed_domains":["https://example.com"], "allowed_actions":["credential_use"], "credential_policy_ids":["manaba"]}}
    })).unwrap();
    store.org_upsert(&node).unwrap();
}

#[test]
fn browser_doctor_each_missing_item_has_a_fix_without_touching_production() {
    let dir = tempfile::tempdir().unwrap();
    let (doctor, store) = fixture(dir.path());
    let report = doctor.inspect(&store);
    assert!(report.has_missing());
    for check in [
        "ledger",
        "agent-browser",
        "bwrap",
        "sandboxd",
        "egress",
        "egress-resolver",
        "credentiald",
        "attestation-key",
        "site-policies",
        "grant",
    ] {
        assert_eq!(status(&report, check), "NG", "{check}");
        let rows: Vec<_> = report.items.iter().filter(|i| i.check == check).collect();
        assert_eq!(rows.len(), 1, "{check}");
        assert!(rows[0].detail.contains("修正:"));
        assert!(!rows[0].detail.contains('\n'));
    }
}

#[test]
fn browser_doctor_reports_username_selector_and_post_login_per_site_policy() {
    let dir = tempfile::tempdir().unwrap();
    let (doctor, store) = fixture(dir.path());
    add_policy_grant(&store);
    let mut policy = store.browser_site_policy_list().unwrap()[0].policy.clone();
    policy.username_selector = Some("#user".into());
    policy.post_login = Some(task_core::browser_wait::PostLogin {
        read_origins: vec!["https://lms.example.com".into()],
        actions: vec![task_core::browser_wait::PostLoginAction::Snapshot],
    });
    store
        .browser_site_policy_upsert(&policy, "admin", time::OffsetDateTime::now_utc())
        .unwrap();
    let report = doctor.inspect(&store);
    let row = report
        .items
        .iter()
        .find(|i| i.check == "site-policy-login")
        .expect("row");
    assert!(
        row.detail.contains("username_selector=あり"),
        "{}",
        row.detail
    );
    assert!(
        row.detail
            .contains("post_login=https://lms.example.com（snapshot）"),
        "{}",
        row.detail
    );
    assert_eq!(status(&report, "site-policy"), "OK");
}

#[test]
fn browser_doctor_launcher_dns_credential_ping_and_versions_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let (mut doctor, store) = fixture(dir.path());
    install_ledger(&doctor);
    add_policy_grant(&store);
    doctor.config.browser.runtime = "launcher".into();
    let launcher = dir.path().join("launcher.sock");
    doctor.config.browser.launcher_socket = Some(launcher.clone());
    let launcher_server = fake_launcher(&launcher, false);
    let credential = dir.path().join("credential.sock");
    doctor.config.api.browser_credentiald_control_socket = Some(credential.clone());
    let credential_server = fake_credentiald(&credential, true);
    let key = dir.path().join("key.pub");
    std::fs::write(&key, [42u8; 32]).unwrap();
    doctor.config.api.browser_attestation_public_key_file = Some(key);
    let (resolver, dns) = fake_dns();
    doctor.resolver = Some(resolver);
    let report = doctor.inspect(&store);
    launcher_server.join().unwrap();
    credential_server.join().unwrap();
    dns.join().unwrap();
    assert!(!report.has_missing(), "{:?}", report.items);
    for check in [
        "ledger",
        "ledger-backends",
        "launcher",
        "egress-resolver",
        "credentiald",
        "site-policies",
        "grant",
    ] {
        assert_eq!(status(&report, check), "OK", "{check}");
    }
    for check in ["bwrap", "sandboxd", "egress"] {
        assert_eq!(status(&report, check), "SKIP");
    }
    assert!(
        report
            .items
            .iter()
            .find(|i| i.check == "ledger")
            .unwrap()
            .detail
            .contains(SUPPORTED_VERSION)
    );
}

#[test]
fn browser_doctor_rejects_test_launcher_broker_denial_stale_ledger_and_bad_key() {
    let dir = tempfile::tempdir().unwrap();
    let (mut doctor, store) = fixture(dir.path());
    install_ledger(&doctor);
    doctor.release = Some("ffffffffffff".into());
    doctor.config.browser.runtime = "launcher".into();
    let launcher = dir.path().join("launcher.sock");
    doctor.config.browser.launcher_socket = Some(launcher.clone());
    let launcher_server = fake_launcher(&launcher, true);
    let credential = dir.path().join("credential.sock");
    doctor.config.api.browser_credentiald_control_socket = Some(credential.clone());
    let credential_server = fake_credentiald(&credential, false);
    let key = dir.path().join("key.pub");
    std::fs::write(&key, b"bad-key").unwrap();
    doctor.config.api.browser_attestation_public_key_file = Some(key);
    let report = doctor.inspect(&store);
    launcher_server.join().unwrap();
    credential_server.join().unwrap();
    for check in ["ledger", "launcher", "credentiald", "attestation-key"] {
        assert_eq!(status(&report, check), "NG");
    }
    assert!(
        report
            .items
            .iter()
            .find(|i| i.check == "ledger")
            .unwrap()
            .detail
            .contains("stale_release")
    );
}

#[test]
fn browser_doctor_db_is_authoritative_warns_on_seed_difference_and_unknown_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (mut doctor, store) = fixture(dir.path());
    add_policy_grant(&store);
    let mut seed = task_api::browser::TrustedSitePolicy::from(
        store.browser_site_policy_list().unwrap()[0].policy.clone(),
    );
    seed.password_selector = "#old".into();
    doctor.config.api.browser_site_policies.push(seed);
    let mut node = store.org_list().unwrap().remove(0);
    node.profile
        .browser
        .as_mut()
        .unwrap()
        .credential_policy_ids
        .push("absent".into());
    store.org_upsert(&node).unwrap();
    let report = doctor.inspect(&store);
    assert_eq!(status(&report, "grant"), "NG");
    assert!(
        report.items.iter().any(|i| i.check == "site-policy"
            && i.status == "WARN"
            && i.detail.contains("DB が正"))
    );
    assert_eq!(
        store.browser_site_policy_list().unwrap()[0]
            .policy
            .password_selector,
        "#password"
    );
}

#[test]
fn browser_doctor_dns_rejects_wrong_transaction_and_server_error() {
    for invalid in [0, 1] {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = socket.local_addr().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut packet = [0u8; 4096];
            let (n, peer) = socket.recv_from(&mut packet).unwrap();
            packet[2] = 0x81;
            if invalid == 0 {
                packet[0] ^= 1;
            } else {
                packet[3] = 2;
            }
            socket.send_to(&packet[..n], peer).unwrap();
        });
        assert!(!dns_probe(addr));
        server.join().unwrap();
    }
}
