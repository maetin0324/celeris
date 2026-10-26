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
        endpoint: None,
        credential_ref: None,
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
        source(
            "claude_oauth",
            "claude_cli",
            &["ca", "cb"],
            &["sonnet", "opus"],
        ),
    );
    r.sources.insert(
        "codex_oauth".into(),
        source("codex_oauth", "codex_cli", &["xa"], &["gpt"]),
    );
    r.sources.insert(
        "qwen".into(),
        source(
            "openai_compatible",
            "openai_compatible",
            &["shared"],
            &["qwen3"],
        ),
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
fn proxy_route_pick_carries_the_qualified_wire_model() {
    let mut r = registry();
    r.route_defaults
        .insert("claude_code".into(), vec![key("codex_oauth", "gpt")]);
    let pick = route_select(&input("coding"), &r, &all_available(&r))
        .0
        .unwrap();
    assert_eq!(pick.candidate.model.as_ref(), "gpt");
    assert_eq!(pick.wire_model_id.as_ref(), "codex_oauth/gpt");
}

#[test]
fn direct_route_pick_keeps_the_bare_model() {
    let pick = route_select(&input("coding"), &registry(), &all_available(&registry()))
        .0
        .unwrap();
    assert_eq!(pick.candidate.model.as_ref(), "sonnet");
    assert_eq!(pick.wire_model_id.as_ref(), "sonnet");
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
        route_select(&input, &r, &all_available(&r))
            .0
            .unwrap()
            .harness,
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
    state.account_max_runs.insert(pair("claude_oauth", "ca"), 1);
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
    assert!(
        trace
            .rejections
            .iter()
            .any(|x| x.candidate.source == "claude_oauth".into()
                && x.reason == ExclusionReason::AccountConcurrency)
    );
    // Nothing admissible anywhere: fail closed.
    state.accounts.available.clear();
    assert_eq!(
        route_select(&input("coding"), &r, &state).0,
        Err(RouteFailure::NoEligible("claude_code".into()))
    );
}

#[test]
fn cos_route_candidates_keep_priority_and_fail_closed_when_none_is_eligible() {
    let mut r = registry();
    r.cos_route_candidates = vec![key("claude_oauth", "sonnet"), key("codex_oauth", "gpt")];
    let cos_input = RouteInput {
        task_id: "cos-thread".into(),
        harness: task_core::llm_route::HarnessRequest {
            harness: Some("claude_code".into()),
            ..Default::default()
        },
        cos: true,
        ..Default::default()
    };
    let mut state = RouteState::default();
    // The first candidate has no admitted account, so selection proceeds in declaration order.
    state
        .accounts
        .available
        .insert("codex_oauth".into(), vec!["xa".into()]);
    let (selected, trace) = route_select(&cos_input, &r, &state);
    let selected = selected.expect("second candidate is eligible");
    assert_eq!(selected.candidate.key(), key("codex_oauth", "gpt"));
    assert_eq!(trace.candidates, r.cos_route_candidates);

    state.accounts.available.clear();
    let (refused, _) = route_select(&cos_input, &r, &state);
    assert!(matches!(refused, Err(RouteFailure::NoEligible(_))));
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
    assert!(
        trace
            .rejections
            .iter()
            .any(|x| x.candidate.key() == key("codex_oauth", "gpt")
                && x.reason == ExclusionReason::NoCompatibleRoute)
    );
    // No route_defaults for the harness: fail closed even though a compatible source exists.
    r.route_defaults
        .remove(&task_core::llm_route::HarnessId::from("opencode"));
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
    assert_eq!(metadata["version"], 2);
    assert_eq!(metadata["candidates"].as_array().map(Vec::len), Some(1));
    assert_eq!(metadata["decision"]["account"], "shared");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let row: (String, String, String, String, String) = conn
        .query_row(
            "SELECT selected_harness_id, selected_source_id, selected_model_id, \
             selected_account_id, selection_metadata_json FROM runs WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        (
            row.0.as_str(),
            row.1.as_str(),
            row.2.as_str(),
            row.3.as_str()
        ),
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
    assert!(
        progress_msgs(&store, task.id)
            .iter()
            .any(|m| m.contains("route refused") && m.contains("missing"))
    );
}

/// ADR 2026-10-26-ldr-proxy-and-route-fixes C1: an `openai_compatible` source that declares no
/// accounts (production `[llm_sources.qwen]`) offers its source id as its one implicit account, so the
/// dispatcher's route selection picks it instead of excluding it as `AccountUnavailable`.
#[test]
fn route_select_offers_an_undeclared_pool_source_its_implicit_account() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    let mut r = registry();
    r.sources.insert(
        "qwen".into(),
        source("openai_compatible", "openai_compatible", &[], &["qwen3"]),
    );
    d.set_llm_route_registry(r);
    let mut task = new_task(dir.path(), Check::Reviewer, 0);
    task.genre = Some("light".into());
    let req = RouteInput {
        task_id: task.id.to_string(),
        harness: crate::dispatcher::harness_request_for(&task),
        ..Default::default()
    };
    let (outcome, trace) = d.select_route(&req);
    let pick = outcome.unwrap();
    assert_eq!(pick.candidate.key(), key("qwen", "qwen3"));
    assert_eq!(pick.account, AccountId::from("qwen"));
    assert!(
        !trace
            .rejections
            .iter()
            .any(|x| x.reason == ExclusionReason::AccountUnavailable),
        "{:?}",
        trace.rejections
    );
}

