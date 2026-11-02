//! 付記 S2-11: `dispatch_run` の `select_route_with` の直後の `route_priority` の compare。偽の `PolicyScript` だけを使う。
//! どの場合も、route 選択の event（`harness_selected`・`llm_route_selected`・`routing_decided`）・run の行・
//! route の拒否の理由は mode rust・PolicyScript 無しのときと byte 一致する（run id だけを伏せて比べる）。

use super::lane_assist::{hinted_task, jev_ok, question_adapter};
use super::review_route::{CapturingReviewer, reviewer_registry, worker_adapter};
use super::script_compare::{Behavior, FakeScript, bundle_sha};
use super::*;
use crate::script_policy::{PolicyScript, PolicyScriptError, SCRIPT_CALL_DEADLINE};
use task_core::llm_route::{HarnessId, Registry};
use task_core::route_select::{RoutingMode, SpecificityHint};
use task_core::script_compare::{
    CompareOutcome, NewScriptGeneration, ScriptCompareRecord, ScriptDomain, ScriptGenerationState,
    ScriptMode, ScriptModeChange,
};

/// 固定の task id（基準の run と compare の run で event の byte 列を揃える）。
const TASK_ID: &str = "01M6EJYCFFSJXYBVH87Q8FSNYB";

struct Out {
    records: Vec<ScriptCompareRecord>,
    /// route 選択の event と run の行（run id を伏せた直列化）。
    route_bytes: String,
    _ws: tempfile::TempDir,
}

/// 26 文字の ULID（run id・task id）を伏せる。
fn mask_ulids(s: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() == 26 {
            out.push_str("<ULID>");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() || c.is_ascii_uppercase() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn seed_generation(sqlite: &SqliteStore, domains: &[&str]) -> String {
    let manifest = serde_json::json!({ "domains": domains }).to_string();
    sqlite
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: bundle_sha(),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: manifest,
                state: ScriptGenerationState::Loaded,
            },
            OffsetDateTime::now_utc(),
        )
        .unwrap()
        .id
}

fn set_compare(sqlite: &SqliteStore, domain: &str, generation: &str) {
    sqlite
        .script_domain_set_mode(
            &ScriptModeChange {
                domain: domain.into(),
                to_mode: ScriptMode::Compare,
                to_generation: Some(generation.to_owned()),
                expected_version: 0,
                actor: "human:test".into(),
                reason: "script_compare_route test".into(),
            },
            OffsetDateTime::now_utc(),
        )
        .unwrap();
}

fn records_of(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<ScriptCompareRecord> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::ScriptCompareRecorded { record } => Some(*record),
            _ => None,
        })
        .collect()
}

fn route_bytes(store: &Arc<dyn TaskStore>, id: TaskId) -> String {
    let events: Vec<serde_json::Value> = events_of(store, id)
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Event::HarnessSelected { .. }
                    | Event::LlmRouteSelected { .. }
                    | Event::RoutingDecided { .. }
            )
        })
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    let runs: Vec<_> = store
        .runs_for_task(id)
        .unwrap()
        .into_iter()
        .map(|r| (r.role, r.seq, r.adapter, r.model, r.account, r.work_unit_id))
        .collect();
    let refusals: Vec<String> = progress_msgs(store, id)
        .into_iter()
        .filter(|m| m.contains("route refused"))
        .collect();
    mask_ulids(&format!(
        "{}|{runs:?}|{refusals:?}",
        serde_json::to_string(&events).unwrap()
    ))
}

/// 1 件の task を 1 tick だけ dispatch し、走った compare を待つ。`compare` が真なら `route_priority` を compare にする。
async fn run_once(
    compare: bool,
    script: Option<Arc<FakeScript>>,
    edit_registry: impl FnOnce(&mut Registry),
    setup: impl FnOnce(&mut Dispatcher, &Task),
) -> Out {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let mut task = hinted_task(ws.path(), task_core::TierSource::Hint);
    task.id = TASK_ID.parse().unwrap();
    store.insert(&task).unwrap();
    let generation = seed_generation(&sqlite, &["lane_rules", "route_priority"]);
    if compare {
        set_compare(&sqlite, "route_priority", &generation);
    }
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    let mut registry = d.llm_route_registry().clone();
    edit_registry(&mut registry);
    d.set_llm_route_registry(registry);
    setup(&mut d, &task);
    d.set_script_domain_store(sqlite.clone());
    d.set_policy_script(script.map(|s| s as Arc<dyn PolicyScript>));
    d.tick().unwrap();
    d.wait_script_compares().await;
    Out {
        records: records_of(&store, task.id),
        route_bytes: route_bytes(&store, task.id),
        _ws: ws,
    }
}

