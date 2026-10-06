//! 試験専用 loopback 許可の launcher config（ADR 2026-10-05-browser-department-web-live-view 付記 E1/E2）。
use std::path::{Path, PathBuf};

use super::{
    BackendConfig, PRODUCTION_CONFIG, PRODUCTION_SOCKET, PRODUCTION_STATE_DIR, egress_policy,
    refuse_test_loopback_in_production,
};

fn config(extra: &str, socket: &str, state_dir: &str) -> BackendConfig {
    toml::from_str(&format!(
        r#"
socket = "{socket}"
state_dir = "{state_dir}"
session_root = "{state_dir}/sessions"
allowed_uids = [1001]
bwrap = "/usr/bin/bwrap"
sandboxd = "/usr/local/libexec/celeris/celeris-browser-sandboxd"
egress = "/usr/local/libexec/celeris/celeris-browser-egress"
chrome = "/opt/chrome/chrome"
agent_browser = "/opt/agent-browser/agent-browser"
resolver = "127.0.0.53"
{extra}
"#
    ))
    .expect("launcher config")
}

fn test_config(extra: &str) -> BackendConfig {
    config(
        extra,
        "/tmp/celeris-test/launcher.sock",
        "/tmp/celeris-test/state",
    )
}

const DOMAINS: [&str; 3] = [
    "http://127.0.0.1:18080",
    "http://127.0.0.1:18081",
    "https://example.com",
];

fn domains() -> Vec<String> {
    DOMAINS.map(str::to_owned).to_vec()
}

#[test]
fn egress_test_loopback_default_off_when_config_omits_it() {
    let cfg = test_config("");
    assert!(cfg.test_loopback_allow.is_empty());
    cfg.validate_test_loopback().expect("empty is valid");
    let policy = egress_policy(&domains(), cfg.resolver, &cfg.test_loopback_allow);
    assert!(policy.test_loopback_allow.is_empty(), "{policy:?}");
    // allowed_domains の loopback は従来どおり `allow` に入るだけで、check_egress は IpLiteral で拒否する。
    assert!(policy.allow.contains("127.0.0.1:18080"));
    let denied = task_core::browser_isolation::check_egress(
        &policy,
        &task_core::browser_isolation::EgressRequest::Connect {
            host: "127.0.0.1".into(),
            port: 18080,
            resolved: vec!["127.0.0.1".parse().expect("ip")],
        },
    );
    assert!(denied.is_err(), "{denied:?}");
    // 欄なしの launcher は production path でも起動を拒否しない（既定 off は検査の外）。
    let prod = config("", PRODUCTION_SOCKET, PRODUCTION_STATE_DIR);
    refuse_test_loopback_in_production(Path::new(PRODUCTION_CONFIG), &prod, None)
        .expect("default off starts");
}