#[test]
fn source_account_pool_rules() {
    let ids = |v: &[&str]| v.iter().map(|a| AccountId::from(*a)).collect::<Vec<_>>();
    let qwen = SourceId::from("qwen");
    // No account adapter: declared accounts, else the source id as the implicit account.
    assert_eq!(crate::source_account_pool(&qwen, &[], None), ids(&["qwen"]));
    assert_eq!(
        crate::source_account_pool(&qwen, &ids(&["shared"]), None),
        ids(&["shared"])
    );
    // OAuth pool: logged-in directories, narrowed to the declared ones; never an implicit account.
    let claude = SourceId::from("claude_oauth");
    let logged_in = ids(&["ca", "cb"]);
    assert_eq!(
        crate::source_account_pool(&claude, &[], Some(&logged_in)),
        logged_in
    );
    assert_eq!(
        crate::source_account_pool(&claude, &ids(&["cb", "cz"]), Some(&logged_in)),
        ids(&["cb"])
    );
    assert!(crate::source_account_pool(&claude, &[], Some(&[])).is_empty());
}

/// Records the route each run was launched with.
#[derive(Clone, Default)]
struct RouteCapture {
    seen: Arc<StdMutex<Vec<Option<task_worker::protocol::SelectedLlmRoute>>>>,
}

#[async_trait]
impl WorkerAdapter for RouteCapture {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push(req.llm_route.clone());
        std::fs::write(req.workspace.join("touched"), "1").unwrap();
        Ok(done_outcome())
    }
}

/// The routes of [`registry`] with `opencode` (and `claude_code` unless `claude` is false) run by
/// `capture`.
fn route_dispatcher(store: Arc<dyn TaskStore>, capture: &RouteCapture, claude: bool) -> Dispatcher {
    let mut d = dispatcher(store, done_adapter(), 1);
    d.set_llm_route_registry(registry());
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(capture.clone());
    let mut adapters = HashMap::from([("opencode".into(), Arc::clone(&adapter))]);
    if claude {
        adapters.insert("claude_code".into(), adapter);
    }
    d.set_harness_adapters(adapters);
    d
}