async fn baseline(edit_registry: impl FnOnce(&mut Registry)) -> String {
    let out = run_once(false, None, edit_registry, |_, _| {}).await;
    assert!(out.records.is_empty());
    out.route_bytes
}

fn only(records: &[ScriptCompareRecord]) -> &ScriptCompareRecord {
    assert_eq!(records.len(), 1, "{records:?}");
    &records[0]
}

fn no_edit(_: &mut Registry) {}

#[tokio::test]
async fn script_compare_route_priority_match_records_one_per_decision() {
    let base = baseline(no_edit).await;
    assert!(base.contains("harness_selected"), "{base}");
    let script = FakeScript::new(Behavior::Echo);
    let out = run_once(true, Some(script.clone()), no_edit, |_, _| {}).await;
    assert_eq!(script.route_calls(), 1);
    assert_eq!(out.route_bytes, base);
    let r = only(&out.records);
    assert_eq!(r.domain, ScriptDomain::RoutePriority);
    assert_eq!(r.outcome, CompareOutcome::Match);
    assert_eq!(r.call_site, "route_select.route_select_one");
    assert_eq!(r.bundle_sha256, bundle_sha());
    assert!(r.run_id.as_deref().is_some_and(|id| id.len() == 26));
    assert_eq!(
        r.rust,
        serde_json::json!({"basis": "harness", "candidates": [{"source": "p1", "model": "m"}]})
    );
    assert_eq!(r.ts.as_ref(), Some(&r.rust));
    let input = r.input.as_ref().unwrap();
    assert_eq!(input["harness"], "test");
    assert_eq!(input["cos"], false);
    assert_eq!(input["execution_class"], "coding");
    assert!(r.safety_violations.is_empty() && r.error.is_none());
    // 記録の run は実際に作った run。
    let store_runs = out.route_bytes.matches("Worker").count();
    assert!(store_runs >= 1, "{}", out.route_bytes);
}

/// 常に違う候補を返す TS でも、選ばれる route・event・run の行は基準と byte 一致する（S2-6）。
#[tokio::test]
async fn script_compare_route_priority_mismatch_keeps_rust_route_bytes() {
    let base = baseline(no_edit).await;
    for (ts, violations) in [
        // 宣言済み（CoS の列）だが違う列。
        (
            serde_json::json!({"basis": "cos", "candidates": []}),
            vec![],
        ),
        // 入力の table に無い候補。
        (
            serde_json::json!({"basis": "harness", "candidates": [{"source": "evil", "model": "m"}]}),
            vec!["undeclared_candidate".to_owned()],
        ),
    ] {
        let script = FakeScript::new(Behavior::Fixed(ts.clone()));
        let out = run_once(true, Some(script.clone()), no_edit, |_, _| {}).await;
        assert_eq!(script.route_calls(), 1);
        assert_eq!(out.route_bytes, base, "route must stay Rust's for {ts}");
        let r = only(&out.records);
        assert_eq!(r.outcome, CompareOutcome::Mismatch);
        assert_eq!(r.ts.as_ref(), Some(&ts));
        assert_eq!(r.safety_violations, violations);
    }
}

