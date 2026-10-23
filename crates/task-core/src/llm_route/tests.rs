use super::*;

#[test]
fn wire_model_id_qualifies_only_routes_declared_via_llm_proxy() {
    let mut registry = Registry::default();
    let harness: HarnessId = "h".into();
    let key = CandidateKey {
        source: "qwen".into(),
        model: "m".into(),
    };
    registry.llm_routes.push(LlmRoute {
        harness: harness.clone(),
        key: key.clone(),
        via: Some("llm_proxy".into()),
        from_api: None,
        to_api: None,
    });
    assert_eq!(wire_model_id(&registry, &harness, &key).as_ref(), "qwen/m");
    registry.llm_routes[0].via = Some("other".into());
    assert_eq!(wire_model_id(&registry, &harness, &key).as_ref(), "m");
}

#[test]
fn wire_model_id_keeps_a_model_that_already_contains_a_slash() {
    let mut registry = Registry::default();
    let harness: HarnessId = "h".into();
    let key = CandidateKey {
        source: "qwen".into(),
        model: "vendor/model".into(),
    };
    registry.llm_routes.push(LlmRoute {
        harness: harness.clone(),
        key: key.clone(),
        via: Some("llm_proxy".into()),
        from_api: None,
        to_api: None,
    });
    assert_eq!(
        wire_model_id(&registry, &harness, &key).as_ref(),
        "qwen/vendor/model"
    );
}
#[test]
fn route_priority_prefers_the_execution_class_list() {
    let mut r = Registry::default();
    let harness: HarnessId = "claude_code".into();
    let sonnet = key("claude_oauth", "claude-sonnet-5-5");
    let opus = key("claude_oauth", "claude-opus-5-5");
    r.route_defaults
        .insert(harness.clone(), vec![sonnet.clone()]);
    let coding: ExecutionClass = "coding".into();
    r.route_class_defaults
        .entry(harness.clone())
        .or_default()
        .insert(coding.clone(), vec![opus.clone()]);
    assert_eq!(
        route_priority(&r, &harness, None, false, Some(&coding)),
        std::slice::from_ref(&opus)
    );
    assert_eq!(
        route_priority(&r, &harness, None, false, Some(&"planner".into())),
        std::slice::from_ref(&sonnet)
    );
    assert_eq!(
        route_priority(&r, &harness, None, false, None),
        std::slice::from_ref(&sonnet)
    );
}

#[test]
fn route_priority_ignores_the_class_for_cos() {
    let mut r = Registry::default();
    let harness: HarnessId = "claude_code".into();
    let cos = key("claude_oauth", "claude-opus-5-5");
    r.cos_route_candidates = vec![cos.clone()];
    r.route_defaults.insert(
        harness.clone(),
        vec![key("claude_oauth", "claude-sonnet-5-5")],
    );
    r.route_class_defaults
        .entry(harness.clone())
        .or_default()
        .insert(
            "coding".into(),
            vec![key("claude_oauth", "claude-haiku-5-5")],
        );
    let coding: ExecutionClass = "coding".into();
    assert_eq!(
        route_priority(&r, &harness, None, true, Some(&coding)),
        std::slice::from_ref(&cos)
    );
    assert_eq!(
        route_priority(&r, &harness, None, true, None),
        std::slice::from_ref(&cos)
    );
}