#[tokio::test]
async fn route_select_dispatch_run_launches_the_selected_harness_model_and_account() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db.sqlite");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&db).unwrap());
    let capture = RouteCapture::default();
    let mut d = route_dispatcher(store.clone(), &capture, true);
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "test -f touched".into(),
            expect_exit: 0,
        },
        0,
    );
    task.genre = Some("light".into());
    task.objective = "crates/task-core/src/model.rs の小さな変更".into();
    task.routing = Some(task_core::TaskRouting {
        tier_source: task_core::TierSource::Hint,
        ..Default::default()
    });
    store.insert(&task).unwrap();

    assert_eq!(d.tick().unwrap().dispatched, 1);
    let run_id = d
        .running
        .get(&RunKey {
            task: task.id,
            work_unit: None,
        })
        .map(|e| {
            assert_eq!(e.route, Some(pair("qwen", "shared")));
            assert_eq!(e.provider, "qwen");
            e.run_id.clone()
        })
        .expect("the run was started");
    await_worker_completion(&mut d, task.id).await;

    let seen = capture.seen.lock().unwrap().clone();
    let route = seen[0].clone().expect("RunRequest.llm_route");
    assert_eq!(
        (
            route.harness_id.as_str(),
            route.source_id.as_str(),
            route.model_id.as_str(),
            route.account_id.as_str()
        ),
        ("opencode", "qwen", "qwen3", "shared")
    );
    let events = events_of(&store, task.id);
    assert!(events.iter().any(|e| matches!(e,
        Event::WorkerStarted { model, provider, account, .. }
            if model == "qwen3" && provider.as_deref() == Some("qwen") && account.is_none())));
    assert!(events.iter().any(|e| matches!(e,
        Event::HarnessSelected { harness_id } if harness_id == "opencode")));
    assert!(events.iter().any(|e| matches!(e,
        Event::LlmRouteSelected { source_id, model_id, account_id, .. }
            if source_id == "qwen" && model_id == "qwen3" && account_id == "shared")));
    let routing = events
        .iter()
        .find_map(|e| match e {
            Event::RoutingDecided {
                run_id: event_run,
                record,
            } if event_run == &run_id => Some(record),
            _ => None,
        })
        .expect("routing_decided");
    let decision = routing
        .features
        .as_ref()
        .and_then(|features| features.decision.as_deref())
        .expect("route decision in the feature snapshot reference");
    assert_eq!(
        decision.stage,
        task_core::route_select::RoutingStage::Shadow
    );
    assert_eq!(
        decision.features_hash.as_deref(),
        routing.features.as_ref().map(|f| f.hash.as_str())
    );
    assert_eq!(
        decision.features_schema.as_deref(),
        Some("celeris.task-features/1")
    );
    assert!(!decision.decision_id.is_empty());
    assert!(decision.options_hash.is_some());
    let conn = rusqlite::Connection::open(&db).unwrap();
    let decision_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM route_decisions WHERE decision_id = ?1",
            [&decision.decision_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(decision_rows, 1);
    let row: (String, String, String, String) = conn
        .query_row(
            "SELECT selected_harness_id, selected_source_id, selected_model_id, \
             selected_account_id FROM runs WHERE run_id = ?1",
            [&run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (
            row.0.as_str(),
            row.1.as_str(),
            row.2.as_str(),
            row.3.as_str()
        ),
        ("opencode", "qwen", "qwen3", "shared")
    );
}

#[tokio::test]
async fn route_select_dispatch_run_without_a_route_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let capture = RouteCapture::default();
    let mut d = route_dispatcher(store.clone(), &capture, false);
    let check = Check::Command {
        cmd: "true".into(),
        expect_exit: 0,
    };
    // An undeclared harness.
    let mut unknown = new_task(dir.path(), check.clone(), 0);
    unknown.harness_id = Some("missing".into());
    // claude_code: its OAuth pools have no logged-in account (no candidate is eligible), and
    // codex_oauth is only reachable through the registered proxy route — no other source is tried.
    let mut coding = new_task(dir.path(), check.clone(), 0);
    coding.genre = Some("coding".into());
    // A selected route whose harness has no adapter cannot be launched.
    let mut no_adapter = new_task(dir.path(), check, 0);
    no_adapter.harness_id = Some("claude_code".into());
    let mut r = registry();
    r.sources
        .get_mut(&SourceId::from("claude_oauth"))
        .unwrap()
        .kind = "test_api".into();
    r.harnesses
        .get_mut(&task_core::llm_route::HarnessId::from("claude_code"))
        .unwrap()
        .source_kinds
        .insert("test_api".into());
    for t in [&unknown, &coding] {
        store.insert(t).unwrap();
    }
    assert_eq!(d.tick().unwrap().dispatched, 0);
    for t in [&unknown, &coding] {
        assert_eq!(store.get(t.id).unwrap().unwrap().status, Status::Ready);
        assert!(
            progress_msgs(&store, t.id)
                .iter()
                .any(|m| m.contains("route refused")),
            "{:?}",
            progress_msgs(&store, t.id)
        );
        assert!(!events_of(&store, t.id).iter().any(|e| matches!(
            e,
            Event::LlmRouteSelected { .. } | Event::WorkerStarted { .. }
        )));
    }
    // The refusal is recorded once, not on every tick.
    let before = progress_msgs(&store, coding.id).len();
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(progress_msgs(&store, coding.id).len(), before);

    // claude_oauth admits an account now, but claude_code has no adapter: fail closed.
    d.set_llm_route_registry(r);
    store.insert(&no_adapter).unwrap();
    assert_eq!(d.tick().unwrap().dispatched, 0);
    assert_eq!(
        store.get(no_adapter.id).unwrap().unwrap().status,
        Status::Ready
    );
    assert!(
        progress_msgs(&store, no_adapter.id)
            .iter()
            .any(|m| m.contains("route refused: no adapter for harness claude_code"))
    );
    assert!(capture.seen.lock().unwrap().is_empty());
}

