use super::*;

fn grant() -> BrowserCapability {
    BrowserCapability {
        allowed_domains: vec!["https://example.com".into(), "https://*.example.org".into()],
        allowed_actions: None,
        credential_policy_ids: vec!["cred-policy-1".into()],
        live_view_url: None,
    }
}

fn task(actions: &[BrowserAction]) -> BrowserTaskPolicy {
    BrowserTaskPolicy {
        policy_id: "task-policy".into(),
        revision: 1,
        domain_mode: BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec![
            "https://example.com".into(),
            "https://app.example.org".into(),
        ],
        allowed_actions: actions.to_vec(),
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    }
}

fn derive(
    grant: &BrowserCapability,
    task: &BrowserTaskPolicy,
) -> Result<EffectiveBrowserPolicy, BrowserPolicyError> {
    EffectiveBrowserPolicy::derive(grant, Some(task), &BrowserAction::ALL, "0.38.1")
}

#[test]
fn generator_emits_sorted_nonempty_allow_with_default_deny() {
    let policy = derive(
        &grant(),
        &task(&[
            BrowserAction::Snapshot,
            BrowserAction::Extract,
            BrowserAction::Snapshot,
        ]),
    )
    .unwrap();
    let file = policy.harness_action_policy().unwrap();
    assert_eq!(
        serde_json::to_string(&file).unwrap(),
        r#"{"default":"deny","allow":["close","gettext","launch","snapshot"]}"#
    );
    assert_eq!(
        policy.allowed_domains_arg(),
        "https://app.example.org,https://example.com"
    );
    let all = derive(&grant(), &task(&BrowserAction::PHASE1)).unwrap();
    assert_eq!(
        all.harness_action_policy().unwrap().allow,
        HARNESS_UPSTREAM_ACTIONS.map(String::from).to_vec()
    );
    // Phase 2 always gates click/download behind a human even if the task omitted it.
    assert!(all.requires_approval(BrowserAction::Click));
    assert!(all.requires_approval(BrowserAction::Download));
    assert!(!all.requires_approval(BrowserAction::Snapshot));
}

#[test]
fn empty_effective_sets_are_errors_before_launch() {
    assert_eq!(
        derive(&grant(), &task(&[])),
        Err(BrowserPolicyError::EmptyActions)
    );
    // Only actions outside the grant or backend: the intersection is empty.
    let mut narrow = grant();
    narrow.allowed_actions = Some(vec![BrowserAction::Snapshot]);
    assert_eq!(
        derive(&narrow, &task(&[BrowserAction::Click])),
        Err(BrowserPolicyError::EmptyActions)
    );
    assert_eq!(
        EffectiveBrowserPolicy::derive(
            &grant(),
            Some(&task(&[BrowserAction::Click])),
            &[BrowserAction::Snapshot],
            "0.38.1"
        ),
        Err(BrowserPolicyError::EmptyActions)
    );
    // credential_use alone never yields a general harness policy.
    let mut cred = task(&[BrowserAction::CredentialUse]);
    cred.credential_policy_ids = vec!["cred-policy-1".into()];
    let mut cred_grant = grant();
    cred_grant.allowed_actions = Some(BrowserAction::ALL.to_vec());
    assert_eq!(
        derive(&cred_grant, &cred),
        Err(BrowserPolicyError::EmptyActions)
    );
    let mut outside = task(&[BrowserAction::Snapshot]);
    outside.network_domains = vec!["https://evil.example".into(), "https://example.org".into()];
    assert_eq!(
        derive(&grant(), &outside),
        Err(BrowserPolicyError::EmptyDomains)
    );
    assert_eq!(
        EffectiveBrowserPolicy::derive(&grant(), None, &BrowserAction::ALL, "0.38.1"),
        Err(BrowserPolicyError::BrowserPolicyRequired)
    );
}

