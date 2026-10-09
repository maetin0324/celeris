use super::*;

fn good() -> RuntimeFacts {
    RuntimeFacts {
        session_id: "s1".into(),
        host_uid: 1000,
        runtime_uid: 200_001,
        userns_owner_uid: Some(1001),
        namespaces: REQUIRED_NAMESPACES.into_iter().collect(),
        root_readonly: true,
        writable_mounts: vec!["/session/profile".into(), "/session/downloads".into()],
        visible_paths: vec!["/usr".into(), "/etc/ssl".into(), "/session".into()],
        cdp: CdpEndpoint::Pipe,
        no_new_privs: true,
        capabilities_dropped: true,
        pgid: 4242,
    }
}

fn violations(f: &RuntimeFacts) -> Vec<IsolationViolation> {
    verify_isolation(f).unwrap_err()
}

#[test]
fn verified_runtime_is_isolated() {
    let a = verify_isolation(&good()).unwrap();
    assert_eq!(a.isolation(), Isolation::Isolated);
    assert_eq!(a.session_id(), "s1");
    assert_eq!(a.pgid(), 4242);
    assert_eq!(a.userns_owner_uid(), 1001);
}

#[test]
fn daemon_owned_userns_is_rejected() {
    let mut f = good();
    f.userns_owner_uid = Some(f.host_uid);
    assert_eq!(
        violations(&f),
        vec![IsolationViolation::UsernsOwnedByDaemon]
    );
}

#[test]
fn unknown_userns_owner_is_rejected() {
    let mut f = good();
    f.userns_owner_uid = None;
    assert_eq!(violations(&f), vec![IsolationViolation::OwnerUnknown]);
}

#[test]
fn different_userns_owner_is_attested() {
    let f = good();
    let a = verify_isolation(&f).expect("different owner with complete isolation");
    assert_eq!(a.userns_owner_uid(), f.userns_owner_uid.unwrap());
}

#[test]
fn missing_runtime_owner_is_unknown() {
    assert_eq!(collect_userns_owner_uid(-1), None);
}

#[test]
fn same_uid_and_root_are_rejected() {
    let mut f = good();
    f.runtime_uid = 1000;
    assert!(violations(&f).contains(&IsolationViolation::SameUid));
    f.runtime_uid = 0;
    assert!(violations(&f).contains(&IsolationViolation::RootUid));
}

#[test]
fn every_namespace_is_required() {
    for ns in REQUIRED_NAMESPACES {
        let mut f = good();
        f.namespaces.remove(&ns);
        assert_eq!(
            violations(&f),
            vec![IsolationViolation::MissingNamespace { ns }]
        );
    }
}

#[test]
fn root_must_be_readonly_and_writes_stay_in_session() {
    let mut f = good();
    f.root_readonly = false;
    assert_eq!(violations(&f), vec![IsolationViolation::RootWritable]);
    for bad in ["/home/u", "/sessionx", "/session/../etc", "session", "/tmp"] {
        let mut f = good();
        f.writable_mounts.push(bad.into());
        assert_eq!(
            violations(&f),
            vec![IsolationViolation::WritableOutsideSession { path: bad.into() }],
            "{bad}"
        );
    }
}

#[test]
fn broker_and_host_ipc_are_not_visible() {
    for p in [
        "/run/celeris/credentiald",
        "/run/celeris/credentiald/sock",
        "/etc/celeris",
        "/run/celeris",
        "/var/lib/celeris",
    ] {
        let mut f = good();
        f.visible_paths.push(p.into());
        assert!(
            matches!(
                violations(&f)[..],
                [IsolationViolation::BrokerVisible { .. }]
            ),
            "{p}"
        );
    }
    for p in [
        "/run/user/1000",
        "/dev/shm",
        "/tmp/.X11-unix/X0",
        "/run/dbus",
        "/var/run/docker.sock",
    ] {
        let mut f = good();
        f.visible_paths.push(p.into());
        assert!(
            matches!(
                violations(&f)[..],
                [IsolationViolation::HostIpcVisible { .. }]
            ),
            "{p}"
        );
    }
}

