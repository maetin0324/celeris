use super::*;

const BASIC: [Capability; 5] = [
    Capability::Navigate,
    Capability::Snapshot,
    Capability::Click,
    Capability::Fill,
    Capability::Screenshot,
];

fn backend(id: &str, kind: BackendKind, caps: &[Capability]) -> BackendDescriptor {
    BackendDescriptor {
        id: id.into(),
        kind,
        version: "1".into(),
        declared: caps.iter().copied().collect(),
        enabled: true,
    }
}

fn passing(b: &BackendDescriptor) -> ConformanceResult {
    ConformanceResult {
        backend_id: b.id.clone(),
        version: b.version.clone(),
        passed: b
            .declared
            .iter()
            .flat_map(|c| required_cases(*c).iter().copied())
            .collect(),
        evidence: full_evidence(),
    }
}

fn full_evidence() -> Vec<ConformanceEvidence> {
    [
        FixtureCase::IsolationSuite,
        FixtureCase::EgressNegativeSuite,
        FixtureCase::InjectionAttackSuite,
        FixtureCase::AuthSectionObservationStop,
    ]
    .into_iter()
    .flat_map(|case| {
        required_evidence(case)
            .into_iter()
            .map(move |test| ConformanceEvidence {
                case,
                test,
                outcome: EvidenceOutcome::Passed,
                runtime: if matches!(
                    case,
                    FixtureCase::IsolationSuite | FixtureCase::EgressNegativeSuite
                ) {
                    EvidenceRuntime::Launcher
                } else {
                    EvidenceRuntime::Daemon
                },
            })
    })
    .collect()
}

/// ADR-0112: P4-B の件は試験名つきの実測証拠からしか通らない。
#[test]
fn p4b_cases_count_only_with_measured_evidence() {
    let b = backend(
        "acp",
        BackendKind::ExistingLoop,
        &[Capability::Navigate, Capability::CredentialInjection],
    );
    assert!(
        certify(&b, Some(&passing(&b)))
            .unwrap()
            .contains(&Capability::CredentialInjection)
    );
    let gap = |r: &ConformanceResult| {
        matches!(
            &certify(&b, Some(r)).unwrap_err()[..],
            [ConformanceGap::Missing {
                capability: Capability::CredentialInjection,
                ..
            }]
        )
    };
    // 件名だけ（旧形式の ledger）は通らない
    let mut bare = passing(&b);
    bare.evidence.clear();
    assert!(gap(&bare));
    // Isolation and egress require launcher measurements; P4-B observations
    // remain daemon measurements, matching the conformance generator output.
    let mut wrong_runtime = passing(&b);
    for evidence in &mut wrong_runtime.evidence {
        if matches!(
            evidence.case,
            FixtureCase::IsolationSuite | FixtureCase::EgressNegativeSuite
        ) {
            evidence.runtime = EvidenceRuntime::Daemon;
        }
    }
    assert!(gap(&wrong_runtime));
    let mut wrong_runtime = passing(&b);
    for evidence in &mut wrong_runtime.evidence {
        if matches!(
            evidence.case,
            FixtureCase::InjectionAttackSuite | FixtureCase::AuthSectionObservationStop
        ) {
            evidence.runtime = EvidenceRuntime::Launcher;
        }
    }
    assert!(gap(&wrong_runtime));
    // 攻撃の印が 1 つ欠けても通らない
    let mut short = passing(&b);
    short
        .evidence
        .retain(|e| e.test != attack_evidence_name("A8"));
    assert!(gap(&short));
    // 失敗・未実施の証拠が 1 件あれば通らない
    for outcome in [EvidenceOutcome::Failed, EvidenceOutcome::NotRun] {
        let mut bad = passing(&b);
        bad.evidence.push(ConformanceEvidence {
            case: FixtureCase::AuthSectionObservationStop,
            test: P4B_H3_TESTS[0].into(),
            outcome,
            runtime: EvidenceRuntime::Daemon,
        });
        assert!(gap(&bad));
    }
    // 別の件の証拠で代えられない
    let mut swapped = passing(&b);
    for e in &mut swapped.evidence {
        e.case = FixtureCase::IsolationSuite;
    }
    assert!(gap(&swapped));
}

fn req(caps: &[Capability]) -> RoutingRequest {
    RoutingRequest {
        required: caps.iter().copied().collect(),
        explicit: None,
        failed: BTreeSet::new(),
    }
}

#[test]
fn fixture_certifies_declared_capabilities() {
    let b = backend("acp", BackendKind::ExistingLoop, &BASIC);
    assert_eq!(certify(&b, Some(&passing(&b))).unwrap(), b.declared);
    assert_eq!(
        certify(&b, None).unwrap_err(),
        vec![ConformanceGap::NoResult]
    );
    let mut r = passing(&b);
    r.version = "0".into();
    assert!(matches!(
        certify(&b, Some(&r)).unwrap_err()[..],
        [ConformanceGap::StaleVersion { .. }]
    ));
    // 非機密の欠けはその能力だけを落とす
    let mut r = passing(&b);
    r.passed.remove(&FixtureCase::ClickByRef);
    let ok = certify(&b, Some(&r)).unwrap();
    assert!(!ok.contains(&Capability::Click));
    assert!(ok.contains(&Capability::Navigate));
}