#[test]
fn privileged_upstream_actions_never_enter_generated_policy() {
    let mut full = grant();
    full.allowed_actions = Some(BrowserAction::ALL.to_vec());
    let mut t = task(&BrowserAction::ALL);
    t.credential_policy_ids = vec!["cred-policy-1".into()];
    let allow = derive(&full, &t)
        .unwrap()
        .harness_action_policy()
        .unwrap()
        .allow;
    for forbidden in [
        "evaluate",
        "eval",
        "waitforfunction",
        "cookies_get",
        "cookies_set",
        "storage_get",
        "state_save",
        "auth_save",
        "auth_login",
        "cdp_url",
        "fill",
        "type",
        "press",
        "upload",
        "extract",
        CREDENTIAL_PLUGIN_ACTION,
    ] {
        assert!(!allow.iter().any(|a| a == forbidden), "granted {forbidden}");
    }
    assert!(
        allow
            .iter()
            .all(|a| HARNESS_UPSTREAM_ACTIONS.contains(&a.as_str()))
    );
}

#[test]
fn unknown_actions_broken_schema_and_grant_expansion_are_rejected() {
    let base = serde_json::to_value(task(&[BrowserAction::Snapshot])).unwrap();
    for (field, value) in [
        (
            "allowed_actions",
            serde_json::json!(["snapshot", "evaluate"]),
        ),
        ("allowed_actions", serde_json::json!(["cookies_get"])),
        ("allowed_actions", serde_json::json!(["state_save"])),
        ("approval_actions", serde_json::json!(["gettext"])),
    ] {
        let mut v = base.clone();
        v[field] = value;
        assert_eq!(
            BrowserTaskPolicy::from_json(&v.to_string()),
            Err(BrowserPolicyError::UnknownAction),
            "{field}"
        );
    }
    let mut extra = base.clone();
    extra["allowed_domains_extra"] = serde_json::json!(["https://evil.example"]);
    for text in [
        extra.to_string(),
        "{".into(),
        "[]".into(),
        r#"{"policy_id":"p"}"#.into(),
    ] {
        assert_eq!(
            BrowserTaskPolicy::from_json(&text),
            Err(BrowserPolicyError::InvalidPolicy)
        );
    }
    assert!(BrowserTaskPolicy::from_json(&base.to_string()).is_ok());

    let mut approval = task(&[BrowserAction::Snapshot]);
    approval.approval_actions = vec![BrowserAction::Click];
    assert_eq!(
        derive(&grant(), &approval),
        Err(BrowserPolicyError::ApprovalNotAllowed)
    );
    let mut credential = task(&[BrowserAction::Snapshot]);
    credential.credential_policy_ids = vec!["not-granted".into()];
    assert_eq!(
        derive(&grant(), &credential),
        Err(BrowserPolicyError::CredentialPolicyNotGranted)
    );
    let mut origins = task(&[BrowserAction::Snapshot]);
    origins.domain_mode = BrowserDomainMode::SeparateOrigins;
    origins.navigation_origins = vec!["https://example.com".into()];
    assert_eq!(
        derive(&grant(), &origins),
        Err(BrowserPolicyError::UnsupportedOriginSeparation)
    );
    // Known actions outside the grant are dropped by intersection, not widened.
    let mut narrow = grant();
    narrow.allowed_actions = Some(vec![BrowserAction::Snapshot]);
    let effective = derive(&narrow, &task(&BrowserAction::PHASE1)).unwrap();
    assert_eq!(effective.actions, BTreeSet::from([BrowserAction::Snapshot]));
}