#[test]
fn cdp_must_not_be_on_tcp_or_outside_controller_dir() {
    let mut f = good();
    f.cdp = CdpEndpoint::Tcp {
        addr: "127.0.0.1:9222".into(),
    };
    assert_eq!(violations(&f), vec![IsolationViolation::CdpOnTcp]);
    f.cdp = CdpEndpoint::UnixSocket {
        host_path: "/tmp/cdp.sock".into(),
    };
    assert_eq!(
        violations(&f),
        vec![IsolationViolation::CdpOutsideControllerDir]
    );
    f.cdp = CdpEndpoint::UnixSocket {
        host_path: format!("{CONTROLLER_DIR_PREFIX}../x"),
    };
    assert_eq!(
        violations(&f),
        vec![IsolationViolation::CdpOutsideControllerDir]
    );
    f.cdp = CdpEndpoint::UnixSocket {
        host_path: format!("{CONTROLLER_DIR_PREFIX}s1/cdp.sock"),
    };
    assert!(verify_isolation(&f).is_ok());
}

#[test]
fn privileges_and_pgid_are_checked() {
    let mut f = good();
    f.no_new_privs = false;
    f.pgid = 1;
    f.session_id = "../x".into();
    let v = violations(&f);
    assert!(v.contains(&IsolationViolation::PrivilegesKept));
    assert!(v.contains(&IsolationViolation::NoProcessGroup));
    assert!(v.contains(&IsolationViolation::InvalidSession));
}

#[test]
fn bwrap_argv_unshares_everything_and_remounts_ro() {
    let a = bwrap_argv(
        200_001,
        "/var/lib/x/s1",
        "/run/celeris/browser-controller/s1",
    );
    for flag in [
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--die-with-parent",
        "--new-session",
    ] {
        assert!(a.iter().any(|x| x == flag), "{flag}");
    }
    assert!(!a.iter().any(|x| x.contains("credentiald")));
    assert_eq!(a.last().map(String::as_str), Some("/"));
    assert!(a.windows(2).any(|w| w[0] == "--uid" && w[1] == "200001"));
}

fn policy() -> EgressPolicy {
    EgressPolicy {
        allow: ["example.com:443".to_string()].into_iter().collect(),
        resolver: "10.200.0.1".parse().unwrap(),
        allow_ipv6: false,
        test_loopback_allow: BTreeSet::new(),
    }
}

fn connect_port(host: &str, port: u16, ips: &[&str]) -> EgressRequest {
    EgressRequest::Connect {
        host: host.into(),
        port,
        resolved: ips.iter().map(|s| s.parse().unwrap()).collect(),
    }
}

fn loopback_policy(entries: &[&str]) -> EgressPolicy {
    let mut p = policy();
    p.test_loopback_allow = entries.iter().map(|s| s.to_string()).collect();
    p
}

#[test]
fn egress_test_loopback_default_off_keeps_ip_literal() {
    let p = policy();
    assert!(p.test_loopback_allow.is_empty());
    for port in [8080, 443, 1] {
        assert_eq!(
            check_egress(&p, &connect_port("127.0.0.1", port, &[])),
            Err(EgressDenied::IpLiteral),
            "{port}"
        );
        assert_eq!(test_loopback_target(&p, "127.0.0.1", port), None);
    }
    // 既定（空）は JSON に出ず、欄の無い JSON は空として読む。
    let json = serde_json::to_value(&p).unwrap();
    assert!(json.get("test_loopback_allow").is_none(), "{json}");
    let back: EgressPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(back, p);
}

#[test]
fn egress_test_loopback_allows_only_listed_host_port() {
    let p = loopback_policy(&["127.0.0.1:18080"]);
    assert_eq!(
        check_egress(&p, &connect_port("127.0.0.1", 18080, &[])),
        Ok(())
    );
    assert_eq!(
        test_loopback_target(&p, "127.0.0.1", 18080),
        Some("127.0.0.1:18080".parse().unwrap())
    );
    // 別 port は IpLiteral のまま。
    assert_eq!(
        check_egress(&p, &connect_port("127.0.0.1", 18081, &[])),
        Err(EgressDenied::IpLiteral)
    );
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(
        json["test_loopback_allow"],
        serde_json::json!(["127.0.0.1:18080"])
    );
}