#[tokio::test]
async fn script_compare_route_priority_errors_keep_rust_route_bytes() {
    let base = baseline(no_edit).await;
    let many: Vec<serde_json::Value> = (0..257)
        .map(|_| serde_json::json!({"source": "p1", "model": "m"}))
        .collect();
    let cases = [
        (
            Behavior::Fail(PolicyScriptError::new("exception", "boom")),
            CompareOutcome::TsError,
            "exception",
        ),
        (
            Behavior::Fixed(serde_json::json!({"basis": "lane", "candidates": many})),
            CompareOutcome::InvalidOutput,
            "invalid_output",
        ),
        (
            Behavior::Fixed(serde_json::json!({"basis": "best", "candidates": []})),
            CompareOutcome::InvalidOutput,
            "invalid_output",
        ),
        (
            Behavior::Fixed(serde_json::Value::Null),
            CompareOutcome::InvalidOutput,
            "invalid_output",
        ),
    ];
    for (behavior, outcome, kind) in cases {
        let script = FakeScript::new(behavior);
        let out = run_once(true, Some(script.clone()), no_edit, |_, _| {}).await;
        assert_eq!(script.route_calls(), 1);
        assert_eq!(
            out.route_bytes, base,
            "route must stay Rust's for {outcome:?}"
        );
        let r = only(&out.records);
        assert_eq!(r.outcome, outcome);
        assert_eq!(r.error.as_ref().unwrap().kind, kind);
    }
}

#[tokio::test(start_paused = true)]
async fn script_compare_route_priority_timeout_records_ts_error() {
    let base = baseline(no_edit).await;
    let script = FakeScript::new(Behavior::Hang);
    let out = run_once(true, Some(script.clone()), no_edit, |_, _| {}).await;
    assert_eq!(script.route_calls(), 1);
    assert_eq!(out.route_bytes, base);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::TsError);
    assert_eq!(r.error.as_ref().unwrap().kind, "timeout");
    let deadline_us = u64::try_from(SCRIPT_CALL_DEADLINE.as_micros()).unwrap();
    assert!(
        r.elapsed_us >= deadline_us && r.elapsed_us < 2 * deadline_us,
        "elapsed {} us",
        r.elapsed_us
    );
}

#[tokio::test]
async fn script_compare_route_priority_mode_rust_never_calls_ts() {
    let base = baseline(no_edit).await;
    let script = FakeScript::new(Behavior::Echo);
    let out = run_once(false, Some(script.clone()), no_edit, |_, _| {}).await;
    assert_eq!(script.route_calls(), 0);
    assert!(out.records.is_empty());
    assert_eq!(out.route_bytes, base);
}

#[tokio::test]
async fn script_compare_route_priority_other_bundle_is_unavailable() {
    let base = baseline(no_edit).await;
    let script = FakeScript::with_sha("cd".repeat(32), Behavior::Echo);
    let out = run_once(true, Some(script.clone()), no_edit, |_, _| {}).await;
    assert_eq!(script.route_calls(), 0);
    assert_eq!(out.route_bytes, base);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Unavailable);
    assert!(r.ts.is_none());
    // PolicyScript が無いときも unavailable。
    let out = run_once(true, None, no_edit, |_, _| {}).await;
    assert_eq!(only(&out.records).outcome, CompareOutcome::Unavailable);
}

/// failover（1 つ目の harness が断り、2 つ目が選ぶ）でも記録は 1 件で、比べる harness は選んだ harness。
#[tokio::test]
async fn script_compare_route_priority_failover_records_once() {
    let failover = |r: &mut Registry| {
        r.harness_selection
            .execution_class_defaults
            .insert("coding".into(), vec!["claude_code".into(), "test".into()]);
    };
    let base = baseline(failover).await;
    assert!(
        base.contains("claude_code"),
        "first harness must refuse: {base}"
    );
    let script = FakeScript::new(Behavior::Echo);
    let out = run_once(true, Some(script.clone()), failover, |_, _| {}).await;
    assert_eq!(script.route_calls(), 1);
    assert_eq!(out.route_bytes, base);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Match);
    assert_eq!(r.input.as_ref().unwrap()["harness"], "test");
}

/// 空の列（basis none）は記録し、route は `NoDeclaredDefault` で断ったまま（TS が候補を返しても）。
#[tokio::test]
async fn script_compare_route_priority_empty_column_stays_no_declared_default() {
    let empty = |r: &mut Registry| {
        r.route_defaults.insert(HarnessId::from("test"), Vec::new());
    };
    let base = baseline(empty).await;
    assert!(
        base.contains("no route_defaults candidates are declared for harness test"),
        "{base}"
    );
    for (behavior, outcome) in [
        (Behavior::Echo, CompareOutcome::Match),
        (
            Behavior::Fixed(serde_json::json!({"basis": "cos", "candidates": [
                {"source": "p1", "model": "m"}]})),
            CompareOutcome::Mismatch,
        ),
    ] {
        let script = FakeScript::new(behavior);
        let out = run_once(true, Some(script.clone()), empty, |_, _| {}).await;
        assert_eq!(script.route_calls(), 1);
        assert_eq!(out.route_bytes, base);
        let r = only(&out.records);
        assert_eq!(r.outcome, outcome);
        assert_eq!(
            r.rust,
            serde_json::json!({"basis": "none", "candidates": []})
        );
    }
}