#[test]
fn class_candidates_survive_the_harness_loop() {
    let mut r = registry();
    r.harness_selection.execution_class_defaults.insert(
        "coding".into(),
        vec!["claude_code".into(), "opencode".into()],
    );
    r.route_class_defaults.insert(
        "claude_code".into(),
        BTreeMap::from([("coding".into(), vec![key("claude_oauth", "opus")])]),
    );
    let coding = RouteInput {
        harness: task_core::llm_route::HarnessRequest {
            genre: Some("coding".into()),
            execution_class: Some("coding".into()),
            ..Default::default()
        },
        ..input("coding")
    };
    // The class list is used inside the first harness: opus, not the harness default sonnet.
    let state = all_available(&r);
    let (pick, trace) = route_select(&coding, &r, &state);
    assert_eq!(pick.unwrap().candidate.key(), key("claude_oauth", "opus"));
    assert_eq!(trace.candidates, vec![key("claude_oauth", "opus")]);
    // With the claude accounts cooling down, the rejection is for the class candidate only and
    // the loop moves on to the next harness with its own default.
    let mut cooled = all_available(&r);
    cooled.quota.cooling_down.insert(pair("claude_oauth", "ca"));
    cooled.quota.cooling_down.insert(pair("claude_oauth", "cb"));
    let (pick, trace) = route_select(&coding, &r, &cooled);
    assert_eq!(pick.unwrap().harness, "opencode".into());
    let first = &trace.harness_rejections[0];
    assert_eq!(first.harness, "claude_code".into());
    assert!(
        first
            .rejections
            .iter()
            .all(|x| x.candidate.key() == key("claude_oauth", "opus"))
    );
}

// evidence_mode（none / pinned / latest）: none は根拠表を 1 回も読まず、途中で最新版を差し替えても判断が変わらない。
mod evidence_mode {
    use super::*;
    use crate::dispatcher::route_select::RouteContext;
    use task_core::route_select::EvidenceMode;
    use task_core::routing_evidence::{EvidenceKey, EvidenceRow};