#[test]
fn egress_test_loopback_other_private_and_variants_stay_denied() {
    let p = loopback_policy(&[
        "127.0.0.1:18080",
        "localhost:18080",
        "10.0.0.1:18080",
        "[::1]:18080",
        "::1:18080",
        "127.0.0.2:18080",
        "2130706433:18080",
        "0x7f.1:18080",
        "0177.0.0.1:18080",
        "127.0.0.1:53",
        "127.0.0.1:853",
        "127.0.0.1:0",
    ]);
    for h in [
        "10.0.0.1",
        "[::1]",
        "::1",
        "127.0.0.2",
        "2130706433",
        "0x7f.1",
        "0177.0.0.1",
        "0x7f.0.0.1",
        "127.1",
    ] {
        assert_eq!(
            check_egress(&p, &connect_port(h, 18080, &["127.0.0.1"])),
            Err(EgressDenied::IpLiteral),
            "{h}"
        );
        assert_eq!(test_loopback_target(&p, h, 18080), None, "{h}");
    }
    // DNS を経る名前の loopback 解決は従来どおり（許可に無ければ NotAllowed、あっても PrivateAddress）。
    assert_eq!(
        check_egress(&p, &connect_port("localhost", 18080, &["127.0.0.1"])),
        Err(EgressDenied::NotAllowed)
    );
    let mut named = p.clone();
    named.allow.insert("localhost:18080".into());
    assert_eq!(
        check_egress(&named, &connect_port("localhost", 18080, &["127.0.0.1"])),
        Err(EgressDenied::PrivateAddress)
    );
    // DNS/DoT と port 0 は集合にあっても通さない。
    for port in [53, 853, 0] {
        assert_eq!(
            check_egress(&p, &connect_port("127.0.0.1", port, &[])),
            Err(EgressDenied::IpLiteral),
            "{port}"
        );
    }
}

#[test]
fn egress_test_loopback_does_not_change_other_verdicts() {
    let p = loopback_policy(&["127.0.0.1:443"]);
    assert_eq!(
        check_egress(&p, &connect("example.com", &["93.184.216.34"])),
        Ok(())
    );
    assert_eq!(
        check_egress(&p, &connect("example.com", &["93.184.216.34", "127.0.0.1"])),
        Err(EgressDenied::PrivateAddress)
    );
    assert_eq!(
        check_egress(&p, &connect("evil.com", &["93.184.216.34"])),
        Err(EgressDenied::NotAllowed)
    );
    assert_eq!(
        check_egress(&p, &connect("93.184.216.34", &["93.184.216.34"])),
        Err(EgressDenied::IpLiteral)
    );
    assert_eq!(
        check_egress(
            &p,
            &EgressRequest::Dns {
                server: "127.0.0.1".parse().unwrap(),
                port: 53
            }
        ),
        Err(EgressDenied::DnsBypass)
    );
    assert_eq!(
        check_egress(
            &p,
            &EgressRequest::UpstreamProxy {
                host: "127.0.0.1".into(),
                port: 443
            }
        ),
        Err(EgressDenied::ProxyChain)
    );
}

fn connect(host: &str, ips: &[&str]) -> EgressRequest {
    EgressRequest::Connect {
        host: host.into(),
        port: 443,
        resolved: ips.iter().map(|s| s.parse().unwrap()).collect(),
    }
}

#[test]
fn egress_allows_public_allowed_host() {
    assert_eq!(
        check_egress(&policy(), &connect("example.com", &["93.184.216.34"])),
        Ok(())
    );
}

#[test]
fn egress_rejects_private_ranges_and_rebinding() {
    for ip in [
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "127.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "198.18.0.1",
        "240.0.0.1",
        "192.88.99.1",
    ] {
        assert_eq!(
            check_egress(&policy(), &connect("example.com", &["93.184.216.34", ip])),
            Err(EgressDenied::PrivateAddress),
            "{ip}"
        );
    }
}

#[test]
fn egress_rejects_ipv6_private_and_disabled() {
    for ip in [
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "64:ff9b::a00:1",
        "2002:a00:1::",
        "::a00:1",
        "2001:db8::1",
        "100::1",
        "100:0:0:1::1",
        "2001:2::1",
        "2001:20::1",
        "3fff:fff::1",
        "5f00::1",
        "4000::1",
    ] {
        assert_eq!(
            check_egress(&policy(), &connect("example.com", &[ip])),
            Err(EgressDenied::PrivateAddress),
            "{ip}"
        );
    }
    assert_eq!(
        check_egress(&policy(), &connect("example.com", &["2606:2800:220:1::1"])),
        Err(EgressDenied::Ipv6Disabled)
    );
    let mut p = policy();
    p.allow_ipv6 = true;
    for ip in ["100::1", "2001:2::1", "3fff:fff::1", "5f00::1", "4000::1"] {
        assert_eq!(
            check_egress(&p, &connect("example.com", &[ip])),
            Err(EgressDenied::PrivateAddress)
        );
    }
    assert_eq!(
        check_egress(&p, &connect("example.com", &["2606:2800:220:1::1"])),
        Ok(())
    );
    // mapped の公開 v4 は v4 として扱う
    assert_eq!(
        check_egress(
            &policy(),
            &connect("example.com", &["::ffff:93.184.216.34"])
        ),
        Ok(())
    );
}