#[test]
fn egress_test_loopback_policy_is_intersection_with_session_domains() {
    let cfg = test_config(r#"test_loopback_allow = ["127.0.0.1:18080", "127.0.0.1:19090"]"#);
    cfg.validate_test_loopback().expect("valid");
    let policy = egress_policy(&domains(), cfg.resolver, &cfg.test_loopback_allow);
    // 18080 は両方にある。18081 は session にだけ（config に無い）、19090 は config にだけある。
    assert_eq!(
        policy.test_loopback_allow,
        ["127.0.0.1:18080".to_string()].into_iter().collect()
    );
    let without = egress_policy(
        &["https://example.com".to_string()],
        cfg.resolver,
        &cfg.test_loopback_allow,
    );
    assert!(without.test_loopback_allow.is_empty(), "{without:?}");
}

#[test]
fn egress_test_loopback_config_rejects_anything_but_127_0_0_1_port() {
    for bad in [
        "localhost:18080",
        "127.0.0.2:18080",
        "10.0.0.1:80",
        "[::1]:18080",
        "127.0.0.1",
        "127.0.0.1:",
        "127.0.0.1:0",
        "127.0.0.1:53",
        "127.0.0.1:853",
        "127.0.0.1:018080",
        "127.0.0.1:70000",
        "127.0.0.1:+80",
        "2130706433:80",
        " 127.0.0.1:80",
    ] {
        let cfg = test_config(&format!("test_loopback_allow = [{bad:?}]"));
        assert!(
            cfg.validate_test_loopback().is_err(),
            "{bad} must be rejected"
        );
    }
    let ok = test_config(r#"test_loopback_allow = ["127.0.0.1:1", "127.0.0.1:65535"]"#);
    ok.validate_test_loopback().expect("valid");
    // 型の違う値・未知の欄は TOML の読込みで落ちる（deny_unknown_fields）。
    let parse = |extra: &str| {
        toml::from_str::<BackendConfig>(&format!(
            "socket = \"/tmp/s\"\nstate_dir = \"/tmp/d\"\nsession_root = \"/tmp/d/s\"\nallowed_uids = [1]\n\
             bwrap = \"/b\"\nsandboxd = \"/s\"\negress = \"/e\"\nchrome = \"/c\"\nagent_browser = \"/a\"\n\
             resolver = \"127.0.0.53\"\n{extra}"
        ))
    };
    assert!(parse("test_loopback_allow = \"127.0.0.1:80\"").is_err());
    assert!(parse("test_loopback_egress = [\"127.0.0.1:80\"]").is_err());
}

#[test]
fn egress_test_loopback_refuses_production_config_socket_or_state_dir() {
    let allow = r#"test_loopback_allow = ["127.0.0.1:18080"]"#;
    let tmp = tempfile::tempdir().expect("tmp");
    let own_config = tmp.path().join("launcher.toml");
    let own_socket = tmp.path().join("launcher.sock");
    let own_state = tmp.path().join("state");
    let s = |p: &Path| p.display().to_string();

    // 本番と重ならない試験用の path なら起動できる。
    let cfg = config(allow, &s(&own_socket), &s(&own_state));
    refuse_test_loopback_in_production(&own_config, &cfg, None).expect("test paths start");

    // config path が本番。
    let err = refuse_test_loopback_in_production(Path::new(PRODUCTION_CONFIG), &cfg, None)
        .expect_err("production config");
    assert!(err.contains("config"), "{err}");
    // socket が本番（config の socket、socket activation の path）。
    let prod_socket = config(allow, PRODUCTION_SOCKET, &s(&own_state));
    let err = refuse_test_loopback_in_production(&own_config, &prod_socket, None)
        .expect_err("production socket");
    assert!(err.contains("socket"), "{err}");
    let err =
        refuse_test_loopback_in_production(&own_config, &cfg, Some(Path::new(PRODUCTION_SOCKET)))
            .expect_err("activated production socket");
    assert!(err.contains("socket"), "{err}");
    // state_dir が本番・本番の配下。
    for state in [
        PRODUCTION_STATE_DIR.to_string(),
        format!("{PRODUCTION_STATE_DIR}/test"),
        format!("{PRODUCTION_STATE_DIR}/../celeris-browser"),
    ] {
        let prod_state = config(allow, &s(&own_socket), &state);
        let err = refuse_test_loopback_in_production(&own_config, &prod_state, None)
            .expect_err("production state_dir");
        assert!(err.contains("state_dir"), "{state}: {err}");
    }
    // symlink で本番を指す path も正規化して一致とみなす。
    let link = tmp.path().join("etc-link");
    std::os::unix::fs::symlink("/etc", &link).expect("symlink");
    let via_link = link.join("celeris-browser/launcher.toml");
    refuse_test_loopback_in_production(&via_link, &cfg, None)
        .expect_err("symlinked production config");
    // 正規化できない path（相対）は一致とみなす（fail closed）。
    refuse_test_loopback_in_production(&PathBuf::from("launcher.toml"), &cfg, None)
        .expect_err("relative config path");
    let relative_state = config(allow, &s(&own_socket), "state");
    refuse_test_loopback_in_production(&own_config, &relative_state, None)
        .expect_err("relative state_dir");
}
