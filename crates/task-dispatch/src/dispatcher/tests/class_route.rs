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