#[test]
fn egress_rejects_ip_literals_and_unlisted_hosts() {
    for h in [
        "127.0.0.1",
        "2130706433",
        "0x7f.1",
        "0177.0.0.1",
        "[::1]",
        "93.184.216.34",
    ] {
        assert_eq!(
            check_egress(&policy(), &connect(h, &["93.184.216.34"])),
            Err(EgressDenied::IpLiteral),
            "{h}"
        );
    }
    assert_eq!(
        check_egress(&policy(), &connect("evil.com", &["93.184.216.34"])),
        Err(EgressDenied::NotAllowed)
    );
    assert_eq!(
        check_egress(&policy(), &connect("EXAMPLE.com", &["93.184.216.34"])),
        Err(EgressDenied::InvalidHost)
    );
    assert_eq!(
        check_egress(&policy(), &connect("example.com.", &["93.184.216.34"])),
        Err(EgressDenied::InvalidHost)
    );
    assert_eq!(
        check_egress(&policy(), &connect("example.com", &[])),
        Err(EgressDenied::Unresolved)
    );
    let other_port = EgressRequest::Connect {
        host: "example.com".into(),
        port: 8443,
        resolved: vec!["93.184.216.34".parse().unwrap()],
    };
    assert_eq!(
        check_egress(&policy(), &other_port),
        Err(EgressDenied::NotAllowed)
    );
}

/// 本番 2026-10-09: 公開 v6 を併せ持つ dual-stack の名前は、IPv6 無効でも v4 があれば許し、接続先は v4。
/// v6 だけ・v6 側が非公開（rebinding）は従来どおり拒否。
#[test]
fn egress_dual_stack_uses_ipv4_when_ipv6_disabled() {
    let dual = ["93.184.216.34", "2606:2800:220:1::1"];
    assert_eq!(
        check_egress(&policy(), &connect("example.com", &dual)),
        Ok(())
    );
    let resolved: Vec<IpAddr> = dual.iter().map(|s| s.parse().unwrap()).collect();
    let reversed: Vec<IpAddr> = resolved.iter().rev().copied().collect();
    for ips in [&resolved, &reversed] {
        assert_eq!(
            egress_destination(&policy(), ips),
            Some("93.184.216.34".parse().unwrap())
        );
    }
    assert_eq!(
        check_egress(&policy(), &connect("example.com", &["2606:2800:220:1::1"])),
        Err(EgressDenied::Ipv6Disabled)
    );
    assert_eq!(
        check_egress(
            &policy(),
            &connect("example.com", &["93.184.216.34", "fd00::1"])
        ),
        Err(EgressDenied::PrivateAddress)
    );
    let mut p = policy();
    p.allow_ipv6 = true;
    assert_eq!(
        egress_destination(&p, &reversed),
        Some("2606:2800:220:1::1".parse().unwrap())
    );
}