    fn row(source: &str, model: &str, pass_lower: f64) -> EvidenceRow {
        EvidenceRow {
            quota_cost_per_pass: None,
            key: EvidenceKey {
                lane: String::new(),
                genre: "light".into(),
                harness_id: "opencode".into(),
                harness_version: "1.0.0".into(),
                source_id: source.into(),
                model_id: model.into(),
            },
            evidence_version: String::new(),
            n: 30,
            pass: 30,
            pass_lower,
            pass_upper: pass_lower + 0.01,
            pass_boot_lower: pass_lower,
            pass_boot_upper: 0.99,
            wall_p90_s: Some(10.0),
            usd_per_pass: Some(0.1),
            quota_per_run: BTreeMap::new(),
            timeout_rate: 0.0,
            low_confidence: false,
        }
    }

    fn pick(d: &mut Dispatcher, mode: EvidenceMode, version: Option<&str>) -> (String, String) {
        let (out, trace) = d.select_route_with(
            &input("light"),
            &RouteContext {
                evidence_mode: mode,
                evidence_version: version.map(str::to_owned),
                ..Default::default()
            },
        );
        let p = out.expect("route");
        let decision = trace.decision.expect("decision");
        (
            p.candidate.model.to_string(),
            decision.evidence_version.unwrap_or_default(),
        )
    }

    #[test]
    fn none_never_reads_the_table_and_ignores_later_updates() {
        let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
        let store: Arc<dyn TaskStore> = sqlite.clone();
        let mut task = new_task(std::path::Path::new("/tmp"), Check::Reviewer, 0);
        task.genre = Some("light".into());
        store.insert(&task).unwrap();
        // v1: qwen4 が最良。enforce にして最新版が選択を動かす状態にする。
        store
            .replace_routing_evidence(
                "v1",
                &[row("qwen", "qwen4", 0.9), row("qwen", "qwen3", 0.5)],
            )
            .unwrap();
        store
            .set_routing_stage(
                task.id,
                &task_core::route_select::RoutingStageChange {
                    genre: "light".into(),
                    harness_id: "opencode".into(),
                    stage: task_core::route_select::RoutingStage::Enforce,
                    since: "2026-10-17".into(),
                    evidence_version: Some("v1".into()),
                    reason: "test".into(),
                    window: serde_json::json!({}),
                    actor: "human".into(),
                },
            )
            .unwrap();
        let mut d = dispatcher(store.clone(), done_adapter(), 1);
        let mut reg = registry();
        reg.sources
            .get_mut(&SourceId::from("qwen"))
            .unwrap()
            .models
            .push("qwen4".into());
        reg.route_defaults.insert(
            "opencode".into(),
            vec![key("qwen", "qwen3"), key("qwen", "qwen4")],
        );
        d.set_llm_route_registry(reg);

        let (latest_model, latest_version) = pick(&mut d, EvidenceMode::Latest, None);
        assert_eq!(
            (latest_model.as_str(), latest_version.as_str()),
            ("qwen4", "v1")
        );
        // 根拠表の lookup は store の接続を 1 回取る。none の取得回数を latest と比べて 0 回を示す。
        let locks = || {
            let c = sqlite.lock_counts();
            c.writer + c.reader
        };
        let before_latest = locks();
        pick(&mut d, EvidenceMode::Latest, None);
        let latest_cost = locks() - before_latest;
        let before_none = locks();
        let (none_model, none_version) = pick(&mut d, EvidenceMode::None, None);
        let none_cost = locks() - before_none;
        assert!(
            latest_cost > none_cost,
            "latest は根拠表 lookup の分だけ接続を多く取る（latest={latest_cost}, none={none_cost}）"
        );
        assert_eq!(none_model, "qwen3", "none は固定の候補順");
        assert_eq!(none_version, "", "none は table 版なし");

        // 最新根拠表を途中で差し替えても none の判断は同じ（latest は動く）。
        store
            .replace_routing_evidence(
                "v2",
                &[row("qwen", "qwen4", 0.5), row("qwen", "qwen3", 0.95)],
            )
            .unwrap();
        let before = locks();
        assert_eq!(
            pick(&mut d, EvidenceMode::None, None),
            (none_model, none_version)
        );
        assert_eq!(locks() - before, none_cost, "none は更新後も lookup 0 回");
        assert_eq!(pick(&mut d, EvidenceMode::Latest, None).1, "v2");

        // pinned は指定版だけ（最新の v2 ではなく v1）。
        let (m, v) = pick(&mut d, EvidenceMode::Pinned, Some("v1"));
        assert_eq!((m.as_str(), v.as_str()), ("qwen4", "v1"));
    }
}

