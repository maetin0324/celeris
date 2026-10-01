use super::*;

fn grant() -> BrowserCapability {
    BrowserCapability {
        allowed_domains: vec!["example.com".into(), "*.example.org".into()],
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
        network_domains: vec!["example.com".into(), "app.example.org".into()],
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
    assert_eq!(policy.allowed_domains_arg(), "app.example.org,example.com");
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
    outside.network_domains = vec!["evil.example".into(), "example.org".into()];
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
    extra["allowed_domains_extra"] = serde_json::json!(["evil.example"]);
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
        "https://example.com",
        "example.com/path",
        "example..com",
        "a.*.example.com",
        "ｅxample.com",
        "bücher.example",
    ] {
        assert_eq!(
            normalize_host_pattern(bad),
            Err(BrowserPolicyError::InvalidDomain),
            "{bad}"
        );
    }
    assert_eq!(
        normalize_host_pattern("App.Example.COM").unwrap(),
        "app.example.com"
    );
    assert_eq!(
        intersect_hosts("*.example.org", "app.example.org").as_deref(),
        Some("app.example.org")
    );
    assert_eq!(
        intersect_hosts("*.example.org", "*.sub.example.org").as_deref(),
        Some("*.sub.example.org")
    );
    assert_eq!(intersect_hosts("*.example.org", "example.org"), None);
    assert_eq!(intersect_hosts("example.org", "evil-example.org"), None);
    assert_eq!(intersect_hosts("*.example.org", "notexample.org"), None);
    let mut t = task(&[BrowserAction::Snapshot]);
    t.network_domains = vec![
        "*.example.org".into(),
        "a.example.org".into(),
        "EXAMPLE.com".into(),
    ];
    assert_eq!(
        derive(&grant(), &t).unwrap().allowed_domains,
        ["*.example.org", "example.com"]
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
    narrower.network_domains = vec!["example.com".into()];
    assert!(narrower.check_narrowing(&current).is_ok());
    type Edit = Box<dyn Fn(&mut BrowserTaskPolicy)>;
    let expansions: Vec<Edit> = vec![
        Box::new(|p| p.allowed_actions.push(BrowserAction::Download)),
        Box::new(|p| p.network_domains.push("evil.example".into())),
        Box::new(|p| p.network_domains = vec!["*.example.org".into()]),
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
    regrant.allowed_domains = vec!["example.com".into()];
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
        r#"{"allowed_domains":["example.com"]}"#
    );
    let mut t = task(&[BrowserAction::CredentialUse, BrowserAction::Snapshot]);
    t.network_domains = vec!["example.com".into()];
    let effective = derive(&legacy, &t).unwrap();
    assert!(!effective.actions.contains(&BrowserAction::CredentialUse));
}