/// 本番 2026-10-09: task policy `https://*.tsukuba.ac.jp` は egress 許可 `*.tsukuba.ac.jp:443` になる。
/// 完全一致しか見ないと manaba.tsukuba.ac.jp が not_allowed になり、browser はどこにも届かなかった。
#[test]
fn egress_wildcard_entry_covers_subdomains_only() {
    let mut p = policy();
    p.allow = ["*.tsukuba.ac.jp:443".to_string()].into_iter().collect();
    for host in [
        "manaba.tsukuba.ac.jp",
        "www.tsukuba.ac.jp",
        "a.b.tsukuba.ac.jp",
    ] {
        assert_eq!(
            check_egress(&p, &connect(host, &["122.249.253.244"])),
            Ok(()),
            "{host}"
        );
    }
    // apex・似た名前・別 port・別の suffix は許さない。
    for host in [
        "tsukuba.ac.jp",
        "eviltsukuba.ac.jp",
        "manaba.tsukuba.ac.jp.evil.com",
        "ac.jp",
    ] {
        assert_eq!(
            check_egress(&p, &connect(host, &["122.249.253.244"])),
            Err(EgressDenied::NotAllowed),
            "{host}"
        );
    }
    let other_port = connect_port("manaba.tsukuba.ac.jp", 8443, &["122.249.253.244"]);
    assert_eq!(check_egress(&p, &other_port), Err(EgressDenied::NotAllowed));
    // wildcard に当たっても、解決先の検査（private・IPv6・未解決）は変わらない。
    assert_eq!(
        check_egress(&p, &connect("manaba.tsukuba.ac.jp", &["10.0.0.5"])),
        Err(EgressDenied::PrivateAddress)
    );
    assert_eq!(
        check_egress(
            &p,
            &connect("manaba.tsukuba.ac.jp", &["2606:2800:220:1::1"])
        ),
        Err(EgressDenied::Ipv6Disabled)
    );
    assert_eq!(
        check_egress(&p, &connect("manaba.tsukuba.ac.jp", &[])),
        Err(EgressDenied::Unresolved)
    );
    // public suffix への wildcard・壊れた pattern は何も許さない。
    for (entry, host) in [
        ("*.com:443", "evil.com"),
        ("*.ac.jp:443", "manaba.tsukuba.ac.jp"),
        ("*.github.io:443", "evil.github.io"),
        ("*.:443", "manaba.tsukuba.ac.jp"),
        ("*..ac.jp:443", "manaba.tsukuba.ac.jp"),
        ("**.tsukuba.ac.jp:443", "manaba.tsukuba.ac.jp"),
        ("*.tsukuba.ac.jp", "manaba.tsukuba.ac.jp"),
    ] {
        p.allow = [entry.to_string()].into_iter().collect();
        assert_eq!(
            check_egress(&p, &connect(host, &["122.249.253.244"])),
            Err(EgressDenied::NotAllowed),
            "{entry} {host}"
        );
    }
}

#[test]
fn egress_rejects_dns_bypass_and_proxy_chain() {
    let p = policy();
    assert_eq!(
        check_egress(
            &p,
            &EgressRequest::Dns {
                server: p.resolver,
                port: 53
            }
        ),
        Ok(())
    );
    assert_eq!(
        check_egress(
            &p,
            &EgressRequest::Dns {
                server: "8.8.8.8".parse().unwrap(),
                port: 53
            }
        ),
        Err(EgressDenied::DnsBypass)
    );
    assert_eq!(
        check_egress(
            &p,
            &EgressRequest::Dns {
                server: p.resolver,
                port: 853
            }
        ),
        Err(EgressDenied::DnsBypass)
    );
    assert_eq!(
        check_egress(
            &p,
            &EgressRequest::UpstreamProxy {
                host: "example.com".into(),
                port: 443
            }
        ),
        Err(EgressDenied::ProxyChain)
    );
}

#[test]
fn orphans_are_labelled_runtime_groups_without_live_session() {
    let procs = vec![
        ProcessRecord {
            pid: 10,
            pgid: 10,
            uid: 200_001,
            session_label: Some("dead".into()),
        },
        ProcessRecord {
            pid: 11,
            pgid: 10,
            uid: 200_001,
            session_label: Some("dead".into()),
        },
        ProcessRecord {
            pid: 20,
            pgid: 20,
            uid: 200_002,
            session_label: Some("live".into()),
        },
        ProcessRecord {
            pid: 30,
            pgid: 30,
            uid: 1000,
            session_label: Some("dead".into()),
        },
        ProcessRecord {
            pid: 40,
            pgid: 40,
            uid: 200_001,
            session_label: None,
        },
        ProcessRecord {
            pid: 50,
            pgid: 50,
            uid: 0,
            session_label: Some("dead".into()),
        },
        ProcessRecord {
            pid: 60,
            pgid: 60,
            uid: 300_000,
            session_label: Some("dead".into()),
        },
        ProcessRecord {
            pid: 70,
            pgid: 1,
            uid: 200_001,
            session_label: Some("dead".into()),
        },
    ];
    let live = ["live".to_string()].into_iter().collect();
    let uids = [200_001, 200_002].into_iter().collect();
    assert_eq!(orphan_groups(&procs, &live, &uids, 1000), vec![10]);
}