#[test]
fn sensitive_declaration_requires_p4a_and_p4b_conformance() {
    let mut caps = BASIC.to_vec();
    caps.push(Capability::CredentialInjection);
    let b = backend("spec", BackendKind::BrowserSpecialist, &caps);
    for missing in [
        FixtureCase::IsolationSuite,
        FixtureCase::EgressNegativeSuite,
        FixtureCase::InjectionAttackSuite,
        FixtureCase::AuthSectionObservationStop,
    ] {
        let mut r = passing(&b);
        r.passed.remove(&missing);
        // 機密の宣言が裏付けられない backend は丸ごと使わない
        let gaps = certify(&b, Some(&r)).unwrap_err();
        assert!(matches!(
            &gaps[..],
            [ConformanceGap::Missing {
                capability: Capability::CredentialInjection,
                ..
            }]
        ));
        let results = [(b.id.clone(), r)].into_iter().collect();
        assert!(
            route(
                std::slice::from_ref(&b),
                &results,
                &req(&[Capability::Navigate])
            )
            .is_err()
        );
    }
    let rb = backend(
        "id",
        BackendKind::BrowserSpecialist,
        &[Capability::IdentityRestore],
    );
    for missing in [
        FixtureCase::IsolationSuite,
        FixtureCase::EgressNegativeSuite,
        FixtureCase::InjectionAttackSuite,
        FixtureCase::AuthSectionObservationStop,
    ] {
        let mut r = passing(&rb);
        r.passed.remove(&missing);
        assert!(certify(&rb, Some(&r)).is_err(), "missing {missing:?}");
        let results = [(rb.id.clone(), r)].into_iter().collect();
        assert!(
            route(
                std::slice::from_ref(&rb),
                &results,
                &req(&[Capability::IdentityRestore])
            )
            .is_err()
        );
    }
}

fn fleet() -> (Vec<BackendDescriptor>, BTreeMap<String, ConformanceResult>) {
    let mut full = BASIC.to_vec();
    full.push(Capability::LiveView);
    let bs = vec![
        backend("acp", BackendKind::ExistingLoop, &full),
        backend("claude", BackendKind::ExistingLoop, &BASIC),
        backend("specialist", BackendKind::BrowserSpecialist, &full),
    ];
    let rs = bs.iter().map(|b| (b.id.clone(), passing(b))).collect();
    (bs, rs)
}

#[test]
fn routing_prefers_specialist_then_existing_loops_and_fallback_keeps_capabilities() {
    let (bs, rs) = fleet();
    let d = route(
        &bs,
        &rs,
        &req(&[Capability::Navigate, Capability::LiveView]),
    )
    .unwrap();
    assert_eq!(d.primary, "specialist");
    // claude は LiveView を持たないので fallback に入らない
    assert_eq!(d.fallbacks, vec!["acp".to_string()]);
    let d = route(&bs, &rs, &req(&[Capability::Navigate])).unwrap();
    assert_eq!(d.fallbacks, vec!["acp".to_string(), "claude".to_string()]);
}

#[test]
fn specialist_disabled_until_decision_reuses_existing_loop() {
    let (mut bs, rs) = fleet();
    bs[2].enabled = false;
    let d = route(&bs, &rs, &req(&[Capability::Navigate])).unwrap();
    assert_eq!(d.primary, "acp");
    assert!(!d.fallbacks.contains(&"specialist".to_string()));
}

#[test]
fn fallback_after_failure_never_drops_capability() {
    let (bs, rs) = fleet();
    let mut r = req(&[Capability::LiveView]);
    r.failed = ["specialist".to_string()].into_iter().collect();
    assert_eq!(route(&bs, &rs, &r).unwrap().primary, "acp");
    r.failed.insert("acp".into());
    assert_eq!(
        route(&bs, &rs, &r).unwrap_err(),
        RoutingError::NoCapableBackend { missing: vec![] }
    );
    let err = route(&bs, &rs, &req(&[Capability::Download])).unwrap_err();
    assert_eq!(
        err,
        RoutingError::NoCapableBackend {
            missing: vec![Capability::Download]
        }
    );
}

#[test]
fn explicit_backend_is_honoured_or_refused() {
    let (bs, rs) = fleet();
    let mut r = req(&[Capability::Navigate]);
    r.explicit = Some("claude".into());
    let d = route(&bs, &rs, &r).unwrap();
    assert_eq!(d.primary, "claude");
    r.required.insert(Capability::LiveView);
    assert_eq!(
        route(&bs, &rs, &r).unwrap_err(),
        RoutingError::ExplicitUnavailable {
            backend: "claude".into()
        }
    );
}

#[test]
fn same_task_evaluation_ranks_and_refuses_mixed_fixtures() {
    let run = |id: &str, ok: bool, viol: u32, cost: f64| SameTaskRun {
        backend_id: id.into(),
        task_fixture: "login-and-download".into(),
        accepted: ok,
        policy_violations: viol,
        recoveries: 0,
        cost_usd: cost,
        wall_secs: 60,
    };
    let runs = vec![
        run("a", true, 1, 0.1),
        run("b", true, 0, 0.5),
        run("c", false, 0, 0.1),
        run("d", true, 0, 0.2),
    ];
    assert_eq!(rank_same_task(&runs).unwrap(), vec!["d", "b", "c", "a"]);
    let mut mixed = runs.clone();
    mixed[0].task_fixture = "other".into();
    assert!(rank_same_task(&mixed).is_none());
    assert!(rank_same_task(&[]).is_none());
}
