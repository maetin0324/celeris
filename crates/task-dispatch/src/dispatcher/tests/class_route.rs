//! Deterministic planner/reviewer execution-class route admission coverage.
use crate::dispatcher::route_select::{RouteFailure, RouteInput, RouteState, route_select};
use std::collections::BTreeMap;
use task_core::llm_route::{
    AccountId, CandidateKey, HarnessId, HarnessRequest, HarnessRule, HarnessSpec, Registry,
    SourceId, SourceSpec,
};

fn registry(classes: bool) -> Registry {
    let mut r = Registry::default();
    for (id, kind, api, account, model) in [
        ("claude_oauth", "claude", "claude_cli", "ca", "sonnet"),
        ("codex_oauth", "codex", "codex_cli", "xa", "gpt"),
    ] {
        r.sources.insert(
            id.into(),
            SourceSpec {
                kind: kind.into(),
                api_format: api.into(),
                accounts: vec![account.into()],
                models: vec![model.into()],
                max_runs: None,
                account_max_runs: BTreeMap::new(),
                quota: None,
                endpoint: None,
                credential_ref: None,
            },
        );
        r.harnesses.insert(
            if kind == "claude" {
                "claude_code"
            } else {
                "codex"
            }
            .into(),
            HarnessSpec {
                kind: "cli".into(),
                api_formats: [api.into()].into(),
                source_kinds: [kind.into()].into(),
                capabilities: Default::default(),
                command: None,
                args: vec![],
            },
        );
        let harness: HarnessId = if kind == "claude" {
            "claude_code"
        } else {
            "codex"
        }
        .into();
        r.route_defaults.insert(
            harness,
            vec![CandidateKey {
                source: id.into(),
                model: model.into(),
            }],
        );
    }
    r.harness_selection.rules.push(HarnessRule {
        genre: Some("coding".into()),
        harness: "claude_code".into(),
        ..Default::default()
    });
    if classes {
        r.harness_selection
            .execution_class_defaults
            .insert("planner".into(), vec!["claude_code".into(), "codex".into()]);
        r.harness_selection
            .execution_class_defaults
            .insert("review".into(), vec!["claude_code".into(), "codex".into()]);
    }
    r
}

fn state(r: &Registry, claude_full: bool, codex_full: bool) -> RouteState {
    let mut s = RouteState::default();
    for (id, spec) in &r.sources {
        s.accounts
            .available
            .insert(id.clone(), spec.accounts.clone());
        if (id.as_ref() == "claude_oauth" && claude_full)
            || (id.as_ref() == "codex_oauth" && codex_full)
        {
            s.account_max_runs
                .insert((id.clone(), spec.accounts[0].clone()), 1);
            s.reservations
                .account_in_use
                .insert((id.clone(), spec.accounts[0].clone()), 1);
        }
    }
    s
}