// ADR-0138 D-L: launcher session 証明。

const LAUNCHER_UID: u32 = 1001;

fn proof() -> LauncherSessionProof {
    LauncherSessionProof {
        session_id: "s1".into(),
        instance_id: "inst-a".into(),
        pid: 5151,
        starttime: 987_654,
        ns_owner_uid: Some(1001),
        launcher_uid: LAUNCHER_UID,
        isolation_ok: true,
        ns_inodes: Default::default(),
    }
}

fn seen() -> LauncherObservation {
    LauncherObservation {
        session_id: "s1".into(),
        instance_id: "inst-a".into(),
        peer_uid: Some(LAUNCHER_UID),
        configured_launcher_uid: LAUNCHER_UID,
        runtime_pid: 5151,
        runtime_starttime: Some(987_654),
    }
}

fn invalid(defect: LauncherProofDefect) -> IsolationViolation {
    IsolationViolation::LauncherProofInvalid { defect }
}

fn proof_rejected(
    f: &RuntimeFacts,
    p: &LauncherSessionProof,
    o: &LauncherObservation,
) -> Vec<IsolationViolation> {
    verify_launcher_session(f, Some(p), o).unwrap_err()
}

#[test]
fn valid_launcher_proof_creates_attestation() {
    let a = verify_launcher_session(&good(), Some(&proof()), &seen()).expect("valid proof");
    assert_eq!(a.session_id(), "s1");
    assert_eq!(a.instance_id(), "inst-a");
    assert_eq!(a.pid(), 5151);
    assert_eq!(a.starttime(), 987_654);
    assert_eq!(a.launcher_uid(), LAUNCHER_UID);
    assert_eq!(a.isolation(), Isolation::Isolated);
    assert_eq!(a.isolation_attestation().userns_owner_uid(), 1001);
}

#[test]
fn owner_check_alone_without_proof_is_rejected() {
    // owner 検査を含む隔離条件は全部通る。
    assert!(verify_isolation(&good()).is_ok());
    assert_eq!(
        verify_launcher_session(&good(), None, &seen()).unwrap_err(),
        vec![IsolationViolation::LauncherProofMissing]
    );
}

#[test]
fn proof_does_not_override_isolation_violations() {
    let mut f = good();
    f.runtime_uid = f.host_uid;
    f.userns_owner_uid = Some(f.host_uid);
    let v = verify_launcher_session(&f, Some(&proof()), &seen()).unwrap_err();
    assert!(v.contains(&IsolationViolation::SameUid));
    assert!(v.contains(&IsolationViolation::UsernsOwnedByDaemon));
    // proof の owner (1001) と daemon が採った owner (1000) が食い違う。
    assert!(v.contains(&invalid(LauncherProofDefect::OwnerMismatch)));

    let mut f = good();
    f.namespaces.remove(&Namespace::Net);
    assert_eq!(
        verify_launcher_session(&f, Some(&proof()), &seen()).unwrap_err(),
        vec![IsolationViolation::MissingNamespace { ns: Namespace::Net }]
    );
    // 非隔離かつ証明なしは両方を返す。
    assert_eq!(
        verify_launcher_session(&f, None, &seen()).unwrap_err(),
        vec![
            IsolationViolation::MissingNamespace { ns: Namespace::Net },
            IsolationViolation::LauncherProofMissing
        ]
    );
}

#[test]
fn pid_and_starttime_must_bind_the_runtime() {
    let mut o = seen();
    o.runtime_pid = 5152;
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::PidMismatch)]
    );
    let mut p = proof();
    p.pid = 1;
    let mut o = seen();
    o.runtime_pid = 1;
    assert_eq!(
        proof_rejected(&good(), &p, &o),
        vec![invalid(LauncherProofDefect::PidMismatch)]
    );
    let mut o = seen();
    o.runtime_starttime = Some(987_655);
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::StarttimeMismatch)]
    );
    // 採取できない starttime（session 終了など）を一致とみなさない。
    let mut o = seen();
    o.runtime_starttime = None;
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::StarttimeUnavailable)]
    );
}