/// lane_assist の試走（`select_route` を lane ごとに呼ぶ）は比べない。記録は本物の判定の 1 件だけ。
#[tokio::test]
async fn script_compare_route_priority_skips_lane_assist_trials() {
    let assisted = |d: &mut Dispatcher, task: &Task| {
        d.set_routing_mode(RoutingMode::DecisionAssisted);
        d.classification_board().put(
            &task.id.to_string(),
            jev_ok(SpecificityHint::InsufficientInfo),
        );
    };
    let script = FakeScript::new(Behavior::Echo);
    let out = run_once(true, Some(script.clone()), no_edit, assisted).await;
    assert_eq!(script.route_calls(), 1);
    assert_eq!(only(&out.records).outcome, CompareOutcome::Match);
}

/// CoS の判定（`cos == true`）は比べない。
#[tokio::test]
async fn script_compare_route_priority_skips_cos() {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = hinted_task(ws.path(), task_core::TierSource::Hint);
    store.insert(&task).unwrap();
    let generation = seed_generation(&sqlite, &["route_priority"]);
    set_compare(&sqlite, "route_priority", &generation);
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_script_domain_store(sqlite.clone());
    let script = FakeScript::new(Behavior::Echo);
    d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    let mut input = crate::dispatcher::route_select::RouteInput {
        task_id: task.id.to_string(),
        harness: crate::dispatcher::route_select::harness_request_for(&task),
        model_hint: None,
        lane: Some("standard".into()),
        cos: true,
    };
    let (outcome, trace) = d.select_route(&input);
    assert!(outcome.is_ok(), "{outcome:?}");
    d.compare_route_priority(task.id, "01RUNCOS", &input, &trace);
    d.wait_script_compares().await;
    assert_eq!(script.route_calls(), 0);
    assert!(records_of(&store, task.id).is_empty());
    // 同じ入力で cos だけを外すと比べる（試験の前提の確認）。
    input.cos = false;
    let (_, trace) = d.select_route(&input);
    d.compare_route_priority(task.id, "01RUNTASK", &input, &trace);
    d.wait_script_compares().await;
    assert_eq!(script.route_calls(), 1);
    assert_eq!(
        only(&records_of(&store, task.id)).run_id.as_deref(),
        Some("01RUNTASK")
    );
}

/// reviewer の run の route 選択（review_spawn）は比べない。記録は worker の run の 1 件だけ。
#[tokio::test]
async fn script_compare_route_priority_skips_review_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let task = new_task(dir.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let generation = seed_generation(&sqlite, &["route_priority"]);
    set_compare(&sqlite, "route_priority", &generation);
    let worker = worker_adapter();
    let mut d = dispatcher(store.clone(), worker.clone(), 2);
    d.set_llm_route_registry(reviewer_registry("fake"));
    let seen = Arc::new(StdMutex::new(Vec::new()));
    d.set_harness_adapters(HashMap::from([
        (HarnessId::from("h1"), worker),
        (
            HarnessId::from("review_h"),
            Arc::new(CapturingReviewer { seen: seen.clone() }) as Arc<dyn WorkerAdapter>,
        ),
    ]));
    d.set_script_domain_store(sqlite.clone());
    let script = FakeScript::new(Behavior::Echo);
    d.set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
    let report = run_until_idle(&mut d, 300).await;
    d.wait_script_compares().await;
    assert!(report.idle);
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert_eq!(seen.lock().unwrap().len(), 1, "the reviewer ran");
    assert_eq!(script.route_calls(), 1);
    let r = only(&records_of(&store, task.id)).clone();
    assert_eq!(r.input.as_ref().unwrap()["harness"], "h1");
}