#[test]
fn domain_patterns_are_normalized_and_intersected_by_containment() {
    for bad in [
        "*",
        "",
        "*.com",
        "exa mple.com",
        "user@example.com",
        "example.com:443",
        "ftp://example.com",
        "example.com/path",
        "example..com",
        "a.*.example.com",
        "ｅxample.com",
        "bücher.example",
    ] {
        assert_eq!(
            parse_allowed_origin(bad).map(|o| o.canonical()),
            Err(BrowserPolicyError::InvalidDomain),
            "{bad}"
        );
    }
    assert_eq!(
        parse_allowed_origin("https://App.Example.COM:443")
            .unwrap()
            .canonical(),
        "https://app.example.com"
    );
    assert_eq!(
        intersect_origins("https://*.example.org", "https://app.example.org").as_deref(),
        Some("https://app.example.org")
    );
    assert_eq!(
        intersect_origins("https://*.example.org", "https://*.sub.example.org").as_deref(),
        Some("https://*.sub.example.org")
    );
    assert_eq!(
        intersect_origins("https://*.example.org", "https://example.org"),
        None
    );
    assert_eq!(
        intersect_origins("https://example.org", "https://evil-example.org"),
        None
    );
    assert_eq!(
        intersect_origins("https://*.example.org", "https://notexample.org"),
        None
    );
    let mut t = task(&[BrowserAction::Snapshot]);
    t.network_domains = vec![
        "https://*.example.org".into(),
        "https://a.example.org".into(),
        "https://EXAMPLE.com".into(),
    ];
    assert_eq!(
        derive(&grant(), &t).unwrap().allowed_domains,
        ["https://*.example.org", "https://example.com"]
    );
    assert_eq!(
        normalize_https_origin("https://Example.com:443").as_deref(),
        Some("https://example.com")
    );
    for bad in [
        "http://example.com",
        "https://u@example.com",
        "https://example.com/",
        "https://example.com:0",
    ] {
        assert_eq!(normalize_https_origin(bad), None, "{bad}");
    }
}

#[test]
fn page_or_model_originated_expansion_is_rejected_and_hash_binds_changes() {
    let current = derive(
        &grant(),
        &task(&[BrowserAction::Snapshot, BrowserAction::Extract]),
    )
    .unwrap();
    let mut narrower = task(&[BrowserAction::Snapshot]);
    narrower.revision = 2;
    narrower.network_domains = vec!["https://example.com".into()];
    assert!(narrower.check_narrowing(&current).is_ok());
    type Edit = Box<dyn Fn(&mut BrowserTaskPolicy)>;
    let expansions: Vec<Edit> = vec![
        Box::new(|p| p.allowed_actions.push(BrowserAction::Download)),
        Box::new(|p| p.network_domains.push("https://evil.example".into())),
        Box::new(|p| p.network_domains = vec!["https://*.example.org".into()]),
        Box::new(|p| p.credential_policy_ids = vec!["cred-policy-1".into()]),
        Box::new(|p| p.revision = 1),
        Box::new(|p| p.policy_id = "other".into()),
        Box::new(|p| p.artifact_policy_id = Some("keep-all".into())),
    ];
    for expand in expansions {
        let mut request = narrower.clone();
        expand(&mut request);
        assert_eq!(
            request.check_narrowing(&current),
            Err(BrowserPolicyError::PolicyExpansion)
        );
    }

    let binding = current.binding();
    assert_eq!(binding.revision, 1);
    assert!(binding.hash.starts_with("sha256:") && binding.hash.len() == 71);
    assert_eq!(
        binding.hash,
        derive(
            &grant(),
            &task(&[BrowserAction::Extract, BrowserAction::Snapshot])
        )
        .unwrap()
        .hash()
    );
    let mut bumped = task(&[BrowserAction::Snapshot, BrowserAction::Extract]);
    bumped.revision = 2;
    assert_ne!(binding.hash, derive(&grant(), &bumped).unwrap().hash());
    let mut regrant = grant();
    regrant.allowed_domains = vec!["https://example.com".into()];
    assert_ne!(
        binding.hash,
        derive(
            &regrant,
            &task(&[BrowserAction::Snapshot, BrowserAction::Extract])
        )
        .unwrap()
        .hash()
    );
}