#[test]
fn proof_owner_must_be_known_and_not_daemon() {
    let mut p = proof();
    p.ns_owner_uid = None;
    assert_eq!(
        proof_rejected(&good(), &p, &seen()),
        vec![invalid(LauncherProofDefect::OwnerUnknown)]
    );
    let mut p = proof();
    p.ns_owner_uid = Some(1000);
    assert_eq!(
        proof_rejected(&good(), &p, &seen()),
        vec![invalid(LauncherProofDefect::OwnerIsDaemon)]
    );
    let mut p = proof();
    p.ns_owner_uid = Some(1002);
    assert_eq!(
        proof_rejected(&good(), &p, &seen()),
        vec![invalid(LauncherProofDefect::OwnerMismatch)]
    );
}

#[test]
fn launcher_uid_must_match_peer_and_config() {
    let mut o = seen();
    o.peer_uid = None;
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::PeerUidUnavailable)]
    );
    let mut o = seen();
    o.peer_uid = Some(1003);
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::LauncherUidMismatch)]
    );
    let mut o = seen();
    o.configured_launcher_uid = 1003;
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::LauncherUidMismatch)]
    );
    // daemon 自身が launcher を名乗る（daemon 起動の runtime）。
    let mut p = proof();
    p.launcher_uid = 1000;
    let mut o = seen();
    o.peer_uid = Some(1000);
    o.configured_launcher_uid = 1000;
    assert_eq!(
        proof_rejected(&good(), &p, &o),
        vec![invalid(LauncherProofDefect::LauncherUidPrivileged)]
    );
}

#[test]
fn isolation_not_ok_and_session_mismatch_are_rejected() {
    let mut p = proof();
    p.isolation_ok = false;
    assert_eq!(
        proof_rejected(&good(), &p, &seen()),
        vec![invalid(LauncherProofDefect::IsolationNotOk)]
    );
    let mut p = proof();
    p.session_id = "s2".into();
    assert_eq!(
        proof_rejected(&good(), &p, &seen()),
        vec![invalid(LauncherProofDefect::SessionMismatch)]
    );
    let mut o = seen();
    o.instance_id = "inst-b".into();
    assert_eq!(
        proof_rejected(&good(), &proof(), &o),
        vec![invalid(LauncherProofDefect::SessionMismatch)]
    );
}

// ADR-0138 D-L / prod-facts: 別 UID の runtime は daemon UID から `/proc/<pid>/ns/*` を開けない
// （実 launcher の Chrome で EACCES を観測）。本番の事実は daemon が読める status・mountinfo と、
// launcher の束縛（ns inode・owner・pid/starttime）から組む。

const RUNTIME_STATUS: &str = "Name:\tchrome\nUid:\t200001\t200001\t200001\t200001\nNoNewPrivs:\t1\nCapPrm:\t0000000000000000\nCapEff:\t0000000000000000\n";
const RUNTIME_MOUNTINFO: &str = "\
1 0 0:1 / / ro,nosuid - tmpfs tmpfs ro
2 1 0:2 / /usr ro,nosuid - ext4 /dev/sda1 ro
3 1 0:3 / /session/profile rw,nosuid - tmpfs tmpfs rw
4 1 0:4 / /proc rw,nosuid - proc proc rw
";

fn own_ns() -> BTreeMap<Namespace, u64> {
    REQUIRED_NAMESPACES
        .into_iter()
        .zip(4_026_531_835u64..)
        .collect()
}

fn launched_proof() -> LauncherSessionProof {
    LauncherSessionProof {
        ns_inodes: REQUIRED_NAMESPACES
            .into_iter()
            .zip(4_026_532_900u64..)
            .collect(),
        ..proof()
    }
}

fn launched(p: &LauncherSessionProof) -> RuntimeFacts {
    launched_runtime_facts(
        "s1",
        4242,
        1000,
        &own_ns(),
        ProcView::parse(RUNTIME_STATUS, RUNTIME_MOUNTINFO),
        p,
    )
}

#[test]
fn launched_facts_from_launcher_binding_pass_production_admission() {
    let f = launched(&launched_proof());
    assert_eq!(f.runtime_uid, 200_001);
    assert_eq!(f.userns_owner_uid, Some(1001));
    assert_eq!(f.namespaces, REQUIRED_NAMESPACES.into_iter().collect());
    assert!(f.root_readonly && f.no_new_privs && f.capabilities_dropped);
    assert_eq!(f.writable_mounts, vec!["/session/profile".to_owned()]);
    let a = verify_launcher_session(&f, Some(&launched_proof()), &seen()).expect("attested");
    assert_eq!(a.pid(), 5151);
    assert_eq!(a.isolation(), Isolation::Isolated);
}