fn key(source: &str, model: &str) -> CandidateKey {
    CandidateKey {
        source: source.into(),
        model: model.into(),
    }
}
fn harness(id: &str) -> HarnessId {
    id.into()
}
fn fixture() -> Registry {
    let mut r = Registry::default();
    r.harnesses.insert(
        harness("h"),
        HarnessSpec {
            kind: "cli".into(),
            api_formats: [ApiFormat::from("openai")].into_iter().collect(),
            source_kinds: [SourceKind::from("openai")].into_iter().collect(),
            capabilities: BTreeSet::new(),
            command: None,
            args: vec![],
        },
    );
    r.sources.insert(
        "s".into(),
        SourceSpec {
            kind: "openai".into(),
            api_format: "openai".into(),
            accounts: vec!["a".into()],
            models: vec!["m".into()],
            max_runs: Some(2),
            account_max_runs: BTreeMap::from([("a".into(), 1)]),
            quota: None,
            endpoint: None,
            credential_ref: None,
        },
    );
    r
}
fn candidate() -> Candidate {
    Candidate {
        source: "s".into(),
        model: "m".into(),
        api_format: "openai".into(),
    }
}
#[test]
fn harness_select_is_deterministic_and_rejects_same_precedence_duplicates() {
    let mut r = fixture();
    r.harnesses
        .insert("h2".into(), r.harnesses[&harness("h")].clone());
    r.harness_selection.rules = vec![
        HarnessRule {
            genre: Some("g".into()),
            harness: "h".into(),
            ..Default::default()
        },
        HarnessRule {
            genre: Some("g".into()),
            harness: "h2".into(),
            ..Default::default()
        },
    ];
    let req = HarnessRequest {
        genre: Some("g".into()),
        ..Default::default()
    };
    assert!(matches!(
        select_harness(&req, &r),
        Err(HarnessSelectionError::Ambiguous(_))
    ));
    r.harness_selection.rules.pop();
    assert_eq!(select_harness(&req, &r), Ok(harness("h")));
    assert_eq!(select_harness(&req, &r), Ok(harness("h")));
}
#[test]
fn route_select_filters_explicit_model_and_lane_is_ignored() {
    let r = fixture();
    let a = enumerate_candidates(
        &RouteRequest {
            task_id: "t".into(),
            harness: "h".into(),
            model_hint: Some("missing".into()),
            lane: Some("cheap".into()),
            execution_class: None,
        },
        &r,
    );
    assert!(a.is_empty());
}
#[test]
fn route_select_excludes_incompatible_harness() {
    let mut r = fixture();
    let mut h = r.harnesses[&harness("h")].clone();
    h.source_kinds.clear();
    let (_, rejected) = constrain_for_harness(vec![candidate()], &h, &r, &harness("h"));
    assert_eq!(rejected[0].reason, ExclusionReason::NoCompatibleRoute);
    r.llm_routes.clear();
}
#[test]
fn route_select_excludes_quota_cooldown_and_concurrency() {
    let r = fixture();
    let c = candidate();
    let mut q = QuotaSnapshot::default();
    let ac = AccountSnapshot {
        available: BTreeMap::from([("s".into(), vec!["a".into()])]),
        ..Default::default()
    };
    q.cooling_down.insert(("s".into(), "a".into()));
    assert_eq!(
        admit_source(&c, &ac, &q, &ReservationSnapshot::default(), &r),
        Admission::Rejected(ExclusionReason::Cooldown)
    );
    q.cooling_down.clear();
    let rs = ReservationSnapshot {
        account_in_use: BTreeMap::from([(("s".into(), "a".into()), 1)]),
        ..Default::default()
    };
    assert_eq!(
        admit_source(&c, &ac, &q, &rs, &r),
        Admission::Rejected(ExclusionReason::AccountConcurrency)
    );
    let source_busy = ReservationSnapshot {
        source_in_use: BTreeMap::from([("s".into(), 2)]),
        ..Default::default()
    };
    assert_eq!(
        admit_source(&c, &ac, &QuotaSnapshot::default(), &source_busy, &r),
        Admission::Rejected(ExclusionReason::SourceConcurrency)
    );
    let account_empty = QuotaSnapshot {
        account_remaining: BTreeMap::from([(("s".into(), "a".into()), 0.0)]),
        ..Default::default()
    };
    assert_eq!(
        admit_source(&c, &ac, &account_empty, &ReservationSnapshot::default(), &r),
        Admission::Rejected(ExclusionReason::AccountQuota)
    );
    q.source_remaining.insert("s".into(), 0.0);
    assert_eq!(
        admit_source(&c, &ac, &q, &ReservationSnapshot::default(), &r),
        Admission::Rejected(ExclusionReason::SourceQuota)
    );
}
#[test]
fn route_priority_is_the_lane_column_then_the_harness_column_then_cos() {
    let mut r = fixture();
    let harness_wide = CandidateKey {
        source: "s".into(),
        model: "m".into(),
    };
    let cheap = CandidateKey {
        source: "backup".into(),
        model: "m2".into(),
    };
    let cos = CandidateKey {
        source: "chat".into(),
        model: "m3".into(),
    };
    r.route_defaults
        .insert(harness("h"), vec![harness_wide.clone()]);
    r.lane_route_defaults
        .insert((harness("h"), "cheap".into()), vec![cheap.clone()]);
    r.cos_route_candidates = vec![cos.clone()];
    // A lane column wins over the harness-wide column; an unknown lane falls back to it.
    assert_eq!(
        route_priority(&r, &harness("h"), Some("cheap"), false, None).to_vec(),
        vec![cheap]
    );
    assert_eq!(
        route_priority(&r, &harness("h"), Some("frontier"), false, None).to_vec(),
        vec![harness_wide.clone()]
    );
    assert_eq!(
        route_priority(&r, &harness("h"), None, false, None).to_vec(),
        vec![harness_wide]
    );
    // CoS chat keeps its own declared list, and an undeclared harness has no default at all
    // (the caller fails closed; there is no lexicographic fallback).
    assert_eq!(
        route_priority(&r, &harness("h"), Some("cheap"), true, None).to_vec(),
        vec![cos]
    );
    assert!(route_priority(&r, &harness("missing"), None, false, None).is_empty());
}

/// An execution-class list replaces every other column, the lane column included (no fallback
/// from a declared class list); a class without a list leaves the lane column in charge.
#[test]
fn route_priority_class_list_wins_over_the_lane_column() {
    let mut r = fixture();
    let harness_wide = CandidateKey {
        source: "s".into(),
        model: "m".into(),
    };
    let cheap = CandidateKey {
        source: "backup".into(),
        model: "m2".into(),
    };
    let coding = CandidateKey {
        source: "s".into(),
        model: "opus".into(),
    };
    r.route_defaults.insert(harness("h"), vec![harness_wide]);
    r.lane_route_defaults
        .insert((harness("h"), "cheap".into()), vec![cheap.clone()]);
    r.route_class_defaults
        .entry(harness("h"))
        .or_default()
        .insert(ExecutionClass::from("coding"), vec![coding.clone()]);
    assert_eq!(
        route_priority(
            &r,
            &harness("h"),
            Some("cheap"),
            false,
            Some(&ExecutionClass::from("coding"))
        )
        .to_vec(),
        vec![coding]
    );
    assert_eq!(
        route_priority(
            &r,
            &harness("h"),
            Some("cheap"),
            false,
            Some(&ExecutionClass::from("planner"))
        )
        .to_vec(),
        vec![cheap]
    );
}