#[test]
fn legacy_grant_json_keeps_phase1_actions_and_no_credentials() {
    let legacy: BrowserCapability =
        serde_json::from_str(r#"{"allowed_domains":["example.com"]}"#).unwrap();
    assert!(legacy.credential_policy_ids.is_empty());
    assert_eq!(
        serde_json::to_string(&legacy).unwrap(),
        r#"{"allowed_domains":["https://example.com"]}"#
    );
    let mut t = task(&[BrowserAction::CredentialUse, BrowserAction::Snapshot]);
    t.network_domains = vec!["https://example.com".into()];
    let effective = derive(&legacy, &t).unwrap();
    assert!(!effective.actions.contains(&BrowserAction::CredentialUse));
    assert_eq!(effective.allowed_domains, ["https://example.com"]);
}

#[test]
fn browser_allowed_domains_reject_invalid_origins() {
    for bad in [
        "*",
        "https://*",
        "https://*.com",
        "https://*.co.jp",
        "https://*.github.io",
        "https://*.example.jp",
        "https://*.192.0.2.1",
        "https://u:p@example.com",
        "https://example.com/path",
        "https://example.com/?q=1",
        "https://example.com/#fragment",
        "ftp://example.com",
        "http://example.com",
        "https://example.com:0",
        "https://example.com:65536",
        "https://[::1]evil",
    ] {
        assert!(parse_allowed_origin(bad).is_err(), "{bad}");
    }
    assert_eq!(
        parse_allowed_origin("http://localhost:80/")
            .unwrap()
            .canonical(),
        "http://localhost"
    );
    assert_eq!(
        parse_allowed_origin("http://[::1]:3000")
            .unwrap()
            .canonical(),
        "http://[::1]:3000"
    );
}

#[test]
fn browser_allowed_domains_intersection_checks_scheme_host_and_port() {
    assert_eq!(
        intersect_origins("https://*.example.com", "https://app.example.com"),
        Some("https://app.example.com".into())
    );
    assert_eq!(
        intersect_origins(
            "https://*.example.com:8443",
            "https://*.sub.example.com:8443"
        ),
        Some("https://*.sub.example.com:8443".into())
    );
    assert_eq!(
        intersect_origins("https://*.example.com", "https://example.com"),
        None
    );
    assert_eq!(
        intersect_origins("https://*.example.com", "https://app.example.com:8443"),
        None
    );
    assert_eq!(
        intersect_origins("http://localhost:3000", "https://localhost:3000"),
        None
    );
    let mut grant = grant();
    grant.allowed_domains = vec!["https://*.example.com:8443".into()];
    let mut task = task(&[BrowserAction::Snapshot]);
    task.network_domains = vec!["https://app.example.com:8443".into()];
    assert_eq!(
        derive(&grant, &task).unwrap().allowed_domains,
        ["https://app.example.com:8443"]
    );
    task.network_domains = vec!["https://app.example.com".into()];
    assert_eq!(derive(&grant, &task), Err(BrowserPolicyError::EmptyDomains));
}

#[test]
fn browser_allowed_domains_containment_respects_apex_and_default_port() {
    assert!(origin_covers(
        "https://*.example.com",
        "https://app.example.com:443"
    ));
    assert!(origin_covers(
        "https://*.example.com",
        "https://*.sub.example.com"
    ));
    assert!(!origin_covers(
        "https://*.example.com",
        "https://example.com"
    ));
    assert!(!origin_covers(
        "https://*.example.com",
        "https://app.example.com:8443"
    ));
    assert!(!origin_covers("http://localhost", "https://localhost"));
}

#[test]
fn browser_allowed_domains_derive_minimizes_overlapping_intersections() {
    let mut grant = grant();
    grant.allowed_domains = vec![
        "https://*.example.com".into(),
        "https://app.example.com".into(),
    ];
    let mut task = task(&[BrowserAction::Snapshot]);
    task.network_domains = vec![
        "https://*.sub.example.com".into(),
        "https://app.sub.example.com".into(),
    ];
    assert_eq!(
        derive(&grant, &task).unwrap().allowed_domains,
        ["https://*.sub.example.com"]
    );
}

#[test]
fn browser_allowed_domains_seed_grants_validate() {
    let toml: toml::Value =
        toml::from_str(include_str!("../../../../config/org.example.toml")).unwrap();
    let from_toml: BrowserCapability = toml["org"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"].as_str() == Some("browser-execution"))
        .unwrap()["profile"]["browser"]
        .clone()
        .try_into()
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../docs/ops/browser-department-org.json"
    ))
    .unwrap();
    let from_json: BrowserCapability =
        serde_json::from_value(json["profile"]["browser"].clone()).unwrap();
    let expected = ["http://localhost:3000", "http://127.0.0.1:3000"];
    for grant in [from_toml, from_json] {
        grant.validate().unwrap();
        assert_eq!(grant.allowed_domains, expected);
    }
}

