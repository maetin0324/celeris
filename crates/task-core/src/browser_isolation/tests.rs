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
    }
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

// ADR-0116 D-L: launcher session 証明。

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