fn input(class: Option<&str>) -> RouteInput {
    RouteInput {
        task_id: "class-route".into(),
        harness: HarnessRequest {
            genre: Some("coding".into()),
            execution_class: class.map(Into::into),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn planner_and_review_class_routes_skip_full_first_harness_and_defer_when_all_full() {
    let r = registry(true);
    for class in ["planner", "review"] {
        let (pick, trace) = route_select(&input(Some(class)), &r, &state(&r, true, false));
        assert_eq!(pick.unwrap().harness, "codex".into());
        assert_eq!(trace.harness_rejections[0].harness, "claude_code".into());
        assert!(matches!(
            route_select(&input(Some(class)), &r, &state(&r, true, true)).0,
            Err(RouteFailure::NoEligible(_))
        ));
    }
}

#[test]
fn undeclared_class_uses_only_normal_genre_selection() {
    let r = registry(false);
    let (pick, trace) = route_select(&input(Some("planner")), &r, &state(&r, true, false));
    assert!(matches!(pick, Err(RouteFailure::NoEligible(_))));
    assert_eq!(trace.harness_rejections.len(), 1);
    assert_eq!(trace.harness_rejections[0].harness, "claude_code".into());
}

#[test]
fn an_ineligible_harness_wins_over_a_later_undeclared_default() {
    let mut r = registry(true);
    r.harness_selection
        .execution_class_defaults
        .insert("planner".into(), vec!["claude_code".into(), "codex".into()]);
    r.route_defaults.remove(&"codex".into());
    let (pick, _) = route_select(&input(Some("planner")), &r, &state(&r, true, false));
    assert!(matches!(pick, Err(RouteFailure::NoEligible(_))));
}

#[allow(dead_code)]
fn _type_check_reservation_ids(_: (SourceId, AccountId)) {}

/// claude_code on claude_oauth: the harness default is sonnet; the coding class list is opus.
fn override_registry() -> Registry {
    let mut r = Registry::default();
    r.sources.insert(
        "claude_oauth".into(),
        SourceSpec {
            kind: "claude_oauth".into(),
            api_format: "claude_cli".into(),
            accounts: vec!["ca".into()],
            models: vec!["claude-sonnet-5-5".into(), "claude-opus-5-5".into()],
            max_runs: None,
            account_max_runs: BTreeMap::new(),
            quota: None,
            endpoint: None,
            credential_ref: None,
        },
    );
    r.harnesses.insert(
        "claude_code".into(),
        HarnessSpec {
            kind: "claude_code".into(),
            api_formats: ["claude_cli".into()].into(),
            source_kinds: ["claude_oauth".into()].into(),
            capabilities: Default::default(),
            command: None,
            args: vec![],
        },
    );
    r.route_defaults.insert(
        "claude_code".into(),
        vec![CandidateKey {
            source: "claude_oauth".into(),
            model: "claude-sonnet-5-5".into(),
        }],
    );
    r.route_class_defaults.insert(
        "claude_code".into(),
        BTreeMap::from([(
            "coding".into(),
            vec![CandidateKey {
                source: "claude_oauth".into(),
                model: "claude-opus-5-5".into(),
            }],
        )]),
    );
    r.harness_selection.rules.push(HarnessRule {
        genre: Some("coding".into()),
        harness: "claude_code".into(),
        ..Default::default()
    });
    r
}

fn override_state(r: &Registry) -> RouteState {
    let mut s = RouteState::default();
    for (id, spec) in &r.sources {
        s.accounts
            .available
            .insert(id.clone(), spec.accounts.clone());
    }
    s
}

fn override_input(class: Option<&str>) -> RouteInput {
    RouteInput {
        task_id: "class-route-override".into(),
        harness: HarnessRequest {
            genre: Some("coding".into()),
            execution_class: class.map(Into::into),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn picked_model(r: &Registry, class: Option<&str>) -> String {
    let (pick, _) = route_select(&override_input(class), r, &override_state(r));
    pick.unwrap().candidate.model.as_ref().to_string()
}

#[test]
fn class_route_override_selects_opus_for_coding_and_sonnet_for_other_classes() {
    let r = override_registry();
    assert_eq!(picked_model(&r, Some("coding")), "claude-opus-5-5");
    assert_eq!(picked_model(&r, Some("planner")), "claude-sonnet-5-5");
    assert_eq!(picked_model(&r, Some("review")), "claude-sonnet-5-5");
    assert_eq!(picked_model(&r, None), "claude-sonnet-5-5");
}

#[test]
fn coding_class_picks_the_class_candidate_first() {
    let mut r = override_registry();
    r.route_class_defaults
        .get_mut(&HarnessId::from("claude_code"))
        .unwrap()
        .insert(
            "coding".into(),
            vec![
                CandidateKey {
                    source: "claude_oauth".into(),
                    model: "claude-opus-5-5".into(),
                },
                CandidateKey {
                    source: "claude_oauth".into(),
                    model: "claude-sonnet-5-5".into(),
                },
            ],
        );
    assert_eq!(picked_model(&r, Some("coding")), "claude-opus-5-5");
    // The opus candidate is ineligible (explicit model hint excludes it); the next class candidate is used.
    let mut input = override_input(Some("coding"));
    input.model_hint = Some("claude-sonnet-5-5".into());
    let (pick, _) = route_select(&input, &r, &override_state(&r));
    assert_eq!(pick.unwrap().candidate.model.as_ref(), "claude-sonnet-5-5");
}

#[test]
fn class_list_does_not_fall_back_to_the_harness_defaults() {
    let r = override_registry();
    let mut state = override_state(&r);
    // Every account is cooling down: the opus class candidate is ineligible. The sonnet harness
    // default is also on this account, so it must not be picked as a substitute either.
    state
        .quota
        .cooling_down
        .insert(("claude_oauth".into(), "ca".into()));
    let (pick, trace) = route_select(&override_input(Some("coding")), &r, &state);
    assert_eq!(pick, Err(RouteFailure::NoEligible("claude_code".into())));
    // Only the class candidate was considered and rejected; the harness default never entered.
    assert!(!trace.rejections.is_empty());
    assert!(
        trace
            .rejections
            .iter()
            .all(|x| x.candidate.model.as_ref() == "claude-opus-5-5")
    );
}