fn requirements(origins: &[&str]) -> crate::TaskRequirements {
    crate::TaskRequirements {
        browser: Some(crate::BrowserRequirements {
            allowed_domains: origins.iter().map(|o| o.to_string()).collect(),
        }),
    }
}

/// D2.0: 実効許可は task の requirements ∩ 保存 policy ∩ grant。wildcard の grant から task の
/// 単一 origin だけが残り、grant 全体には広がらない。
#[test]
fn browser_allowed_domains_intersection_is_task_and_grant() {
    let grant = BrowserCapability {
        allowed_domains: vec![
            "https://*.example.com".into(),
            "http://127.0.0.1:3000".into(),
        ],
        ..Default::default()
    };
    let mut stored = task(&[BrowserAction::Navigate]);
    stored.network_domains = vec!["https://*.example.com".into()];
    let policy = task_run_policy(
        &requirements(&["https://billing.example.com", "https://other.test"]),
        Some(&stored),
    )
    .unwrap()
    .unwrap();
    assert_eq!(policy.network_domains, ["https://billing.example.com"]);
    let effective = derive(&grant, &policy).unwrap();
    assert_eq!(effective.allowed_domains, ["https://billing.example.com"]);
    // Narrowing twice (dispatch, then worker) is idempotent and binds the same hash.
    let again = task_run_policy(
        &requirements(&["https://billing.example.com", "https://other.test"]),
        Some(&policy),
    )
    .unwrap()
    .unwrap();
    assert_eq!(derive(&grant, &again).unwrap().hash(), effective.hash());
    // Tasks created before the requirement keep their stored policy as is.
    assert_eq!(
        task_run_policy(&crate::TaskRequirements::default(), Some(&stored)).unwrap(),
        Some(stored.clone())
    );
    assert_eq!(task_run_policy(&requirements(&[]), None).unwrap(), None);
}

/// D2.0: scheme・port が違う origin は交差に入らず、交差が空なら固定コードで拒否する。
#[test]
fn browser_allowed_domains_scheme_and_port_mismatch_leave_empty_intersection() {
    let mut stored = task(&[BrowserAction::Navigate]);
    stored.network_domains = vec![
        "https://*.example.com".into(),
        "http://127.0.0.1:3000".into(),
    ];
    for outside in [
        "https://127.0.0.1:3000",
        "http://127.0.0.1:3001",
        "https://billing.example.com:8443",
        "https://example.com",
        "https://billing.example.net",
    ] {
        assert_eq!(
            task_run_policy(&requirements(&[outside]), Some(&stored)),
            Err(BrowserPolicyError::EmptyDomains),
            "{outside}"
        );
    }
    // Plain HTTP outside loopback is not an origin at all: also refused.
    assert_eq!(
        task_run_policy(
            &requirements(&["http://billing.example.com"]),
            Some(&stored)
        ),
        Err(BrowserPolicyError::InvalidDomain)
    );
    assert_eq!(
        task_run_policy(&requirements(&[]), Some(&stored)),
        Err(BrowserPolicyError::EmptyDomains)
    );
    // Within the task but outside the grant: the effective set is empty, so no run.
    let grant = BrowserCapability {
        allowed_domains: vec!["https://app.example.com".into()],
        ..Default::default()
    };
    let policy = task_run_policy(
        &requirements(&["https://billing.example.com"]),
        Some(&stored),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        derive(&grant, &policy).unwrap_err(),
        BrowserPolicyError::EmptyDomains
    );
    assert_eq!(
        BrowserPolicyError::EmptyDomains.code(),
        "empty_browser_domains"
    );
}