// ADR 2026-10-22: harness の残量による並べ替え、frontier lane の規則、ε 探索の位置。
mod harness_balance {
    use super::*;
    use task_core::llm_route::{ExecutionClass, HarnessRequest};
    use task_core::route_select::RoutingStage;
    use task_core::routing_evidence::{EvidenceKey, EvidenceRow};

    fn two_harnesses(classes: &[&str]) -> Registry {
        let mut r = Registry::default();
        r.sources.insert(
            "claude_oauth".into(),
            source("claude_oauth", "claude_cli", &["ca"], &["sonnet", "opus"]),
        );
        r.sources.insert(
            "codex_oauth".into(),
            source("codex_oauth", "codex_cli", &["xa"], &["gpt"]),
        );
        r.harnesses.insert(
            "claude_code".into(),
            harness(&["claude_oauth"], &["claude_cli"], &["tools"]),
        );
        r.harnesses.insert(
            "codex".into(),
            harness(&["codex_oauth"], &["codex_cli"], &["tools"]),
        );
        r.route_defaults.insert(
            "claude_code".into(),
            vec![key("claude_oauth", "sonnet"), key("claude_oauth", "opus")],
        );
        r.route_defaults
            .insert("codex".into(), vec![key("codex_oauth", "gpt")]);
        r.lane_route_defaults.insert(
            ("claude_code".into(), "frontier".into()),
            vec![key("claude_oauth", "opus")],
        );
        for class in ["planner", "coding"] {
            r.harness_selection
                .execution_class_defaults
                .insert(class.into(), vec!["claude_code".into(), "codex".into()]);
        }
        r.route_selection.harness_balance_classes =
            classes.iter().map(|c| (*c).to_owned()).collect();
        r
    }

    fn with_headroom(r: &Registry, claude: f64, codex: f64) -> RouteState {
        let mut state = all_available(r);
        state
            .quota
            .source_headroom
            .insert("claude_oauth".into(), claude);
        state
            .quota
            .source_headroom
            .insert("codex_oauth".into(), codex);
        state
    }

    fn class_input(class: &str, lane: Option<&str>) -> RouteInput {
        RouteInput {
            task_id: "t".into(),
            harness: HarnessRequest {
                genre: Some("coding".into()),
                execution_class: Some(ExecutionClass::from(class)),
                ..Default::default()
            },
            lane: lane.map(str::to_owned),
            ..Default::default()
        }
    }

    fn harness_of(
        r: &Registry,
        state: &RouteState,
        input: &RouteInput,
    ) -> (String, Option<task_core::route_select::HarnessOrderRecord>) {
        let (pick, trace) = route_select(input, r, state);
        (
            pick.expect("route").harness.to_string(),
            trace.decision.and_then(|d| d.harness_order),
        )
    }

    #[test]
    fn a_class_that_is_not_enabled_keeps_the_declared_order() {
        let r = two_harnesses(&["review"]);
        let state = with_headroom(&r, 0.05, 0.95);
        let (harness, order) = harness_of(&r, &state, &class_input("planner", None));
        assert_eq!(harness, "claude_code");
        assert!(order.is_none(), "no record for a class that is not enabled");
    }