#[test]
fn launched_facts_reject_binding_without_or_with_partial_ns_inodes() {
    // v2 の束縛（inode 無し）: どの namespace も「別」と数えない。
    let v2 = proof();
    let v = proof_rejected(&launched(&v2), &v2, &seen());
    for ns in REQUIRED_NAMESPACES {
        assert!(
            v.contains(&IsolationViolation::MissingNamespace { ns }),
            "{v:?}"
        );
    }
    // 1 つ欠けた束縛。
    let mut partial = launched_proof();
    partial.ns_inodes.remove(&Namespace::Net);
    assert_eq!(
        proof_rejected(&launched(&partial), &partial, &seen()),
        vec![IsolationViolation::MissingNamespace { ns: Namespace::Net }]
    );
}

#[test]
fn launched_facts_reject_namespace_shared_with_daemon() {
    let mut p = launched_proof();
    p.ns_inodes
        .insert(Namespace::Pid, own_ns()[&Namespace::Pid]);
    assert_eq!(
        proof_rejected(&launched(&p), &p, &seen()),
        vec![IsolationViolation::MissingNamespace { ns: Namespace::Pid }]
    );
}

#[test]
fn launched_facts_reject_unknown_or_daemon_owner_from_binding() {
    let mut p = launched_proof();
    p.ns_owner_uid = None;
    let v = proof_rejected(&launched(&p), &p, &seen());
    assert!(v.contains(&IsolationViolation::OwnerUnknown), "{v:?}");
    assert!(
        v.contains(&invalid(LauncherProofDefect::OwnerUnknown)),
        "{v:?}"
    );
    p.ns_owner_uid = Some(1000);
    let v = proof_rejected(&launched(&p), &p, &seen());
    assert!(
        v.contains(&IsolationViolation::UsernsOwnedByDaemon),
        "{v:?}"
    );
    assert!(
        v.contains(&invalid(LauncherProofDefect::OwnerIsDaemon)),
        "{v:?}"
    );
}

#[test]
fn launched_facts_keep_daemon_observed_violations() {
    // daemon が自分で読んだ status・mountinfo の違反は束縛で消えない。
    let status = RUNTIME_STATUS
        .replace("NoNewPrivs:\t1", "NoNewPrivs:\t0")
        .replace("Uid:\t200001", "Uid:\t1000");
    let mountinfo = RUNTIME_MOUNTINFO.replace("/ / ro,nosuid", "/ / rw,nosuid");
    let f = launched_runtime_facts(
        "s1",
        4242,
        1000,
        &own_ns(),
        ProcView::parse(&status, &mountinfo),
        &launched_proof(),
    );
    let v = proof_rejected(&f, &launched_proof(), &seen());
    for want in [
        IsolationViolation::SameUid,
        IsolationViolation::RootWritable,
        IsolationViolation::PrivilegesKept,
    ] {
        assert!(v.contains(&want), "{want:?} missing in {v:?}");
    }
    // 解釈できない uid は root 扱い（安全値で埋めない）。
    let f = launched_runtime_facts(
        "s1",
        4242,
        1000,
        &own_ns(),
        ProcView::parse("NoNewPrivs:\t1\n", RUNTIME_MOUNTINFO),
        &launched_proof(),
    );
    assert!(proof_rejected(&f, &launched_proof(), &seen()).contains(&IsolationViolation::RootUid));
}

#[test]
fn collect_launched_runtime_facts_errors_instead_of_filling() {
    assert_eq!(collect_ns_inodes("self").expect("own ns").len(), 6);
    assert!(collect_ns_inodes("-1").is_err());
    // 消えた process は Err（呼び出し側は拒否する）。
    assert!(collect_launched_runtime_facts("s1", -1, 4242, &launched_proof()).is_err());
    // 自分自身を runtime と偽っても、束縛の inode が daemon と同じなら namespace は別にならない。
    let mut p = launched_proof();
    p.ns_inodes = collect_ns_inodes("self").expect("own ns");
    let pid = std::process::id() as i32;
    let f = collect_launched_runtime_facts("s1", pid, 4242, &p).expect("own facts");
    assert!(f.namespaces.is_empty(), "{:?}", f.namespaces);
    assert!(verify_launcher_session(&f, Some(&p), &seen()).is_err());
}
