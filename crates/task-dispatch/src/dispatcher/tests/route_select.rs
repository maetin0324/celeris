//! ADR 2026-10-11 llm-source-harness-selection: harness selection, source×model choice,
//! admission, harness/source compatibility, and the selection record.

use super::*;
use crate::dispatcher::route_select::{RouteFailure, RouteInput, RouteState, route_select};
use std::collections::BTreeMap;
use task_core::llm_route::{
    AccountId, CandidateKey, ExclusionReason, HarnessRule, HarnessSelectionError, HarnessSpec,
    LlmRoute, Registry, SourceId, SourceSpec,
};

fn source(kind: &str, api: &str, accounts: &[&str], models: &[&str]) -> SourceSpec {
    SourceSpec {
        kind: kind.into(),
        api_format: api.into(),
        accounts: accounts.iter().map(|a| (*a).into()).collect(),
        models: models.iter().map(|m| (*m).into()).collect(),
        max_runs: None,
        account_max_runs: BTreeMap::new(),
        quota: None,
    }
}

fn harness(kinds: &[&str], apis: &[&str], caps: &[&str]) -> HarnessSpec {
    HarnessSpec {
        kind: "cli".into(),
        api_formats: apis.iter().map(|a| (*a).into()).collect(),
        source_kinds: kinds.iter().map(|k| (*k).into()).collect(),
        capabilities: caps.iter().map(|c| (*c).to_string()).collect(),
        command: None,
        args: vec![],
    }
}

fn key(source: &str, model: &str) -> CandidateKey {
    CandidateKey {
        source: source.into(),
        model: model.into(),
    }
}

/// claude_code runs on claude_oauth (then codex_oauth via a registered proxy route);
/// opencode runs on qwen. `[harness_selection]`: coding → claude_code, light → opencode.
fn registry() -> Registry {
    let mut r = Registry::default();
    r.sources.insert(
        "claude_oauth".into(),
        source("claude_oauth", "claude_cli", &["ca", "cb"], &["sonnet", "opus"]),
    );
    r.sources.insert(
        "codex_oauth".into(),
        source("codex_oauth", "codex_cli", &["xa"], &["gpt"]),
    );
    r.sources.insert(
        "qwen".into(),
        source("openai_compatible", "openai_compatible", &["shared"], &["qwen3"]),
    );
    r.harnesses.insert(
        "claude_code".into(),
        harness(
            &["claude_oauth", "codex_oauth"],
            &["claude_cli"],
            &["tools", "repo", "browser_context"],
        ),
    );
    r.harnesses.insert(
        "opencode".into(),
        harness(&["openai_compatible"], &["openai_compatible"], &["tools"]),
    );
    r.llm_routes.push(LlmRoute {
        harness: "claude_code".into(),
        key: key("codex_oauth", "gpt"),
        via: Some("llm_proxy".into()),
        from_api: Some("codex_cli".into()),
        to_api: Some("claude_cli".into()),
    });
    r.route_defaults.insert(
        "claude_code".into(),
        vec![key("claude_oauth", "sonnet"), key("codex_oauth", "gpt")],
    );
    r.route_defaults
        .insert("opencode".into(), vec![key("qwen", "qwen3")]);
    r.harness_selection.rules = vec![
        HarnessRule {
            genre: Some("coding".into()),
            harness: "claude_code".into(),
            ..Default::default()
        },
        HarnessRule {
            genre: Some("light".into()),
            harness: "opencode".into(),
            ..Default::default()
        },
    ];
    r
}

fn all_available(r: &Registry) -> RouteState {
    let mut state = RouteState::default();
    for (id, s) in &r.sources {
        state
            .accounts
            .available
            .insert(id.clone(), s.accounts.clone());
    }
    state
}