    #[test]
    fn less_claude_headroom_puts_codex_first_and_records_inputs_and_result() {
        let r = two_harnesses(&["planner"]);
        let state = with_headroom(&r, 0.2, 0.9);
        let (harness, order) = harness_of(&r, &state, &class_input("planner", None));
        assert_eq!(harness, "codex");
        let order = order.expect("harness_order on the decision");
        assert!(order.enabled && !order.tripped && order.switched);
        assert_eq!(order.declared, ["claude_code", "codex"]);
        assert_eq!(order.ordered, ["codex", "claude_code"]);
        assert_eq!(order.scores["claude_code"], 0.2);
        assert_eq!(order.scores["codex"], 0.9);
        assert_eq!(order.tolerance, 0.15);
    }

    #[test]
    fn a_difference_inside_the_tolerance_keeps_the_declared_order() {
        let r = two_harnesses(&["planner"]);
        let state = with_headroom(&r, 0.8, 0.9);
        let (harness, order) = harness_of(&r, &state, &class_input("planner", None));
        assert_eq!(harness, "claude_code");
        let order = order.expect("recorded even when nothing moves");
        assert!(!order.switched);
        assert_eq!(order.ordered, ["claude_code", "codex"]);
    }

    #[test]
    fn a_tripped_class_goes_back_to_the_declared_order() {
        let r = two_harnesses(&["planner"]);
        let mut state = with_headroom(&r, 0.1, 0.9);
        state.balance_tripped.insert("planner".into());
        let (harness, order) = harness_of(&r, &state, &class_input("planner", None));
        assert_eq!(harness, "claude_code");
        let order = order.expect("record");
        assert!(order.tripped && !order.switched);
    }

    #[test]
    fn when_the_first_harness_is_full_the_ordered_next_one_is_tried() {
        let r = two_harnesses(&["planner"]);
        // codex is ahead on headroom but its only account is cooling down: claude still runs.
        let mut state = with_headroom(&r, 0.3, 0.9);
        state.quota.cooling_down.insert(pair("codex_oauth", "xa"));
        let (pick, trace) = route_select(&class_input("planner", None), &r, &state);
        assert_eq!(pick.unwrap().harness.to_string(), "claude_code");
        assert_eq!(trace.harness_rejections[0].harness.to_string(), "codex");
    }

    // --- frontier lane ---

    fn evidence_row(lane: &str, model: &str, pass_lower: f64) -> EvidenceRow {
        EvidenceRow {
            key: EvidenceKey {
                genre: "coding".into(),
                harness_id: "claude_code".into(),
                harness_version: "2".into(),
                lane: lane.into(),
                source_id: "claude_oauth".into(),
                model_id: model.into(),
            },
            evidence_version: "completion-1".into(),
            n: 100,
            pass: 90,
            pass_lower,
            pass_upper: pass_lower + 0.05,
            pass_boot_lower: pass_lower,
            pass_boot_upper: pass_lower + 0.05,
            wall_p90_s: Some(100.0),
            usd_per_pass: Some(1.0),
            quota_cost_per_pass: Some(if model == "sonnet" { 0.01 } else { 0.03 }),
            quota_per_run: BTreeMap::new(),
            timeout_rate: 0.0,
            low_confidence: false,
        }
    }

    /// enforce の claude_code。根拠表は sonnet が opus と同等で quota コストが安い（旧来の選択は sonnet）。
    fn enforced(r: &Registry, seed: &str) -> RouteState {
        let mut state = all_available(r);
        let h: task_core::llm_route::HarnessId = "claude_code".into();
        let rows = ["standard", "frontier"]
            .into_iter()
            .flat_map(|lane| {
                [
                    evidence_row(lane, "sonnet", 0.84),
                    evidence_row(lane, "opus", 0.85),
                ]
            })
            .collect();
        state.evidence.insert(h.clone(), rows);
        state.stages.insert(h.clone(), RoutingStage::Enforce);
        state.harness_versions.insert(h, "2.1.0".into());
        state.run_seed = Some(seed.into());
        state
    }

    fn model_of(r: &Registry, state: &RouteState, lane: &str) -> String {
        let (pick, _) = route_select(&class_input("coding", Some(lane)), r, state);
        pick.expect("route").candidate.model.to_string()
    }