fn input(genre: &str) -> RouteInput {
    RouteInput {
        task_id: "t".into(),
        harness: task_core::llm_route::HarnessRequest {
            genre: Some(genre.into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn pair(source: &str, account: &str) -> (SourceId, AccountId) {
    (source.into(), account.into())
}

#[test]
fn harness_select_rules_are_deterministic() {
    let r = registry();
    let state = all_available(&r);
    let first = route_select(&input("coding"), &r, &state);
    for _ in 0..5 {
        assert_eq!(route_select(&input("coding"), &r, &state), first);
    }
    let pick = first.0.unwrap();
    assert_eq!(pick.harness, "claude_code".into());
    assert_eq!(
        route_select(&input("light"), &r, &state).0.unwrap().harness,
        "opencode".into()
    );
}

#[test]
fn harness_select_explicit_task_field_wins_over_rules() {
    let r = registry();
    let mut req = input("coding");
    req.harness.harness = Some("opencode".into());
    let (pick, trace) = route_select(&req, &r, &all_available(&r));
    let pick = pick.unwrap();
    assert_eq!(pick.harness, "opencode".into());
    assert_eq!(pick.candidate.key(), key("qwen", "qwen3"));
    assert_eq!(trace.harness, Some("opencode".into()));
    // An explicit but undeclared harness is refused, not replaced by the rules.
    req.harness.harness = Some("aider".into());
    assert_eq!(
        route_select(&req, &r, &all_available(&r)).0,
        Err(RouteFailure::Harness(HarnessSelectionError::Unknown(
            "aider".into()
        )))
    );
}

#[test]
fn harness_select_ambiguous_or_missing_rules_fail_closed() {
    let mut r = registry();
    r.harness_selection.rules.push(HarnessRule {
        genre: Some("coding".into()),
        harness: "opencode".into(),
        ..Default::default()
    });
    let state = all_available(&r);
    assert!(matches!(
        route_select(&input("coding"), &r, &state).0,
        Err(RouteFailure::Harness(HarnessSelectionError::Ambiguous(_)))
    ));
    assert_eq!(
        route_select(&input("research"), &r, &state).0,
        Err(RouteFailure::Harness(HarnessSelectionError::NoneEligible))
    );
}

#[test]
fn harness_select_reads_structured_task_fields_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut task = new_task(dir.path(), Check::Reviewer, 0);
    task.genre = Some("coding".into());
    task.objective = "use opencode with qwen please".into();
    task.worker_hint.adapter = Some("acp".into());
    let req = crate::dispatcher::harness_request_for(&task);
    assert_eq!(req.genre.as_deref(), Some("coding"));
    assert_eq!(req.harness, None);
    assert!(req.required_capabilities.is_empty());
    let r = registry();
    let input = RouteInput {
        task_id: task.id.to_string(),
        harness: req,
        ..Default::default()
    };
    assert_eq!(
        route_select(&input, &r, &all_available(&r)).0.unwrap().harness,
        "claude_code".into()
    );
    task.harness_id = Some("opencode".into());
    task.requirements.browser = Some(Default::default());
    let req = crate::dispatcher::harness_request_for(&task);
    assert_eq!(req.harness, Some("opencode".into()));
    assert!(req.required_capabilities.contains("browser_context"));
}

#[test]
fn route_select_follows_declared_candidate_order() {
    let r = registry();
    let mut state = all_available(&r);
    let (pick, trace) = route_select(&input("coding"), &r, &state);
    let pick = pick.unwrap();
    assert_eq!(pick.candidate.key(), key("claude_oauth", "sonnet"));
    assert_eq!(pick.account, "ca".into());
    assert_eq!(
        trace.candidates,
        vec![key("claude_oauth", "sonnet"), key("codex_oauth", "gpt")]
    );
    // Both Claude accounts cooling down: the next declared candidate, never an undeclared one.
    state.quota.cooling_down.insert(pair("claude_oauth", "ca"));
    state.quota.cooling_down.insert(pair("claude_oauth", "cb"));
    let (pick, trace) = route_select(&input("coding"), &r, &state);
    assert_eq!(pick.unwrap().candidate.key(), key("codex_oauth", "gpt"));
    assert!(
        trace
            .rejections
            .iter()
            .any(|x| x.candidate.key() == key("claude_oauth", "sonnet")
                && x.reason == ExclusionReason::Cooldown)
    );
}

#[test]
fn route_select_model_hint_narrows_and_unmatched_fails() {
    let r = registry();
    let state = all_available(&r);
    let mut req = input("coding");
    req.model_hint = Some("gpt".into());
    assert_eq!(
        route_select(&req, &r, &state).0.unwrap().candidate.key(),
        key("codex_oauth", "gpt")
    );
    // `opus` is offered by claude_oauth but is not a declared candidate for claude_code.
    req.model_hint = Some("opus".into());
    assert_eq!(
        route_select(&req, &r, &state).0,
        Err(RouteFailure::ModelHintUnmatched("opus".into()))
    );
    // The lane label is carried but does not change the choice.
    let mut laned = input("coding");
    laned.lane = Some("cheap".into());
    assert_eq!(
        route_select(&laned, &r, &state).0,
        route_select(&input("coding"), &r, &state).0
    );
}

#[test]
fn route_select_admission_excludes_full_and_exhausted_accounts() {
    let r = registry();
    let mut state = all_available(&r);
    state
        .account_max_runs
        .insert(pair("claude_oauth", "ca"), 1);
    state
        .reservations
        .account_in_use
        .insert(pair("claude_oauth", "ca"), 1);
    let pick = route_select(&input("coding"), &r, &state).0.unwrap();
    assert_eq!(pick.account, "cb".into());
    state
        .quota
        .account_remaining
        .insert(pair("claude_oauth", "cb"), 0.0);
    let (pick, trace) = route_select(&input("coding"), &r, &state);
    assert_eq!(pick.unwrap().candidate.key(), key("codex_oauth", "gpt"));
    assert!(trace.rejections.iter().any(|x| x.candidate.source == "claude_oauth".into()
        && x.reason == ExclusionReason::AccountConcurrency));
    // Nothing admissible anywhere: fail closed.
    state.accounts.available.clear();
    assert_eq!(
        route_select(&input("coding"), &r, &state).0,
        Err(RouteFailure::NoEligible("claude_code".into()))
    );
}

#[test]
fn route_select_rejects_unregistered_combination_before_dispatch() {
    let mut r = registry();
    // Without the proxy route codex_oauth cannot feed claude_code's API format.
    r.llm_routes.clear();
    let mut state = all_available(&r);
    state.quota.cooling_down.insert(pair("claude_oauth", "ca"));
    state.quota.cooling_down.insert(pair("claude_oauth", "cb"));
    let (pick, trace) = route_select(&input("coding"), &r, &state);
    assert_eq!(pick, Err(RouteFailure::NoEligible("claude_code".into())));
    assert!(trace.rejections.iter().any(|x| x.candidate.key() == key("codex_oauth", "gpt")
        && x.reason == ExclusionReason::NoCompatibleRoute));
    // No route_defaults for the harness: fail closed even though a compatible source exists.
    r.route_defaults.remove(&task_core::llm_route::HarnessId::from("opencode"));
    assert_eq!(
        route_select(&input("light"), &r, &state).0,
        Err(RouteFailure::NoDeclaredDefault("opencode".into()))
    );
}

#[test]
fn route_select_records_events_and_run_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let sqlite = Arc::new(SqliteStore::open(&db).unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    let r = registry();
    d.set_llm_route_registry(r.clone());
    let mut task = new_task(dir.path(), Check::Reviewer, 0);
    task.genre = Some("light".into());
    store.insert(&task).unwrap();
    let run_id = "01RUNSELECTION";
    store
        .run_index_start(task_core::RunRow {
            run_id: run_id.into(),
            task_id: task.id.to_string(),
            work_unit_id: None,
            role: task_core::RunIndexRole::Worker,
            seq: 1,
            status: task_core::RunIndexStatus::Running,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-10-11T00:00:00Z".into(),
            finished_at: None,
        })
        .unwrap();
    let req = RouteInput {
        task_id: task.id.to_string(),
        harness: crate::dispatcher::harness_request_for(&task),
        ..Default::default()
    };
    // Without `[accounts]` the OAuth pools have no logged-in account; qwen uses its declared one.
    let (outcome, trace) = d.select_route(&req);
    let pick = outcome.clone().unwrap();
    assert_eq!(pick.candidate.key(), key("qwen", "qwen3"));
    d.record_route_selection(task.id, Some(run_id), &req, &outcome, &trace)
        .unwrap();
    let events = events_of(&store, task.id);
    assert!(events.iter().any(|e| matches!(e,
        Event::HarnessSelected { harness_id } if harness_id == "opencode")));
    let metadata = events
        .iter()
        .find_map(|e| match e {
            Event::LlmRouteSelected {
                source_id,
                model_id,
                account_id,
                metadata,
            } if source_id == "qwen" && model_id == "qwen3" && account_id == "shared" => {
                Some(metadata.clone())
            }
            _ => None,
        })
        .expect("llm_route_selected");
    assert_eq!(metadata["version"], 1);
    assert_eq!(metadata["candidates"].as_array().map(Vec::len), Some(1));
    assert_eq!(metadata["decision"]["account"], "shared");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let row: (String, String, String, String, String) = conn
        .query_row(
            "SELECT selected_harness_id, selected_source_id, selected_model_id, \
             selected_account_id, selection_metadata_json FROM runs WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (row.0.as_str(), row.1.as_str(), row.2.as_str(), row.3.as_str()),
        ("opencode", "qwen", "qwen3", "shared")
    );
    let stored: serde_json::Value = serde_json::from_str(&row.4).unwrap();
    assert_eq!(stored, metadata);

    // A refusal records the harness and the reason, and selects nothing.
    let mut refused = req.clone();
    refused.model_hint = Some("missing".into());
    let (outcome, trace) = d.select_route(&refused);
    assert_eq!(
        outcome,
        Err(RouteFailure::ModelHintUnmatched("missing".into()))
    );
    d.record_route_selection(task.id, None, &refused, &outcome, &trace)
        .unwrap();
    assert!(progress_msgs(&store, task.id)
        .iter()
        .any(|m| m.contains("route refused") && m.contains("missing")));
}