    #[test]
    fn a_frontier_lane_gets_the_frontier_model_whatever_the_table_says() {
        let mut r = two_harnesses(&[]);
        r.route_selection.exploration_epsilon = 0.0;
        let state = enforced(&r, "run-1");
        // 根拠表の同等判定では sonnet（quota コストが安い）が選ばれる lane は今のまま。
        assert_eq!(model_of(&r, &state, "standard"), "sonnet");
        // frontier lane は規則で lane の route_defaults（opus）。
        assert_eq!(model_of(&r, &state, "frontier"), "opus");
        let (_, trace) = route_select(&class_input("coding", Some("frontier")), &r, &state);
        let decision = trace.decision.expect("decision");
        assert_eq!(decision.lane.as_deref(), Some("frontier"));
        assert_eq!(decision.propensity, 1.0);
    }

    #[test]
    fn exploration_happens_only_inside_the_frontier_lane() {
        let mut r = two_harnesses(&[]);
        r.route_selection.exploration_epsilon = 0.5;
        let mut explored_in_frontier = 0;
        let mut kept_in_frontier = 0;
        for i in 0..64u64 {
            let state = enforced(
                &r,
                &format!("{}{}", "x".repeat((i % 7) as usize), i * 2_654_435_761u64),
            );
            // frontier 以外の lane では ε を使わない（常に根拠表の選択）。
            assert_eq!(model_of(&r, &state, "standard"), "sonnet");
            let (pick, trace) = route_select(&class_input("coding", Some("frontier")), &r, &state);
            let pick = pick.unwrap();
            let decision = trace.decision.unwrap();
            let propensity = decision.propensity;
            if pick.candidate.model.as_ref() == "opus" {
                kept_in_frontier += 1;
                assert!((propensity - 0.5).abs() < 1e-9);
            } else {
                explored_in_frontier += 1;
                assert_eq!(pick.candidate.model.as_ref(), "sonnet");
                assert!((propensity - 0.5).abs() < 1e-9);
            }
        }
        assert!(
            explored_in_frontier > 0 && kept_in_frontier > 0,
            "explored={explored_in_frontier} kept={kept_in_frontier}"
        );
    }
}

// ADR 2026-10-22 D1: `QuotaSnapshot.source_remaining` / `source_headroom` は account book の観測値から入る。
#[test]
fn route_state_fills_source_remaining_and_headroom_from_the_account_book() {
    use task_core::{AccountAdapter, RateLimitObservation, RateWindow};
    let dir = accounts_fixture();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut d = pool_dispatcher(store, done_adapter(), None, dir.path().to_path_buf(), 2, 2);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let observe = |five_hour: f64| RateLimitObservation {
        five_hour: Some(RateWindow {
            utilization: five_hour,
            resets_at: now + 3_600,
        }),
        // 7d は半分経過（t7 = 0.5）で 30% 使用 → h_7d = min(1, 0.7 / 0.5) = 1。
        seven_day: Some(RateWindow {
            utilization: 0.30,
            resets_at: now + 302_400,
        }),
        one_month: None,
        status: None,
        resets_at: None,
        observed_at: now,
    };
    {
        let book = d.account_book(AccountAdapter::ClaudeCode).expect("book");
        let mut book = book.lock().unwrap();
        book.record_observation("a", observe(0.60), crate::accounts::ObservationSource::Run);
        book.record_observation("b", observe(0.90), crate::accounts::ObservationSource::Run);
    }
    let state = d.route_state(false);
    let source: SourceId = AccountAdapter::ClaudeCode.as_str().into();
    // 最良 account（a）: h_5h = 0.4 が最小。b は 0.1。席の減点は入れない。
    let headroom = state.quota.source_headroom[&source];
    let remaining = state.quota.source_remaining[&source];
    assert!((headroom - 0.4).abs() < 1e-6, "headroom {headroom}");
    assert!((remaining - 0.4).abs() < 1e-6, "remaining {remaining}");
}
